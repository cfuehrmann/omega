//! The event sink (§17 of `docs/monitors-design.html`, Phase A).
//!
//! ## Why this exists
//!
//! Historically, emitting an [`OmegaEvent`] was a side-effect of the agent
//! turn loop ([`Agent::run`](crate::Agent)): the loop appended to
//! `events.jsonl` and then *yielded* the event on its stream so the server
//! could forward it to the WebSocket.  Events born **outside** a turn (a
//! monitor's stderr line read while the agent is parked, a halt click, a
//! mid-turn model switch) had no clean home — they were parked on side
//! queues and only committed late, sometimes re-stamped at drain time.
//!
//! The [`EventSink`] is the out-of-band home for those events.  A single call
//! to [`EventSink::emit`] both **appends** the event to `events.jsonl` and
//! **broadcasts** it to whichever WebSocket is currently connected.  The
//! event already carries its true event-*time* (stamped at the moment of
//! occurrence by the caller); the sink only *commits* it.  Event-time and
//! commit-time are independent and both preserved — the log is never sorted
//! by time, the UI shows file/arrival order with each row's own `time`.
//!
//! ## What still uses the loop
//!
//! Phase A is **additive**.  The loop's existing append-and-yield path is
//! untouched: conversation events, turn lifecycle, monitor delivery, etc.
//! still flow through `run()`.  Each event source uses exactly **one** path,
//! so nothing is emitted twice.  The sink is for the three out-of-band
//! sources migrated in Phase A: monitor **stderr**, **halt** requests, and
//! **model / effort** changes.
//!
//! ## The broadcaster
//!
//! The WS half is abstracted behind [`EventBroadcaster`] so this crate need
//! not depend on the server's `WsMessage` type.  The server installs a
//! concrete broadcaster (resolving the *current* `ws_tx`, which is replaced
//! on reconnect) via [`EventSink::set_broadcaster`].  Headless / CLI / test
//! callers leave it unset, in which case `emit` still appends to disk and the
//! broadcast is a no-op.

use std::sync::{Arc, Mutex};

use omega_store::EventStore;
use omega_types::OmegaEvent;

/// The WebSocket half of the sink.
///
/// Implemented by the server with a handle that resolves the *current*
/// `ws_tx` at broadcast time (the sender is `Option` and replaced on
/// reconnect).  `broadcast` must be cheap and non-blocking — it is called
/// synchronously from [`EventSink::emit`] to preserve arrival order on the
/// wire.
pub trait EventBroadcaster: Send + Sync {
    /// Forward `event` to the currently-connected client, if any.
    fn broadcast(&self, event: &OmegaEvent);
}

/// Appends an event to `events.jsonl` and broadcasts it to the current WS,
/// from any caller at any time.
///
/// Holds an [`Arc<EventStore>`] (per-line-atomic, safe under concurrent
/// callers) plus a swappable [`EventBroadcaster`].  No locking enforces time
/// order: each event is stamped at occurrence by its caller and committed
/// whenever the sink gets to it.
pub struct EventSink {
    store: Arc<EventStore>,
    broadcaster: Mutex<Option<Arc<dyn EventBroadcaster>>>,
}

impl std::fmt::Debug for EventSink {
    // Cosmetic, hand-written because `dyn EventBroadcaster` is not `Debug`
    // (so the struct cannot derive it).  The exact rendering is not behaviour
    // any test should pin, so the body-replacement mutant has nothing to
    // catch it — skip rather than assert on debug text.
    #[mutants::skip]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let has_broadcaster = self.broadcaster.lock().is_ok_and(|g| g.is_some());
        f.debug_struct("EventSink")
            .field("has_broadcaster", &has_broadcaster)
            .finish_non_exhaustive()
    }
}

impl EventSink {
    /// Build a sink over `store` with no broadcaster installed yet.
    #[must_use]
    pub fn new(store: Arc<EventStore>) -> Self {
        Self {
            store,
            broadcaster: Mutex::new(None),
        }
    }

    /// Install (or replace) the WS broadcaster.  Called by the server once
    /// per session; the broadcaster itself resolves the live `ws_tx`.
    pub fn set_broadcaster(&self, broadcaster: Arc<dyn EventBroadcaster>) {
        *self
            .broadcaster
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(broadcaster);
    }

    /// Borrow the backing store (used by handles that need to read the log
    /// back, e.g. control-handle tests).
    #[must_use]
    pub fn store(&self) -> &Arc<EventStore> {
        &self.store
    }

    /// Commit `event`: append to `events.jsonl`, then broadcast to the
    /// current WS.  Returns the event unchanged so callers can keep using it
    /// (e.g. update an info cache).  Append happens before broadcast so a
    /// client that reconnects right after never sees a wire event that is not
    /// yet on disk.
    pub async fn emit(&self, event: OmegaEvent) -> OmegaEvent {
        let _ = self.store.append(&event).await;
        self.broadcast(&event);
        event
    }

    /// Fire-and-forget emit for synchronous callers (e.g. the monitor stderr
    /// reader, whose [`MonitorSink`](omega_tools::MonitorSink) method is not
    /// `async`).  The broadcast happens **synchronously and in-order** so the
    /// wire reflects production order; the disk append is spawned (its
    /// commit-time may lag, and per §17 file order is explicitly allowed to
    /// differ from time order).
    pub fn emit_detached(self: &Arc<Self>, event: OmegaEvent) {
        self.broadcast(&event);
        let sink = Arc::clone(self);
        tokio::spawn(async move {
            let _ = sink.store.append(&event).await;
        });
    }

    /// Broadcast helper shared by [`Self::emit`] and [`Self::emit_detached`].
    fn broadcast(&self, event: &OmegaEvent) {
        let broadcaster = self
            .broadcaster
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(b) = broadcaster {
            b.broadcast(event);
        }
    }
}
