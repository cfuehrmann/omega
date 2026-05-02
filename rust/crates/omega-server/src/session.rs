//! `ActiveSession` — the server's in-memory representation of the one live session.
//!
//! Phase 1e.1: struct definition only.  The WebSocket sender type is a
//! `serde_json::Value` placeholder; the concrete WS-message type lands in 1e.2.

use std::sync::Arc;

use omega_agent::{Agent, ControlHandle};
use omega_store::SessionPaths;
use tokio::sync::Mutex;
use tokio::sync::mpsc::UnboundedSender;

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
    /// `None` until a WebSocket connection upgrades (1e.2).  The element type
    /// is `serde_json::Value` as a placeholder; the concrete `WsEvent` type
    /// lands in Phase 1e.2.
    pub ws_tx: Option<UnboundedSender<serde_json::Value>>,
}
