//! The event sink — the unified emit path for all `OmegaEvent`s
//! (`docs/uniform-emission-spike.md`, Phase 2 / slice b3).
//!
//! ## Why this exists
//!
//! Historically, emitting an [`OmegaEvent`] was a side-effect of the agent
//! turn loop ([`Agent::run`](crate::Agent)): the loop appended to
//! `events.jsonl` and then *yielded* the event on its stream so the server
//! could forward it to the WebSocket.  Events born **outside** a turn (a
//! monitor's stderr line read while the agent is parked, a halt click, a
//! mid-turn model switch) had no clean home.
//!
//! The [`EventSink`] is now the **unified emit point** for ALL events.  A
//! single call to [`EventSink::emit`] both **appends** the event to
//! `events.jsonl` and **pushes it onto the ordered wire** that the server
//! (and test harness) drains.  In-turn events use the same [`EventSink::emit`]
//! path via the `commit_event` helper in the loop; out-of-band events (model
//! changes, halt clicks, monitor stderr) call `emit` or `emit_detached`
//! directly.  Either way, every event reaches disk and the WS through ONE
//! code path.
//!
//! ## The wire
//!
//! The server (and the test harness) calls [`EventSink::take_wire_receiver`]
//! once per session, then drives `agent.run()` and the wire drain
//! concurrently (`join!(run, drain(rx))`).  The wire is an ordered,
//! unbounded MPSC channel that carries both [`OmegaEvent`]s (via `emit` /
//! `emit_detached`) and ephemeral [`StreamSignal`]s (via `emit_signal`) in
//! causal order.  Signals are never appended to disk; events always are.
//! The broadcaster / subscriber-registry machinery (Phase A) has been retired
//! (slice b3); nothing installs a broadcaster any more.

use std::sync::{Arc, Mutex};

use omega_core::AgentItem;
use omega_store::EventStore;
use omega_types::{OmegaEvent, StreamSignal};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

/// Appends an event to `events.jsonl` and pushes it onto the ordered wire,
/// from any caller at any time.
///
/// Holds an [`Arc<EventStore>`] (per-line-atomic, safe under concurrent
/// callers) and an **ordered wire** — a single MPSC channel that carries
/// events AND signals in causal order to the server's drain loop.  The wire
/// is inert until [`Self::take_wire_receiver`] activates it; before that,
/// `emit` / `emit_signal` are behaviour-identical to pre-wire operation.
/// No locking enforces time order: each event is stamped at occurrence by its
/// caller and committed whenever the sink gets to it.
pub struct EventSink {
    store: Arc<EventStore>,
    /// The ordered "wire": a single channel that carries **events and signals**
    /// to the server's drain loop in causal order.  Inert (`None`) until
    /// [`Self::take_wire_receiver`] activates it, so a sink with no receiver
    /// taken is behaviour-identical to pre-wire.
    wire_tx: Mutex<Option<UnboundedSender<AgentItem>>>,
}

impl std::fmt::Debug for EventSink {
    // Cosmetic: the wire_tx Mutex<Option<…>> does not implement Debug in a
    // useful way, so we hand-write a minimal representation.  The exact
    // rendering is not behaviour any test should pin, so the body-replacement
    // mutant has nothing to catch it — skip rather than assert on debug text.
    #[mutants::skip]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let wire_active = self.wire_tx.lock().is_ok_and(|g| g.is_some());
        f.debug_struct("EventSink")
            .field("wire_active", &wire_active)
            .finish_non_exhaustive()
    }
}

impl EventSink {
    /// Build a sink over `store`.
    #[must_use]
    pub fn new(store: Arc<EventStore>) -> Self {
        Self {
            store,
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

    /// Close the wire by dropping the sender, so a draining consumer's
    /// `recv()` returns `None` once buffered items are consumed.  Called by
    /// the consumer after `run()` returns, so the drain loop terminates with
    /// the session instead of parking forever (the sender otherwise lives as
    /// long as the agent).  Idempotent.
    pub fn close_wire(&self) {
        *self
            .wire_tx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    /// Push an [`AgentItem`] onto the wire if active; a no-op otherwise.
    /// A closed receiver is ignored (the session is winding down).
    pub(crate) fn push_to_wire(&self, item: AgentItem) {
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

    /// Borrow the backing store (used by handles that need to read the log
    /// back, e.g. control-handle tests).
    #[must_use]
    pub fn store(&self) -> &Arc<EventStore> {
        &self.store
    }

    /// Emit `event`: append to `events.jsonl`, then push onto the wire.
    /// Returns the event unchanged so callers can keep using it.  This is the
    /// **unified emit path** for both in-turn events (called via the
    /// `commit_event` chokepoint in the run loop) and out-of-band events
    /// (model changes, halt clicks).  Append happens before push so a client
    /// that reconnects right after never sees a wire event not yet on disk.
    pub async fn emit(&self, event: OmegaEvent) -> OmegaEvent {
        let _ = self.store.append(&event).await;
        self.push_to_wire(AgentItem::event(event.clone()));
        event
    }

    /// Fire-and-forget emit for synchronous callers (e.g. the monitor stderr
    /// reader, whose [`MonitorSink`](omega_tools::MonitorSink) method is not
    /// `async`).  The wire push happens **synchronously and in-order** so the
    /// consumer sees events in production order; the disk append is spawned
    /// (its commit-time may lag, and per §17 file order is explicitly allowed
    /// to differ from time order for stderr).
    pub fn emit_detached(self: &Arc<Self>, event: OmegaEvent) {
        self.push_to_wire(AgentItem::event(event.clone()));
        let sink = Arc::clone(self);
        tokio::spawn(async move {
            let _ = sink.store.append(&event).await;
        });
    }
}

#[cfg(test)]
mod tests {
    // Carve-out: these unit tests target `EventSink` directly rather than
    // through `Agent::send_message`.  The wire ordering and signal delivery
    // are self-contained properties of the sink; exercising them through a
    // full agent run would be disproportionate setup for the logic under test.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use omega_types::events::AgentErrorEvent;

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

    // --- wire tests -------------------------------------------------------

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

    /// The wire is a single FIFO channel, so events and signals are delivered
    /// in exactly the causal order they were emitted — the property the live
    /// UI relies on (a `Text` signal must land between its surrounding
    /// response events).
    #[tokio::test]
    async fn wire_preserves_event_signal_interleaving_order() {
        let sink = sink();
        let mut rx = sink.take_wire_receiver();

        sink.emit(err_event("1")).await;
        sink.emit_signal(StreamSignal::Text {
            index: 0,
            text: "x".to_owned(),
        });
        sink.emit(err_event("2")).await;

        assert!(
            matches!(next(&mut rx).await, AgentItem::Event(_)),
            "1st: event"
        );
        assert!(
            matches!(next(&mut rx).await, AgentItem::Signal(_)),
            "2nd: signal"
        );
        assert!(
            matches!(next(&mut rx).await, AgentItem::Event(_)),
            "3rd: event"
        );
    }

    /// `close_wire` must drop the sender so the consumer's drain loop ends with
    /// the session — but buffered items (e.g. a final `TurnEnd`) must still be
    /// delivered first, never dropped.  The bounded final read turns a
    /// `close_wire` no-op mutant into a CAUGHT failure (fast `Err`) rather than
    /// a hang.
    #[tokio::test]
    async fn close_wire_delivers_buffered_items_then_terminates() {
        let sink = sink();
        let mut rx = sink.take_wire_receiver();

        sink.emit(err_event("a")).await;
        sink.emit(err_event("b")).await;
        sink.close_wire();

        // Buffered items survive the close.
        assert!(
            matches!(next(&mut rx).await, AgentItem::Event(_)),
            "buffered a"
        );
        assert!(
            matches!(next(&mut rx).await, AgentItem::Event(_)),
            "buffered b"
        );

        // Then the wire ends (sender dropped).
        let ended = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv()).await;
        assert_eq!(
            ended,
            Ok(None),
            "close_wire must drop the sender so the drain loop terminates"
        );
    }
}
