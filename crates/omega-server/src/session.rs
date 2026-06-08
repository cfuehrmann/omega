//! `ActiveSession` — the server's in-memory representation of the one live session.
//!
//! Phase 1e.2: the placeholder `UnboundedSender<serde_json::Value>` is replaced
//! with the concrete [`UnboundedSender<WsMessage>`](crate::ws_message::WsMessage),
//! which the WebSocket writer task drains and serialises.

use std::sync::Arc;

use omega_agent::{Agent, ControlHandle, EventBroadcaster, InputQueue, ModelEffortHandle};
use omega_core::AgentItem;
use omega_store::SessionPaths;
use omega_tools::MonitorManager;
use omega_types::{FeatureFlags, OmegaEvent};
use tokio::sync::Mutex;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::ws_message::WsMessage;

/// Shared, reconnect-stable holder for the current WebSocket sender.
///
/// `ws_tx` is `None` until a socket upgrades and is **replaced** on every
/// reconnect.  Wrapping it in `Arc<Mutex<Option<…>>>` lets two readers share
/// the SAME live value: the per-turn forwarder ([`send_to_active`]) and the
/// out-of-band [`EventSink`](omega_agent::EventSink) broadcaster
/// ([`WsEventBroadcaster`]).  A `std::sync::Mutex` (not tokio) is deliberate —
/// the guard is held only for a single non-blocking `try_send`/`send`, never
/// across an `.await`, so the broadcaster's `broadcast` can stay synchronous
/// and preserve wire order.
pub type WsTxCell = Arc<std::sync::Mutex<Option<UnboundedSender<WsMessage>>>>;

/// Replace the sender held by a [`WsTxCell`] (poison-tolerant: a poisoned
/// lock still recovers the inner `Option`, since a stale `ws_tx` carries no
/// invariant worth panicking over).
pub(crate) fn set_ws_tx(cell: &WsTxCell, tx: Option<UnboundedSender<WsMessage>>) {
    *cell
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = tx;
}

/// Send one frame to the sender currently in a [`WsTxCell`], if any.  Drops
/// the frame silently when no client is connected or the channel is closed.
pub(crate) fn send_via_ws_tx(cell: &WsTxCell, msg: WsMessage) {
    if let Some(tx) = cell
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
    {
        let _ = tx.send(msg);
    }
}

/// [`EventBroadcaster`] that fans an out-of-band [`OmegaEvent`] onto the
/// session's current WebSocket (§17, Phase A).
///
/// Resolves the CURRENT `ws_tx` from a shared [`WsTxCell`] at broadcast time,
/// so events emitted after a reconnect still reach the live socket.  Wraps
/// the event in the same `WsMessage::Item(AgentItem::Event(…))` frame the
/// per-turn run stream uses, so the client decodes it identically.
#[derive(Debug, Clone)]
pub struct WsEventBroadcaster {
    ws_tx: WsTxCell,
}

impl WsEventBroadcaster {
    /// Bind a broadcaster to a session's `ws_tx` cell.
    #[must_use]
    pub fn new(ws_tx: WsTxCell) -> Self {
        Self { ws_tx }
    }
}

impl EventBroadcaster for WsEventBroadcaster {
    fn broadcast(&self, event: &OmegaEvent) {
        // Hold the std lock only for the non-blocking `send`; never across an
        // await.  A dropped message (no client / closed channel) is fine —
        // the canonical copy is already (or will be) on disk.
        if let Ok(guard) = self.ws_tx.lock()
            && let Some(tx) = guard.as_ref()
        {
            let _ = tx.send(WsMessage::Item(Box::new(AgentItem::Event(Box::new(
                event.clone(),
            )))));
        }
    }
}

/// Snapshot of the fields needed to build a [`WsMessage::SessionInfo`]
/// without locking the agent. Lets handlers (notably `handle_halt`) and
/// the per-turn streaming loop broadcast session info while another task
/// holds the agent mutex.
#[derive(Clone, Debug)]
pub struct SessionInfoCache {
    pub dir: String,
    pub model: String,
    pub effort: String,
    pub cwd: String,
    pub name: Option<String>,
    /// Whether the working tree had uncommitted changes when this session
    /// was created.  Computed once by `git status --porcelain` and carried
    /// in every `session_info` broadcast so the client can show a warning.
    pub has_pending_changes: bool,
    /// Runtime feature flags active for this session.
    /// Forwarded verbatim onto every [`WsMessage::SessionInfo`] frame so
    /// the UI can display capability badges without re-reading the event log.
    pub features: FeatureFlags,
}

/// All state belonging to the currently-active session.
///
/// There is at most one `ActiveSession` alive at a time, held behind an
/// `Arc<Mutex<Option<ActiveSession>>>` in [`AppState`](crate::AppState).
/// `POST /api/sessions` replaces the slot; `GET /api/sessions` reads the
/// sessions root directory and never touches this slot.
pub struct ActiveSession {
    /// The running agent.  Wrapped in `Arc<Mutex<…>>` so the WebSocket
    /// handler can hold a reference while the HTTP handler also holds one.
    pub agent: Arc<Mutex<Agent>>,
    /// Halt / resume / abort handle cloned from the agent before the
    /// session slot is filled.  Drives the §15 three-controls model.
    pub controls: ControlHandle,
    /// Resolved file paths for this session (dir, context.jsonl, events.jsonl).
    pub paths: SessionPaths,
    /// WebSocket broadcast channel to the connected browser client.
    ///
    /// `None` until a WebSocket connection upgrades.  Replaced (not
    /// fanned-out) on every reconnect, matching the TS server's
    /// single-WS-at-a-time model.
    ///
    /// A shared [`WsTxCell`] (not a bare `Option`) so the out-of-band
    /// [`WsEventBroadcaster`] resolves the SAME live sender as the per-turn
    /// forwarder (§17, Phase A).
    pub ws_tx: WsTxCell,
    /// Handle to the persistent per-session `Agent::run` task (§15 Unified
    /// Input Model, U1).
    ///
    /// Spawned **once** at reset/resume; it owns the agent lock for the
    /// session's life, parks on an empty [`Self::input_queue`], and forwards
    /// the run stream to the WebSocket continuously.  Consumed by graceful
    /// shutdown / session teardown so the server can cancel
    /// ([`Self::run_cancel`]) and `join` the task (with a deadline) before
    /// reaping monitors.
    pub current_turn: Option<JoinHandle<()>>,
    /// Shared inspectable input queue for the persistent run loop (§15 U1).
    ///
    /// `handle_user_message` calls [`InputQueue::push`] instead of sending
    /// on a bare `mpsc::Sender` — this preserves the no-agent-lock contract
    /// while also making pending items visible via [`InputQueue::snapshot`]
    /// for server→client queue-visualisation pushes.  Multiple producers
    /// (server WS handler, future monitor callbacks in U2) share the same
    /// `Arc`-backed handle via `Clone`.
    pub input_queue: InputQueue,
    /// Run-level cancel token for the persistent run task.  Firing it tells
    /// the run loop to terminate (and aborts any in-flight turn via the
    /// forwarder), releasing the agent lock so teardown can reap monitors.
    pub run_cancel: CancellationToken,
    /// Lock-free handle for model/effort changes (§15 Unified Input Model).
    /// Lets `handle_set_model` / `handle_set_effort` mutate model/effort and
    /// persist the change without acquiring the agent lock — which the
    /// persistent run task holds for the session's life.
    pub model_effort: ModelEffortHandle,
    /// Derived turn state (`idle` / `running` / `halt_requested` / `halted`).
    /// §15 (U3) block-boundary markers: `running` = inside a block;
    /// `halted` = parked at a halt seam; `idle` = parked at an empty-queue
    /// seam.
    ///
    /// Updated by the WebSocket router when transition-carrying events flow
    /// through the per-turn stream (and explicitly from `handle_halt`, since
    /// `halt_requested` is logged outside the agent generator). Wrapped in
    /// `Arc<Mutex<…>>`
    /// so the streaming task can update it without re-locking the
    /// `active_session` slot.
    pub turn_state: Arc<Mutex<String>>,
    /// Cached projection of the fields needed to build a
    /// [`WsMessage::SessionInfo`] without locking the agent. Populated at
    /// session creation and refreshed by `handle_set_model`,
    /// `handle_set_effort`, and `handle_rename_session`.
    pub info_cache: Arc<Mutex<SessionInfoCache>>,
    /// Runtime feature flags for this session.  A `Copy` snapshot of the
    /// flags resolved at startup; available without acquiring any lock so
    /// future branching code can check `active_session.features.repl` etc.
    /// directly.
    pub features: FeatureFlags,
    /// Live monitor roster manager.  Stashed here so the WebSocket handler
    /// can push ephemeral [`WsMessage::MonitorRoster`] snapshots on client
    /// connect and after each monitor lifecycle event (Phase 3 badge/modal).
    /// The manager is the same `Arc` the agent holds, so reads reflect the
    /// latest state without any extra synchronisation.
    pub monitor_manager: Arc<MonitorManager>,
}
