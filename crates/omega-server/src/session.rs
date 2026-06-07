//! `ActiveSession` — the server's in-memory representation of the one live session.
//!
//! Phase 1e.2: the placeholder `UnboundedSender<serde_json::Value>` is replaced
//! with the concrete [`UnboundedSender<WsMessage>`](crate::ws_message::WsMessage),
//! which the WebSocket writer task drains and serialises.

use std::sync::Arc;

use omega_agent::{Agent, ControlHandle, InputQueue, ModelEffortHandle};
use omega_store::SessionPaths;
use omega_tools::MonitorManager;
use omega_types::FeatureFlags;
use tokio::sync::Mutex;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::ws_message::WsMessage;

/// Snapshot of the fields needed to build a [`WsMessage::SessionInfo`]
/// without locking the agent. Lets handlers (notably `handle_pause`) and
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
    /// Pause / continue / abort handle cloned from the agent before the
    /// session slot is filled.  Used by future control endpoints.
    pub controls: ControlHandle,
    /// Resolved file paths for this session (dir, context.jsonl, events.jsonl).
    pub paths: SessionPaths,
    /// WebSocket broadcast channel to the connected browser client.
    ///
    /// `None` until a WebSocket connection upgrades.  Replaced (not
    /// fanned-out) on every reconnect, matching the TS server's
    /// single-WS-at-a-time model.
    pub ws_tx: Option<UnboundedSender<WsMessage>>,
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
    /// Derived turn state (`idle` / `running` / `pause_requested` / `paused`).
    ///
    /// Mirrors the TS server's `currentTurnState`. Updated by the WebSocket
    /// router when transition-carrying events flow through the per-turn
    /// stream (and explicitly from `handle_pause`, since `pause_requested`
    /// is logged outside the agent generator). Wrapped in `Arc<Mutex<…>>`
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
