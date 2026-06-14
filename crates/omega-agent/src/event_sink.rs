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

use omega_core::AgentItem;
use omega_store::EventStore;
use omega_types::{OmegaEvent, StreamSignal};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

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

/// Appends an event to `events.jsonl` and broadcasts it to every registered
/// subscriber, from any caller at any time.
///
/// Holds an [`Arc<EventStore>`] (per-line-atomic, safe under concurrent
/// callers) plus a **registry** of [`EventBroadcaster`] subscribers.  Today
/// the server installs exactly one (the WS broadcaster); the registry is the
/// groundwork for "observers as projections" (uniform-emission Phase 2,
/// `docs/uniform-emission-spike.md`), where the WS forwarder, the server's
/// control-reactions, and a test recorder all subscribe to the same emit.
/// No locking enforces time order: each event is stamped at occurrence by its
/// caller and committed whenever the sink gets to it.
pub struct EventSink {
    store: Arc<EventStore>,
    subscribers: Mutex<Vec<Arc<dyn EventBroadcaster>>>,
    /// The ordered "wire" (uniform-emission Phase 2): a single channel that
    /// carries **events and signals** to the server's drain loop in causal
    /// order.  Inert (`None`) until [`Self::take_wire_receiver`] activates it,
    /// so a sink with no receiver taken is behaviour-identical to pre-wire.
    wire_tx: Mutex<Option<UnboundedSender<AgentItem>>>,
}

impl std::fmt::Debug for EventSink {
    // Cosmetic, hand-written because `dyn EventBroadcaster` is not `Debug`
    // (so the struct cannot derive it).  The exact rendering is not behaviour
    // any test should pin, so the body-replacement mutant has nothing to
    // catch it — skip rather than assert on debug text.
    #[mutants::skip]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let subscriber_count = self.subscribers.lock().map_or(0, |g| g.len());
        f.debug_struct("EventSink")
            .field("subscriber_count", &subscriber_count)
            .finish_non_exhaustive()
    }
}

impl EventSink {
    /// Build a sink over `store` with no broadcaster installed yet.
    #[must_use]
    pub fn new(store: Arc<EventStore>) -> Self {
        Self {
            store,
            subscribers: Mutex::new(Vec::new()),
            wire_tx: Mutex::new(None),
        }
    }

    /// Activate the wire and take its receiver.  The wire is the ordered
    /// channel that carries events **and** signals to the server's drain loop
    /// (uniform-emission Phase 2, `docs/uniform-emission-spike.md`).  Until
    /// this is called the wire is inert: [`Self::emit`] / [`Self::emit_signal`]
    /// push nowhere, so the sink is behaviour-identical to pre-wire.  Called
    /// once per session by the server (the drain loop reads the receiver while
    /// the run-future borrows the agent).
    pub fn take_wire_receiver(&self) -> UnboundedReceiver<AgentItem> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        *self
            .wire_tx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(tx);
        rx
    }

    /// Push an [`AgentItem`] onto the wire if active; a no-op otherwise.
    /// A closed receiver is ignored (the session is winding down).
    fn push_to_wire(&self, item: AgentItem) {
        if let Some(tx) = self
            .wire_tx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            let _ = tx.send(item);
        }
    }

    /// Push a streaming [`StreamSignal`] onto the wire.  Signals are ephemeral
    /// live-render fragments: never appended to `events.jsonl`, never
    /// broadcast as events — they only ride the wire to the UI, interleaved
    /// in causal order with the events emitted around them.
    pub fn emit_signal(&self, signal: StreamSignal) {
        self.push_to_wire(AgentItem::Signal(signal));
    }

    /// Install the WS broadcaster as the **sole** subscriber, replacing any
    /// existing ones.  Called by the server once per session; the broadcaster
    /// itself resolves the live `ws_tx`.  (Replace semantics preserve the
    /// historical single-broadcaster behaviour; use [`Self::add_subscriber`]
    /// to register additional observers without displacing this one.)
    pub fn set_broadcaster(&self, broadcaster: Arc<dyn EventBroadcaster>) {
        let mut subs = self
            .subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        subs.clear();
        subs.push(broadcaster);
    }

    /// Register an additional subscriber without displacing existing ones.
    /// Every registered subscriber receives every emitted event, in
    /// registration order, on each [`Self::emit`] / [`Self::emit_detached`].
    pub fn add_subscriber(&self, subscriber: Arc<dyn EventBroadcaster>) {
        self.subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(subscriber);
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
        self.push_to_wire(AgentItem::event(event.clone()));
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
        self.push_to_wire(AgentItem::event(event.clone()));
        let sink = Arc::clone(self);
        tokio::spawn(async move {
            let _ = sink.store.append(&event).await;
        });
    }

    /// Broadcast helper shared by [`Self::emit`] and [`Self::emit_detached`].
    /// Fans the event out to every registered subscriber, in registration
    /// order.  The lock is released before broadcasting (the snapshot is
    /// cloned) so a subscriber can never deadlock the registry.
    fn broadcast(&self, event: &OmegaEvent) {
        let subscribers = self
            .subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        for subscriber in &subscribers {
            subscriber.broadcast(event);
        }
    }
}

#[cfg(test)]
mod tests {
    // Carve-out: these unit tests target `EventSink` directly rather than
    // through `Agent::send_message`.  The subscriber-registry fan-out is a
    // self-contained property of the sink; exercising multi-subscriber
    // registration through a full agent run (which only ever installs one
    // broadcaster) would be disproportionate setup for the logic under test.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use omega_types::events::AgentErrorEvent;

    #[derive(Default)]
    struct Rec {
        seen: Mutex<Vec<String>>,
    }
    impl Rec {
        fn count(&self) -> usize {
            self.seen.lock().unwrap().len()
        }
    }
    impl EventBroadcaster for Rec {
        fn broadcast(&self, event: &OmegaEvent) {
            if let OmegaEvent::AgentError(e) = event {
                self.seen.lock().unwrap().push(e.error.clone());
            }
        }
    }

    fn sink() -> EventSink {
        let dir = tempfile::tempdir().unwrap();
        EventSink::new(Arc::new(EventStore::new(dir.path().join("events.jsonl"))))
    }

    fn err_event(msg: &str) -> OmegaEvent {
        OmegaEvent::AgentError(AgentErrorEvent {
            time: "2026-01-01T00:00:00.000Z".to_owned(),
            error: msg.to_owned(),
        })
    }

    #[tokio::test]
    async fn emit_fans_out_to_every_subscriber_in_order() {
        let sink = sink();
        let a = Arc::new(Rec::default());
        let b = Arc::new(Rec::default());
        sink.add_subscriber(Arc::clone(&a) as Arc<dyn EventBroadcaster>);
        sink.add_subscriber(Arc::clone(&b) as Arc<dyn EventBroadcaster>);

        sink.emit(err_event("boom")).await;

        assert_eq!(a.seen.lock().unwrap().as_slice(), ["boom"]);
        assert_eq!(b.seen.lock().unwrap().as_slice(), ["boom"]);
    }

    #[tokio::test]
    async fn set_broadcaster_replaces_all_existing_subscribers() {
        let sink = sink();
        let old = Arc::new(Rec::default());
        let new = Arc::new(Rec::default());
        sink.add_subscriber(Arc::clone(&old) as Arc<dyn EventBroadcaster>);
        sink.set_broadcaster(Arc::clone(&new) as Arc<dyn EventBroadcaster>);

        sink.emit(err_event("x")).await;

        assert_eq!(old.count(), 0, "replaced subscriber must not receive");
        assert_eq!(new.count(), 1, "installed subscriber must receive");
    }

    #[tokio::test]
    async fn add_subscriber_appends_without_displacing() {
        let sink = sink();
        let first = Arc::new(Rec::default());
        let second = Arc::new(Rec::default());
        sink.add_subscriber(Arc::clone(&first) as Arc<dyn EventBroadcaster>);
        sink.add_subscriber(Arc::clone(&second) as Arc<dyn EventBroadcaster>);

        sink.emit(err_event("y")).await;

        assert_eq!(first.count(), 1);
        assert_eq!(second.count(), 1);
    }

    // --- wire (uniform-emission Phase 2, slice b1) -----------------------

    /// Bounded wire read: a missing push fails fast (panic) instead of
    /// hanging, so a `push_to_wire`/`emit_signal` no-op mutant is CAUGHT
    /// rather than timing out.
    async fn next(rx: &mut UnboundedReceiver<AgentItem>) -> AgentItem {
        tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .expect("timed out waiting for a wire item")
            .expect("wire closed unexpectedly")
    }

    #[tokio::test]
    async fn emit_pushes_event_onto_active_wire() {
        let sink = sink();
        let mut rx = sink.take_wire_receiver();

        sink.emit(err_event("boom")).await;

        match next(&mut rx).await {
            AgentItem::Event(ev) => {
                assert!(matches!(*ev, OmegaEvent::AgentError(e) if e.error == "boom"));
            }
            other @ AgentItem::Signal(_) => panic!("expected event on wire, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn emit_signal_pushes_signal_onto_wire() {
        let sink = sink();
        let mut rx = sink.take_wire_receiver();

        sink.emit_signal(StreamSignal::Text {
            index: 0,
            text: "hi".to_owned(),
        });

        assert!(matches!(next(&mut rx).await, AgentItem::Signal(_)));
    }

    #[tokio::test]
    async fn wire_is_inert_until_receiver_taken() {
        let sink = sink();
        // No receiver taken yet: emit must not panic and must push nowhere.
        sink.emit(err_event("before")).await;

        let mut rx = sink.take_wire_receiver();
        sink.emit(err_event("after")).await;

        // Only the post-activation event is on the wire — the pre-activation
        // emit was inert (behaviour-preserving until the wire is active).
        match next(&mut rx).await {
            AgentItem::Event(ev) => {
                assert!(matches!(*ev, OmegaEvent::AgentError(e) if e.error == "after"));
            }
            other @ AgentItem::Signal(_) => {
                panic!("expected only the post-activation event, got {other:?}")
            }
        }
        assert!(
            rx.try_recv().is_err(),
            "the pre-activation emit must not be buffered on the wire"
        );
    }
}
