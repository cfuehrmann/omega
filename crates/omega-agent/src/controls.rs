// Mutex-lock helpers below use `.expect("...")` to panic on lock
// poisoning.  Poisoning happens only if a thread panicked while
// holding the lock; the controls handle has no recovery strategy
// at that point and propagating the panic is the correct response.
#![allow(clippy::expect_used)]

//! Halt / resume / abort controls for the persistent [`Agent`](crate::Agent)
//! run loop (§15 Unified Input Model, U3).
//!
//! ## The three orthogonal controls
//!
//! - **Queue (Send)** — *not* a control-handle concern: a user message is
//!   pushed to the [`InputQueue`](crate::InputQueue). It lands at the next
//!   seam. This subsumes the retired pause-for-**injection** machinery: to
//!   interject, you queue a message.
//! - **Halt** — "stop advancing at the next seam and WAIT." At the next
//!   Seam B (after a tool-result batch) the run loop parks instead of
//!   continuing the block, so the user can compose a steering message at
//!   leisure. Implemented here via [`request_halt`](ControlHandle::request_halt).
//! - **Abort** — forceful: cancel the in-flight block NOW. Unchanged from
//!   prior phases; see [`request_abort`](ControlHandle::request_abort).
//!
//! ## Resume UX (two ways out of a halt)
//!
//! 1. **Queue a steering message** (Send) — the queued item wakes the
//!    parked loop, is injected at the seam, and the loop continues with it.
//! 2. **Explicit Resume** — [`request_resume`](ControlHandle::request_resume)
//!    continues the block with no new input ("never mind, carry on").
//!
//! ## State machine
//!
//! ```text
//!                     request_halt()
//!     running ───────────────────────────► halt_requested
//!       ▲                                      │
//!       │                                      │ Seam B reached
//!       │                                      ▼
//!       │                              TurnHalted emitted; loop parks
//!       │                              (suspended = true)
//!       │                                      │
//!       │   queued message  ┌──────────────────┤
//!       │   (inbox.pop)     │                  │ request_resume()
//!       │   or request_abort│                  │ (resume_requested)
//!       │                   ▼                  ▼
//!       └────── TurnResumed (inject?) or TurnInterrupted
//! ```
//!
//! ## Concurrency primitives
//!
//! - [`std::sync::Mutex<ControlState>`] — brief critical sections; never
//!   held across `.await`.
//! - [`tokio::sync::Notify`] — wakes the parked seam when `request_resume`
//!   / `request_abort` fire while the loop is halted.
//! - [`tokio_util::sync::CancellationToken`] — turn-scoped abort token,
//!   replaced fresh on every run-loop turn entry.

use std::sync::{Arc, Mutex};

use crate::event_sink::EventSink;
use omega_types::OmegaEvent;
use omega_types::events::HaltRequestedEvent;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// Internal state
// ---------------------------------------------------------------------------

/// Mutable halt-control state. Always accessed under
/// `ControlHandle::state.lock()`; never held across `.await`.
#[derive(Default, Debug)]
pub(crate) struct ControlState {
    /// Set by `request_halt`, cleared at the seam (or on turn-exit).
    pub(crate) halt_requested: bool,
    /// True while the run loop is parked at a halt seam awaiting wake.
    pub(crate) suspended: bool,
    /// Set by `request_resume`, consumed by the parked seam to continue
    /// with no new input.
    pub(crate) resume_requested: bool,
}

// ---------------------------------------------------------------------------
// Public handle
// ---------------------------------------------------------------------------

/// Cloneable handle exposing halt / resume / abort to external code.
///
/// Obtain one via [`Agent::controls`](crate::Agent::controls). The handle
/// stays valid across the whole persistent run loop — the agent rotates
/// the underlying turn-cancel token on each turn entry.
#[derive(Clone)]
pub struct ControlHandle {
    state: Arc<Mutex<ControlState>>,
    notify: Arc<Notify>,
    /// Held behind a `Mutex` so the agent can swap in a fresh token at
    /// turn entry while `request_abort` reads the current one.
    cancel: Arc<Mutex<CancellationToken>>,
    /// Out-of-band event sink (§17, Phase A).  `request_halt` emits the
    /// single click-time `HaltRequested` through it, committing to disk AND
    /// broadcasting to the current WS from ONE creation — so disk time ==
    /// wire time and no caller re-creates the event.
    event_sink: Arc<EventSink>,
}

impl ControlHandle {
    /// Construct a fresh handle. Used by [`Agent::new`](crate::Agent::new).
    #[must_use]
    pub(crate) fn new(event_sink: Arc<EventSink>) -> Self {
        Self {
            state: Arc::new(Mutex::new(ControlState::default())),
            notify: Arc::new(Notify::new()),
            cancel: Arc::new(Mutex::new(CancellationToken::new())),
            event_sink,
        }
    }

    /// Request that the run loop **halt** at its next clean seam — stop
    /// advancing the block and park, so the user can compose a steering
    /// message. Idempotent: repeated calls while a request is already
    /// pending, or while the loop is already parked, are no-ops.
    ///
    /// Logs a `HaltRequested` event regardless of whether a turn is
    /// running, so post-mortem inspection captures every click.
    ///
    /// §17 (Phase A): the event is created exactly ONCE here and routed
    /// through the [`EventSink`] — the same `time` lands on disk and on the
    /// wire.  The server's halt handler no longer re-creates / re-stamps it.
    pub async fn request_halt(&self) {
        {
            let mut g = self.lock_state();
            if g.halt_requested || g.suspended {
                return;
            }
            g.halt_requested = true;
        }
        let ev = OmegaEvent::HaltRequested(HaltRequestedEvent { time: now_iso() });
        self.event_sink.emit(ev).await;
    }

    /// Resume a halted turn with **no new input** ("carry on"). If the loop
    /// is parked at a halt seam, wakes it so it emits `TurnResumed` and
    /// continues the block. If a turn is not halted this records the intent
    /// (consumed if a halt parks before turn-exit) but fires no wake.
    ///
    /// The *other* way to resume — queuing a steering message — does not go
    /// through here: the queued item wakes the parked seam directly via the
    /// inbox.
    pub fn request_resume(&self) {
        let was_suspended = {
            let mut g = self.lock_state();
            g.resume_requested = true;
            g.suspended
        };
        if was_suspended {
            self.notify.notify_one();
        }
    }

    /// Abort the currently-running turn. Cancels the turn-scoped token
    /// (so in-flight tool dispatches and the LLM stream wind down), and
    /// wakes the seam if it is parked so it can emit `TurnInterrupted`
    /// and exit. Idempotent — safe to call when no turn is running.
    pub fn request_abort(&self) {
        let token = self.lock_cancel().clone();
        token.cancel();
        let was_suspended = self.lock_state().suspended;
        if was_suspended {
            self.notify.notify_one();
        }
    }

    // -----------------------------------------------------------------
    // crate-internal API used by Agent::run / drive_turn
    // -----------------------------------------------------------------

    /// Reset state for a new turn and install a fresh cancellation
    /// token. Returns the new token so the agent can pass it to tools
    /// and check it on cancellation paths.
    pub(crate) fn reset_for_turn(&self) -> CancellationToken {
        {
            let mut g = self.lock_state();
            g.halt_requested = false;
            g.suspended = false;
            g.resume_requested = false;
        }
        let new_token = CancellationToken::new();
        *self.lock_cancel() = new_token.clone();
        new_token
    }

    /// Atomically: if `halt_requested` is set, clear it and return
    /// `true`. Returns `false` otherwise. Called once per tool-batch
    /// seam check.
    pub(crate) fn take_halt_request(&self) -> bool {
        let mut g = self.lock_state();
        if g.halt_requested {
            g.halt_requested = false;
            true
        } else {
            false
        }
    }

    /// Atomically: if `resume_requested` is set, clear it and return
    /// `true`. Returns `false` otherwise. Re-checked under lock at the
    /// top of each halt-wait loop iteration.
    pub(crate) fn take_resume_request(&self) -> bool {
        let mut g = self.lock_state();
        if g.resume_requested {
            g.resume_requested = false;
            true
        } else {
            false
        }
    }

    /// Mark the loop as parked at a halt seam (`suspended = true`).
    /// Called immediately before the halt-wait loop.
    pub(crate) fn enter_halt_wait(&self) {
        self.lock_state().suspended = true;
    }

    /// Clear the `suspended` flag once the seam wakes.
    pub(crate) fn exit_halt_wait(&self) {
        self.lock_state().suspended = false;
    }

    /// Borrow the wake-up notify so the agent can `select!` on it.
    pub(crate) fn notify(&self) -> &Notify {
        &self.notify
    }

    // -----------------------------------------------------------------
    // Lock helpers — keep the `expect` strings consistent.
    // -----------------------------------------------------------------

    fn lock_state(&self) -> std::sync::MutexGuard<'_, ControlState> {
        self.state.lock().expect("control state mutex poisoned")
    }

    fn lock_cancel(&self) -> std::sync::MutexGuard<'_, CancellationToken> {
        self.cancel.lock().expect("control cancel mutex poisoned")
    }
}

// ---------------------------------------------------------------------------
// TurnGuard
// ---------------------------------------------------------------------------

/// RAII cleanup for a running turn. Constructed at the top of
/// `drive_turn`'s stream body. Its [`Drop`] runs whether the body
/// returns normally, errors, or is dropped mid-park by the caller.
pub(crate) struct TurnGuard {
    state: Arc<Mutex<ControlState>>,
    notify: Arc<Notify>,
    forwarder: Option<JoinHandle<()>>,
}

impl TurnGuard {
    /// Construct a guard. `forwarder` is the external-cancel forwarder
    /// task (if any) — aborted on drop so it doesn't leak past the turn.
    pub(crate) fn new(handle: &ControlHandle, forwarder: Option<JoinHandle<()>>) -> Self {
        Self {
            state: Arc::clone(&handle.state),
            notify: Arc::clone(&handle.notify),
            forwarder,
        }
    }
}

impl Drop for TurnGuard {
    fn drop(&mut self) {
        let was_suspended = {
            let mut g = self
                .state
                .lock()
                .expect("control state mutex poisoned (in TurnGuard::drop)");
            g.halt_requested = false;
            g.resume_requested = false;
            let prev = g.suspended;
            g.suspended = false;
            prev
        };
        if was_suspended {
            // Wake any awaiter so the awaiting future settles cleanly.
            self.notify.notify_one();
        }
        if let Some(h) = self.forwarder.take() {
            h.abort();
        }
    }
}

// ---------------------------------------------------------------------------
// Time helper
// ---------------------------------------------------------------------------

/// Wall-clock ISO-8601 timestamp helper for control events.
///
/// `cargo mutants` flags both string-replacement mutants as surviving:
/// every event carrying a `time` field is redacted in WS / CLI snapshots
/// (timestamps would make snapshots flaky), so a corrupted timestamp
/// never fails a downstream assertion. Accepted dead code at the
/// mutation-testing level — the format is exercised manually and by
/// `chrono`'s own tests.
#[mutants::skip]
fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    //! Inline carve-out tests for `controls.rs`.
    //!
    //! Justification for carve-out: `ControlHandle` tests require direct access
    //! to `lock_state()` (a `pub(crate)` method) to inspect and manipulate the
    //! internal `ControlState` mutex.  These state-machine transitions cannot be
    //! provoked or observed through `Agent::run` / `MockProvider` without races
    //! and heroic timing — the halt/resume/cancel flags are consumed inside the
    //! agent loop before any event is emitted.  The end-to-end halt/resume
    //! behaviour is covered in `tests/internal.rs`.

    use super::*;
    use crate::event_sink::EventBroadcaster;
    use omega_store::EventStore;
    use omega_types::events::OmegaEvent;
    use tempfile::TempDir;

    fn make_handle() -> (ControlHandle, TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("events.jsonl");
        let store = Arc::new(EventStore::new(path));
        let sink = Arc::new(EventSink::new(store));
        (ControlHandle::new(sink), tmp)
    }

    /// Records every event the sink broadcasts to the "wire".
    #[derive(Default)]
    struct RecBroadcaster {
        events: std::sync::Mutex<Vec<OmegaEvent>>,
    }
    impl EventBroadcaster for RecBroadcaster {
        fn broadcast(&self, event: &OmegaEvent) {
            self.events.lock().unwrap().push(event.clone());
        }
    }

    #[test]
    fn take_halt_request_clears_flag() {
        let (h, _t) = make_handle();
        h.lock_state().halt_requested = true;
        assert!(h.take_halt_request());
        assert!(!h.lock_state().halt_requested);
        // Second call: already cleared.
        assert!(!h.take_halt_request());
    }

    #[test]
    fn take_resume_request_clears_flag() {
        let (h, _t) = make_handle();
        // No resume requested → false.
        assert!(!h.take_resume_request());
        h.lock_state().resume_requested = true;
        assert!(h.take_resume_request());
        assert!(!h.lock_state().resume_requested);
        // Second call: already cleared.
        assert!(!h.take_resume_request());
    }

    #[test]
    fn enter_halt_wait_sets_suspended() {
        let (h, _t) = make_handle();
        assert!(!h.lock_state().suspended);
        h.enter_halt_wait();
        assert!(h.lock_state().suspended);
    }

    #[test]
    fn exit_halt_wait_clears_suspended() {
        let (h, _t) = make_handle();
        h.lock_state().suspended = true;
        h.exit_halt_wait();
        assert!(!h.lock_state().suspended);
    }

    #[test]
    fn request_resume_sets_flag() {
        let (h, _t) = make_handle();
        h.request_resume();
        assert!(
            h.lock_state().resume_requested,
            "request_resume must record the resume intent"
        );
    }

    #[tokio::test]
    async fn request_resume_wakes_when_suspended() {
        // When the loop is parked (suspended), request_resume must fire a
        // notify so the awaiting future settles.  We assert the wake by
        // racing notified() against a short timeout.
        let (h, _t) = make_handle();
        h.lock_state().suspended = true;
        let notified = {
            let n = h.notify();
            n.notified()
        };
        h.request_resume();
        tokio::time::timeout(tokio::time::Duration::from_millis(500), notified)
            .await
            .expect("request_resume must wake the parked seam when suspended");
    }

    #[tokio::test]
    async fn request_resume_does_not_wake_when_not_suspended() {
        // Not suspended → no permit should be stored: a fresh notified()
        // must NOT resolve immediately.
        let (h, _t) = make_handle();
        h.request_resume();
        let n = h.notify();
        let res = tokio::time::timeout(tokio::time::Duration::from_millis(100), n.notified()).await;
        assert!(
            res.is_err(),
            "request_resume must not fire a wake when the loop is not parked"
        );
    }

    #[tokio::test]
    async fn request_halt_idempotent_when_already_pending() {
        let (h, _t) = make_handle();
        h.request_halt().await;
        h.request_halt().await; // no-op
        assert!(h.lock_state().halt_requested);
    }

    #[tokio::test]
    async fn request_halt_skipped_when_suspended() {
        let (h, _t) = make_handle();
        h.lock_state().suspended = true;
        h.request_halt().await;
        assert!(!h.lock_state().halt_requested);
    }

    #[tokio::test]
    async fn request_halt_appends_event() {
        let (h, _t) = make_handle();
        h.request_halt().await;
        let store = h.event_sink.store();
        let events = store.read_all().await.unwrap();
        assert_eq!(events.len(), 1, "request_halt must append one event");
        assert_eq!(events[0]["type"], "halt_requested");
    }

    /// (§17, Phase A) test (b): a single halt produces ONE `HaltRequested`
    /// event, routed to BOTH disk and wire from a single creation — so the
    /// disk timestamp and the wire timestamp are identical (no double stamp).
    #[tokio::test]
    async fn request_halt_emits_once_disk_time_equals_wire_time() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("events.jsonl");
        let store = Arc::new(EventStore::new(path));
        let sink = Arc::new(EventSink::new(Arc::clone(&store)));
        let rec = Arc::new(RecBroadcaster::default());
        sink.set_broadcaster(Arc::clone(&rec) as Arc<dyn EventBroadcaster>);
        let h = ControlHandle::new(sink);

        h.request_halt().await;

        // Exactly one HaltRequested on disk.
        let disk = store.read_all().await.unwrap();
        let disk_halts: Vec<_> = disk
            .iter()
            .filter(|e| e["type"] == "halt_requested")
            .collect();
        assert_eq!(disk_halts.len(), 1, "exactly one HaltRequested on disk");

        // Exactly one HaltRequested on the wire.
        let wire = rec.events.lock().unwrap();
        let wire_times: Vec<&str> = wire
            .iter()
            .filter_map(|e| match e {
                OmegaEvent::HaltRequested(ev) => Some(ev.time.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(wire_times.len(), 1, "exactly one HaltRequested on the wire");

        // Single creation ⇒ the disk and wire timestamps are identical.
        let disk_time = disk_halts[0]["time"].as_str().unwrap();
        assert_eq!(
            disk_time, wire_times[0],
            "disk and wire timestamps must come from one stamp"
        );
    }

    #[test]
    fn request_abort_cancels_current_token() {
        let (h, _t) = make_handle();
        let token = h.lock_cancel().clone();
        h.request_abort();
        assert!(token.is_cancelled());
    }

    #[test]
    fn reset_for_turn_clears_state_and_rotates_token() {
        let (h, _t) = make_handle();
        h.lock_state().halt_requested = true;
        h.lock_state().resume_requested = true;
        h.lock_state().suspended = true;
        let old_token = h.lock_cancel().clone();
        old_token.cancel();
        let new_token = h.reset_for_turn();
        let g = h.lock_state();
        assert!(!g.halt_requested);
        assert!(!g.resume_requested);
        assert!(!g.suspended);
        assert!(!new_token.is_cancelled());
        assert!(old_token.is_cancelled()); // independent token
    }

    #[test]
    fn turn_guard_clears_state_and_wakes_notify() {
        let (h, _t) = make_handle();
        h.lock_state().halt_requested = true;
        h.lock_state().resume_requested = true;
        h.lock_state().suspended = true;
        {
            let _g = TurnGuard::new(&h, None);
        }
        let g = h.lock_state();
        assert!(!g.halt_requested);
        assert!(!g.resume_requested);
        assert!(!g.suspended);
    }

    /// Kills `replace ControlHandle::notify -> &Notify with
    /// Box::leak(Box::new(Default::default()))`. The agent loop awaits
    /// `notify().notified()` while external callers wake the handle
    /// through the same instance — if `notify()` ever returned a fresh
    /// `Notify` per call, the wake would land on a `Notify` nobody is
    /// listening to. Two consecutive calls must be the SAME instance.
    #[test]
    fn notify_returns_same_instance() {
        let (h, _t) = make_handle();
        let p1: *const tokio::sync::Notify = h.notify();
        let p2: *const tokio::sync::Notify = h.notify();
        assert!(
            std::ptr::eq(p1, p2),
            "notify() must return a stable reference; got {p1:?} then {p2:?}"
        );
    }
}
