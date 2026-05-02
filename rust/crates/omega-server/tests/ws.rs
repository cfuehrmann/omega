//! Integration tests for the `/ws` WebSocket route (Phase 1e.2).
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
use omega_core::{AgentItem, AgentItemStream, LlmError, LlmRequest, Provider};
use omega_protocol::events::{LlmResponseEvent, ToolCallEvent};
use omega_protocol::{LlmResponseUsage, OmegaEvent, StreamSignal};
use omega_server::{AppState, build_router};
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
    AppState::new(provider, sessions_root, PathBuf::from("/dev/null"))
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

fn llm_response(stop_reason: &str, text: Option<&str>) -> AgentItem {
    AgentItem::event(OmegaEvent::LlmResponse(LlmResponseEvent {
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
        },
        context_hash: String::new(),
        text: text.map(str::to_owned),
        thinking: None,
        streaming_start: None,
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
            id: tool_id.to_owned(),
            name: tool_name.to_owned(),
            input,
            context_hash: String::new(),
        }))),
        Ok(llm_response("tool_use", None)),
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
    // One LLM call: a Signal::Text "hello" then end_turn LlmResponse.
    provider.push(vec![
        Ok(AgentItem::Signal(StreamSignal::Text {
            text: "hello".to_owned(),
        })),
        Ok(llm_response("end_turn", Some("hello"))),
    ]);

    let state = make_test_state(provider, tmp.path().join("sessions"));
    let addr = spawn_server(state).await;

    let mut ws = connect(addr).await;
    // Initial ready.
    assert_eq!(recv_json(&mut ws).await["type"], "ready");

    // Reset → another ready.
    send_json(&mut ws, serde_json::json!({ "type": "reset" })).await;
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
    send_json(&mut ws, serde_json::json!({ "type": "reset" })).await;
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
    provider.push(vec![Ok(llm_response("end_turn", Some("done")))]);

    let state = make_test_state(provider, tmp.path().join("sessions"));
    let addr = spawn_server(state).await;

    let mut ws = connect(addr).await;
    assert_eq!(recv_json(&mut ws).await["type"], "ready");

    send_json(&mut ws, serde_json::json!({ "type": "reset" })).await;
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
    provider.push(vec![Ok(llm_response("end_turn", Some("done")))]);

    let state = make_test_state(provider, tmp.path().join("sessions"));
    let addr = spawn_server(state).await;

    let mut ws = connect(addr).await;
    assert_eq!(recv_json(&mut ws).await["type"], "ready");

    send_json(&mut ws, serde_json::json!({ "type": "reset" })).await;
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
    send_json(&mut ws1, serde_json::json!({ "type": "reset" })).await;
    let _ = recv_until_type(&mut ws1, "ready").await;

    // Disconnect.
    ws1.close(None).await.ok();
    drop(ws1);
    // Brief delay so the server's read loop observes the close before the
    // reconnect arrives — keeps the assertion crisp.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Reconnect — new WS must accept and emit `ready`.
    let mut ws2 = connect(addr).await;
    assert_eq!(recv_json(&mut ws2).await["type"], "ready");
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
    send_json(&mut ws, serde_json::json!({ "type": "reset" })).await;
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
