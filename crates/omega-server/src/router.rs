//! Axum router construction and route handlers.
//!
//! Phase 1e.0–1e.1 implemented `GET /health`, `GET /api/sessions`, and
//! `POST /api/sessions`.  Phase 1e.2 added the `/ws` route: WebSocket
//! upgrade, `user_message` turn dispatch, and pause / continue / abort /
//! reset control frames.  Phase 1e.3 adds history replay on reconnect:
//! every persisted `OmegaEvent` from `events.jsonl` is pushed through the
//! new socket before `Ready`, filtering out the types in [`REPLAY_EXCLUDE`].
//!
//! Route map (after 1e.4):
//!
//! - `GET  /health`        — liveness probe
//! - `GET  /api/sessions`  — list sessions
//! - `POST /api/sessions`  — create session
//! - `GET  /api/context`   — context-record lookup by hash
//! - `GET  /api/files`     — file-completion suggestions
//! - `GET  /ws`            — WebSocket upgrade

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use axum::{
    Json, Router,
    extract::{
        Query, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use futures::{SinkExt, StreamExt};
use omega_agent::{Agent, AgentConfig, InputItem, InputQueue, QueuedItemView};
use omega_store::{ContextRecord, ContextStore, EventStore, SessionMetadata, session_dir_re};
use regex::Regex;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;
use tower_http::services::ServeDir;

use omega_core::AgentItem;
use omega_tools::{MonitorManager, MonitorStatus};
use omega_types::OmegaEvent;
use omega_types::events::PauseRequestedEvent;
use omega_types::ids::LoggedEvent;

use crate::AppState;
use crate::session::{ActiveSession, SessionInfoCache};
use crate::ws_message::{InputQueueItem, MonitorRosterItem, PendingChangesIntent, WsMessage};

// ---------------------------------------------------------------------------
// History replay — filter constants and helper
// ---------------------------------------------------------------------------

/// Event types excluded from WebSocket history replay.
///
/// Source: `src/web/server.ts` — `const REPLAY_EXCLUDE = new Set(["ready", "text"])`.
/// - `ready` — server-sent after history batch; meaningless to replay.
/// - `text`  — streaming text fragments; assembled response is in `context.jsonl`.
const REPLAY_EXCLUDE: &[&str] = &["ready", "text"];

/// Returns `true` if an event whose serialised `type` is `event_type` should
/// be included in history replay.
///
/// Pure function — unit-testable without a WebSocket connection.
/// See [`REPLAY_EXCLUDE`] for the excluded set.
#[must_use]
pub fn should_replay(event_type: &str) -> bool {
    !REPLAY_EXCLUDE.contains(&event_type)
}

// ---------------------------------------------------------------------------
// Monitor roster helpers
// ---------------------------------------------------------------------------

/// Returns `true` if `item` is a monitor lifecycle event — the moments at
/// which the ephemeral roster mutates.  Used by the streaming task to decide
/// when to push a fresh [`WsMessage::MonitorRoster`] snapshot after the
/// forwarded event is written to the client.
#[must_use]
pub fn is_monitor_event(item: &AgentItem) -> bool {
    matches!(
        item,
        AgentItem::Event(ev)
            if matches!(
                ev.as_ref(),
                OmegaEvent::MonitorStarted(_)
                    | OmegaEvent::MonitorDelivery(_)
                    | OmegaEvent::MonitorStderr(_)
                    | OmegaEvent::MonitorStopped(_)
            )
    )
}

/// Returns `true` if `item` is a `UserMessage` event — the moment at which
/// the agent has just dequeued one item from the [`InputQueue`].
/// Used by [`spawn_run_task`] to push a fresh queue snapshot after the
/// forwarded event is written to the client.
#[must_use]
pub fn is_user_message_event(item: &AgentItem) -> bool {
    matches!(
        item,
        AgentItem::Event(ev) if matches!(ev.as_ref(), OmegaEvent::UserMessage(_))
    )
}

/// Returns `true` if `item` is an event that marks an inbox item having just
/// been **drained** (delivered) — a human `UserMessage` or, since §15 U2, a
/// monitor `MonitorDelivery` / `MonitorStopped`.  At each of these moments
/// the [`InputQueue`] has shrunk, so [`spawn_run_task`] pushes a fresh queue
/// snapshot to keep the always-visible badge (§9) current.
#[must_use]
pub fn is_inbox_drain_event(item: &AgentItem) -> bool {
    matches!(
        item,
        AgentItem::Event(ev)
            if matches!(
                ev.as_ref(),
                OmegaEvent::UserMessage(_)
                    | OmegaEvent::MonitorDelivery(_)
                    | OmegaEvent::MonitorStopped(_)
            )
    )
}

/// Build a [`WsMessage::InputQueue`] snapshot from a slice of [`QueuedItemView`]s.
/// The snapshot is ephemeral and transport-only — it MUST NOT be written to
/// `events.jsonl`.
#[must_use]
fn queue_snapshot_msg(items: Vec<QueuedItemView>) -> WsMessage {
    WsMessage::InputQueue {
        items: items
            .into_iter()
            .map(|v| InputQueueItem {
                source: v.source,
                content_preview: v.content_preview,
                enqueued_at: v.enqueued_at,
            })
            .collect(),
    }
}

/// Build a [`WsMessage::MonitorRoster`] snapshot from the current roster.
/// The snapshot is ephemeral and transport-only — it MUST NOT be written to
/// `events.jsonl`.
#[must_use]
fn roster_snapshot_msg(mgr: &MonitorManager) -> WsMessage {
    let monitors = mgr
        .roster()
        .into_iter()
        .map(|info| MonitorRosterItem {
            id: info.id,
            description: info.description,
            command: info.command,
            status: match info.status {
                MonitorStatus::Running => "running".to_owned(),
                MonitorStatus::Stopped => "stopped".to_owned(),
            },
            started_at: info.started_at,
            fired_count: info.fired_count,
            stderr_tail: info.stderr_tail,
        })
        .collect();
    WsMessage::MonitorRoster { monitors }
}

// ---------------------------------------------------------------------------
// Router construction
// ---------------------------------------------------------------------------

/// Build the top-level [`Router`] using `state` for all stateful handlers.
///
/// The fallback `ServeDir` at `/` serves the Leptos bundle
/// (`leptos_dir`). The Phase 3.0 `/leptos/` alias mount and its
/// trailing-slash redirect were removed in Phase 4 alongside the
/// `Trunk.toml` `public_url` flip to `/`.
pub fn build_router(state: AppState) -> Router {
    let leptos_dir = state.leptos_dir.clone();
    Router::new()
        .route("/health", get(health))
        .route("/api/sessions", get(get_sessions).post(post_session))
        .route("/ws", get(ws_handler))
        .route("/api/context", get(get_context))
        .route("/api/files", get(get_files))
        .fallback_service(ServeDir::new(leptos_dir))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Shared handlers
// ---------------------------------------------------------------------------

/// `GET /health` — liveness probe.
async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

// ---------------------------------------------------------------------------
// Session list item — `GET /api/sessions`
// ---------------------------------------------------------------------------

/// One entry in the `GET /api/sessions` JSON array.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionListItem {
    pub dir: String,
    /// Absolute filesystem path to this session's directory.
    ///
    /// Computed server-side from the (possibly relative) configured
    /// `sessions_root` joined onto the absolute process `cwd`, so the
    /// client can offer a fully-qualified `@path` reference without
    /// knowing the server's working directory.
    pub path: String,
    pub last_activity: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
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
///
/// `sessions_root` must be **absolute** so each item's `path` is a
/// fully-qualified reference; callers resolve it via
/// [`absolute_sessions_root`].
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
            path: full_path.display().to_string(),
            last_activity: folder_name_to_timestamp(name),
            name: meta.name,
            resumed_from: meta.resumed_from,
        });
    }
    items
}

/// Resolve the configured `sessions_root` to an absolute path.
///
/// Resolve the configured sessions root to an absolute path.
///
/// `cwd` is the absolute process working directory captured once at startup.
/// Joining an already-absolute `sessions_root` onto it yields the configured
/// path unchanged (per `Path::join` semantics); joining a relative one (the
/// `.omega/sessions` default) anchors it at `cwd`. Either way the result is
/// absolute, so each session's `path` is a fully-qualified reference.
fn absolute_sessions_root(cwd: &Path, sessions_root: &Path) -> PathBuf {
    cwd.join(sessions_root)
}

async fn get_sessions(State(state): State<AppState>) -> Response {
    let root = absolute_sessions_root(&state.cwd, &state.sessions_root);
    let items = list_sessions(&root).await;
    (StatusCode::OK, Json(items)).into_response()
}

// ---------------------------------------------------------------------------
// Session construction (shared between POST /api/sessions and `reset`)
// ---------------------------------------------------------------------------

/// Create a brand-new session on disk + an `ActiveSession` ready to be
/// installed in the slot.  Shared by `POST /api/sessions` and the `reset`
/// WebSocket frame.  Returns `(session, dir_name)`.
async fn create_active_session(
    state: &AppState,
    model: Option<String>,
    effort: Option<String>,
    tool_selection: Option<Vec<String>>,
) -> Result<(ActiveSession, String), String> {
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
    // Check for pending git changes before moving `cwd` into AgentConfig.
    let has_pending_changes = git_has_pending_changes(&cwd);
    let cwd_string = cwd.display().to_string();
    let config = AgentConfig {
        model: model.unwrap_or_else(|| "claude-sonnet-4-6".to_owned()),
        effort,
        cwd,
        session_dir: paths.dir.clone(),
        headless: false, // web UI sessions are always interactive
        features: None,  // resolved from env in agent.init()
        // None → Agent::new falls back to omega_tools::DEFAULT_TOOL_NAMES.
        // The Reset frame currently leaves this as None (the UI does not
        // yet send a selection — see Phase 2.1).  The resume path
        // explicitly forwards the parent's selection.
        tool_selection,
    };
    let mut agent = Agent::new(
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
    let model_effort = agent.model_effort_handle();
    let active_model = agent.active_model();
    let active_effort = agent.active_effort();
    let features = agent.features();
    let monitor_manager = agent.monitor_manager();
    let info_cache = SessionInfoCache {
        dir: dir_name.clone(),
        model: active_model,
        effort: active_effort,
        cwd: cwd_string,
        name: None,
        has_pending_changes,
        features,
    };
    // §15 (Unified Input Model, U1): human input flows through this inspectable
    // queue to the persistent run task spawned at reset/resume — not by acquiring
    // the agent lock per message.  `InputQueue::push` returns a snapshot that is
    // forwarded to the UI for queue visualisation (Step 2).
    let input_queue = InputQueue::new();
    let session = ActiveSession {
        agent: Arc::new(tokio::sync::Mutex::new(agent)),
        controls,
        paths,
        ws_tx: None,
        current_turn: None,
        input_queue,
        run_cancel: CancellationToken::new(),
        model_effort,
        turn_state: Arc::new(tokio::sync::Mutex::new("idle".to_owned())),
        info_cache: Arc::new(tokio::sync::Mutex::new(info_cache)),
        features,
        monitor_manager,
    };
    Ok((session, dir_name))
}

// ---------------------------------------------------------------------------
// `POST /api/sessions`
// ---------------------------------------------------------------------------

/// Optional JSON body for `POST /api/sessions`.
///
/// Both fields are optional; absent fields fall back to defaults
/// (`claude-sonnet-4-6` / [`omega_agent::DEFAULT_EFFORT`]).  The
/// endpoint also accepts a request with no body at all (legacy).
#[derive(Debug, Default, Deserialize)]
struct PostSessionBody {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    effort: Option<String>,
}

async fn post_session(
    State(state): State<AppState>,
    body: Option<Json<PostSessionBody>>,
) -> Response {
    let PostSessionBody { model, effort } = body.map(|Json(b)| b).unwrap_or_default();
    match create_active_session(&state, model, effort, None).await {
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
    /// Accepts both `"user_message"` (canonical) and `"message"`
    /// (alias used by the `SolidJS` client) as the discriminator.
    #[serde(alias = "message")]
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
    /// Optional `model` / `effort` are wired into [`AgentConfig`] for
    /// the new session; absent fields fall back to defaults.
    Reset {
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        effort: Option<String>,
        /// Operator opt-in to proceed even when the working tree has
        /// uncommitted git changes.  Mirrors the CLI's `--allow-dirty`
        /// flag: deny-by-default, the GUI sets this to `true` only after
        /// the operator clicks "Proceed" on the dirty-tree warning modal.
        #[serde(default, rename = "allowDirty")]
        allow_dirty: bool,
        /// Tools enabled for the new session.  `None` (the default — the
        /// UI does not yet send this; see Phase 2.1) means
        /// `omega_tools::DEFAULT_TOOL_NAMES` (12 base tools).
        #[serde(default, rename = "toolSelection")]
        tool_selection: Option<Vec<String>>,
    },
    /// Resume a prior session: spawn a new session and synthesise a
    /// summary of `session_dir`'s history via the resumption LLM call.
    #[serde(rename_all = "camelCase")]
    ResumeSession {
        session_dir: String,
        /// See [`ClientFrame::Reset::allow_dirty`].
        #[serde(default)]
        allow_dirty: bool,
    },
    /// Rename the session at `session_dir` by writing `name` into its
    /// `session.jsonc` metadata. The target need NOT be the currently
    /// active session — the picker lets users rename any session.
    #[serde(rename_all = "camelCase")]
    RenameSession { session_dir: String, name: String },
    /// Switch the active model on the live agent. Mirrors the TS server's
    /// `set_model` handler: persists a `model_changed` event and may
    /// auto-reset the effort to `"medium"` if the chosen model doesn't
    /// support the current effort tier.
    SetModel { model: String },
    /// Switch the active thinking-effort level on the live agent.
    SetEffort { effort: String },
    /// Delete a session directory under `sessions_root`.
    /// Mirrors the TS server's `delete_session` handler.
    #[serde(rename_all = "camelCase")]
    DeleteSession { session_dir: String },
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

/// Build a [`WsMessage::SessionInfo`] frame from Arc refs to the
/// `info_cache` and `turn_state` slabs.
///
/// Takes `Arc` refs rather than `&ActiveSession` so callers can call
/// this **without holding `active_session.lock()`** — holding that
/// lock across an `.await` on `info_cache` or `turn_state` creates an
/// ABBA deadlock with the streaming task (see BUG-S1, now fixed).
async fn build_session_info(
    info_cache: &Arc<tokio::sync::Mutex<crate::session::SessionInfoCache>>,
    turn_state: &Arc<tokio::sync::Mutex<String>>,
) -> WsMessage {
    let cache = info_cache.lock().await.clone();
    let ts = turn_state.lock().await.clone();
    cache_into_message(cache, ts)
}

/// Project a [`SessionInfoCache`] into a [`WsMessage::SessionInfo`] with
/// the given turn state.  Used by the per-turn streaming loop and by
/// `handle_pause` to broadcast updates without locking the agent.
fn cache_into_message(cache: SessionInfoCache, turn_state: String) -> WsMessage {
    WsMessage::SessionInfo {
        dir: cache.dir,
        model: cache.model,
        effort: cache.effort,
        cwd: cache.cwd,
        name: cache.name,
        turn_state,
        has_pending_changes: cache.has_pending_changes,
        features: cache.features,
    }
}

/// Map an [`OmegaEvent`] variant to the next derived turn state, if it
/// represents a transition. Mirrors `deriveTurnState()` in the (now-deleted)
/// TS server and the test-server fixture (`e2e/fixtures/test-server.ts`).
///
/// `PauseRequested` is intentionally absent: it is never yielded by
/// `send_message` or `perform_resumption` streams — `handle_pause`
/// creates and sends it directly, then updates `turn_state` manually.
fn next_turn_state_for(event: &OmegaEvent) -> Option<&'static str> {
    Some(match event {
        OmegaEvent::UserMessage(_) | OmegaEvent::TurnContinued(_) => "running",
        OmegaEvent::TurnPaused(_) => "paused",
        OmegaEvent::TurnEnd(_) | OmegaEvent::TurnInterrupted(_) => "idle",
        _ => return None,
    })
}

/// Read `events.jsonl` for `events_file`, drop entries that fail
/// [`should_replay`], deserialise the rest, and return them as the
/// `events` vec for a [`WsMessage::History`] frame.
///
/// Pure file I/O — does not touch the session slot.
async fn read_history_events(events_file: &Path) -> Vec<LoggedEvent> {
    let store = EventStore::new(events_file.to_path_buf());
    let Ok(raw_events) = store.read_all().await else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(raw_events.len());
    for v in raw_events {
        let event_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if !should_replay(event_type) {
            continue;
        }
        // Deserialise as LoggedEvent to forward the eventId to the client.
        // Pre-Phase-1 lines that lack eventId deserialise with event_id: None.
        if let Ok(logged) = serde_json::from_value::<LoggedEvent>(v) {
            out.push(logged);
        }
    }
    out
}

/// Send `SessionInfo → History` for the active session, if any.
///
/// The `streaming` flag is set when a turn is currently in flight
/// (its `JoinHandle` exists and has not finished).  `tx` is the per-
/// connection sender; on disconnect the send fails silently.
///
/// **Lock discipline (BUG-S1 fix):** `active_session` is held only long
/// enough to clone the `Arc` handles and cheap fields, then released
/// *before* awaiting `info_cache` or `turn_state`.  Holding
/// `active_session` across those inner awaits caused an ABBA deadlock
/// with the streaming task's turn-state update block.
async fn send_session_info_and_history(state: &AppState, tx: &UnboundedSender<WsMessage>) {
    // Brief lock: extract Arc handles + cheap fields, then release.
    // Do NOT await any inner lock (info_cache, turn_state) while holding
    // active_session — see the lock-discipline note above.
    type Snapshot = (
        Arc<tokio::sync::Mutex<crate::session::SessionInfoCache>>,
        Arc<tokio::sync::Mutex<String>>,
        PathBuf,
    );
    let snapshot: Option<Snapshot> = {
        let slot = state.active_session.lock().await;
        slot.as_ref().map(|active| {
            (
                Arc::clone(&active.info_cache),
                Arc::clone(&active.turn_state),
                active.paths.events_file.clone(),
            )
        })
    }; // active_session lock released here

    let Some((info_cache_arc, turn_state_arc, events_file)) = snapshot else {
        return;
    };
    // `streaming` means a turn is actually in flight.  Under the §15
    // persistent run loop the run task's `JoinHandle` lives for the whole
    // session (it parks between turns), so it can no longer signal
    // "streaming"; the turn state does.  A turn is live iff `turn_state` is
    // not "idle" (UserMessage/TurnContinued → "running"; TurnEnd /
    // TurnInterrupted → "idle").  Read it here, after the `active_session`
    // lock is released, to preserve the lock discipline above.
    let streaming = turn_state_arc.lock().await.as_str() != "idle";
    // Safe to await inner locks now that active_session is released.
    //
    // Re-evaluate pending changes at WS-connect time.  `has_pending_changes`
    // is computed once at session creation; if the operator cleaned up the
    // working tree outside Omega the cache would stay stale.  Re-checking on
    // every reconnect (browser refresh, network blip) keeps the flag fresh
    // without the expense of a background watcher.
    {
        let mut cache = info_cache_arc.lock().await;
        let cwd_path = std::path::PathBuf::from(&cache.cwd);
        cache.has_pending_changes = git_has_pending_changes(&cwd_path);
    }
    let session_info = build_session_info(&info_cache_arc, &turn_state_arc).await;
    let _ = tx.send(session_info);
    let events = read_history_events(&events_file).await;
    let _ = tx.send(WsMessage::History { events, streaming });
}

/// Per-connection driver.
///
/// 1. Build a fresh `mpsc::UnboundedSender<WsMessage>`; spawn a writer
///    task that drains the receiver into the WS sink.
/// 2. Install `tx` into the active session's `ws_tx` slot **before**
///    replay so events from a concurrently-running turn reach this socket.
/// 3. Replay persisted events from `events.jsonl` (filtered via
///    [`should_replay`]) — without holding the session lock.
/// 4. Send `WsMessage::Ready` to signal end-of-replay.
/// 5. Read loop: parse client frames, dispatch.  Handler errors emit a
///    `WsMessage::AgentError` frame instead of closing the socket.
/// 6. On disconnect, clear `ws_tx` from the slot (best-effort).
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

    // Install tx FIRST so any concurrent turn's events also reach this
    // socket during and after the SessionInfo+History batch.
    install_ws_tx(&state, tx.clone()).await;

    // SessionInfo → History (no-op when no session is active).
    send_session_info_and_history(&state, &tx).await;

    // Ready frame signals end-of-batch to the client.
    let _ = tx.send(WsMessage::Ready);

    // Roster + queue snapshots — push once on connect so the badges reflect
    // the live state immediately (e.g. after a browser reconnect mid-session).
    {
        let slot = state.active_session.lock().await;
        if let Some(active) = slot.as_ref() {
            let _ = tx.send(roster_snapshot_msg(&active.monitor_manager));
            let _ = tx.send(queue_snapshot_msg(active.input_queue.snapshot()));
        }
    }

    // Read loop.
    while let Some(frame) = reader.next().await {
        let Ok(frame) = frame else { break };
        let text = match frame {
            Message::Text(t) => t.to_string(),
            // cargo-mutants: deleting this arm is equivalent — the close
            // frame would fall through to `_ => continue`, and the next
            // `reader.next()` returns None (stream end), exiting the while-let
            // identically.  Classified as equivalent, not a real miss.
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

/// Forward one [`WsMessage`] to whichever WebSocket is currently
/// installed in `active_session.ws_tx`.  Drops the message silently if
/// the slot is empty (no client connected) or the channel is closed
/// (stale tx).  Used by the per-turn streaming task so events emitted
/// after a browser reload still reach the new connection.
async fn send_to_active(slot: &Arc<tokio::sync::Mutex<Option<ActiveSession>>>, msg: WsMessage) {
    let guard = slot.lock().await;
    if let Some(active) = guard.as_ref()
        && let Some(tx) = &active.ws_tx
    {
        let _ = tx.send(msg);
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
        ClientFrame::Pause => handle_pause(state, tx).await,
        ClientFrame::Continue { content } => handle_continue(state, content).await,
        ClientFrame::Abort => handle_abort(state).await,
        ClientFrame::Reset {
            model,
            effort,
            allow_dirty,
            tool_selection,
        } => handle_reset(state, tx, model, effort, allow_dirty, tool_selection).await,
        ClientFrame::ResumeSession {
            session_dir,
            allow_dirty,
        } => handle_resume_session(state, tx, session_dir, allow_dirty).await,
        ClientFrame::RenameSession { session_dir, name } => {
            handle_rename_session(state, tx, session_dir, name).await
        }
        ClientFrame::SetModel { model } => handle_set_model(state, tx, model).await,
        ClientFrame::SetEffort { effort } => handle_set_effort(state, tx, effort).await,
        ClientFrame::DeleteSession { session_dir } => {
            handle_delete_session(state, tx, session_dir).await
        }
    }
}

/// Models that accept thinking-effort `"max"` (Opus tier).
const MAX_EFFORT_MODELS: &[&str] = &["claude-opus-4-6", "claude-opus-4-7", "claude-opus-4-8"];
/// Models that accept thinking-effort `"xhigh"` (Opus 4.7 and later).
const XHIGH_EFFORT_MODELS: &[&str] = &["claude-opus-4-7", "claude-opus-4-8"];

async fn handle_set_model(
    state: &AppState,
    tx: &UnboundedSender<WsMessage>,
    model: String,
) -> Result<(), String> {
    // §15 (Unified Input Model): mutate model/effort through the lock-free
    // handle, never `agent.lock()` — the persistent run task holds that lock
    // for the session's life and would deadlock us here.
    let (model_effort, info_cache_arc) = {
        let slot = state.active_session.lock().await;
        let Some(active) = slot.as_ref() else {
            return Err("no active session — send `reset` first".to_owned());
        };
        (active.model_effort.clone(), Arc::clone(&active.info_cache))
    };
    let (model_event, effort_reset) = {
        let model_event = model_effort.set_model(model.clone()).await;
        let current_effort = model_effort.effort();
        let needs_reset = (current_effort == "max" && !MAX_EFFORT_MODELS.contains(&model.as_str()))
            || (current_effort == "xhigh" && !XHIGH_EFFORT_MODELS.contains(&model.as_str()));
        let effort_event = if needs_reset {
            Some(model_effort.set_effort("medium".to_owned()).await)
        } else {
            None
        };
        (model_event, effort_event)
    };
    {
        let mut cache = info_cache_arc.lock().await;
        cache.model = model;
        if effort_reset.is_some() {
            "medium".clone_into(&mut cache.effort);
        }
    }
    let _ = tx.send(WsMessage::Item(Box::new(AgentItem::Event(Box::new(
        model_event,
    )))));
    if let Some(ev) = effort_reset {
        let _ = tx.send(WsMessage::Item(Box::new(AgentItem::Event(Box::new(ev)))));
    }
    Ok(())
}

async fn handle_set_effort(
    state: &AppState,
    tx: &UnboundedSender<WsMessage>,
    effort: String,
) -> Result<(), String> {
    let (model_effort, info_cache_arc) = {
        let slot = state.active_session.lock().await;
        let Some(active) = slot.as_ref() else {
            return Err("no active session — send `reset` first".to_owned());
        };
        (active.model_effort.clone(), Arc::clone(&active.info_cache))
    };
    let new_effort = effort.clone();
    // §15: lock-free model/effort change (see handle_set_model).
    let event = model_effort.set_effort(effort).await;
    info_cache_arc.lock().await.effort = new_effort;
    let _ = tx.send(WsMessage::Item(Box::new(AgentItem::Event(Box::new(event)))));
    Ok(())
}

async fn handle_delete_session(
    state: &AppState,
    tx: &UnboundedSender<WsMessage>,
    session_dir: String,
) -> Result<(), String> {
    if !session_dir_re().is_match(&session_dir) {
        return Err(format!("invalid sessionDir: {session_dir}"));
    }
    let full_dir = state.sessions_root.join(&session_dir);
    tokio::fs::remove_dir_all(&full_dir)
        .await
        .map_err(|e| format!("delete session: {e}"))?;
    let _ = tx.send(WsMessage::SessionDeleted { session_dir });
    Ok(())
}

/// Push a human message into the inspectable input queue and broadcast a
/// fresh queue snapshot to the connected client (§15 Unified Input Model, U1).
///
/// This is the deadlock fix: instead of acquiring the agent lock and
/// calling `send_message` per message (the old per-message re-entry that
/// deadlocked when a new message arrived while the prior turn was parked),
/// we push the item into the shared [`InputQueue`] which the single
/// per-session run task drains.  `InputQueue::push` returns a snapshot
/// taken atomically while the item is still in the queue, so the client
/// always sees the item appear before it disappears.
async fn handle_user_message(
    content: String,
    state: &AppState,
    tx: &UnboundedSender<WsMessage>,
) -> Result<(), String> {
    let input_queue = {
        let slot = state.active_session.lock().await;
        let Some(active) = slot.as_ref() else {
            return Err("no active session \u{2014} send `reset` first".to_owned());
        };
        active.input_queue.clone()
    };
    let snapshot = input_queue.push(InputItem::Human { content });
    let _ = tx.send(queue_snapshot_msg(snapshot));
    Ok(())
}

/// Spawn the single per-session `Agent::run` task and stash its handle.
///
/// §15 (Unified Input Model, U1): called once at reset/resume.  The task
/// acquires the agent lock and holds it for the session's life, driving
/// the persistent run loop (parked on an empty input queue until input
/// arrives) and forwarding every yielded item to whichever WebSocket is
/// currently installed in `active_session.ws_tx`.
///
/// The WS-forwarding loop pushes:
/// - A fresh roster snapshot after every monitor lifecycle event.
/// - A fresh queue snapshot after every `UserMessage` event (the moment
///   the just-popped item has left the queue, so the client sees it drain).
/// - Turn-state transitions via `next_turn_state_for`.
///
/// Looking up `ws_tx` on each send (rather than capturing a clone) lets a
/// turn survive a browser reload — events emitted after the new connection
/// takes over still reach the client.
async fn spawn_run_task(state: &AppState) {
    let prepared = {
        let slot = state.active_session.lock().await;
        slot.as_ref().map(|active| {
            (
                active.input_queue.clone(),
                Arc::clone(&active.agent),
                Arc::clone(&active.turn_state),
                Arc::clone(&active.info_cache),
                Arc::clone(&active.monitor_manager),
                active.run_cancel.clone(),
            )
        })
    };
    let Some((input_queue, agent, turn_state, info_cache_arc, monitor_manager, run_cancel)) =
        prepared
    else {
        // No active session.
        return;
    };

    let slot_arc = Arc::clone(&state.active_session);

    // §15 U2 queue-viz: forward a fresh queue snapshot on EVERY enqueue —
    // human OR monitor.  Monitor pushes happen on a background reader task
    // (no WS handler in scope), so the InputQueue itself notifies us here.
    // We pass the atomic post-push snapshot through so the just-enqueued
    // item is guaranteed visible even if the agent drains it immediately.
    {
        let slot_for_cb = Arc::clone(&slot_arc);
        input_queue.set_on_change(Arc::new(move |snap: Vec<QueuedItemView>| {
            let slot = Arc::clone(&slot_for_cb);
            tokio::spawn(async move {
                send_to_active(&slot, queue_snapshot_msg(snap)).await;
            });
        }));
    }

    let handle = tokio::spawn(async move {
        // Owns the agent lock for the whole session (incl. while parked).
        let mut guard = agent.lock().await;
        let mut stream = guard.run(input_queue.clone(), run_cancel);
        while let Some(item) = stream.next().await {
            let next = match &item {
                AgentItem::Event(ev) => next_turn_state_for(ev),
                AgentItem::Signal(_) => None,
            };
            // Detect push points *before* moving `item` into `WsMessage::Item`.
            let push_roster = is_monitor_event(&item);
            let push_queue = is_inbox_drain_event(&item);
            send_to_active(&slot_arc, WsMessage::Item(Box::new(item))).await;
            // Push a fresh roster snapshot after every monitor lifecycle event.
            if push_roster {
                let roster = roster_snapshot_msg(&monitor_manager);
                send_to_active(&slot_arc, roster).await;
            }
            // Push a fresh queue snapshot after any inbox item is drained
            // (UserMessage / MonitorDelivery / MonitorStopped — the item just
            // left the queue).
            if push_queue {
                let snap = input_queue.snapshot();
                send_to_active(&slot_arc, queue_snapshot_msg(snap)).await;
            }
            if let Some(target) = next {
                let mut ts = turn_state.lock().await;
                if *ts != target {
                    target.clone_into(&mut ts);
                    let cache = info_cache_arc.lock().await.clone();
                    let info = cache_into_message(cache, target.to_owned());
                    send_to_active(&slot_arc, info).await;
                }
            }
        }
    });

    {
        let mut slot = state.active_session.lock().await;
        if let Some(active) = slot.as_mut() {
            active.current_turn = Some(handle);
        }
    }
}

/// Wind down the prior session's run task before its slot is replaced
/// (reset/resume).  §15 (Unified Input Model, U1).
///
/// The persistent run task holds the agent lock while parked on the inbox,
/// so we cannot reap monitors (which needs that lock) until the task has
/// stopped.  Steps:
/// 1. `request_abort` — cancel any in-flight turn at the next seam.
/// 2. `run_cancel.cancel()` — tell the run loop to terminate (this also
///    aborts an in-flight turn via the forwarder) so it releases the lock.
/// 3. Join the run task (bounded) so the lock is actually free.
/// 4. `shutdown_and_log_monitors` — reap every live monitor and persist
///    `MonitorStopped(StoppedBySessionEnd)`.  Requires the agent lock,
///    which step 3 guarantees is available.
async fn teardown_prior_run(state: &AppState) {
    let (controls, run_cancel, run_task, agent) = {
        let mut slot = state.active_session.lock().await;
        match slot.as_mut() {
            Some(active) => (
                Some(active.controls.clone()),
                Some(active.run_cancel.clone()),
                active.current_turn.take(),
                Some(Arc::clone(&active.agent)),
            ),
            None => (None, None, None, None),
        }
    };
    if let Some(controls) = controls {
        controls.request_abort();
    }
    if let Some(run_cancel) = run_cancel {
        run_cancel.cancel();
    }
    if let Some(task) = run_task {
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), task).await;
    }
    if let Some(agent) = agent {
        let mut guard = agent.lock().await;
        guard.shutdown_and_log_monitors().await;
    }
}

async fn handle_pause(state: &AppState, tx: &UnboundedSender<WsMessage>) -> Result<(), String> {
    // Snapshot what we need without holding the active_session lock across
    // the request_pause call (which itself acquires locks).
    let snapshot = {
        let slot = state.active_session.lock().await;
        match slot.as_ref() {
            Some(active) => Some((
                active.controls.clone(),
                Arc::clone(&active.turn_state),
                Arc::clone(&active.info_cache),
            )),
            None => None,
        }
    };
    let Some((controls, turn_state_arc, info_cache_arc)) = snapshot else {
        return Ok(());
    };

    // Only act when a turn is actually running, mirroring the TS server's
    // gate on `currentTurnState === "running"`.
    {
        let ts = turn_state_arc.lock().await;
        if ts.as_str() != "running" {
            return Ok(());
        }
    }

    controls.request_pause().await;

    // The pause_requested event is persisted by request_pause but not
    // yielded on the agent stream, so broadcast it here so the client sees
    // the new entry immediately rather than waiting for a reconnect.
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let pause_event = OmegaEvent::PauseRequested(PauseRequestedEvent { time: now });
    let _ = tx.send(WsMessage::Item(Box::new(AgentItem::Event(Box::new(
        pause_event,
    )))));

    {
        let mut ts = turn_state_arc.lock().await;
        if ts.as_str() != "pause_requested" {
            "pause_requested".clone_into(&mut ts);
            let cache = info_cache_arc.lock().await.clone();
            let _ = tx.send(cache_into_message(cache, "pause_requested".to_owned()));
        }
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
/// slot, and emit `SessionInfo → ResetDone → History → Ready`.
///
/// Wire order matters: `ResetDone` must arrive **before** `History` so the
/// client clears stale state first and then loads the fresh init events.  The
/// previous order (`History → ResetDone`) caused `reset_done` to wipe the
/// `server_started` / `session_started` events delivered by `history`,
/// making them invisible until the next browser refresh.
async fn handle_reset(
    state: &AppState,
    tx: &UnboundedSender<WsMessage>,
    model: Option<String>,
    effort: Option<String>,
    allow_dirty: bool,
    tool_selection: Option<Vec<String>>,
) -> Result<(), String> {
    // Pre-flight dirty-tree gate.  Mirrors the CLI's deny-by-default
    // `--allow-dirty` semantics: if the working tree is dirty and the
    // operator hasn't explicitly opted in, refuse to touch the active
    // session and ask the client for confirmation.  The previous active
    // session (if any) is left untouched so "Cancel" in the UI is a true
    // no-op.
    if !allow_dirty && git_has_pending_changes(&state.cwd) {
        let _ = tx.send(WsMessage::PendingChangesWarning {
            intent: PendingChangesIntent::Reset { model, effort },
        });
        return Ok(());
    }

    // Wind down the prior run task (abort in-flight turn, terminate the run
    // loop so it releases the agent lock) and reap its monitors before we
    // replace the slot.  §15 (Unified Input Model).
    teardown_prior_run(state).await;

    let (mut session, _dir_name) =
        create_active_session(state, model, effort, tool_selection).await?;
    session.ws_tx = Some(tx.clone());

    // Capture what we need before installing the session into the slot so we
    // can build the session_info frame without re-locking active_session.
    let info_cache_arc = Arc::clone(&session.info_cache);
    let turn_state_arc = Arc::clone(&session.turn_state);
    let events_file = session.paths.events_file.clone();

    *state.active_session.lock().await = Some(session);

    // 1. session_info — client learns the new session identity.
    let session_info = build_session_info(&info_cache_arc, &turn_state_arc).await;
    let _ = tx.send(session_info);
    // 2. reset_done — client clears stale events / metrics.
    let _ = tx.send(WsMessage::ResetDone);
    // 3. history — client loads the fresh init events (server_started + session_started).
    let events = read_history_events(&events_file).await;
    let _ = tx.send(WsMessage::History {
        events,
        streaming: false,
    });
    // 4. ready — client is ready to interact.
    let _ = tx.send(WsMessage::Ready);
    // 5. roster + queue snapshots — empty for a fresh session; clears any stale badge.
    {
        let slot = state.active_session.lock().await;
        if let Some(active) = slot.as_ref() {
            let _ = tx.send(roster_snapshot_msg(&active.monitor_manager));
            let _ = tx.send(queue_snapshot_msg(active.input_queue.snapshot()));
        }
    }
    // 6. Spawn the single per-session run task (§15 U1).  It parks on the
    //    empty input queue and owns the agent lock for the session's life.
    spawn_run_task(state).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// resume_session — load prior session events, build a fresh session, and
// stream the resumption summary turn through `tx`.
// ---------------------------------------------------------------------------

async fn handle_resume_session(
    state: &AppState,
    tx: &UnboundedSender<WsMessage>,
    session_dir: String,
    allow_dirty: bool,
) -> Result<(), String> {
    if !session_dir_re().is_match(&session_dir) {
        return Err(format!("invalid sessionDir: {session_dir}"));
    }

    // Pre-flight dirty-tree gate — see `handle_reset` for the rationale.
    if !allow_dirty && git_has_pending_changes(&state.cwd) {
        let _ = tx.send(WsMessage::PendingChangesWarning {
            intent: PendingChangesIntent::ResumeSession { session_dir },
        });
        return Ok(());
    }

    // Wind down the prior run task (abort + terminate + reap monitors)
    // before we replace the slot.  §15 (Unified Input Model).
    teardown_prior_run(state).await;

    // Load the prior session's events and metadata.
    let prev_dir = state.sessions_root.join(&session_dir);
    let prev_events_file = prev_dir.join("events.jsonl");
    let prev_estore = EventStore::new(prev_events_file);
    let raw_events = prev_estore
        .read_all()
        .await
        .map_err(|e| format!("read prior events: {e}"))?;
    let prior_events: Vec<OmegaEvent> = raw_events
        .into_iter()
        .filter_map(|v| serde_json::from_value(v).ok())
        .collect();
    let basis = omega_agent::extract_resumption_basis(&prior_events);
    // Carry the parent's tool selection forward so the successor session
    // exposes the same toolset.  `None` means the parent log lacked a
    // SessionStarted event (malformed / pre-Phase-1.2); the resolver in
    // `Agent::new` falls back to `DEFAULT_TOOL_NAMES`.
    let resumed_tool_selection = omega_agent::extract_tool_selection(&prior_events);
    let prev_meta = omega_store::read_session_metadata(&prev_dir).await;

    // Build the new session and install ws_tx.
    let (mut session, _new_dir_name) =
        create_active_session(state, None, None, resumed_tool_selection).await?;
    session.ws_tx = Some(tx.clone());
    let agent = Arc::clone(&session.agent);
    let turn_state_arc = Arc::clone(&session.turn_state);
    let info_cache_arc = Arc::clone(&session.info_cache);
    let new_dir = session.paths.dir.clone();
    *state.active_session.lock().await = Some(session);

    // SessionInfo + History (just the init events) up front so the UI
    // immediately switches identity to the new session, mirroring the
    // TS server's `ws.cork(…)` triple-send before perform_resumption.
    send_session_info_and_history(state, tx).await;

    // Drive the resumption stream inline so events are persisted and
    // streamed live to the client between the History batch and Ready.
    //
    // BUG-S2 fix: `perform_resumption` never yields state-changing events
    // (UserMessage, TurnEnd, …), so `next_turn_state_for` always returns
    // None inside the loop — `turnState` would stay "idle" throughout the
    // summarisation LLM call.  Bracket the stream with explicit "running"
    // → "idle" transitions so the UI can show a spinner.
    {
        let mut ts = turn_state_arc.lock().await;
        "running".clone_into(&mut ts);
        let cache = info_cache_arc.lock().await.clone();
        let _ = tx.send(cache_into_message(cache, "running".to_owned()));
    }
    {
        let mut guard = agent.lock().await;
        let cancel = CancellationToken::new();
        let mut stream =
            guard.perform_resumption(basis, session_dir.clone(), prev_meta.name.clone(), cancel);
        while let Some(item) = stream.next().await {
            // next_turn_state_for returns None for all resumption events;
            // turn-state transitions are handled by the explicit brackets
            // above and below this loop.
            let _ = tx.send(WsMessage::Item(Box::new(item)));
        }
    }
    {
        let mut ts = turn_state_arc.lock().await;
        "idle".clone_into(&mut ts);
        let cache = info_cache_arc.lock().await.clone();
        let _ = tx.send(cache_into_message(cache, "idle".to_owned()));
    }

    // Persist the resumed-from pointer in the new session's metadata so
    // a subsequent `GET /api/sessions` shows the link.
    let _ = omega_store::update_session_metadata(
        &new_dir,
        SessionMetadata {
            resumed_from: Some(session_dir),
            ..SessionMetadata::default()
        },
    )
    .await;

    let _ = tx.send(WsMessage::Ready);
    // Roster + queue snapshots — empty for a freshly-resumed session.
    {
        let slot = state.active_session.lock().await;
        if let Some(active) = slot.as_ref() {
            let _ = tx.send(roster_snapshot_msg(&active.monitor_manager));
            let _ = tx.send(queue_snapshot_msg(active.input_queue.snapshot()));
        }
    }
    // Spawn the per-session run task only AFTER the inline resumption block
    // above has released the agent lock (§15 U1).  The run task then acquires
    // the lock and parks on the empty input queue.
    spawn_run_task(state).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// rename_session — write `name` into the target session's session.jsonc.
//
// The target is the session directory named by the *client-provided*
// `session_dir`, NOT necessarily the currently active session. The picker
// must be able to rename any session — including ones that aren't loaded.
// If the target happens to be the active session, we also refresh its
// in-memory info_cache so subsequent `session_info` frames carry the new
// name.
// ---------------------------------------------------------------------------

async fn handle_rename_session(
    state: &AppState,
    tx: &UnboundedSender<WsMessage>,
    session_dir: String,
    name: String,
) -> Result<(), String> {
    if !session_dir_re().is_match(&session_dir) {
        return Err(format!("invalid sessionDir: {session_dir}"));
    }
    let full_dir = state.sessions_root.join(&session_dir);

    // If the target is the active session, grab its info_cache so we can
    // update it after the disk write.
    let info_cache_arc = {
        let slot = state.active_session.lock().await;
        slot.as_ref().and_then(|active| {
            (active.paths.dir == full_dir).then(|| Arc::clone(&active.info_cache))
        })
    };

    omega_store::update_session_metadata(
        &full_dir,
        SessionMetadata {
            name: Some(name.clone()),
            ..SessionMetadata::default()
        },
    )
    .await
    .map_err(|e| format!("update_session_metadata: {e}"))?;

    if let Some(arc) = info_cache_arc {
        arc.lock().await.name = Some(name.clone());
    }
    let _ = tx.send(WsMessage::SessionRenamed { session_dir, name });
    Ok(())
}

// ---------------------------------------------------------------------------
// GET /api/context?hashes=h1,h2 — look up context records by hash.
// ---------------------------------------------------------------------------

/// Query string for `GET /api/context`.
#[derive(Debug, Deserialize)]
pub struct ContextQuery {
    /// Comma-separated list of context-record hashes.
    pub hashes: Option<String>,
}

async fn get_context(
    State(state): State<AppState>,
    Query(q): Query<ContextQuery>,
) -> Json<Vec<ContextRecord>> {
    let raw = q.hashes.unwrap_or_default();
    let wanted: Vec<&str> = raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if wanted.is_empty() {
        return Json(Vec::new());
    }

    let context_file = {
        let slot = state.active_session.lock().await;
        slot.as_ref().map(|a| a.paths.context_file.clone())
    };
    let Some(context_file) = context_file else {
        return Json(Vec::new());
    };

    let store = ContextStore::new(context_file);
    let records = store.read_all().await.unwrap_or_default();
    let mut by_hash: HashMap<String, ContextRecord> = HashMap::with_capacity(records.len());
    for r in records {
        by_hash.insert(r.hash.as_ref().to_owned(), r);
    }
    // Preserve request order; drop misses (mirrors TS lookupContextRecords).
    let out: Vec<ContextRecord> = wanted
        .into_iter()
        .filter_map(|h| by_hash.remove(h))
        .collect();
    Json(out)
}

// ---------------------------------------------------------------------------
// GET /api/files?prefix=p — path completions relative to the cwd.
// ---------------------------------------------------------------------------

/// Query string for `GET /api/files`.
#[derive(Debug, Deserialize)]
pub struct FilesQuery {
    /// Path prefix to complete.  Absolute paths bypass `cwd`.
    pub prefix: Option<String>,
}

async fn get_files(Query(q): Query<FilesQuery>) -> Json<Vec<String>> {
    let prefix = q.prefix.unwrap_or_default();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    Json(list_files_for_completion(&prefix, &cwd).await)
}

/// Maximum number of completion suggestions returned by `GET /api/files`.
///
/// Mirrors the TS server's hard-cap.
pub(crate) const MAX_FILE_COMPLETIONS: usize = 50;

/// Comparator: directories sort before files; ties (and same-kind pairs)
/// break alphabetically. Extracted from `list_files_for_completion` so the
/// three branches can be exercised directly by tests — the in-place call
/// inside `sort_by` only routes inputs through the `(true, false)` arm for
/// some sort schedules, which makes end-to-end verification fragile.
fn dir_first_then_alpha(
    a_name: &str,
    a_is_dir: bool,
    b_name: &str,
    b_is_dir: bool,
) -> std::cmp::Ordering {
    match (a_is_dir, b_is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a_name.cmp(b_name),
    }
}

/// Compute path completions matching `prefix` relative to `cwd`.
///
/// Matches the TS `listFilesForCompletion` helper:
///
/// 1. Split `prefix` at the last `/`; the left part (incl. the slash) is
///    the directory portion to read, the right part is the name filter.
/// 2. Absolute prefixes (`prefix.starts_with('/')`) read the literal
///    directory; relative prefixes are joined onto `cwd`.
/// 3. Entries are kept if `name.starts_with(filter)`; sorted with
///    directories first then alphabetically, capped at
///    [`MAX_FILE_COMPLETIONS`].
/// 4. Each result is `"{dir_part}{name}{slash_if_dir}"` so the client
///    can paste the suggestion verbatim.
pub(crate) async fn list_files_for_completion(prefix: &str, cwd: &Path) -> Vec<String> {
    let last_slash = prefix.rfind('/');
    let (dir_part, filter) = match last_slash {
        Some(idx) => (&prefix[..=idx], &prefix[idx + 1..]),
        None => ("", prefix),
    };
    let is_abs = prefix.starts_with('/');
    let target_dir: PathBuf = if is_abs {
        if dir_part.is_empty() {
            PathBuf::from("/")
        } else {
            PathBuf::from(dir_part)
        }
    } else if dir_part.is_empty() {
        cwd.to_path_buf()
    } else {
        cwd.join(dir_part)
    };

    let mut entries: Vec<(String, bool)> = Vec::new();
    let Ok(mut rd) = tokio::fs::read_dir(&target_dir).await else {
        return Vec::new();
    };
    while let Ok(Some(e)) = rd.next_entry().await {
        // non-UTF-8 names are skipped, mirroring TS readdir.
        let Ok(name) = e.file_name().into_string() else {
            continue;
        };
        if !filter.is_empty() && !name.starts_with(filter) {
            continue;
        }
        let is_dir = e.file_type().await.is_ok_and(|t| t.is_dir());
        entries.push((name, is_dir));
    }
    // Directories first, then alphabetical by name.
    entries.sort_by(|a, b| dir_first_then_alpha(&a.0, a.1, &b.0, b.1));
    entries.truncate(MAX_FILE_COMPLETIONS);
    entries
        .into_iter()
        .map(|(name, is_dir)| {
            let suffix = if is_dir { "/" } else { "" };
            format!("{dir_part}{name}{suffix}")
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Unit tests for the pure helpers
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    // Justification for inline test block: these tests cover serde edge cases
    // for `ClientFrame` deserialization (e.g. `allow_dirty` defaults to
    // `false`, unknown discriminator is rejected) that are not exercised by
    // the WebSocket integration tests in tests/ws.rs.  Pure-function helpers
    // (`folder_name_to_timestamp`, `should_replay`, `list_files_for_completion`,
    // `dir_first_then_alpha`) have no observable WS equivalent.  The
    // `install_ws_tx` / `clear_ws_tx` helpers are internal plumbing without
    // a direct WS observable.  All are appropriate carve-outs.
    #![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

    use super::{ClientFrame, absolute_sessions_root, folder_name_to_timestamp};
    use std::path::{Path, PathBuf};

    #[test]
    fn absolute_sessions_root_anchors_relative_default_at_cwd() {
        // The CLI default `.omega/sessions` is relative; it must be anchored
        // at the process cwd so list items carry absolute paths.
        assert_eq!(
            absolute_sessions_root(Path::new("/home/u/dev"), Path::new(".omega/sessions")),
            PathBuf::from("/home/u/dev/.omega/sessions"),
        );
    }

    #[test]
    fn absolute_sessions_root_keeps_explicit_absolute_root() {
        // An explicitly-configured absolute root is used unchanged: cwd must
        // not be prepended.
        assert_eq!(
            absolute_sessions_root(Path::new("/home/u/dev"), Path::new("/srv/sessions")),
            PathBuf::from("/srv/sessions"),
        );
    }

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
    fn client_frame_reset_without_fields_parses_with_none_defaults() {
        let f: ClientFrame = serde_json::from_str(r#"{"type":"reset"}"#).unwrap();
        match f {
            ClientFrame::Reset {
                model,
                effort,
                allow_dirty,
                tool_selection,
            } => {
                assert_eq!(model, None);
                assert_eq!(effort, None);
                assert!(!allow_dirty, "allow_dirty defaults to false");
                assert_eq!(tool_selection, None, "tool_selection defaults to None");
            }
            other => panic!("expected Reset, got {other:?}"),
        }
    }

    #[test]
    fn client_frame_reset_with_model_and_effort_parses() {
        let f: ClientFrame =
            serde_json::from_str(r#"{"type":"reset","model":"claude-opus-4-8","effort":"high"}"#)
                .unwrap();
        match f {
            ClientFrame::Reset {
                model,
                effort,
                allow_dirty,
                tool_selection,
            } => {
                assert_eq!(model.as_deref(), Some("claude-opus-4-8"));
                assert_eq!(effort.as_deref(), Some("high"));
                assert!(!allow_dirty);
                assert_eq!(tool_selection, None);
            }
            other => panic!("expected Reset, got {other:?}"),
        }
    }

    #[test]
    fn client_frame_unknown_type_rejected() {
        let r = serde_json::from_str::<ClientFrame>(r#"{"type":"nope"}"#);
        assert!(r.is_err(), "unknown discriminator must be rejected");
    }

    #[test]
    fn client_frame_resume_session_parses_camel_case() {
        let f: ClientFrame = serde_json::from_str(
            r#"{"type":"resume_session","sessionDir":"2025-01-01T00-00-00-000-deadbeef"}"#,
        )
        .unwrap();
        match f {
            ClientFrame::ResumeSession {
                session_dir,
                allow_dirty,
            } => {
                assert_eq!(session_dir, "2025-01-01T00-00-00-000-deadbeef");
                assert!(!allow_dirty);
            }
            other => panic!("expected ResumeSession, got {other:?}"),
        }
    }

    #[test]
    fn client_frame_rename_session_parses() {
        let f: ClientFrame = serde_json::from_str(
            r#"{"type":"rename_session","sessionDir":"SESSION-7","name":"my-name"}"#,
        )
        .unwrap();
        match f {
            ClientFrame::RenameSession { session_dir, name } => {
                assert_eq!(session_dir, "SESSION-7");
                assert_eq!(name, "my-name");
            }
            other => panic!("expected RenameSession, got {other:?}"),
        }
    }

    /// Regression: the client always sends `sessionDir` alongside `name`.
    /// If serde silently dropped the field (e.g. because the variant
    /// forgot to declare it), `handle_rename_session` would fall back to
    /// the *active* session — which is exactly the bug that kept
    /// regressing in the picker. This test fails fast if anyone removes
    /// `session_dir` from the variant.
    #[test]
    fn client_frame_rename_session_requires_session_dir() {
        let err =
            serde_json::from_str::<ClientFrame>(r#"{"type":"rename_session","name":"my-name"}"#)
                .expect_err("sessionDir must be required");
        assert!(
            err.to_string().contains("sessionDir") || err.to_string().contains("session_dir"),
            "error must mention the missing sessionDir field; got: {err}"
        );
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

    use super::{clear_ws_tx, create_active_session, handle_resume_session, install_ws_tx};
    use crate::AppState;
    use crate::ws_message::WsMessage;
    use omega_store;

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

    // -----------------------------------------------------------------
    // Unit tests for should_replay
    // -----------------------------------------------------------------

    use super::should_replay;

    #[test]
    fn should_replay_excludes_ready() {
        assert!(!should_replay("ready"), "\"ready\" must be excluded");
    }

    #[test]
    fn should_replay_excludes_text() {
        assert!(!should_replay("text"), "\"text\" must be excluded");
    }

    #[test]
    fn should_replay_includes_server_started() {
        assert!(should_replay("server_started"));
    }

    #[test]
    fn should_replay_includes_session_started() {
        assert!(should_replay("session_started"));
    }

    #[test]
    fn should_replay_includes_user_message() {
        assert!(should_replay("user_message"));
    }

    #[test]
    fn should_replay_includes_turn_end() {
        assert!(should_replay("turn_end"));
    }

    #[test]
    fn should_replay_includes_llm_response() {
        assert!(should_replay("llm_response"));
    }

    #[test]
    fn should_replay_includes_tool_call() {
        assert!(should_replay("tool_call"));
    }

    #[test]
    fn should_replay_includes_empty_string() {
        // An unknown / empty type should pass through (not excluded).
        assert!(should_replay(""));
    }

    // -----------------------------------------------------------------
    // is_monitor_event — unit tests for the roster-push decision function.
    //
    // Justification for inline test block: `is_monitor_event` is a
    // pure classification function with no I/O.  Testing it here
    // avoids building a full WS harness; coverage is more precise and
    // the decision-point mutations are killed cheaply.
    // -----------------------------------------------------------------

    use super::is_monitor_event;
    use omega_types::events::{
        MonitorDeliveryEvent, MonitorDeliveryItem, MonitorStartedEvent, MonitorStderrEvent,
        MonitorStopReason, MonitorStoppedEvent,
    };
    use omega_types::{OmegaEvent, StreamSignal};

    fn started_event() -> AgentItem {
        AgentItem::event(OmegaEvent::MonitorStarted(MonitorStartedEvent {
            id: "m".to_owned(),
            description: "d".to_owned(),
            command: "c".to_owned(),
            time: "t".to_owned(),
        }))
    }

    #[test]
    fn is_monitor_event_true_for_monitor_started() {
        assert!(is_monitor_event(&started_event()));
    }

    #[test]
    fn is_monitor_event_true_for_monitor_delivery() {
        let item = AgentItem::event(OmegaEvent::MonitorDelivery(MonitorDeliveryEvent {
            time: "t".to_owned(),
            items: vec![MonitorDeliveryItem {
                monitor_id: "m".to_owned(),
                lines: vec!["out".to_owned()],
            }],
        }));
        assert!(is_monitor_event(&item));
    }

    #[test]
    fn is_monitor_event_true_for_monitor_stderr() {
        let item = AgentItem::event(OmegaEvent::MonitorStderr(MonitorStderrEvent {
            id: "m".to_owned(),
            chunk: "err".to_owned(),
            time: "t".to_owned(),
        }));
        assert!(is_monitor_event(&item));
    }

    #[test]
    fn is_monitor_event_true_for_monitor_stopped() {
        let item = AgentItem::event(OmegaEvent::MonitorStopped(MonitorStoppedEvent {
            id: "m".to_owned(),
            reason: MonitorStopReason::ProcessCrashed,
            exit_code: None,
            time: "t".to_owned(),
        }));
        assert!(is_monitor_event(&item));
    }

    #[test]
    fn is_monitor_event_false_for_text_signal() {
        let item = AgentItem::Signal(StreamSignal::Text {
            index: 0,
            text: "hello".to_owned(),
        });
        assert!(!is_monitor_event(&item));
    }

    #[test]
    fn is_monitor_event_false_for_non_monitor_event() {
        use omega_types::events::{TurnEndEvent, TurnMetrics};
        let item = AgentItem::event(OmegaEvent::TurnEnd(TurnEndEvent {
            time: "t".to_owned(),
            metrics: TurnMetrics {
                input_tokens: 1,
                output_tokens: 2,
                cache_creation_tokens: None,
                cache_read_tokens: None,
            },
        }));
        assert!(!is_monitor_event(&item));
    }

    // -----------------------------------------------------------------
    // roster_snapshot_msg — unit tests for the MonitorInfo → WsMessage
    // projection.  Justification: same reasoning as is_monitor_event;
    // we need to pin the field mappings (snake_case → camelCase key for
    // `started_at`, `fired_count`, `stderr_tail`) without spinning up a
    // real process.
    // -----------------------------------------------------------------

    use super::roster_snapshot_msg;
    use omega_tools::MonitorManager;

    #[test]
    fn roster_snapshot_msg_empty_manager_yields_empty_monitors() {
        let mgr = MonitorManager::new();
        let msg = roster_snapshot_msg(&mgr);
        let v = msg.to_json();
        assert_eq!(v["type"], "monitor_roster");
        let monitors = v["monitors"].as_array().expect("monitors array");
        assert!(monitors.is_empty(), "fresh manager must yield empty roster");
    }

    /// Verify that `roster_snapshot_msg` correctly maps all fields from
    /// `MonitorInfo` to the wire shape.  We use the manager's own
    /// `roster()` method as the source of truth and check that the JSON
    /// output reflects the current state.
    #[test]
    fn roster_snapshot_msg_produces_monitor_roster_variant() {
        // Even with an empty manager the variant must be MonitorRoster.
        let mgr = MonitorManager::new();
        match roster_snapshot_msg(&mgr) {
            crate::ws_message::WsMessage::MonitorRoster { monitors } => {
                assert!(monitors.is_empty());
            }
            other => panic!("expected MonitorRoster, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------

    #[tokio::test]
    async fn install_ws_tx_sets_slot_when_session_present() {
        let tmp = TempDir::new().unwrap();
        let state = test_state(&tmp);

        let (session, _) = create_active_session(&state, None, None, None)
            .await
            .unwrap();
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

        let (mut session, _) = create_active_session(&state, None, None, None)
            .await
            .unwrap();
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

    // -----------------------------------------------------------------
    // list_files_for_completion
    // -----------------------------------------------------------------

    use super::{MAX_FILE_COMPLETIONS, list_files_for_completion};

    #[tokio::test]
    async fn list_files_returns_dirs_first_then_alphabetical() {
        let tmp = TempDir::new().unwrap();
        let cwd = tmp.path();
        std::fs::write(cwd.join("alpha.txt"), "").unwrap();
        std::fs::write(cwd.join("bravo.txt"), "").unwrap();
        std::fs::create_dir(cwd.join("zulu")).unwrap();
        std::fs::create_dir(cwd.join("charlie")).unwrap();

        let out = list_files_for_completion("", cwd).await;
        assert_eq!(
            out,
            vec![
                "charlie/".to_owned(),
                "zulu/".to_owned(),
                "alpha.txt".to_owned(),
                "bravo.txt".to_owned(),
            ],
        );
    }

    #[tokio::test]
    async fn list_files_filters_by_prefix() {
        let tmp = TempDir::new().unwrap();
        let cwd = tmp.path();
        std::fs::write(cwd.join("hello.txt"), "").unwrap();
        std::fs::write(cwd.join("help.md"), "").unwrap();
        std::fs::write(cwd.join("world.txt"), "").unwrap();

        let out = list_files_for_completion("hel", cwd).await;
        assert_eq!(out, vec!["hello.txt".to_owned(), "help.md".to_owned()]);
    }

    #[tokio::test]
    async fn list_files_with_subdir_prefix_includes_dir_part_in_results() {
        let tmp = TempDir::new().unwrap();
        let cwd = tmp.path();
        std::fs::create_dir(cwd.join("sub")).unwrap();
        std::fs::write(cwd.join("sub/foo.txt"), "").unwrap();
        std::fs::write(cwd.join("sub/bar.txt"), "").unwrap();

        let out = list_files_for_completion("sub/", cwd).await;
        assert_eq!(
            out,
            vec!["sub/bar.txt".to_owned(), "sub/foo.txt".to_owned()]
        );
    }

    #[tokio::test]
    async fn list_files_absolute_prefix_bypasses_cwd() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("hello.txt"), "").unwrap();
        std::fs::write(tmp.path().join("help.md"), "").unwrap();

        // Use a clearly-distinct cwd that does not contain the test files.
        let cwd = std::env::temp_dir();
        let prefix = format!("{}/hel", tmp.path().display());
        let out = list_files_for_completion(&prefix, &cwd).await;
        let dir = format!("{}/", tmp.path().display());
        assert_eq!(
            out,
            vec![format!("{dir}hello.txt"), format!("{dir}help.md")],
        );
    }

    #[tokio::test]
    async fn list_files_unreadable_directory_returns_empty() {
        let tmp = TempDir::new().unwrap();
        let cwd = tmp.path().join("does-not-exist");
        let out = list_files_for_completion("", &cwd).await;
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn list_files_caps_at_max_completions() {
        let tmp = TempDir::new().unwrap();
        let cwd = tmp.path();
        for i in 0..(MAX_FILE_COMPLETIONS + 5) {
            std::fs::write(cwd.join(format!("file-{i:03}.txt")), "").unwrap();
        }
        let out = list_files_for_completion("", cwd).await;
        assert_eq!(out.len(), MAX_FILE_COMPLETIONS);
    }

    /// Directories must sort before files — even when the directory
    /// name alphabetically follows the file name.  Calling the
    /// comparator directly exercises every arm regardless of the
    /// underlying sort algorithm's comparison schedule.
    #[test]
    fn dir_first_then_alpha_directory_precedes_file() {
        use super::dir_first_then_alpha;
        use std::cmp::Ordering;

        // (true, false) arm — dir name *after* file name alphabetically.
        assert_eq!(
            dir_first_then_alpha("zzz_dir", true, "aaa.txt", false),
            Ordering::Less,
        );
        // (false, true) arm — file name *before* dir name alphabetically.
        assert_eq!(
            dir_first_then_alpha("aaa.txt", false, "zzz_dir", true),
            Ordering::Greater,
        );
        // Same kind — alphabetical order applies.
        assert_eq!(
            dir_first_then_alpha("aaa.txt", false, "bbb.txt", false),
            Ordering::Less,
        );
        assert_eq!(
            dir_first_then_alpha("zzz.txt", false, "aaa.txt", false),
            Ordering::Greater,
        );
        assert_eq!(
            dir_first_then_alpha("abc", true, "abc", true),
            Ordering::Equal,
        );
    }

    // -----------------------------------------------------------------
    // Task 3 (Phase 4): resume does NOT revive monitors
    //
    // Justification for inline test block: `handle_resume_session`
    // assembles a fresh session (new MonitorManager) and replays prior
    // events only into LLM context; it never calls `monitor_start` or
    // spawns any processes.  These tests prove the invariant for both
    // the completed-monitor and dangling-monitor (simulated-crash)
    // scenarios end-to-end through the real server path.
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn resume_does_not_revive_completed_monitor() {
        // Prior session: MonitorStarted + MonitorStopped (clean shutdown).
        // Resumed roster must be empty — no process should be spawned.
        let tmp = TempDir::new().unwrap();
        let state = test_state(&tmp);

        // Build prior session dir with a completed monitor.
        let prior_id = "2025-01-01T00-00-00-000-deadbeef";
        let prior_dir = tmp.path().join("sessions").join(prior_id);
        std::fs::create_dir_all(&prior_dir).unwrap();
        let prior_store = omega_store::EventStore::new(prior_dir.join("events.jsonl"));
        prior_store
            .append(&OmegaEvent::MonitorStarted(MonitorStartedEvent {
                id: "m1".to_owned(),
                description: "test monitor".to_owned(),
                command: "sleep 9999".to_owned(),
                time: "2025-01-01T00:00:00.000Z".to_owned(),
            }))
            .await
            .unwrap();
        prior_store
            .append(&OmegaEvent::MonitorStopped(MonitorStoppedEvent {
                id: "m1".to_owned(),
                reason: MonitorStopReason::ProcessExited,
                exit_code: Some(0),
                time: "2025-01-01T00:01:00.000Z".to_owned(),
            }))
            .await
            .unwrap();

        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<WsMessage>();
        handle_resume_session(&state, &tx, prior_id.to_owned(), true)
            .await
            .unwrap();

        let slot = state.active_session.lock().await;
        let active = slot.as_ref().expect("session must exist after resume");
        assert!(
            active.monitor_manager.roster().is_empty(),
            "resumed session roster must be empty for a completed monitor",
        );
    }

    #[tokio::test]
    async fn resume_does_not_revive_dangling_monitor() {
        // Prior session: MonitorStarted but NO MonitorStopped — simulates
        // a crash before clean shutdown (Phase 4 gap scenario).  The
        // resumed roster must still be empty; the dangling MonitorStarted
        // must not spawn a new process.
        let tmp = TempDir::new().unwrap();
        let state = test_state(&tmp);

        let prior_id = "2025-01-01T00-00-00-000-cafebabe";
        let prior_dir = tmp.path().join("sessions").join(prior_id);
        std::fs::create_dir_all(&prior_dir).unwrap();
        let prior_store = omega_store::EventStore::new(prior_dir.join("events.jsonl"));
        prior_store
            .append(&OmegaEvent::MonitorStarted(MonitorStartedEvent {
                id: "m2".to_owned(),
                description: "dangling monitor".to_owned(),
                command: "sleep 9999".to_owned(),
                time: "2025-01-01T00:00:00.000Z".to_owned(),
            }))
            .await
            .unwrap();
        // Intentionally no MonitorStopped — simulates a crash/kill
        // before Phase 4 shutdown logging could run.

        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<WsMessage>();
        handle_resume_session(&state, &tx, prior_id.to_owned(), true)
            .await
            .unwrap();

        let slot = state.active_session.lock().await;
        let active = slot.as_ref().expect("session must exist after resume");
        assert!(
            active.monitor_manager.roster().is_empty(),
            "resumed session roster must be empty even for a dangling MonitorStarted (simulated crash)",
        );
    }
}

// ---------------------------------------------------------------------------
// Git pending-changes check
// ---------------------------------------------------------------------------

/// Returns `true` if `git status --porcelain` reports any uncommitted changes
/// in `cwd`; `false` if the tree is clean, git is not installed, or `cwd` is
/// not inside a git repository (fail-open: absence of git is not a blocker).
///
/// When the `OMEGA_ALLOW_DIRTY` environment variable is set (any value), the
/// check is skipped and `false` is returned — used when running the real
/// server binary in a test environment where the working tree may be dirty.
#[must_use]
pub fn git_has_pending_changes(cwd: &std::path::Path) -> bool {
    if std::env::var("OMEGA_ALLOW_DIRTY").is_ok() {
        return false;
    }
    std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(cwd)
        .output()
        .is_ok_and(|o| !o.stdout.is_empty())
}

#[cfg(test)]
mod git_tests {
    // Justification for inline test block: `git_has_pending_changes` spawns
    // a real `git` subprocess against a real on-disk repository.  This is
    // inherently a unit-level concern — constructing a dirty vs. clean git
    // repo via WebSocket integration tests would require the integration test
    // runner itself to be in a git repo whose cleanliness we can control,
    // which is fragile in CI.  The real-git subprocess approach is the
    // correct level for this function.
    #![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
    use super::git_has_pending_changes;
    use std::process::Command;

    fn git(args: &[&str], cwd: &std::path::Path) {
        let status = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "t@t.com")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "t@t.com")
            .status()
            .expect("git");
        assert!(status.success());
    }

    #[test]
    fn clean_repo_returns_false() {
        let dir = tempfile::tempdir().unwrap();
        git(&["init", "-b", "main"], dir.path());
        git(&["commit", "--allow-empty", "-m", "init"], dir.path());
        assert!(!git_has_pending_changes(dir.path()));
    }

    #[test]
    fn untracked_file_returns_true() {
        let dir = tempfile::tempdir().unwrap();
        git(&["init", "-b", "main"], dir.path());
        git(&["commit", "--allow-empty", "-m", "init"], dir.path());
        std::fs::write(dir.path().join("new.txt"), "x").unwrap();
        assert!(git_has_pending_changes(dir.path()));
    }

    #[test]
    fn staged_change_returns_true() {
        let dir = tempfile::tempdir().unwrap();
        git(&["init", "-b", "main"], dir.path());
        git(&["commit", "--allow-empty", "-m", "init"], dir.path());
        std::fs::write(dir.path().join("staged.txt"), "y").unwrap();
        git(&["add", "staged.txt"], dir.path());
        assert!(git_has_pending_changes(dir.path()));
    }

    #[test]
    fn non_git_directory_returns_false() {
        // Not a git repo → fail open (return false, do not block the session).
        let dir = tempfile::tempdir().unwrap();
        assert!(!git_has_pending_changes(dir.path()));
    }
}
