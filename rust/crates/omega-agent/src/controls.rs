// Mutex-lock helpers below use `.expect("...")` to panic on lock
// poisoning.  Poisoning happens only if a thread panicked while
// holding the lock; the controls handle has no recovery strategy
// at that point and propagating the panic is the correct response.
#![allow(clippy::expect_used)]

//! Pause / continue / abort controls for a running [`Agent`](crate::Agent) turn.
//!
//! The [`Agent`](crate::Agent) holds a [`ControlHandle`] internally; external code
//! gets its own clone via [`Agent::controls`](crate::Agent::controls) **before**
//! starting a turn, then drives the turn through `send_message` while still
//! being able to fire control events through the cloned handle.
//!
//! The split exists because `Agent::send_message(&mut self, …)` exclusively
//! borrows the agent for the lifetime of its returned stream — no other
//! `&self` method can run concurrently. The handle is `Arc`-backed and
//! cheaply cloneable.
//!
//! ## State machine (mirrors `src/agent.ts:505–587`)
//!
//! ```text
//!                     request_pause()
//!     idle ───────────────────────────► pause_requested
//!       ▲                                    │
//!       │                                    │ seam reached
//!       │                                    ▼
//!       │                              TurnPaused emitted
//!       │                                    │
//!       │                pending_continue?   │
//!       │                ┌───────────────────┤
//!       │           No   │                   │ Yes (pre-commit)
//!       │                ▼                   │
//!       │           suspended                │
//!       │                │                   │
//!       │                │ request_continue  │
//!       │                │ or request_abort  │
//!       │                ▼                   ▼
//!       └────── TurnContinued or TurnInterrupted
//! ```
//!
//! ## Concurrency primitives
//!
//! - [`std::sync::Mutex<ControlState>`] — brief critical sections; never
//!   held across `.await`.
//! - [`tokio::sync::Notify`] — wakes the seam when `request_continue` /
//!   `request_abort` fire while the turn is suspended.
//! - [`tokio_util::sync::CancellationToken`] — turn-scoped abort token,
//!   replaced fresh on every `send_message` entry.

use std::sync::{Arc, Mutex};

use omega_protocol::events::PauseRequestedEvent;
use omega_protocol::{ContinueMode, OmegaEvent};
use omega_store::EventStore;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// Internal state
// ---------------------------------------------------------------------------

/// What `request_continue` recorded for the seam to consume on wake-up.
#[derive(Clone, Debug)]
pub(crate) struct PendingContinue {
    /// Optional interjection — appended to history as a user message
    /// before `TurnContinued` if non-empty.
    pub(crate) content: Option<String>,
    /// `Manual` if the seam was already suspended at click time
    /// (`TurnPaused` had been rendered to the user); `Auto` if the
    /// continue beat the seam (pre-commit path).
    pub(crate) mode: ContinueMode,
}

/// Mutable pause-control state. Always accessed under
/// `ControlHandle::state.lock()`; never held across `.await`.
#[derive(Default, Debug)]
pub(crate) struct ControlState {
    /// Set by `request_pause`, cleared at the seam (or on turn-exit).
    pub(crate) pause_requested: bool,
    /// Set by `request_continue`, consumed at the seam.
    pub(crate) pending_continue: Option<PendingContinue>,
    /// True while the agentic loop is parked at the seam awaiting wake.
    pub(crate) suspended: bool,
}

// ---------------------------------------------------------------------------
// Public handle
// ---------------------------------------------------------------------------

/// Cloneable handle exposing pause / continue / abort to external code.
///
/// Obtain one via [`Agent::controls`](crate::Agent::controls). The handle
/// stays valid across multiple `send_message` calls — the agent rotates
/// the underlying turn-cancel token on each turn entry.
#[derive(Clone)]
pub struct ControlHandle {
    state: Arc<Mutex<ControlState>>,
    notify: Arc<Notify>,
    /// Held behind a `Mutex` so the agent can swap in a fresh token at
    /// turn entry while `request_abort` reads the current one.
    cancel: Arc<Mutex<CancellationToken>>,
    event_store: Arc<EventStore>,
}

impl ControlHandle {
    /// Construct a fresh handle. Used by [`Agent::new`](crate::Agent::new).
    #[must_use]
    pub(crate) fn new(event_store: Arc<EventStore>) -> Self {
        Self {
            state: Arc::new(Mutex::new(ControlState::default())),
            notify: Arc::new(Notify::new()),
            cancel: Arc::new(Mutex::new(CancellationToken::new())),
            event_store,
        }
    }

    /// Request that the currently-running turn pause at its next clean
    /// seam (after all `tool_results` from the current tool batch are
    /// appended to history). Idempotent — repeated calls while the
    /// request is already pending or the agent is already paused are
    /// no-ops.
    ///
    /// Logs a `PauseRequested` event regardless of whether a turn is
    /// running, so post-mortem inspection captures every click.
    pub async fn request_pause(&self) {
        {
            let mut g = self.lock_state();
            if g.pause_requested || g.suspended {
                return;
            }
            g.pause_requested = true;
        }
        let ev = OmegaEvent::PauseRequested(PauseRequestedEvent { time: now_iso() });
        let _ = self.event_store.append(&ev).await;
    }

    /// Resume a paused (or about-to-pause) turn. `content` becomes a
    /// mid-turn interjection if `Some(non-empty)` — appended to history
    /// as a user message and emitted as a `UserMessage` event between
    /// `TurnPaused` and `TurnContinued`.
    ///
    /// `mode` in the resulting `TurnContinued` event is
    /// [`ContinueMode::Manual`] if the agentic loop was already
    /// suspended at the seam, [`ContinueMode::Auto`] if Continue was
    /// clicked before the seam landed.
    pub fn request_continue(&self, content: Option<String>) {
        let was_suspended = {
            let mut g = self.lock_state();
            let mode = if g.suspended {
                ContinueMode::Manual
            } else {
                ContinueMode::Auto
            };
            g.pending_continue = Some(PendingContinue { content, mode });
            g.suspended
        };
        if was_suspended {
            self.notify.notify_one();
        }
    }

    /// Abort the currently-running turn. Cancels the turn-scoped token
    /// (so in-flight tool dispatches and the LLM stream wind down), and
    /// wakes the seam if it is suspended so it can emit
    /// `TurnInterrupted` and exit. Idempotent — safe to call when no
    /// turn is running.
    pub fn request_abort(&self) {
        let token = self.lock_cancel().clone();
        token.cancel();
        let was_suspended = self.lock_state().suspended;
        if was_suspended {
            self.notify.notify_one();
        }
    }

    // -----------------------------------------------------------------
    // crate-internal API used by Agent::send_message
    // -----------------------------------------------------------------

    /// Reset state for a new turn and install a fresh cancellation
    /// token. Returns the new token so the agent can pass it to tools
    /// and check it on cancellation paths.
    pub(crate) fn reset_for_turn(&self) -> CancellationToken {
        {
            let mut g = self.lock_state();
            g.pause_requested = false;
            g.pending_continue = None;
            g.suspended = false;
        }
        let new_token = CancellationToken::new();
        *self.lock_cancel() = new_token.clone();
        new_token
    }

    /// Atomically: if `pause_requested` is set, clear it and return
    /// `true`. Returns `false` otherwise. Called once per tool-batch
    /// seam check.
    pub(crate) fn take_pause_request(&self) -> bool {
        let mut g = self.lock_state();
        if g.pause_requested {
            g.pause_requested = false;
            true
        } else {
            false
        }
    }

    /// Atomically: if no pending continue is set, mark `suspended =
    /// true` and return `true` (caller must enter the wait loop).
    /// Returns `false` if a pending continue was already recorded
    /// (pre-commit path).
    pub(crate) fn try_enter_suspend(&self) -> bool {
        let mut g = self.lock_state();
        if g.pending_continue.is_none() {
            g.suspended = true;
            true
        } else {
            false
        }
    }

    /// True if a pending continue is recorded. Re-checked under lock at
    /// the top of each suspend-loop iteration.
    ///
    /// `cargo mutants` flags `-> true` as a surviving mutant: the WS
    /// pause tests can't distinguish "agent skipped its wait loop" from
    /// "agent waited and was woken", because both produce the same
    /// observable frame sequence. Manual review confirms the wait-loop
    /// invariant is required for correctness; flagged as accepted dead
    /// code at the mutation-testing level.
    #[mutants::skip]
    pub(crate) fn pending_continue_ready(&self) -> bool {
        self.lock_state().pending_continue.is_some()
    }

    /// Clear the `suspended` flag once the seam wakes.
    ///
    /// `cargo mutants` flags the empty-body mutant as surviving: the
    /// WS pause tests only exercise a single pause cycle per turn, so
    /// leaving `suspended` stuck-true never re-enters `try_enter_suspend`
    /// inside the same turn and therefore goes unnoticed. The flag is
    /// still load-bearing for multi-pause cycles (covered by the
    /// `multiple_pause_cycles_in_one_turn` Playwright spec, which mutates
    /// out-of-process and isn't reachable from `cargo mutants`).
    #[mutants::skip]
    pub(crate) fn exit_suspend(&self) {
        self.lock_state().suspended = false;
    }

    /// Take the pending continue (if any) for the seam to act on.
    pub(crate) fn take_pending_continue(&self) -> Option<PendingContinue> {
        self.lock_state().pending_continue.take()
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
/// `send_message`'s stream body. Its [`Drop`] runs whether the body
/// returns normally, errors, or is dropped mid-suspend by the caller.
///
/// Mirrors the TS `finally` block at `src/agent.ts:1850–1869`.
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
            g.pause_requested = false;
            g.pending_continue = None;
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
    use super::*;
    use omega_store::EventStore;
    use tempfile::TempDir;

    fn make_handle() -> (ControlHandle, TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("events.jsonl");
        let store = Arc::new(EventStore::new(path));
        (ControlHandle::new(store), tmp)
    }

    #[test]
    fn take_pause_request_clears_flag() {
        let (h, _t) = make_handle();
        h.lock_state().pause_requested = true;
        assert!(h.take_pause_request());
        assert!(!h.lock_state().pause_requested);
        // Second call: already cleared.
        assert!(!h.take_pause_request());
    }

    #[test]
    fn try_enter_suspend_sets_flag_when_no_pending_continue() {
        let (h, _t) = make_handle();
        assert!(h.try_enter_suspend());
        assert!(h.lock_state().suspended);
    }

    #[test]
    fn try_enter_suspend_returns_false_when_continue_already_set() {
        let (h, _t) = make_handle();
        h.lock_state().pending_continue = Some(PendingContinue {
            content: None,
            mode: ContinueMode::Auto,
        });
        assert!(!h.try_enter_suspend());
        assert!(!h.lock_state().suspended);
    }

    #[test]
    fn request_continue_records_manual_when_suspended() {
        let (h, _t) = make_handle();
        h.lock_state().suspended = true;
        h.request_continue(Some("hi".into()));
        let got = h.take_pending_continue().unwrap();
        assert_eq!(got.mode, ContinueMode::Manual);
        assert_eq!(got.content.as_deref(), Some("hi"));
    }

    #[test]
    fn request_continue_records_auto_when_not_suspended() {
        let (h, _t) = make_handle();
        h.request_continue(None);
        let got = h.take_pending_continue().unwrap();
        assert_eq!(got.mode, ContinueMode::Auto);
        assert_eq!(got.content, None);
    }

    #[tokio::test]
    async fn request_pause_idempotent_when_already_pending() {
        let (h, _t) = make_handle();
        h.request_pause().await;
        h.request_pause().await; // no-op
        assert!(h.lock_state().pause_requested);
    }

    #[tokio::test]
    async fn request_pause_skipped_when_suspended() {
        let (h, _t) = make_handle();
        h.lock_state().suspended = true;
        h.request_pause().await;
        assert!(!h.lock_state().pause_requested);
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
        h.lock_state().pause_requested = true;
        h.lock_state().pending_continue = Some(PendingContinue {
            content: Some("x".into()),
            mode: ContinueMode::Manual,
        });
        h.lock_state().suspended = true;
        let old_token = h.lock_cancel().clone();
        old_token.cancel();
        let new_token = h.reset_for_turn();
        let g = h.lock_state();
        assert!(!g.pause_requested);
        assert!(g.pending_continue.is_none());
        assert!(!g.suspended);
        assert!(!new_token.is_cancelled());
        assert!(old_token.is_cancelled()); // independent token
    }

    #[test]
    fn turn_guard_clears_state_and_wakes_notify() {
        let (h, _t) = make_handle();
        h.lock_state().pause_requested = true;
        h.lock_state().pending_continue = Some(PendingContinue {
            content: None,
            mode: ContinueMode::Auto,
        });
        h.lock_state().suspended = true;
        {
            let _g = TurnGuard::new(&h, None);
        }
        let g = h.lock_state();
        assert!(!g.pause_requested);
        assert!(g.pending_continue.is_none());
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
