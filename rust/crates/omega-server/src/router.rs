//! Axum router construction and route handlers.
//!
//! Phase 1e.0–1e.1 implemented `GET /health`, `GET /api/sessions`, and
//! `POST /api/sessions`.  Phase 1e.2 adds the `/ws` route: WebSocket
//! upgrade, `user_message` turn dispatch, and pause / continue / abort /
//! reset control frames.  History replay on reconnect is deliberately
//! deferred to 1e.3.
//!
//! Route map (after 1e.2):
//!
//! - `GET  /health`        — liveness probe
//! - `GET  /api/sessions`  — list sessions
//! - `POST /api/sessions`  — create session
//! - `GET  /ws`            — WebSocket upgrade
//! - `/context`, `/files`  — placeholder (1e.4)

use std::path::Path;
use std::sync::{Arc, OnceLock};

use axum::{
    Json, Router,
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{any, get},
};
use futures::{SinkExt, StreamExt};
use omega_agent::{Agent, AgentConfig};
use omega_store::{ContextStore, EventStore, session_dir_re};
use regex::Regex;
use serde::Serialize;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;
use tower_http::services::ServeDir;

use crate::AppState;
use crate::session::ActiveSession;
use crate::ws_message::WsMessage;

// ---------------------------------------------------------------------------
// Router construction
// ---------------------------------------------------------------------------

/// Build the top-level [`Router`] using `state` for all stateful handlers.
pub fn build_router(state: AppState) -> Router {
    let public_dir = state.public_dir.clone();
    Router::new()
        .route("/health", get(health))
        .route("/api/sessions", get(get_sessions).post(post_session))
        .route("/ws", get(ws_handler))
        .route("/context", any(not_implemented))
        .route("/files", any(not_implemented))
        .fallback_service(ServeDir::new(public_dir))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Shared handlers
// ---------------------------------------------------------------------------

/// `GET /health` — liveness probe.
async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

/// Placeholder for routes whose real implementation lands in later sub-phases.
async fn not_implemented() -> StatusCode {
    StatusCode::NOT_IMPLEMENTED
}

// ---------------------------------------------------------------------------
// Session list item — `GET /api/sessions`
// ---------------------------------------------------------------------------

/// One entry in the `GET /api/sessions` JSON array.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionListItem {
    pub dir: String,
    pub last_activity: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resumed_from: Option<String>,
}

// ---------------------------------------------------------------------------
// `GET /api/sessions`
// ---------------------------------------------------------------------------

/// `"2025-07-11T09-14-22-037-a8c3f1b2"` → `"2025-07-11T09:14:22.037Z"`
fn folder_name_to_timestamp(name: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        #[allow(clippy::unwrap_used)]
        Regex::new(r"^(\d{4}-\d{2}-\d{2})T(\d{2})-(\d{2})-(\d{2})(?:-(\d{3}))?").unwrap()
    });

    if let Some(caps) = re.captures(name) {
        let date = caps.get(1).map_or("", |m| m.as_str());
        let h = caps.get(2).map_or("", |m| m.as_str());
        let min = caps.get(3).map_or("", |m| m.as_str());
        let s = caps.get(4).map_or("", |m| m.as_str());
        match caps.get(5) {
            Some(ms) => format!("{date}T{h}:{min}:{s}.{}Z", ms.as_str()),
            None => format!("{date}T{h}:{min}:{s}Z"),
        }
    } else {
        name.to_owned()
    }
}

/// Enumerate session directories under `sessions_root`, sort newest-first,
/// and attach metadata.
pub async fn list_sessions(sessions_root: &Path) -> Vec<SessionListItem> {
    let Ok(mut dir_reader) = tokio::fs::read_dir(sessions_root).await else {
        return Vec::new();
    };

    let mut names: Vec<String> = Vec::new();
    while let Ok(Some(entry)) = dir_reader.next_entry().await {
        if let Some(name) = entry.file_name().to_str()
            && session_dir_re().is_match(name)
        {
            names.push(name.to_owned());
        }
    }

    names.sort_unstable();
    names.reverse();

    let mut items = Vec::with_capacity(names.len());
    for name in &names {
        let full_path = sessions_root.join(name);
        let meta = omega_store::read_session_metadata(&full_path).await;
        items.push(SessionListItem {
            dir: name.clone(),
            last_activity: folder_name_to_timestamp(name),
            name: meta.name,
            description: meta.description,
            resumed_from: meta.resumed_from,
        });
    }
    items
}

async fn get_sessions(State(state): State<AppState>) -> Response {
    let items = list_sessions(&state.sessions_root).await;
    (StatusCode::OK, Json(items)).into_response()
}

// ---------------------------------------------------------------------------
// Session construction (shared between POST /api/sessions and `reset`)
// ---------------------------------------------------------------------------

/// Create a brand-new session on disk + an `ActiveSession` ready to be
/// installed in the slot.  Shared by `POST /api/sessions` and the `reset`
/// WebSocket frame.  Returns `(session, dir_name)`.
async fn create_active_session(state: &AppState) -> Result<(ActiveSession, String), String> {
    let paths = omega_store::make_session_dir(&state.sessions_root)
        .await
        .map_err(|e| format!("make_session_dir failed: {e}"))?;

    let dir_name = paths
        .dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    let context_store = ContextStore::new(paths.context_file.clone());
    let event_store = EventStore::new(paths.events_file.clone());
    let cwd = std::env::current_dir().unwrap_or_default();
    let config = AgentConfig {
        model: "claude-sonnet-4-6".to_owned(),
        cwd,
        system_prompt_append: None,
        session_dir: paths.dir.clone(),
    };
    let agent = Agent::new(
        Arc::clone(&state.provider),
        context_store,
        event_store,
        config,
    );

    agent
        .init()
        .await
        .map_err(|e| format!("agent.init() failed: {e}"))?;

    let controls = agent.controls();
    let session = ActiveSession {
        agent: Arc::new(tokio::sync::Mutex::new(agent)),
        controls,
        paths,
        ws_tx: None,
    };
    Ok((session, dir_name))
}

// ---------------------------------------------------------------------------
// `POST /api/sessions`
// ---------------------------------------------------------------------------

async fn post_session(State(state): State<AppState>) -> Response {
    match create_active_session(&state).await {
        Ok((session, dir_name)) => {
            *state.active_session.lock().await = Some(session);
            (
                StatusCode::CREATED,
                Json(serde_json::json!({ "dir": dir_name })),
            )
                .into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

// ---------------------------------------------------------------------------
// `GET /ws` — WebSocket upgrade (Phase 1e.2)
// ---------------------------------------------------------------------------

/// Frames a connected client may send.
///
/// `#[serde(tag = "type", rename_all = "snake_case")]` — frame discriminator
/// matches the literals listed in the Phase 1e.2 task spec
/// (`user_message`, `pause`, `continue`, `abort`, `reset`).
#[derive(Debug, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientFrame {
    /// Send a user message → drive one agent turn.
    UserMessage { content: String },
    /// Pause the in-flight turn at the next pause seam.
    Pause,
    /// Resume a paused turn, optionally injecting `content` as a user message.
    #[serde(rename = "continue")]
    Continue {
        #[serde(default)]
        content: Option<String>,
    },
    /// Cancel the in-flight turn.
    Abort,
    /// Drop any prior session and create a fresh one on the same WS.
    Reset,
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

/// Per-connection driver.
///
/// 1. Build a fresh `mpsc::UnboundedSender<WsMessage>`; spawn a writer
///    task that drains the receiver into the WS sink.
/// 2. If a session already exists, install `tx` into its `ws_tx` slot.
/// 3. Send `WsMessage::Ready`.
/// 4. Read loop: parse client frames, dispatch.  Handler errors emit a
///    `WsMessage::AgentError` frame instead of closing the socket.
/// 5. On disconnect, clear `ws_tx` from the slot (best-effort).
async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sink, mut reader) = socket.split();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<WsMessage>();

    // Writer task — drains rx → ws sink.
    let writer = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            let text = msg.to_text();
            if sink.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
        // Best-effort close — ignore errors.
        let _ = sink.close().await;
    });

    // Install tx into the session slot if we already have one.
    install_ws_tx(&state, tx.clone()).await;

    // Initial ready frame.
    let _ = tx.send(WsMessage::Ready);

    // Read loop.
    while let Some(frame) = reader.next().await {
        let Ok(frame) = frame else { break };
        let text = match frame {
            Message::Text(t) => t.to_string(),
            Message::Close(_) => break,
            // Binary / Ping / Pong are ignored — TS server only speaks JSON text.
            _ => continue,
        };
        if let Err(e) = dispatch_text_frame(&text, &state, &tx).await {
            let _ = tx.send(WsMessage::AgentError(e));
        }
    }

    // Disconnect cleanup: drop our reference to the slot's tx.
    clear_ws_tx(&state).await;
    drop(tx);
    let _ = writer.await;
}

/// Set `slot.ws_tx = Some(tx)` if a session exists.  No-op otherwise.
async fn install_ws_tx(state: &AppState, tx: UnboundedSender<WsMessage>) {
    let mut slot = state.active_session.lock().await;
    if let Some(active) = slot.as_mut() {
        active.ws_tx = Some(tx);
    }
}

/// Clear the slot's `ws_tx` on disconnect.
async fn clear_ws_tx(state: &AppState) {
    let mut slot = state.active_session.lock().await;
    if let Some(active) = slot.as_mut() {
        active.ws_tx = None;
    }
}

/// Parse one text frame and dispatch it.  Errors bubble back to the
/// caller, which forwards them as a `WsMessage::AgentError` frame.
async fn dispatch_text_frame(
    text: &str,
    state: &AppState,
    tx: &UnboundedSender<WsMessage>,
) -> Result<(), String> {
    let frame: ClientFrame =
        serde_json::from_str(text).map_err(|e| format!("invalid client frame: {e}"))?;
    dispatch_client_frame(frame, state, tx).await
}

async fn dispatch_client_frame(
    frame: ClientFrame,
    state: &AppState,
    tx: &UnboundedSender<WsMessage>,
) -> Result<(), String> {
    match frame {
        ClientFrame::UserMessage { content } => handle_user_message(content, state, tx).await,
        ClientFrame::Pause => handle_pause(state).await,
        ClientFrame::Continue { content } => handle_continue(state, content).await,
        ClientFrame::Abort => handle_abort(state).await,
        ClientFrame::Reset => handle_reset(state, tx).await,
    }
}

/// Spawn a task that drives one agent turn and forwards every yielded
/// item through `tx`.  We don't await the task: pause/continue/abort
/// frames must be processable while the turn is in flight.
async fn handle_user_message(
    content: String,
    state: &AppState,
    tx: &UnboundedSender<WsMessage>,
) -> Result<(), String> {
    let agent = {
        let slot = state.active_session.lock().await;
        let Some(active) = slot.as_ref() else {
            return Err("no active session — send `reset` first".to_owned());
        };
        Arc::clone(&active.agent)
    };

    let tx_for_turn = tx.clone();
    tokio::spawn(async move {
        let mut guard = agent.lock().await;
        let cancel = CancellationToken::new();
        let mut stream = guard.send_message(content, cancel);
        while let Some(item) = stream.next().await {
            if tx_for_turn.send(WsMessage::Item(Box::new(item))).is_err() {
                // Receiver gone — client disconnected.  Drain the stream
                // so the agent finishes and persists events to disk.
                while stream.next().await.is_some() {}
                break;
            }
        }
    });
    Ok(())
}

async fn handle_pause(state: &AppState) -> Result<(), String> {
    let controls = {
        let slot = state.active_session.lock().await;
        slot.as_ref().map(|a| a.controls.clone())
    };
    if let Some(controls) = controls {
        controls.request_pause().await;
    }
    Ok(())
}

async fn handle_continue(state: &AppState, content: Option<String>) -> Result<(), String> {
    let controls = {
        let slot = state.active_session.lock().await;
        slot.as_ref().map(|a| a.controls.clone())
    };
    if let Some(controls) = controls {
        controls.request_continue(content);
    }
    Ok(())
}

async fn handle_abort(state: &AppState) -> Result<(), String> {
    let controls = {
        let slot = state.active_session.lock().await;
        slot.as_ref().map(|a| a.controls.clone())
    };
    if let Some(controls) = controls {
        controls.request_abort();
    }
    Ok(())
}

/// Drop any existing session, build a fresh one, install `tx` into its
/// slot, and emit a new `Ready`.
async fn handle_reset(state: &AppState, tx: &UnboundedSender<WsMessage>) -> Result<(), String> {
    // Tell any in-flight turn to wind down so the orphan agent doesn't
    // keep using the cwd / disk paths after we replace the slot.
    {
        let slot = state.active_session.lock().await;
        if let Some(active) = slot.as_ref() {
            active.controls.request_abort();
        }
    }

    let (mut session, _dir_name) = create_active_session(state).await?;
    session.ws_tx = Some(tx.clone());
    *state.active_session.lock().await = Some(session);
    let _ = tx.send(WsMessage::Ready);
    Ok(())
}

// ---------------------------------------------------------------------------
// Unit tests for the pure helpers
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

    use super::{ClientFrame, folder_name_to_timestamp};

    #[test]
    fn timestamp_conversion_with_millis() {
        assert_eq!(
            folder_name_to_timestamp("2025-07-11T09-14-22-037-a8c3f1b2"),
            "2025-07-11T09:14:22.037Z"
        );
    }

    #[test]
    fn timestamp_conversion_without_millis() {
        assert_eq!(
            folder_name_to_timestamp("2025-07-11T09-14-22"),
            "2025-07-11T09:14:22Z"
        );
    }

    #[test]
    fn timestamp_conversion_passthrough_for_non_matching() {
        assert_eq!(folder_name_to_timestamp("not-a-date"), "not-a-date");
    }

    #[test]
    fn client_frame_user_message_parses() {
        let f: ClientFrame =
            serde_json::from_str(r#"{"type":"user_message","content":"hi"}"#).unwrap();
        match f {
            ClientFrame::UserMessage { content } => assert_eq!(content, "hi"),
            other => panic!("expected UserMessage, got {other:?}"),
        }
    }

    #[test]
    fn client_frame_pause_parses() {
        let f: ClientFrame = serde_json::from_str(r#"{"type":"pause"}"#).unwrap();
        assert!(matches!(f, ClientFrame::Pause));
    }

    #[test]
    fn client_frame_continue_with_content_parses() {
        let f: ClientFrame =
            serde_json::from_str(r#"{"type":"continue","content":"go on"}"#).unwrap();
        match f {
            ClientFrame::Continue { content } => assert_eq!(content.as_deref(), Some("go on")),
            other => panic!("expected Continue, got {other:?}"),
        }
    }

    #[test]
    fn client_frame_continue_without_content_parses() {
        let f: ClientFrame = serde_json::from_str(r#"{"type":"continue"}"#).unwrap();
        match f {
            ClientFrame::Continue { content } => assert_eq!(content, None),
            other => panic!("expected Continue, got {other:?}"),
        }
    }

    #[test]
    fn client_frame_abort_parses() {
        let f: ClientFrame = serde_json::from_str(r#"{"type":"abort"}"#).unwrap();
        assert!(matches!(f, ClientFrame::Abort));
    }

    #[test]
    fn client_frame_reset_parses() {
        let f: ClientFrame = serde_json::from_str(r#"{"type":"reset"}"#).unwrap();
        assert!(matches!(f, ClientFrame::Reset));
    }

    #[test]
    fn client_frame_unknown_type_rejected() {
        let r = serde_json::from_str::<ClientFrame>(r#"{"type":"nope"}"#);
        assert!(r.is_err(), "unknown discriminator must be rejected");
    }

    // -----------------------------------------------------------------
    // Direct unit tests for the slot-mutating helpers
    // (`install_ws_tx`, `clear_ws_tx`).  They have no observable
    // side effect from outside the crate yet — 1e.3 history replay
    // will read `ActiveSession::ws_tx` — so we test them directly
    // here to keep mutation testing honest.
    // -----------------------------------------------------------------

    use std::sync::Arc;

    use futures::stream::BoxStream;
    use omega_core::{AgentItem, AgentItemStream, LlmError, LlmRequest, Provider};
    use tempfile::TempDir;

    use super::{clear_ws_tx, create_active_session, install_ws_tx};
    use crate::AppState;
    use crate::ws_message::WsMessage;

    /// Provider stub yielding an empty stream — fine for `Agent::init`,
    /// which never invokes the provider.
    struct EmptyProvider;
    impl Provider for EmptyProvider {
        fn stream(&self, _req: LlmRequest) -> AgentItemStream {
            let s: BoxStream<'static, Result<AgentItem, LlmError>> =
                Box::pin(futures::stream::empty());
            s
        }
    }

    fn test_state(tmp: &TempDir) -> AppState {
        AppState::new(
            Arc::new(EmptyProvider),
            tmp.path().join("sessions"),
            tmp.path().to_path_buf(),
        )
    }

    #[tokio::test]
    async fn install_ws_tx_sets_slot_when_session_present() {
        let tmp = TempDir::new().unwrap();
        let state = test_state(&tmp);

        let (session, _) = create_active_session(&state).await.unwrap();
        *state.active_session.lock().await = Some(session);

        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<WsMessage>();
        install_ws_tx(&state, tx).await;

        let slot = state.active_session.lock().await;
        let active = slot.as_ref().expect("session must still be present");
        assert!(
            active.ws_tx.is_some(),
            "install_ws_tx must populate ws_tx when a session exists",
        );
    }

    #[tokio::test]
    async fn install_ws_tx_is_noop_when_slot_empty() {
        let tmp = TempDir::new().unwrap();
        let state = test_state(&tmp);

        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<WsMessage>();
        install_ws_tx(&state, tx).await;

        let slot = state.active_session.lock().await;
        assert!(
            slot.is_none(),
            "install_ws_tx must not create a session when slot is empty",
        );
    }

    #[tokio::test]
    async fn clear_ws_tx_resets_slot_to_none_when_session_present() {
        let tmp = TempDir::new().unwrap();
        let state = test_state(&tmp);

        let (mut session, _) = create_active_session(&state).await.unwrap();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<WsMessage>();
        session.ws_tx = Some(tx);
        *state.active_session.lock().await = Some(session);

        clear_ws_tx(&state).await;

        let slot = state.active_session.lock().await;
        let active = slot.as_ref().expect("session must still be present");
        assert!(
            active.ws_tx.is_none(),
            "clear_ws_tx must reset ws_tx to None",
        );
    }

    #[tokio::test]
    async fn clear_ws_tx_is_noop_when_slot_empty() {
        let tmp = TempDir::new().unwrap();
        let state = test_state(&tmp);

        // Just must not panic / not create a session.
        clear_ws_tx(&state).await;
        assert!(state.active_session.lock().await.is_none());
    }
}
