#![allow(
    clippy::similar_names,
    clippy::uninlined_format_args,
    clippy::redundant_closure_for_method_calls
)]

//! Integration tests for the `/ws` WebSocket route (Phase 1e.2 + 1e.3).
//!
//! Each test:
//! 1. Builds an [`AppState`] backed by an in-memory `MockProvider`.
//! 2. Spawns the server on `127.0.0.1:0`.
//! 3. Connects a `tokio-tungstenite` client to `/ws`.
//! 4. Drives client→server frames + collects server→client frames as
//!    `serde_json::Value`.
//! 5. Asserts the observed sequence matches the contract.
//!
//! No live LLM API is touched: the mock replays a pre-arranged transcript
//! per call.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::doc_markdown,
    clippy::too_many_lines
)]

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::{SinkExt, StreamExt, stream::BoxStream};
use omega_agent::{Agent, AgentConfig};
use omega_core::{AgentItem, AgentItemStream, LlmError, LlmRequest, Provider};
use omega_server::{ActiveSession, AppState, build_router};
use omega_store::{ContextStore, EventStore, SessionPaths};
use omega_types::events::{LlmResponseEndedEvent, ToolCallEvent};
use omega_types::{LlmResponseUsage, OmegaEvent, StreamSignal};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message as TMessage;

// ---------------------------------------------------------------------------
// MockProvider — replays a queue of transcripts, one per call.
// ---------------------------------------------------------------------------

struct MockProvider {
    responses: Mutex<VecDeque<Vec<Result<AgentItem, LlmError>>>>,
    /// Optional per-item sleep applied to every yielded item.  Used by the
    /// pause test to give the WS round-trip enough headroom to deliver
    /// `pause` before the agent reaches the post-tool-results seam.
    item_delay: Mutex<Option<Duration>>,
}

impl MockProvider {
    fn new() -> Self {
        Self {
            responses: Mutex::new(VecDeque::new()),
            item_delay: Mutex::new(None),
        }
    }

    fn push(&self, items: Vec<Result<AgentItem, LlmError>>) {
        self.responses.lock().unwrap().push_back(items);
    }

    fn set_item_delay(&self, d: Duration) {
        *self.item_delay.lock().unwrap() = Some(d);
    }
}

impl Provider for MockProvider {
    fn stream(&self, _req: LlmRequest) -> AgentItemStream {
        let items = self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_default();
        let delay = *self.item_delay.lock().unwrap();
        let base = futures::stream::iter(items);
        if let Some(d) = delay {
            let slow: BoxStream<'static, Result<AgentItem, LlmError>> =
                Box::pin(base.then(move |x| async move {
                    tokio::time::sleep(d).await;
                    x
                }));
            slow
        } else {
            let s: BoxStream<'static, Result<AgentItem, LlmError>> = Box::pin(base);
            s
        }
    }
}

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

fn make_test_state(provider: Arc<MockProvider>, sessions_root: PathBuf) -> AppState {
    AppState::new(provider, sessions_root)
}

async fn spawn_server(state: AppState) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = build_router(state);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

type WsClient =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect(addr: SocketAddr) -> WsClient {
    let url = format!("ws://{addr}/ws");
    let (ws, _resp) = tokio_tungstenite::connect_async(url).await.unwrap();
    ws
}

/// Receive the next text frame (with timeout); decode as JSON.
async fn recv_json(ws: &mut WsClient) -> serde_json::Value {
    let frame = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("recv timed out")
        .expect("ws stream ended unexpectedly")
        .expect("ws frame error");
    match frame {
        TMessage::Text(t) => serde_json::from_str(&t).expect("decode json"),
        other => panic!("expected Text frame, got {other:?}"),
    }
}

/// Drain frames until one with `type == want` arrives; return all collected
/// frames including the matching one as the last entry.  Times out after
/// 5 s of silence.
async fn recv_until_type(ws: &mut WsClient, want: &str) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    loop {
        let v = recv_json(ws).await;
        let matched = v.get("type").and_then(|t| t.as_str()) == Some(want);
        out.push(v);
        if matched {
            return out;
        }
    }
}

async fn send_json(ws: &mut WsClient, v: serde_json::Value) {
    ws.send(TMessage::Text(v.to_string().into())).await.unwrap();
}

// ---------------------------------------------------------------------------
// Mock transcript helpers (mirrors crates/omega-agent/tests/common/mod.rs)
// ---------------------------------------------------------------------------

fn llm_response(stop_reason: &str) -> AgentItem {
    AgentItem::event(OmegaEvent::LlmResponseEnded(LlmResponseEndedEvent {
        time: "2024-01-01T00:00:00.000Z".to_owned(),
        stop_reason: stop_reason.to_owned(),
        cleared_tool_uses: None,
        cleared_input_tokens: None,
        usage: LlmResponseUsage {
            input_tokens: 1,
            output_tokens: 1,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
            service_tier: None,
            iterations: None,
        },
        context_hash: String::new(),
        response_summary: None,
    }))
}

fn tool_use_items(
    tool_id: &str,
    tool_name: &str,
    input: serde_json::Value,
) -> Vec<Result<AgentItem, LlmError>> {
    vec![
        Ok(AgentItem::event(OmegaEvent::ToolCall(ToolCallEvent {
            time: "2024-01-01T00:00:00.000Z".to_owned(),
            tool_call_id: tool_id.to_owned(),
            name: tool_name.to_owned(),
            input,
            context_hash: String::new(),
        }))),
        Ok(llm_response("tool_use")),
    ]
}

fn write_scratch(dir: &Path) -> PathBuf {
    let p = dir.join("scratch.txt");
    std::fs::write(&p, "hello").unwrap();
    p
}

// ---------------------------------------------------------------------------
// 1. Happy path — reset + user_message → text frame + turn_end.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn happy_path_user_message_yields_text_and_turn_end() {
    let tmp = TempDir::new().unwrap();
    let provider = Arc::new(MockProvider::new());
    // One LLM call: a Signal::Text "hello" then end_turn LlmResponseEnded.
    provider.push(vec![
        Ok(AgentItem::Signal(StreamSignal::Text {
            index: 0,
            text: "hello".to_owned(),
        })),
        Ok(llm_response("end_turn")),
    ]);

    let state = make_test_state(provider, tmp.path().join("sessions"));
    let addr = spawn_server(state).await;

    let mut ws = connect(addr).await;
    // Initial ready.
    assert_eq!(recv_json(&mut ws).await["type"], "ready");

    // Reset → another ready.
    send_json(
        &mut ws,
        serde_json::json!({ "type": "reset", "allowDirty": true }),
    )
    .await;
    let frames = recv_until_type(&mut ws, "ready").await;
    assert!(
        frames.iter().any(|v| v["type"] == "ready"),
        "expected ready after reset"
    );

    // user_message.
    send_json(
        &mut ws,
        serde_json::json!({ "type": "user_message", "content": "hi" }),
    )
    .await;
    let frames = recv_until_type(&mut ws, "turn_end").await;

    let types: Vec<&str> = frames.iter().filter_map(|v| v["type"].as_str()).collect();
    assert!(
        types.contains(&"text"),
        "expected at least one text frame; got {types:?}"
    );
    assert_eq!(
        types.last().copied(),
        Some("turn_end"),
        "last frame must be turn_end; got {types:?}"
    );
}

// ---------------------------------------------------------------------------
// 2. First reset creates a session when none existed.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn first_reset_creates_session_and_sends_ready() {
    let tmp = TempDir::new().unwrap();
    let sessions_root = tmp.path().join("sessions");
    let provider = Arc::new(MockProvider::new());
    let state = make_test_state(provider, sessions_root.clone());
    let addr = spawn_server(state).await;

    let mut ws = connect(addr).await;
    // Initial ready (no session yet).
    assert_eq!(recv_json(&mut ws).await["type"], "ready");

    // No sessions on disk yet.
    assert!(!sessions_root.exists() || sessions_root.read_dir().unwrap().next().is_none());

    // reset → ready, and a session dir now exists on disk.
    send_json(
        &mut ws,
        serde_json::json!({ "type": "reset", "allowDirty": true }),
    )
    .await;
    let frames = recv_until_type(&mut ws, "ready").await;
    assert!(frames.iter().any(|v| v["type"] == "ready"));

    let mut entries = sessions_root.read_dir().unwrap();
    let entry = entries.next().expect("expected at least one session dir");
    let entry = entry.unwrap();
    assert!(entry.path().join("events.jsonl").is_file());
}

// ---------------------------------------------------------------------------
// 3. Pause during a slow turn → turn_paused; continue resumes.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pause_during_turn_emits_turn_paused_then_continue_resumes() {
    let tmp = TempDir::new().unwrap();
    let scratch = write_scratch(tmp.path());
    let provider = Arc::new(MockProvider::new());
    // 30 ms per item slack so `pause` reaches the server before the agent
    // crosses the post-tool-results seam.  See note in MockProvider.
    provider.set_item_delay(Duration::from_millis(30));
    // Turn 1: tool_use; Turn 2 (after continue): final text.
    provider.push(tool_use_items(
        "tu_1",
        "read_file",
        serde_json::json!({ "path": scratch.to_string_lossy() }),
    ));
    provider.push(vec![Ok(llm_response("end_turn"))]);

    let state = make_test_state(provider, tmp.path().join("sessions"));
    let addr = spawn_server(state).await;

    let mut ws = connect(addr).await;
    assert_eq!(recv_json(&mut ws).await["type"], "ready");

    send_json(
        &mut ws,
        serde_json::json!({ "type": "reset", "allowDirty": true }),
    )
    .await;
    let _ = recv_until_type(&mut ws, "ready").await;

    send_json(
        &mut ws,
        serde_json::json!({ "type": "user_message", "content": "hi" }),
    )
    .await;
    // Drive past the first ToolCall — at that point the agent has entered
    // the loop and the next iteration will hit the post-tool-results seam.
    let _ = recv_until_type(&mut ws, "tool_call").await;

    // Pause.
    send_json(&mut ws, serde_json::json!({ "type": "pause" })).await;
    let frames = recv_until_type(&mut ws, "turn_paused").await;
    assert!(frames.iter().any(|v| v["type"] == "turn_paused"));

    // Continue — turn must resume and end normally.
    send_json(&mut ws, serde_json::json!({ "type": "continue" })).await;
    let frames = recv_until_type(&mut ws, "turn_end").await;
    let types: Vec<&str> = frames.iter().filter_map(|v| v["type"].as_str()).collect();
    assert!(
        types.contains(&"turn_continued"),
        "expected turn_continued: {types:?}"
    );
    assert_eq!(types.last().copied(), Some("turn_end"));
}

// ---------------------------------------------------------------------------
// 4. Abort during a turn → turn_interrupted.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn abort_during_turn_emits_turn_interrupted() {
    let tmp = TempDir::new().unwrap();
    let scratch = write_scratch(tmp.path());
    let provider = Arc::new(MockProvider::new());
    // Slow the LLM stream so `abort` lands before the post-tool-results
    // cancel check.
    provider.set_item_delay(Duration::from_millis(30));
    // Turn 1: tool_use.  Turn 2: end_turn — *if* the agent gets that far.
    // Pushing a natural-completion second turn means the test can only
    // pass when `request_abort` actually cancels the turn; without it
    // the agent runs to `turn_end` and `recv_until_type("turn_interrupted")`
    // times out.
    provider.push(tool_use_items(
        "tu_1",
        "read_file",
        serde_json::json!({ "path": scratch.to_string_lossy() }),
    ));
    provider.push(vec![Ok(llm_response("end_turn"))]);

    let state = make_test_state(provider, tmp.path().join("sessions"));
    let addr = spawn_server(state).await;

    let mut ws = connect(addr).await;
    assert_eq!(recv_json(&mut ws).await["type"], "ready");

    send_json(
        &mut ws,
        serde_json::json!({ "type": "reset", "allowDirty": true }),
    )
    .await;
    let _ = recv_until_type(&mut ws, "ready").await;

    send_json(
        &mut ws,
        serde_json::json!({ "type": "user_message", "content": "hi" }),
    )
    .await;
    // Drive past the first ToolCall, then abort at the next loop-top
    // cancel check (matching the omega-agent test pattern).
    let _ = recv_until_type(&mut ws, "tool_call").await;
    send_json(&mut ws, serde_json::json!({ "type": "abort" })).await;

    let frames = recv_until_type(&mut ws, "turn_interrupted").await;
    assert!(frames.iter().any(|v| v["type"] == "turn_interrupted"));
}

// ---------------------------------------------------------------------------
// 5. Reconnect — a new WS gets `ready`.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn reconnect_new_ws_receives_ready() {
    let tmp = TempDir::new().unwrap();
    let provider = Arc::new(MockProvider::new());
    let state = make_test_state(provider, tmp.path().join("sessions"));
    let addr = spawn_server(state).await;

    // First connection.
    let mut ws1 = connect(addr).await;
    assert_eq!(recv_json(&mut ws1).await["type"], "ready");
    send_json(
        &mut ws1,
        serde_json::json!({ "type": "reset", "allowDirty": true }),
    )
    .await;
    let _ = recv_until_type(&mut ws1, "ready").await;

    // Disconnect.
    ws1.close(None).await.ok();
    drop(ws1);
    // Brief delay so the server's read loop observes the close before the
    // reconnect arrives — keeps the assertion crisp.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Reconnect — new WS must accept; with history replay active, the session
    // init events (server_started + session_started) arrive first, then ready.
    let mut ws2 = connect(addr).await;
    let frames = recv_until_type(&mut ws2, "ready").await;
    assert_eq!(
        frames.last().unwrap()["type"],
        "ready",
        "ready must arrive last; got {:?}",
        frames
    );
}

// ---------------------------------------------------------------------------
// 6. Invalid client frame → agent_error frame, socket stays open.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn invalid_client_frame_emits_agent_error_without_closing_socket() {
    let tmp = TempDir::new().unwrap();
    let provider = Arc::new(MockProvider::new());
    let state = make_test_state(provider, tmp.path().join("sessions"));
    let addr = spawn_server(state).await;

    let mut ws = connect(addr).await;
    assert_eq!(recv_json(&mut ws).await["type"], "ready");

    // Garbage payload.
    ws.send(TMessage::Text("not-json".to_owned().into()))
        .await
        .unwrap();
    let v = recv_json(&mut ws).await;
    assert_eq!(v["type"], "agent_error");
    assert!(
        v["message"]
            .as_str()
            .unwrap()
            .contains("invalid client frame")
    );

    // Socket is still alive — a follow-up `reset` works.
    send_json(
        &mut ws,
        serde_json::json!({ "type": "reset", "allowDirty": true }),
    )
    .await;
    let _ = recv_until_type(&mut ws, "ready").await;
}

// ---------------------------------------------------------------------------
// 7. user_message with no active session → agent_error frame.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn user_message_without_session_yields_agent_error() {
    let tmp = TempDir::new().unwrap();
    let provider = Arc::new(MockProvider::new());
    let state = make_test_state(provider, tmp.path().join("sessions"));
    let addr = spawn_server(state).await;

    let mut ws = connect(addr).await;
    assert_eq!(recv_json(&mut ws).await["type"], "ready");

    // No reset → no session.
    send_json(
        &mut ws,
        serde_json::json!({ "type": "user_message", "content": "hi" }),
    )
    .await;
    let v = recv_json(&mut ws).await;
    assert_eq!(v["type"], "agent_error");
    assert!(v["message"].as_str().unwrap().contains("no active session"));
}

// ---------------------------------------------------------------------------
// Phase 1e.3 — History replay on reconnect
// ---------------------------------------------------------------------------

/// Return the lexicographically latest sub-directory of `root`.
/// Session dirs sort by timestamp, so the latest is the most-recently created.
fn latest_session_dir(root: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut entries: Vec<_> = std::fs::read_dir(root)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .collect();
    entries.sort_by_key(|e| e.file_name());
    entries.last().map(|e| e.path())
}

// ---------------------------------------------------------------------------
// 8. Reconnect after a full turn replays events in order,
//    text signals are filtered out, Ready arrives last.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn reconnect_replays_turn_events_filters_text_ready_last() {
    let tmp = TempDir::new().unwrap();
    let sessions_root = tmp.path().join("sessions");
    let provider = Arc::new(MockProvider::new());
    // One turn: a text signal followed by an end_turn response.
    provider.push(vec![
        Ok(AgentItem::Signal(StreamSignal::Text {
            index: 0,
            text: "hello".to_owned(),
        })),
        Ok(llm_response("end_turn")),
    ]);

    let state = make_test_state(Arc::clone(&provider), sessions_root.clone());
    let addr = spawn_server(state).await;

    // First WS: reset + user_message, wait for turn_end.
    let mut ws1 = connect(addr).await;
    assert_eq!(recv_json(&mut ws1).await["type"], "ready");
    send_json(
        &mut ws1,
        serde_json::json!({ "type": "reset", "allowDirty": true }),
    )
    .await;
    let _ = recv_until_type(&mut ws1, "ready").await;
    send_json(
        &mut ws1,
        serde_json::json!({ "type": "user_message", "content": "hi" }),
    )
    .await;
    let live_frames = recv_until_type(&mut ws1, "turn_end").await;

    // The live stream must have contained at least one text frame.
    assert!(
        live_frames.iter().any(|v| v["type"] == "text"),
        "live stream must include text frames; got {:?}",
        live_frames.iter().map(|v| &v["type"]).collect::<Vec<_>>(),
    );

    // Inject a synthetic \"text\" line into events.jsonl to verify the filter
    // works even when the excluded type appears on disk.
    let session_dir = latest_session_dir(&sessions_root).expect("session dir must exist");
    let events_path = session_dir.join("events.jsonl");
    {
        use std::io::Write as _;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&events_path)
            .unwrap();
        f.write_all(b"{\"type\":\"text\",\"text\":\"injected-noise\"}\n")
            .unwrap();
    }

    // Disconnect.
    ws1.close(None).await.ok();
    drop(ws1);
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Reconnect: verify the Phase 2a replay sequence is
    // session_info → history(…) → ready, and that history.events
    // contains the persisted events with `text` filtered out.
    let mut ws2 = connect(addr).await;
    let replay_frames = recv_until_type(&mut ws2, "ready").await;
    let replay_types: Vec<&str> = replay_frames
        .iter()
        .filter_map(|v| v["type"].as_str())
        .collect();

    assert_eq!(
        replay_types,
        vec!["session_info", "history", "ready"],
        "outer replay sequence must be session_info → history → ready; got {replay_types:?}",
    );

    let history = replay_frames
        .iter()
        .find(|v| v["type"] == "history")
        .expect("history frame must be present");
    let history_events = history["events"]
        .as_array()
        .expect("history.events must be an array");
    let event_types: Vec<&str> = history_events
        .iter()
        .filter_map(|v| v["type"].as_str())
        .collect();

    // 1. No "text" entries in history (filter must suppress them).
    assert!(
        !event_types.contains(&"text"),
        "text entries must be filtered from history.events; got {event_types:?}",
    );

    // 2. Expected event types are present in the correct relative order.
    assert_eq!(
        event_types[0], "server_started",
        "first event in history must be server_started; got {event_types:?}",
    );
    assert_eq!(
        event_types[1], "session_started",
        "second event in history must be session_started; got {event_types:?}",
    );
    for ty in &["user_message", "llm_call", "llm_response_ended", "turn_end"] {
        assert!(
            event_types.contains(ty),
            "history.events must contain {ty}; got {event_types:?}",
        );
    }

    // 3. turn_end must be the last persisted event in history.
    assert_eq!(
        event_types.last().copied(),
        Some("turn_end"),
        "turn_end must be last in history.events; got {event_types:?}",
    );
}

// ---------------------------------------------------------------------------
// 9. Empty events.jsonl → reconnect → just Ready, no replay frames.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn replay_with_empty_events_file_yields_only_ready() {
    let tmp = TempDir::new().unwrap();
    let provider = Arc::new(MockProvider::new());
    let sessions_root = tmp.path().join("sessions");
    tokio::fs::create_dir_all(&sessions_root).await.unwrap();

    // Build a session dir manually with an empty events.jsonl.
    // We deliberately skip agent.init() so nothing is written to the file.
    let session_dir = sessions_root.join("2025-01-01T00-00-00-000-deadbeef");
    tokio::fs::create_dir_all(&session_dir).await.unwrap();
    let events_file = session_dir.join("events.jsonl");
    let context_file = session_dir.join("context.jsonl");
    tokio::fs::write(&events_file, b"").await.unwrap();
    tokio::fs::write(&context_file, b"").await.unwrap();

    let paths = SessionPaths {
        dir: session_dir.clone(),
        context_file: context_file.clone(),
        events_file: events_file.clone(),
    };
    let cstore = ContextStore::new(context_file);
    let estore = EventStore::new(events_file);
    let agent = Agent::new(
        Arc::clone(&provider) as Arc<dyn Provider>,
        cstore,
        estore,
        AgentConfig {
            model: "claude-sonnet-4-6".to_owned(),
            effort: None,
            cwd: tmp.path().to_path_buf(),
            session_dir,
        },
    );
    let controls = agent.controls();
    let info_cache = omega_server::session::SessionInfoCache {
        dir: "2025-01-01T00-00-00-000-deadbeef".to_owned(),
        model: "claude-sonnet-4-6".to_owned(),
        effort: omega_agent::DEFAULT_EFFORT.to_owned(),
        cwd: tmp.path().display().to_string(),
        name: None,
        has_pending_changes: false,
    };
    let active = ActiveSession {
        agent: Arc::new(tokio::sync::Mutex::new(agent)),
        controls,
        paths,
        ws_tx: None,
        current_turn: None,
        turn_state: Arc::new(tokio::sync::Mutex::new("idle".to_owned())),
        info_cache: Arc::new(tokio::sync::Mutex::new(info_cache)),
    };

    let state = make_test_state(Arc::clone(&provider), sessions_root);
    *state.active_session.lock().await = Some(active);

    let addr = spawn_server(state).await;
    let mut ws = connect(addr).await;

    // Session exists but events.jsonl is empty.  After Phase 2a the
    // server emits SessionInfo → History(events=[]) → Ready even when
    // no persisted events would be replayed.
    let frames = recv_until_type(&mut ws, "ready").await;
    let types: Vec<&str> = frames.iter().filter_map(|v| v["type"].as_str()).collect();
    assert_eq!(
        types,
        vec!["session_info", "history", "ready"],
        "empty events.jsonl must yield session_info → history → ready; got {types:?}",
    );
    let history = frames
        .iter()
        .find(|v| v["type"] == "history")
        .expect("history frame must be present");
    assert_eq!(
        history["events"],
        serde_json::json!([]),
        "history.events must be empty for an empty events.jsonl",
    );
    assert!(
        history
            .as_object()
            .is_some_and(|o| !o.contains_key("streaming")),
        "streaming flag must be omitted when no turn is in flight; got {history}",
    );
}

// ---------------------------------------------------------------------------
// 10. Reconnect after reset with no turns replays init events, then Ready.
// ---------------------------------------------------------------------------

/// Regression guard for the `reset_done`-before-`history` ordering fix.
/// The `reset` response must arrive in the order:
///   `session_info → reset_done → history → ready`
/// and `history.events` must contain the init pair
/// `[server_started, session_started]`.
///
/// Previously `reset_done` came *after* `history`, which caused the client
/// to clear the events it had just loaded — making `server_started` and
/// `session_started` invisible until the next browser refresh.
#[tokio::test]
async fn reset_frame_order_and_init_events_in_history() {
    let tmp = TempDir::new().unwrap();
    let provider = Arc::new(MockProvider::new());
    let state = make_test_state(Arc::clone(&provider), tmp.path().join("sessions"));
    let addr = spawn_server(state).await;

    let mut ws = connect(addr).await;
    assert_eq!(recv_json(&mut ws).await["type"], "ready"); // pre-session ready

    send_json(
        &mut ws,
        serde_json::json!({ "type": "reset", "allowDirty": true }),
    )
    .await;
    let frames = recv_until_type(&mut ws, "ready").await;

    let types: Vec<&str> = frames.iter().filter_map(|v| v["type"].as_str()).collect();
    assert_eq!(
        types,
        vec!["session_info", "reset_done", "history", "ready"],
        "reset frame order must be session_info → reset_done → history → ready; got {types:?}",
    );

    let history = frames
        .iter()
        .find(|v| v["type"] == "history")
        .expect("history frame present");
    let event_types: Vec<&str> = history["events"]
        .as_array()
        .expect("history.events is an array")
        .iter()
        .filter_map(|v| v["type"].as_str())
        .collect();
    assert_eq!(
        event_types,
        vec!["server_started", "session_started"],
        "history.events must be [server_started, session_started]; got {event_types:?}",
    );

    // Also verify session_started carries the omegaCommit field.
    let ss = history["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["type"] == "session_started")
        .expect("session_started in history");
    assert!(
        ss["omegaCommit"].is_string(),
        "session_started.omegaCommit must be a string; got {:?}",
        ss["omegaCommit"]
    );
}

#[tokio::test]
async fn reconnect_after_reset_replays_init_events_then_ready() {
    let tmp = TempDir::new().unwrap();
    let provider = Arc::new(MockProvider::new());
    let state = make_test_state(Arc::clone(&provider), tmp.path().join("sessions"));
    let addr = spawn_server(state).await;

    // First connection: reset creates a session (writes server_started +
    // session_started via agent.init()).  No user_message is sent.
    let mut ws1 = connect(addr).await;
    assert_eq!(recv_json(&mut ws1).await["type"], "ready"); // pre-session ready
    send_json(
        &mut ws1,
        serde_json::json!({ "type": "reset", "allowDirty": true }),
    )
    .await;
    let _ = recv_until_type(&mut ws1, "ready").await; // ready after reset

    // Disconnect.
    ws1.close(None).await.ok();
    drop(ws1);
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Reconnect: outer frames are session_info → history → ready, and
    // history.events holds the init pair [server_started, session_started].
    let mut ws2 = connect(addr).await;
    let frames = recv_until_type(&mut ws2, "ready").await;
    let types: Vec<&str> = frames.iter().filter_map(|v| v["type"].as_str()).collect();

    assert_eq!(
        types,
        vec!["session_info", "history", "ready"],
        "outer replay sequence must be session_info → history → ready; got {types:?}",
    );

    let history = frames
        .iter()
        .find(|v| v["type"] == "history")
        .expect("history frame must be present");
    let event_types: Vec<&str> = history["events"]
        .as_array()
        .expect("history.events must be an array")
        .iter()
        .filter_map(|v| v["type"].as_str())
        .collect();
    assert_eq!(
        event_types,
        vec!["server_started", "session_started"],
        "history.events must be exactly [server_started, session_started]; got {event_types:?}",
    );
}

// ---------------------------------------------------------------------------
// Phase 1e.4 — rename_session updates the active session's metadata.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rename_session_updates_metadata_for_active_session() {
    let tmp = TempDir::new().unwrap();
    let sessions_root = tmp.path().join("sessions");
    let provider = Arc::new(MockProvider::new());
    let state = make_test_state(provider, sessions_root.clone());
    let addr = spawn_server(state).await;

    let mut ws = connect(addr).await;
    assert_eq!(recv_json(&mut ws).await["type"], "ready");

    send_json(
        &mut ws,
        serde_json::json!({ "type": "reset", "allowDirty": true }),
    )
    .await;
    let _ = recv_until_type(&mut ws, "ready").await;

    // The active session's directory is the only one in `sessions_root`.
    let active_dir = sessions_root
        .read_dir()
        .expect("sessions_root readable")
        .next()
        .expect("a session dir must exist")
        .expect("dir entry")
        .file_name()
        .into_string()
        .expect("utf-8 dir name");
    send_json(
        &mut ws,
        serde_json::json!({
            "type": "rename_session",
            "sessionDir": active_dir,
            "name": "my-name",
        }),
    )
    .await;
    // The server broadcasts a session_renamed envelope after the disk write.
    let frame = recv_json(&mut ws).await;
    assert_eq!(
        frame["type"], "session_renamed",
        "expected session_renamed frame; got {frame:?}"
    );
    assert_eq!(
        frame["name"], "my-name",
        "name field must match; got {frame:?}"
    );
    let session_dir_val = frame["sessionDir"]
        .as_str()
        .expect("sessionDir must be a string");
    assert!(!session_dir_val.is_empty(), "sessionDir must be non-empty");

    let entry = sessions_root
        .read_dir()
        .expect("sessions_root readable")
        .next()
        .expect("a session dir must exist")
        .expect("dir entry");
    // The sessionDir in the envelope must match the on-disk session folder name.
    assert_eq!(
        session_dir_val,
        entry.file_name().to_str().unwrap(),
        "sessionDir in envelope must be the basename of the session dir",
    );
    let meta = omega_store::read_session_metadata(&entry.path()).await;
    assert_eq!(
        meta.name.as_deref(),
        Some("my-name"),
        "name must be updated; got {meta:?}",
    );
}

// ---------------------------------------------------------------------------
// Regression: rename_session must target the *client-provided* session_dir,
// not the currently active session.
//
// Earlier versions of the Rust router declared `RenameSession { name }`
// (without `session_dir`); serde silently dropped the client's `sessionDir`
// field and the handler always renamed whichever session was active. From
// the picker that produced two failure modes:
//   1. With no active session yet: rename appeared to do nothing.
//   2. After starting a new session: renaming a *previous* session in the
//      picker actually renamed the new (active) one.
// This test pre-creates an inactive session on disk, asks the server to
// rename it while a *different* session is active, and asserts that only
// the targeted session's metadata changed.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rename_session_targets_client_provided_dir_not_active_session() {
    let tmp = TempDir::new().unwrap();
    let sessions_root = tmp.path().join("sessions");
    std::fs::create_dir_all(&sessions_root).unwrap();

    // Pre-create an inactive session B on disk with an empty events.jsonl
    // (its existence is enough; we never load it as the active session).
    let session_b_name = "2024-12-31T00-00-00-000-deadbeef";
    let session_b_dir = sessions_root.join(session_b_name);
    std::fs::create_dir_all(&session_b_dir).unwrap();
    std::fs::write(session_b_dir.join("events.jsonl"), "").unwrap();

    let provider = Arc::new(MockProvider::new());
    let state = make_test_state(provider, sessions_root.clone());
    let addr = spawn_server(state).await;

    let mut ws = connect(addr).await;
    assert_eq!(recv_json(&mut ws).await["type"], "ready");

    // Make session A the active session.
    send_json(
        &mut ws,
        serde_json::json!({ "type": "reset", "allowDirty": true }),
    )
    .await;
    let _ = recv_until_type(&mut ws, "ready").await;
    let session_a_name = std::fs::read_dir(&sessions_root)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().into_string().unwrap())
        .find(|n| n != session_b_name)
        .expect("active session A on disk");

    // Rename the *inactive* session B from the picker. The active session
    // is A — the bug would silently rename A instead.
    send_json(
        &mut ws,
        serde_json::json!({
            "type": "rename_session",
            "sessionDir": session_b_name,
            "name": "renamed-b",
        }),
    )
    .await;
    let frame = recv_json(&mut ws).await;
    assert_eq!(frame["type"], "session_renamed", "got: {frame:?}");
    assert_eq!(frame["sessionDir"], session_b_name, "got: {frame:?}");
    assert_eq!(frame["name"], "renamed-b", "got: {frame:?}");

    let meta_b = omega_store::read_session_metadata(&session_b_dir).await;
    assert_eq!(
        meta_b.name.as_deref(),
        Some("renamed-b"),
        "target session B must have the new name; got {meta_b:?}",
    );
    let meta_a = omega_store::read_session_metadata(&sessions_root.join(&session_a_name)).await;
    assert_eq!(
        meta_a.name, None,
        "active session A must NOT be touched; got {meta_a:?}",
    );
}

// ---------------------------------------------------------------------------
// Phase 1e.4 — resume_session aborts current turn, runs the resumption
// summary call against the target session's events, and replays history
// of the new (resumed) session including the resuming_session event.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn resume_session_emits_resuming_session_event_for_target_dir() {
    let tmp = TempDir::new().unwrap();
    let sessions_root = tmp.path().join("sessions");
    std::fs::create_dir_all(&sessions_root).unwrap();

    // Pre-create session B with a valid session_dir name + an events.jsonl
    // containing one session_started event so extract_resumption_basis
    // has something to operate on.
    let session_b_name = "2024-12-31T00-00-00-000-cafefeed";
    let session_b_dir = sessions_root.join(session_b_name);
    std::fs::create_dir_all(&session_b_dir).unwrap();
    let events_b = session_b_dir.join("events.jsonl");
    std::fs::write(
        &events_b,
        "{\"type\":\"session_started\",\"time\":\"2024-12-31T00:00:00.000Z\",\"cwd\":\"/tmp\",\"model\":\"claude-sonnet-4-6\",\"effort\":\"medium\"}\n",
    )
    .expect("write session B events.jsonl");

    let provider = Arc::new(MockProvider::new());
    // Resumption summary turn: a single end_turn LLM response with summary text.
    provider.push(vec![Ok(llm_response("end_turn"))]);

    let state = make_test_state(Arc::clone(&provider), sessions_root.clone());
    let addr = spawn_server(state).await;

    let mut ws = connect(addr).await;
    assert_eq!(recv_json(&mut ws).await["type"], "ready");

    // Reset creates session A (does not invoke the LLM).
    send_json(
        &mut ws,
        serde_json::json!({ "type": "reset", "allowDirty": true }),
    )
    .await;
    let _ = recv_until_type(&mut ws, "ready").await;

    // Capture which session dirs exist *before* the resume so we can
    // identify the directory created specifically by resume_session
    // (the reset above also creates a session, so there are two non-B dirs).
    let dirs_before_resume: std::collections::HashSet<std::path::PathBuf> =
        std::fs::read_dir(&sessions_root)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();

    // Resume from session B.
    send_json(
        &mut ws,
        serde_json::json!({ "type": "resume_session", "sessionDir": session_b_name, "allowDirty": true }),
    )
    .await;
    let frames = recv_until_type(&mut ws, "ready").await;
    let types: Vec<&str> = frames.iter().filter_map(|v| v["type"].as_str()).collect();

    // The resumption stream must include `resuming_session` referencing B.
    let resuming = frames
        .iter()
        .find(|v| v["type"] == "resuming_session")
        .unwrap_or_else(|| panic!("expected resuming_session frame; got {types:?}"));
    assert_eq!(
        resuming["resumedFrom"].as_str(),
        Some(session_b_name),
        "resuming_session.resumed_from must point to B; got {resuming:?}",
    );
    // And it must complete with `session_resumed` before the final ready.
    assert!(
        types.contains(&"session_resumed"),
        "expected session_resumed in stream; got {types:?}",
    );
    assert_eq!(
        types.last().copied(),
        Some("ready"),
        "ready must be the last frame; got {types:?}",
    );

    // The newly-created session must record the resume link in its
    // metadata so a subsequent `GET /api/sessions` exposes it.
    // Find the directory that did NOT exist before the resume_session command
    // (there are two non-B dirs: the one from reset and the one from resume).
    let new_dir = std::fs::read_dir(&sessions_root)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| p.is_dir() && !dirs_before_resume.contains(p))
        .expect("new session dir created on resume");
    let meta = omega_store::read_session_metadata(&new_dir).await;
    assert_eq!(
        meta.resumed_from.as_deref(),
        Some(session_b_name),
        "resumed_from must point to B; got {meta:?}",
    );
}
