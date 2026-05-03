//! Integration tests targeting every `handle_*` arm in `router.rs`.
//!
//! **Scope (TEST-ARCH-2 + BUG-S1 + BUG-S2):** kills all non-equivalent missed
//! mutants in `crates/omega-server/src/router.rs`.
//!
//! **Accepted-miss equivalent — 1 remaining:**
//!
//! - `router.rs:485 delete match arm Message::Close(_)` — documented in the
//!   source: the close frame causes `reader.next()` to return `None` on the
//!   next iteration, exiting the `while-let` identically to `break`.
//!
//! **Formerly accepted misses, now fixed:**
//!
//! - `router.rs:379 delete match arm OmegaEvent::PauseRequested(_)` — arm
//!   removed from `next_turn_state_for` (BUG-S2); the mutation no longer
//!   exists.
//!
//! - `router.rs:878 replace != with == in handle_resume_session` — the inner
//!   turn-state-update block was replaced with explicit "running"/"idle"
//!   brackets; the stale guard is gone (BUG-S2).
//!
//! - `router.rs:424 delete ! in send_session_info_and_history` — killed by
//!   both `history_streaming_flag_*` tests now that BUG-S1 is fixed.
//!
//! Each test:
//! 1. Builds an `AppState` backed by an in-process `MockProvider`.
//! 2. Spawns the server on `127.0.0.1:0`.
//! 3. Connects a `tokio-tungstenite` client to `/ws`.
//! 4. Drives client→server frames and collects server→client frames.
//! 5. Asserts the observed sequence (and in key cases, insta-snapshots the
//!    stable fields of the frame, redacting volatile timestamp / dir / cwd).
//!
//! One subprocess test (`e2e_full_turn_via_http_fake`) uses the HTTP fake from
//! `tests/common/` to validate the `ANTHROPIC_BASE_URL` env-var hook.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::doc_markdown,
    clippy::too_many_lines
)]

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::{SinkExt, StreamExt, stream::BoxStream};
use insta::assert_snapshot;
use omega_core::{AgentItem, AgentItemStream, LlmError, LlmRequest, Provider};
use omega_protocol::events::{LlmResponseEvent, ToolCallEvent};
use omega_protocol::{OmegaEvent, StreamSignal};
use omega_server::{AppState, build_router};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message as TMessage;

mod common;

// ---------------------------------------------------------------------------
// MockProvider — replays a queue of transcripts, one per `stream()` call.
// Same pattern as tests/ws.rs, kept local to avoid cross-test coupling.
// ---------------------------------------------------------------------------

struct MockProvider {
    responses: Mutex<VecDeque<Vec<Result<AgentItem, LlmError>>>>,
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
            Box::pin(base) as AgentItemStream
        }
    }
}

// ---------------------------------------------------------------------------
// Shared test helpers
// ---------------------------------------------------------------------------

type WsClient =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

fn make_state(provider: Arc<MockProvider>, sessions_root: PathBuf) -> AppState {
    AppState::new(provider, sessions_root, PathBuf::from("/dev/null"))
}

async fn spawn_server(state: AppState) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = build_router(state);
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

async fn connect(addr: SocketAddr) -> WsClient {
    let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    ws
}

/// Receive next Text frame with a 5 s timeout; decode as JSON.
async fn recv_json(ws: &mut WsClient) -> serde_json::Value {
    let frame = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("recv timed out")
        .expect("ws stream ended")
        .expect("ws frame error");
    match frame {
        TMessage::Text(t) => serde_json::from_str(&t).expect("json decode"),
        other => panic!("expected Text frame, got {other:?}"),
    }
}

/// Drain frames until one whose `type` equals `want`, returning all frames
/// including the match as the last entry. Times out per-recv after 5 s.
async fn recv_until_type(ws: &mut WsClient, want: &str) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    loop {
        let v = recv_json(ws).await;
        let hit = v.get("type").and_then(|t| t.as_str()) == Some(want);
        out.push(v);
        if hit {
            return out;
        }
    }
}

async fn send_json(ws: &mut WsClient, v: serde_json::Value) {
    ws.send(TMessage::Text(v.to_string().into())).await.unwrap();
}

/// Perform reset and drain to `ready`; return the session dir name from the
/// `session_info` frame that arrives in the batch.
async fn reset_and_ready(ws: &mut WsClient) -> String {
    send_json(ws, serde_json::json!({ "type": "reset" })).await;
    let frames = recv_until_type(ws, "ready").await;
    frames
        .iter()
        .find(|v| v["type"] == "session_info")
        .and_then(|v| v["dir"].as_str())
        .unwrap_or_default()
        .to_owned()
}

/// Redact volatile JSON fields (time, dir, cwd, contextHash) so snapshots
/// are stable across runs.
fn redact(mut v: serde_json::Value) -> serde_json::Value {
    if let Some(obj) = v.as_object_mut() {
        for key in &["time", "dir", "cwd", "contextHash"] {
            if obj.contains_key(*key) {
                obj.insert(
                    (*key).to_owned(),
                    serde_json::Value::String("[REDACTED]".to_owned()),
                );
            }
        }
    }
    v
}

// ---------------------------------------------------------------------------
// Mock transcript helpers
// ---------------------------------------------------------------------------

fn llm_response_event(stop_reason: &str, text: Option<&str>) -> AgentItem {
    AgentItem::event(OmegaEvent::LlmResponse(LlmResponseEvent {
        time: "2024-01-01T00:00:00.000Z".to_owned(),
        stop_reason: stop_reason.to_owned(),
        cleared_tool_uses: None,
        cleared_input_tokens: None,
        usage: omega_protocol::LlmResponseUsage {
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
    id: &str,
    name: &str,
    input: serde_json::Value,
) -> Vec<Result<AgentItem, LlmError>> {
    vec![
        Ok(AgentItem::event(OmegaEvent::ToolCall(ToolCallEvent {
            time: "2024-01-01T00:00:00.000Z".to_owned(),
            id: id.to_owned(),
            name: name.to_owned(),
            input,
            context_hash: String::new(),
        }))),
        Ok(llm_response_event("tool_use", None)),
    ]
}

// ---------------------------------------------------------------------------
// handle_set_model tests (lines 577–589)
// ---------------------------------------------------------------------------

/// `replace handle_set_model -> Ok(())` — without the handler a set_model
/// frame is silently dropped; no `model_changed` event arrives.
///
/// Also anchors the wire format of the model_changed frame.
#[tokio::test]
async fn set_model_emits_model_changed_frame() {
    let tmp = TempDir::new().unwrap();
    let provider = Arc::new(MockProvider::new());
    let addr = spawn_server(make_state(
        Arc::clone(&provider),
        tmp.path().join("sessions"),
    ))
    .await;
    let mut ws = connect(addr).await;
    assert_eq!(recv_json(&mut ws).await["type"], "ready");
    reset_and_ready(&mut ws).await;

    send_json(
        &mut ws,
        serde_json::json!({ "type": "set_model", "model": "claude-opus-4-6" }),
    )
    .await;
    let frame = recv_json(&mut ws).await;
    assert_eq!(
        frame["type"], "model_changed",
        "expected model_changed; got {frame}"
    );
    assert_eq!(frame["model"], "claude-opus-4-6");

    // Snapshot the stable fields; `time` is redacted.
    assert_snapshot!(
        "set_model_frame",
        serde_json::to_string_pretty(&redact(frame)).unwrap()
    );
}

/// Three tests covering all 7 boolean-logic mutants in the effort-reset path
/// of `handle_set_model` (lines 588–589).
///
/// TestSM1: effort="max" + non-Opus model → both model_changed AND
/// effort_changed must arrive (reset expected).
/// Kills: `replace == with != (588)`, `delete ! (588)`,
///        `replace || with && (outer, 589)`.
#[tokio::test]
async fn set_model_max_effort_with_non_opus_resets_effort_to_medium() {
    let tmp = TempDir::new().unwrap();
    let provider = Arc::new(MockProvider::new());
    let addr = spawn_server(make_state(
        Arc::clone(&provider),
        tmp.path().join("sessions"),
    ))
    .await;
    let mut ws = connect(addr).await;
    assert_eq!(recv_json(&mut ws).await["type"], "ready");
    reset_and_ready(&mut ws).await;

    // Elevate effort to "max" (only valid on Opus models, but set_effort
    // accepts any string so it persists regardless).
    send_json(
        &mut ws,
        serde_json::json!({ "type": "set_effort", "effort": "max" }),
    )
    .await;
    let _ = recv_json(&mut ws).await; // effort_changed

    // Switch to a non-Opus model — must trigger an auto-reset to "medium".
    send_json(
        &mut ws,
        serde_json::json!({ "type": "set_model", "model": "claude-sonnet-4-6" }),
    )
    .await;
    let f1 = recv_json(&mut ws).await;
    let f2 = recv_json(&mut ws).await;

    let types: Vec<&str> = [&f1, &f2]
        .iter()
        .filter_map(|v| v["type"].as_str())
        .collect();
    assert!(
        types.contains(&"model_changed"),
        "model_changed must be emitted; got {types:?}",
    );
    assert!(
        types.contains(&"effort_changed"),
        "effort_changed must be emitted on effort reset; got {types:?}",
    );
    let binding = [&f1, &f2];
    let effort_frame = binding
        .iter()
        .find(|v| v["type"] == "effort_changed")
        .unwrap();
    assert_eq!(
        effort_frame["effort"], "medium",
        "effort must reset to medium"
    );
}

/// TestSM2: effort="medium" (default) + non-list model → only model_changed,
/// NO effort_changed.
/// Kills: `replace && with || (588)`, `replace && with || (xhigh, 589)`,
///        `replace == with != (xhigh, 589)`.
#[tokio::test]
async fn set_model_medium_effort_does_not_reset_effort() {
    let tmp = TempDir::new().unwrap();
    let provider = Arc::new(MockProvider::new());
    let addr = spawn_server(make_state(
        Arc::clone(&provider),
        tmp.path().join("sessions"),
    ))
    .await;
    let mut ws = connect(addr).await;
    assert_eq!(recv_json(&mut ws).await["type"], "ready");
    reset_and_ready(&mut ws).await;

    // Default effort is "medium"; switch to a non-Opus model.
    send_json(
        &mut ws,
        serde_json::json!({ "type": "set_model", "model": "claude-sonnet-4-6" }),
    )
    .await;
    let f1 = recv_json(&mut ws).await;
    assert_eq!(
        f1["type"], "model_changed",
        "expected model_changed; got {f1}"
    );

    // A second frame would be effort_changed if the mutant fired.
    // We don't want to block forever, so we attempt a second receive with a
    // short timeout and assert nothing unexpected arrives before the next
    // framing event.  The simplest probe: send another set_model, which will
    // always produce model_changed, and verify that's the *immediate* next frame.
    send_json(
        &mut ws,
        serde_json::json!({ "type": "set_model", "model": "claude-opus-4-6" }),
    )
    .await;
    let f2 = recv_json(&mut ws).await;
    assert_eq!(
        f2["type"], "model_changed",
        "no effort_changed must sneak in before the second model_changed; got {f2}",
    );
}

/// TestSM3: effort="xhigh" + model in MAX_EFFORT_MODELS but NOT XHIGH →
/// effort reset expected.
/// Kills: `replace == with != (xhigh, 589)`, `delete ! (xhigh, 589)`.
#[tokio::test]
async fn set_model_xhigh_effort_with_non_xhigh_model_resets_effort_to_medium() {
    let tmp = TempDir::new().unwrap();
    let provider = Arc::new(MockProvider::new());
    let addr = spawn_server(make_state(
        Arc::clone(&provider),
        tmp.path().join("sessions"),
    ))
    .await;
    let mut ws = connect(addr).await;
    assert_eq!(recv_json(&mut ws).await["type"], "ready");
    reset_and_ready(&mut ws).await;

    send_json(
        &mut ws,
        serde_json::json!({ "type": "set_effort", "effort": "xhigh" }),
    )
    .await;
    let _ = recv_json(&mut ws).await; // effort_changed

    // claude-opus-4-6 is in MAX_EFFORT_MODELS but not XHIGH_EFFORT_MODELS.
    send_json(
        &mut ws,
        serde_json::json!({ "type": "set_model", "model": "claude-opus-4-6" }),
    )
    .await;
    let f1 = recv_json(&mut ws).await;
    let f2 = recv_json(&mut ws).await;

    let types: Vec<&str> = [&f1, &f2]
        .iter()
        .filter_map(|v| v["type"].as_str())
        .collect();
    assert!(
        types.contains(&"effort_changed"),
        "effort reset must fire: {types:?}"
    );
    let binding = [&f1, &f2];
    let ef = binding
        .iter()
        .find(|v| v["type"] == "effort_changed")
        .unwrap();
    assert_eq!(ef["effort"], "medium");
}

// ---------------------------------------------------------------------------
// handle_set_effort test (line 618)
// ---------------------------------------------------------------------------

/// `replace handle_set_effort -> Ok(())` — without the handler no
/// `effort_changed` event would be emitted.
#[tokio::test]
async fn set_effort_emits_effort_changed_frame() {
    let tmp = TempDir::new().unwrap();
    let provider = Arc::new(MockProvider::new());
    let addr = spawn_server(make_state(
        Arc::clone(&provider),
        tmp.path().join("sessions"),
    ))
    .await;
    let mut ws = connect(addr).await;
    assert_eq!(recv_json(&mut ws).await["type"], "ready");
    reset_and_ready(&mut ws).await;

    send_json(
        &mut ws,
        serde_json::json!({ "type": "set_effort", "effort": "low" }),
    )
    .await;
    let frame = recv_json(&mut ws).await;
    assert_eq!(
        frame["type"], "effort_changed",
        "expected effort_changed; got {frame}"
    );
    assert_eq!(frame["effort"], "low");

    assert_snapshot!(
        "set_effort_frame",
        serde_json::to_string_pretty(&redact(frame)).unwrap()
    );
}

// ---------------------------------------------------------------------------
// handle_delete_session tests (lines 640–641)
// ---------------------------------------------------------------------------

/// `replace handle_delete_session -> Ok(())` — without the handler the
/// session directory is never removed and no `session_deleted` frame
/// arrives.
#[tokio::test]
async fn delete_session_removes_dir_and_emits_session_deleted() {
    let tmp = TempDir::new().unwrap();
    let sessions_root = tmp.path().join("sessions");
    let provider = Arc::new(MockProvider::new());
    let addr = spawn_server(make_state(Arc::clone(&provider), sessions_root.clone())).await;
    let mut ws = connect(addr).await;
    assert_eq!(recv_json(&mut ws).await["type"], "ready");
    reset_and_ready(&mut ws).await;

    // Create a second (non-active) session directory with a valid name.
    let other_name = "2024-06-01T00-00-00-000-cafecafe";
    let other_dir = sessions_root.join(other_name);
    tokio::fs::create_dir_all(&other_dir).await.unwrap();
    // The directory must exist before we try to delete it.
    assert!(other_dir.exists(), "pre-condition: dir must exist");

    send_json(
        &mut ws,
        serde_json::json!({ "type": "delete_session", "sessionDir": other_name }),
    )
    .await;
    let frame = recv_json(&mut ws).await;
    assert_eq!(
        frame["type"], "session_deleted",
        "expected session_deleted; got {frame}"
    );
    assert_eq!(frame["sessionDir"], other_name);

    assert!(
        !other_dir.exists(),
        "delete_session must remove the directory from disk",
    );

    assert_snapshot!(
        "session_deleted_frame",
        serde_json::to_string_pretty(&frame).unwrap()
    );
}

/// `delete ! in handle_delete_session` — without the guard, sending a
/// syntactically invalid session dir (path traversal or wrong shape)
/// would not be rejected.
#[tokio::test]
async fn delete_session_with_invalid_dir_emits_agent_error() {
    let tmp = TempDir::new().unwrap();
    let provider = Arc::new(MockProvider::new());
    let addr = spawn_server(make_state(
        Arc::clone(&provider),
        tmp.path().join("sessions"),
    ))
    .await;
    let mut ws = connect(addr).await;
    assert_eq!(recv_json(&mut ws).await["type"], "ready");

    // No reset needed — handle_delete_session validates before touching state.
    send_json(
        &mut ws,
        serde_json::json!({ "type": "delete_session", "sessionDir": "../../../etc" }),
    )
    .await;
    let frame = recv_json(&mut ws).await;
    assert_eq!(
        frame["type"], "agent_error",
        "expected agent_error for invalid dir; got {frame}"
    );
    assert!(
        frame["message"]
            .as_str()
            .unwrap()
            .contains("invalid sessionDir"),
        "error must mention invalid sessionDir; got {frame}",
    );
}

// ---------------------------------------------------------------------------
// next_turn_state_for and session_info turn-state propagation tests
// (lines 380–381)
// ---------------------------------------------------------------------------

/// `delete match arm OmegaEvent::TurnEnd(_) | OmegaEvent::TurnInterrupted(_)`
/// — without this arm the `session_info` frame with `turnState: "idle"` is
/// never sent after a successful turn.
///
/// Also snapshots the `session_info` frame that follows `turn_end`.
#[tokio::test]
async fn turn_end_emits_session_info_with_idle_turn_state() {
    let tmp = TempDir::new().unwrap();
    let provider = Arc::new(MockProvider::new());
    provider.push(vec![
        Ok(AgentItem::Signal(StreamSignal::Text {
            text: "hi".to_owned(),
        })),
        Ok(llm_response_event("end_turn", Some("hi"))),
    ]);
    let addr = spawn_server(make_state(
        Arc::clone(&provider),
        tmp.path().join("sessions"),
    ))
    .await;
    let mut ws = connect(addr).await;
    assert_eq!(recv_json(&mut ws).await["type"], "ready");
    reset_and_ready(&mut ws).await;

    send_json(
        &mut ws,
        serde_json::json!({ "type": "user_message", "content": "go" }),
    )
    .await;
    // Drain until turn_end (inclusive).
    let _ = recv_until_type(&mut ws, "turn_end").await;

    // The frame immediately after turn_end must be session_info with
    // turnState == "idle".
    let info = recv_json(&mut ws).await;
    assert_eq!(
        info["type"], "session_info",
        "expected session_info after turn_end; got {info}"
    );
    assert_eq!(
        info["turnState"], "idle",
        "turnState must be \"idle\" after turn ends; got {info}",
    );

    assert_snapshot!(
        "turn_end_session_info",
        serde_json::to_string_pretty(&redact(info)).unwrap()
    );
}

/// `delete match arm OmegaEvent::TurnInterrupted(_)` (same arm as TurnEnd —
/// both are covered by the union arm in `next_turn_state_for`).
///
/// Drive an abort so a `TurnInterrupted` event is emitted, then assert
/// the trailing `session_info.turnState == "idle"`.
#[tokio::test]
async fn turn_interrupted_emits_session_info_with_idle_turn_state() {
    let tmp = TempDir::new().unwrap();
    let scratch = {
        let p = tmp.path().join("scratch.txt");
        std::fs::write(&p, "x").unwrap();
        p
    };
    let provider = Arc::new(MockProvider::new());
    provider.set_item_delay(Duration::from_millis(30));
    provider.push(tool_use_items(
        "tu_abort",
        "read_file",
        serde_json::json!({ "path": scratch.to_string_lossy() }),
    ));
    // Second turn — only reached if abort failed.
    provider.push(vec![Ok(llm_response_event("end_turn", Some("done")))]);

    let addr = spawn_server(make_state(
        Arc::clone(&provider),
        tmp.path().join("sessions"),
    ))
    .await;
    let mut ws = connect(addr).await;
    assert_eq!(recv_json(&mut ws).await["type"], "ready");
    reset_and_ready(&mut ws).await;

    send_json(
        &mut ws,
        serde_json::json!({ "type": "user_message", "content": "go" }),
    )
    .await;
    let _ = recv_until_type(&mut ws, "tool_call").await;
    send_json(&mut ws, serde_json::json!({ "type": "abort" })).await;
    let _ = recv_until_type(&mut ws, "turn_interrupted").await;

    // session_info with turnState="idle" must immediately follow.
    let info = recv_json(&mut ws).await;
    assert_eq!(info["type"], "session_info");
    assert_eq!(
        info["turnState"], "idle",
        "turnState must be idle after abort; got {info}"
    );
}

/// `delete match arm OmegaEvent::TurnPaused(_)` (line 380) — without this
/// arm the `session_info` frame with `turnState: "paused"` is never sent.
///
/// Drives a pause/continue cycle and asserts `session_info.turnState` is
/// `"paused"` immediately after `turn_paused`.
#[tokio::test]
async fn turn_paused_emits_session_info_with_paused_turn_state() {
    let tmp = TempDir::new().unwrap();
    let scratch = {
        let p = tmp.path().join("scratch.txt");
        std::fs::write(&p, "y").unwrap();
        p
    };
    let provider = Arc::new(MockProvider::new());
    provider.set_item_delay(Duration::from_millis(30));
    provider.push(tool_use_items(
        "tu_pause",
        "read_file",
        serde_json::json!({ "path": scratch.to_string_lossy() }),
    ));
    provider.push(vec![Ok(llm_response_event("end_turn", Some("done")))]);

    let addr = spawn_server(make_state(
        Arc::clone(&provider),
        tmp.path().join("sessions"),
    ))
    .await;
    let mut ws = connect(addr).await;
    assert_eq!(recv_json(&mut ws).await["type"], "ready");
    reset_and_ready(&mut ws).await;

    send_json(
        &mut ws,
        serde_json::json!({ "type": "user_message", "content": "go" }),
    )
    .await;
    let _ = recv_until_type(&mut ws, "tool_call").await;
    send_json(&mut ws, serde_json::json!({ "type": "pause" })).await;
    // Drain until turn_paused.
    let frames = recv_until_type(&mut ws, "turn_paused").await;

    // The session_info with turnState="paused" must be the frame
    // immediately after turn_paused.
    let info = recv_json(&mut ws).await;
    assert_eq!(info["type"], "session_info");
    assert_eq!(
        info["turnState"], "paused",
        "turnState must be paused after turn_paused; got {info}\n(preceding frames: {frames:?})",
    );

    // Clean up: continue the paused turn so the session winds down cleanly.
    send_json(&mut ws, serde_json::json!({ "type": "continue" })).await;
    let _ = recv_until_type(&mut ws, "turn_end").await;
}

// ---------------------------------------------------------------------------
// handle_pause — turn-state guard (line 752)
// ---------------------------------------------------------------------------

/// `replace != with == in handle_pause` (line 752, the `"pause_requested"`
/// guard) — without this guard, a second `pause` sent while the state is
/// already `"pause_requested"` would re-send a `session_info` frame with
/// stale data. More importantly, the mutant flips the condition so the
/// session_info frame is never sent when a *first* pause arrives in
/// `"running"` state.
///
/// Assert that after `controls.request_pause()` is called, a `session_info`
/// frame with `turnState: "pause_requested"` arrives on the socket.
#[tokio::test]
async fn pause_emits_session_info_with_pause_requested_turn_state() {
    let tmp = TempDir::new().unwrap();
    let scratch = {
        let p = tmp.path().join("scratch.txt");
        std::fs::write(&p, "z").unwrap();
        p
    };
    let provider = Arc::new(MockProvider::new());
    provider.set_item_delay(Duration::from_millis(30));
    provider.push(tool_use_items(
        "tu_pr",
        "read_file",
        serde_json::json!({ "path": scratch.to_string_lossy() }),
    ));
    provider.push(vec![Ok(llm_response_event("end_turn", Some("done")))]);

    let addr = spawn_server(make_state(
        Arc::clone(&provider),
        tmp.path().join("sessions"),
    ))
    .await;
    let mut ws = connect(addr).await;
    assert_eq!(recv_json(&mut ws).await["type"], "ready");
    reset_and_ready(&mut ws).await;

    send_json(
        &mut ws,
        serde_json::json!({ "type": "user_message", "content": "go" }),
    )
    .await;
    let _ = recv_until_type(&mut ws, "tool_call").await;
    send_json(&mut ws, serde_json::json!({ "type": "pause" })).await;

    // Collect frames until we see pause_requested event, then check the
    // immediately following session_info.
    let pause_frames = recv_until_type(&mut ws, "pause_requested").await;
    // session_info with "pause_requested" must follow.
    let info = recv_json(&mut ws).await;
    assert_eq!(
        info["type"], "session_info",
        "expected session_info after pause_requested; got {info}\n(pause frames: {pause_frames:?})"
    );
    assert_eq!(
        info["turnState"], "pause_requested",
        "turnState must be pause_requested; got {info}",
    );

    // Clean up.
    send_json(&mut ws, serde_json::json!({ "type": "continue" })).await;
    let _ = recv_until_type(&mut ws, "turn_end").await;
}

// ---------------------------------------------------------------------------
// send_session_info_and_history — `streaming` flag (line 424)
// ---------------------------------------------------------------------------

/// `delete ! in send_session_info_and_history` — without the `!`, `streaming`
/// is `true` when the turn IS finished (wrong).
///
/// **Case A (BUG-S1 regression guard):** reconnect while a turn is in flight
/// → `history.streaming` must be `true`. Also verifies BUG-S1 stays fixed:
/// if `send_session_info_and_history` re-introduces the ABBA lock cycle the
/// test hangs for 5 s and fails.
#[tokio::test(flavor = "multi_thread")]
async fn history_streaming_flag_true_when_turn_in_flight() {
    let tmp = TempDir::new().unwrap();
    let scratch = {
        let p = tmp.path().join("scratch.txt");
        std::fs::write(&p, "stream_test").unwrap();
        p
    };
    let provider = Arc::new(MockProvider::new());
    // 40 ms per item — gives ~80 ms window to connect WS2 and check the flag.
    provider.set_item_delay(Duration::from_millis(40));
    provider.push(tool_use_items(
        "tu_stream",
        "read_file",
        serde_json::json!({ "path": scratch.to_string_lossy() }),
    ));
    provider.push(vec![Ok(llm_response_event("end_turn", Some("done")))]);

    let state = make_state(Arc::clone(&provider), tmp.path().join("sessions"));
    let addr = spawn_server(state).await;

    // WS 1: reset + start user_message; wait until the streaming task is
    // definitely running (tool_call received = first item delivered).
    let mut ws1 = connect(addr).await;
    assert_eq!(recv_json(&mut ws1).await["type"], "ready");
    reset_and_ready(&mut ws1).await;
    send_json(
        &mut ws1,
        serde_json::json!({ "type": "user_message", "content": "go" }),
    )
    .await;
    let _ = recv_until_type(&mut ws1, "tool_call").await;

    // WS 2: connect while turn is in flight (streaming task sleeping 40 ms).
    let mut ws2 = connect(addr).await;
    let frames2 = recv_until_type(&mut ws2, "ready").await;
    let history2 = frames2
        .iter()
        .find(|v| v["type"] == "history")
        .expect("history frame");
    assert_eq!(
        history2["streaming"], true,
        "history.streaming must be true while the turn is in flight; got {history2}",
    );

    // After install_ws_tx swapped ws_tx to tx2, all remaining streaming
    // events go to ws2, not ws1 — drain ws2 to let the turn finish.
    let _ = recv_until_type(&mut ws2, "turn_end").await;
}

/// **Case B:** reconnect after the turn has finished → `history.streaming`
/// must be absent (field omitted when false per the wire contract).
#[tokio::test(flavor = "multi_thread")]
async fn history_streaming_flag_absent_after_turn_completes() {
    let tmp = TempDir::new().unwrap();
    let provider = Arc::new(MockProvider::new());
    provider.push(vec![
        Ok(AgentItem::Signal(StreamSignal::Text {
            text: "hi".to_owned(),
        })),
        Ok(llm_response_event("end_turn", Some("hi"))),
    ]);

    let state = make_state(Arc::clone(&provider), tmp.path().join("sessions"));
    let addr = spawn_server(state).await;

    let mut ws1 = connect(addr).await;
    assert_eq!(recv_json(&mut ws1).await["type"], "ready");
    reset_and_ready(&mut ws1).await;
    send_json(
        &mut ws1,
        serde_json::json!({ "type": "user_message", "content": "hi" }),
    )
    .await;
    let _ = recv_until_type(&mut ws1, "turn_end").await;
    // Drain the trailing session_info frame.
    let _ = recv_json(&mut ws1).await;

    // Close WS 1 and reconnect.
    ws1.close(None).await.ok();
    drop(ws1);
    tokio::time::sleep(Duration::from_millis(60)).await;

    let mut ws2 = connect(addr).await;
    let frames2 = recv_until_type(&mut ws2, "ready").await;
    let history2 = frames2
        .iter()
        .find(|v| v["type"] == "history")
        .expect("history frame");
    assert!(
        history2
            .as_object()
            .is_some_and(|o| !o.contains_key("streaming")),
        "history.streaming must be absent after turn completes; got {history2}",
    );
}

// ---------------------------------------------------------------------------
// handle_resume_session — turn-state brackets (BUG-S2)
// ---------------------------------------------------------------------------

/// BUG-S2: `perform_resumption` never yields state-changing events, so
/// `session_info.turnState` previously stayed `"idle"` throughout the entire
/// summarisation call. The fix brackets the stream with explicit
/// `"running"` / `"idle"` transitions.
///
/// Asserts:
/// - `session_info(turnState="running")` arrives before the resumption events.
/// - `session_info(turnState="idle")` arrives after `session_resumed`,
///   immediately before `ready`.
#[tokio::test(flavor = "multi_thread")]
async fn resume_session_emits_running_then_idle_session_info() {
    let tmp = TempDir::new().unwrap();
    let sessions_root = tmp.path().join("sessions");
    std::fs::create_dir_all(&sessions_root).unwrap();

    let session_b_name = "2024-11-01T00-00-00-000-beefdead";
    let session_b_dir = sessions_root.join(session_b_name);
    std::fs::create_dir_all(&session_b_dir).unwrap();
    std::fs::write(
        session_b_dir.join("events.jsonl"),
        "{\"type\":\"session_started\",\"time\":\"2024-11-01T00:00:00.000Z\",\
         \"cwd\":\"/tmp\",\"model\":\"claude-sonnet-4-6\",\"effort\":\"medium\"}\n",
    )
    .unwrap();

    let provider = Arc::new(MockProvider::new());
    provider.push(vec![Ok(llm_response_event(
        "end_turn",
        Some("<summary>ctx</summary>"),
    ))]);

    let addr = spawn_server(make_state(Arc::clone(&provider), sessions_root)).await;
    let mut ws = connect(addr).await;
    assert_eq!(recv_json(&mut ws).await["type"], "ready");
    reset_and_ready(&mut ws).await;

    send_json(
        &mut ws,
        serde_json::json!({ "type": "resume_session", "sessionDir": session_b_name }),
    )
    .await;
    let frames = recv_until_type(&mut ws, "ready").await;

    let running_pos = frames
        .iter()
        .position(|v| v["type"] == "session_info" && v["turnState"] == "running")
        .expect("session_info(running) must appear at start of resumption");
    let idle_pos = frames
        .iter()
        .rposition(|v| v["type"] == "session_info" && v["turnState"] == "idle")
        .expect("session_info(idle) must appear at end of resumption");
    let resumed_pos = frames
        .iter()
        .position(|v| v["type"] == "session_resumed")
        .expect("session_resumed must appear");
    let ready_pos = frames.len() - 1;

    assert!(
        running_pos < resumed_pos,
        "session_info(running) must precede session_resumed; frames: {frames:?}"
    );
    assert!(
        resumed_pos < idle_pos,
        "session_resumed must precede session_info(idle); frames: {frames:?}"
    );
    assert_eq!(frames[ready_pos]["type"], "ready");
    assert_eq!(
        idle_pos,
        ready_pos - 1,
        "session_info(idle) must immediately precede ready; frames: {frames:?}"
    );
}

// ---------------------------------------------------------------------------
// handle_rename_session — info_cache update (line 929)
// ---------------------------------------------------------------------------

/// `replace == with != in handle_rename_session` (line 929) — without the
/// guard, renaming the *active* session updates the info_cache only when the
/// session is NOT active (flipped logic). The bug: a subsequent `session_info`
/// frame (triggered by set_effort) would show the old name instead of the new.
#[tokio::test]
async fn rename_active_session_info_cache_reflects_new_name() {
    let tmp = TempDir::new().unwrap();
    let provider = Arc::new(MockProvider::new());
    let addr = spawn_server(make_state(
        Arc::clone(&provider),
        tmp.path().join("sessions"),
    ))
    .await;
    let mut ws = connect(addr).await;
    assert_eq!(recv_json(&mut ws).await["type"], "ready");
    let session_dir = reset_and_ready(&mut ws).await;
    assert!(
        !session_dir.is_empty(),
        "session_dir must be populated after reset"
    );

    // Rename the active session.
    send_json(
        &mut ws,
        serde_json::json!({ "type": "rename_session", "sessionDir": session_dir, "name": "renamed-cache-test" }),
    )
    .await;
    let renamed = recv_json(&mut ws).await;
    assert_eq!(renamed["type"], "session_renamed");

    // Trigger a session_info by setting the effort — any set_* handler emits
    // an event followed by an updated session_info.
    send_json(
        &mut ws,
        serde_json::json!({ "type": "set_effort", "effort": "low" }),
    )
    .await;
    let _ = recv_json(&mut ws).await; // effort_changed event

    // The next session_info must carry the updated name from the cache.
    // There's no guaranteed session_info after set_effort in the current
    // implementation — set_effort only emits the effort_changed event.
    // Instead, reconnect: the reconnect replays history including the cached
    // session info.
    ws.close(None).await.ok();
    drop(ws);
    tokio::time::sleep(Duration::from_millis(60)).await;

    let mut ws2 = connect(addr).await;
    let frames = recv_until_type(&mut ws2, "ready").await;
    let si = frames
        .iter()
        .find(|v| v["type"] == "session_info")
        .expect("session_info must be present on reconnect");
    assert_eq!(
        si["name"], "renamed-cache-test",
        "session_info.name must reflect the cache-updated name; got {si}",
    );
}

// ---------------------------------------------------------------------------
// Subprocess + HTTP fake test — validates ANTHROPIC_BASE_URL hook (TEST-ARCH-2)
// ---------------------------------------------------------------------------

/// End-to-end test that spawns the real `omega-server` binary, points it at
/// the HTTP fake via `ANTHROPIC_BASE_URL`, opens a WebSocket, and drives a
/// single text turn. Validates that the `ANTHROPIC_BASE_URL` env-var hook
/// added to `main.rs` is wired correctly all the way through
/// `AnthropicProvider → RetryingProvider → Agent`.
///
/// This is the one test in the suite that exercises the real LLM HTTP layer;
/// all other tests in this file use the in-process `MockProvider` to keep
/// mutation tests fast.
#[tokio::test(flavor = "multi_thread")]
async fn e2e_full_turn_via_http_fake() {
    use assert_cmd::cargo::cargo_bin;
    use std::process::{Command, Stdio};

    let mock = common::MockServer::start(vec![common::MockResponse::Text {
        text: "omega e2e text".to_owned(),
        input_tokens: 5,
        output_tokens: 3,
    }])
    .await;

    let tmp = TempDir::new().unwrap();
    let sessions_root = tmp.path().join("sessions");
    std::fs::create_dir_all(&sessions_root).unwrap();

    // Find a free port.
    let free_port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };

    let mut child = Command::new(cargo_bin("omega-server"))
        .env("ANTHROPIC_API_KEY", "sk-test-e2e")
        .env("ANTHROPIC_BASE_URL", &mock.base_url)
        .env("OMEGA_RETRY_INITIAL_MS", "1")
        .args([
            "--port",
            &free_port.to_string(),
            "--sessions-root",
            sessions_root.to_str().unwrap(),
            "--public-dir",
            "/dev/null",
        ])
        .stderr(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("failed to spawn omega-server");

    // Wait for the startup banner on stderr (up to 10 s).
    // We do a simple poll: attempt to connect until it succeeds.
    let ws_url = format!("ws://127.0.0.1:{free_port}/ws");
    let mut ws = {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            match tokio_tungstenite::connect_async(&ws_url).await {
                Ok((ws, _)) => break ws,
                Err(_) if std::time::Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(e) => panic!("omega-server did not come up within 10 s: {e}"),
            }
        }
    };

    // Initial ready frame.
    let r0 = recv_json(&mut ws).await;
    assert_eq!(r0["type"], "ready");

    // Reset — creates a session.
    send_json(&mut ws, serde_json::json!({ "type": "reset" })).await;
    let _ = recv_until_type(&mut ws, "ready").await;

    // Send a user message — the server calls the HTTP fake.
    send_json(
        &mut ws,
        serde_json::json!({ "type": "user_message", "content": "hello e2e" }),
    )
    .await;
    let frames = recv_until_type(&mut ws, "turn_end").await;

    let types: Vec<&str> = frames.iter().filter_map(|v| v["type"].as_str()).collect();
    assert!(
        types.contains(&"text"),
        "expected text frame in turn: {types:?}"
    );
    assert_eq!(types.last().copied(), Some("turn_end"));

    // The text frame must carry the mock's response.
    let text_frame = frames.iter().find(|v| v["type"] == "text").unwrap();
    assert_eq!(text_frame["text"], "omega e2e text");

    // Shut down cleanly.
    child.kill().ok();
    child.wait().ok();
}
