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
//! - `router.rs:379 delete match arm OmegaEvent::HaltRequested(_)` — arm
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
use omega_server::{AppState, build_router};
use omega_types::events::{LlmResponseEndedEvent, ToolCallEvent};
use omega_types::{OmegaEvent, StreamSignal};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message as TMessage;

mod common;

// ---------------------------------------------------------------------------
// MockProvider — replays a queue of transcripts, one per `stream()` call.
// Same pattern as tests/ws.rs, kept local to avoid cross-test coupling.
//
// `step_gate` is the deterministic-sync hook used by the pause/abort tests
// to block the LLM stream between items.  See tests/ws.rs for the full
// rationale; the short version is that the prior timing-based
// `set_item_delay(30 ms)` slack was flaky under heavy parallel-test CPU
// load (the WS control-frame round-trip didn't fit in 30 ms).
// ---------------------------------------------------------------------------

struct MockProvider {
    responses: Mutex<VecDeque<Vec<Result<AgentItem, LlmError>>>>,
    /// If `Some`, the stream awaits `notify_one()` between successive items
    /// within a single response (after the first item).  Lets the test park
    /// the agent at a known point so a WS control frame can be processed
    /// server-side before the agent advances.
    step_gate: Mutex<Option<Arc<tokio::sync::Notify>>>,
    /// Per-item sleep (only used by the `history_streaming_flag_*` tests,
    /// which need wall-clock slack to connect a second WS during streaming;
    /// the pause/abort tests use `step_gate` instead).
    item_delay: Mutex<Option<Duration>>,
}

impl MockProvider {
    fn new() -> Self {
        Self {
            responses: Mutex::new(VecDeque::new()),
            step_gate: Mutex::new(None),
            item_delay: Mutex::new(None),
        }
    }

    fn push(&self, items: Vec<Result<AgentItem, LlmError>>) {
        self.responses.lock().unwrap().push_back(items);
    }

    /// Install a step gate; the returned `Notify` releases one item per
    /// `notify_one()` call.  Notifications are not lost — `Notify` stores
    /// one permit if no waiter is currently parked.
    fn enable_step_gate(&self) -> Arc<tokio::sync::Notify> {
        let n = Arc::new(tokio::sync::Notify::new());
        *self.step_gate.lock().unwrap() = Some(Arc::clone(&n));
        n
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
        let gate = self.step_gate.lock().unwrap().clone();
        let delay = *self.item_delay.lock().unwrap();
        let base = futures::stream::iter(items.into_iter().enumerate());
        let s: BoxStream<'static, Result<AgentItem, LlmError>> =
            Box::pin(base.then(move |(i, item)| {
                let gate = gate.clone();
                async move {
                    // `step_gate` only fires *between* items (after the
                    // first), to give the test a parking point for the
                    // agent.  `item_delay` matches the original behaviour
                    // and sleeps before every item including the first.
                    if i > 0
                        && let Some(g) = gate
                    {
                        g.notified().await;
                    }
                    if let Some(d) = delay {
                        tokio::time::sleep(d).await;
                    }
                    item
                }
            }));
        s
    }
}

// ---------------------------------------------------------------------------
// Shared test helpers
// ---------------------------------------------------------------------------

type WsClient =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

fn make_state(provider: Arc<MockProvider>, sessions_root: PathBuf) -> AppState {
    AppState::new(provider, sessions_root, PathBuf::from("."))
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

/// Receive next Text frame with a 30 s timeout; decode as JSON.
///
/// 30 s (increased from 5 s) makes the helper robust when the host is under
/// CPU load from parallel Rust compilation (e.g. during mutation sweeps).
/// Normal test execution sees frames in < 500 ms; the generous ceiling only
/// matters when the tokio scheduler is starved.
async fn recv_json(ws: &mut WsClient) -> serde_json::Value {
    let frame = tokio::time::timeout(Duration::from_secs(30), ws.next())
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

/// Perform reset and drain to `ready`, `monitor_roster`, and `input_queue`;
/// return the session dir name from the `session_info` frame.
async fn reset_and_ready(ws: &mut WsClient) -> String {
    send_json(
        ws,
        serde_json::json!({ "type": "reset", "allowDirty": true }),
    )
    .await;
    let frames = recv_until_type(ws, "ready").await;
    // Drain the ephemeral snapshots pushed right after `ready`.
    let roster = recv_json(ws).await;
    assert_eq!(
        roster["type"], "monitor_roster",
        "expected monitor_roster after reset ready; got {roster:?}"
    );
    let queue = recv_json(ws).await;
    assert_eq!(
        queue["type"], "input_queue",
        "expected input_queue after monitor_roster; got {queue:?}"
    );
    frames
        .iter()
        .find(|v| v["type"] == "session_info")
        .and_then(|v| v["dir"].as_str())
        .unwrap_or_default()
        .to_owned()
}

/// Redact volatile JSON fields (time, dir, cwd, contextHash, hasPendingChanges)
/// so snapshots are stable across runs.
fn redact(mut v: serde_json::Value) -> serde_json::Value {
    if let Some(obj) = v.as_object_mut() {
        for key in &["time", "dir", "cwd", "contextHash", "hasPendingChanges"] {
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

/// Like [`llm_response_event`] but includes a `Signal:Text` so
/// `assistant_blocks` is non-empty, preventing the empty-response guard
/// from injecting an unwanted continuation user message.
fn text_llm_response_event(stop_reason: &str) -> Vec<Result<AgentItem, LlmError>> {
    vec![
        Ok(AgentItem::Signal(StreamSignal::Text {
            index: 0,
            text: "ok".to_owned(),
        })),
        Ok(llm_response_event(stop_reason)),
    ]
}

fn llm_response_event(stop_reason: &str) -> AgentItem {
    AgentItem::event(OmegaEvent::LlmResponseEnded(LlmResponseEndedEvent {
        time: "2024-01-01T00:00:00.000Z".to_owned(),
        stop_reason: stop_reason.to_owned(),
        cleared_tool_uses: None,
        cleared_input_tokens: None,
        usage: omega_types::LlmResponseUsage {
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
    id: &str,
    name: &str,
    input: serde_json::Value,
) -> Vec<Result<AgentItem, LlmError>> {
    vec![
        Ok(AgentItem::event(OmegaEvent::ToolCall(ToolCallEvent {
            time: "2024-01-01T00:00:00.000Z".to_owned(),
            tool_call_id: id.to_owned(),
            name: name.to_owned(),
            input,
            context_hash: String::new(),
        }))),
        Ok(llm_response_event("tool_use")),
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
            index: 0,
            text: "hi".to_owned(),
        })),
        Ok(llm_response_event("end_turn")),
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
    // Park the agent on the step gate after consuming item 0 so the WS
    // round-trip for `abort` is guaranteed to land before the agent advances.
    let step_gate = provider.enable_step_gate();
    provider.push(tool_use_items(
        "tu_abort",
        "read_file",
        serde_json::json!({ "path": scratch.to_string_lossy() }),
    ));
    // Second turn — only reached if abort failed.
    provider.push(text_llm_response_event("end_turn"));

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
    // `llm_response_started` is the earliest WS frame that proves the
    // agent has consumed item 0 and is parked on the step gate.  We can't
    // wait for `tool_call` here because that's only emitted post-drain.
    let _ = recv_until_type(&mut ws, "llm_response_started").await;
    send_json(&mut ws, serde_json::json!({ "type": "abort" })).await;
    // `handle_abort` has no server→client ack frame, so use an
    // invalid-JSON sync probe: WS frames are processed in order, so once
    // `agent_error` for the probe arrives, the prior `abort` has been
    // processed and the cancel token cancelled.
    ws.send(TMessage::Text("sync-probe".to_owned().into()))
        .await
        .unwrap();
    let _ = recv_until_type(&mut ws, "agent_error").await;
    step_gate.notify_one();
    let _ = recv_until_type(&mut ws, "turn_interrupted").await;

    // session_info with turnState="idle" must immediately follow.
    let info = recv_json(&mut ws).await;
    assert_eq!(info["type"], "session_info");
    assert_eq!(
        info["turnState"], "idle",
        "turnState must be idle after abort; got {info}"
    );
}

/// `delete match arm OmegaEvent::TurnHalted(_)` (line 380) — without this
/// arm the `session_info` frame with `turnState: "halted"` is never sent.
///
/// §15 (U3): drives a halt/resume cycle and asserts `session_info.turnState`
/// is `"halted"` immediately after `turn_halted`, then `resume` (no input)
/// continues the parked turn to `turn_end`.
#[tokio::test]
async fn turn_halted_emits_session_info_with_halted_turn_state() {
    let tmp = TempDir::new().unwrap();
    let scratch = {
        let p = tmp.path().join("scratch.txt");
        std::fs::write(&p, "y").unwrap();
        p
    };
    let provider = Arc::new(MockProvider::new());
    let step_gate = provider.enable_step_gate();
    provider.push(tool_use_items(
        "tu_pause",
        "read_file",
        serde_json::json!({ "path": scratch.to_string_lossy() }),
    ));
    provider.push(text_llm_response_event("end_turn"));

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
    // Wait for `llm_response_started` — the agent is now parked on the
    // step gate after consuming item 0.  See the abort variant above
    // for why we don't wait for `tool_call` here.
    let _ = recv_until_type(&mut ws, "llm_response_started").await;
    send_json(&mut ws, serde_json::json!({ "type": "halt" })).await;
    // `handle_halt` emits `halt_requested` *after* `request_halt()` has
    // set the agent's halt flag, so observing that frame is a positive
    // confirmation that the flag is set.
    let _ = recv_until_type(&mut ws, "halt_requested").await;
    step_gate.notify_one();
    // Drain until turn_halted: the agent appended the tool_result and
    // PARKED at the next seam instead of starting the next block.
    let frames = recv_until_type(&mut ws, "turn_halted").await;

    // The session_info with turnState="halted" must be the frame
    // immediately after turn_halted.
    let info = recv_json(&mut ws).await;
    assert_eq!(info["type"], "session_info");
    assert_eq!(
        info["turnState"], "halted",
        "turnState must be halted after turn_halted; got {info}\n(preceding frames: {frames:?})",
    );

    // Resume with NO new input ("never mind, carry on"): the parked loop
    // wakes and continues the turn.
    send_json(&mut ws, serde_json::json!({ "type": "resume" })).await;
    let _ = recv_until_type(&mut ws, "turn_resumed").await;
    // text_llm_response_event produces 2 items; item 1 (llm_response_event)
    // is gated by the step gate — release it so the second block can finish.
    step_gate.notify_one();
    let _ = recv_until_type(&mut ws, "turn_end").await;
}

// ---------------------------------------------------------------------------
// handle_halt — turn-state guard (line 752)
// ---------------------------------------------------------------------------

/// `replace != with == in handle_halt` (line 752, the `"halt_requested"`
/// guard) — without this guard, a second `halt` sent while the state is
/// already `"halt_requested"` would re-send a `session_info` frame with
/// stale data. More importantly, the mutant flips the condition so the
/// session_info frame is never sent when a *first* halt arrives in
/// `"running"` state.
///
/// Assert that after `controls.request_halt()` is called, a `session_info`
/// frame with `turnState: "halt_requested"` arrives on the socket.
#[tokio::test]
async fn halt_emits_session_info_with_halt_requested_turn_state() {
    let tmp = TempDir::new().unwrap();
    let scratch = {
        let p = tmp.path().join("scratch.txt");
        std::fs::write(&p, "z").unwrap();
        p
    };
    let provider = Arc::new(MockProvider::new());
    let step_gate = provider.enable_step_gate();
    provider.push(tool_use_items(
        "tu_pr",
        "read_file",
        serde_json::json!({ "path": scratch.to_string_lossy() }),
    ));
    provider.push(text_llm_response_event("end_turn"));

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
    // Agent is parked on the step gate once we see `llm_response_started`.
    let _ = recv_until_type(&mut ws, "llm_response_started").await;
    send_json(&mut ws, serde_json::json!({ "type": "halt" })).await;

    // Collect frames until we see halt_requested event, then check the
    // immediately following session_info.  Because the agent is parked on
    // the gate, no agent-emitted frames can interleave between
    // `halt_requested` and the session_info `handle_halt` sends next.
    let halt_frames = recv_until_type(&mut ws, "halt_requested").await;
    // session_info with "halt_requested" must follow.
    let info = recv_json(&mut ws).await;
    assert_eq!(
        info["type"], "session_info",
        "expected session_info after halt_requested; got {info}\n(halt frames: {halt_frames:?})"
    );
    assert_eq!(
        info["turnState"], "halt_requested",
        "turnState must be halt_requested; got {info}",
    );

    // Release the gate so the agent reaches the seam and parks (turn_halted),
    // then resume to wind the session down cleanly.
    step_gate.notify_one();
    let _ = recv_until_type(&mut ws, "turn_halted").await;
    send_json(&mut ws, serde_json::json!({ "type": "resume" })).await;
    // text_llm_response_event has 2 items; item 1 (llm_response_event) needs
    // a second gate release for the resumed (second) block.
    step_gate.notify_one();
    let _ = recv_until_type(&mut ws, "turn_end").await;
}

// ---------------------------------------------------------------------------
// §15 (U3) DOG-FOODING: ordinary human↔agent coding turns end-to-end.
// ---------------------------------------------------------------------------

/// **Migration lifeline (non-negotiable acceptance criterion).** An ordinary
/// human turn must work end-to-end over the WebSocket under the unified input
/// model: `user_message` enqueues, the persistent run loop drains it, a
/// multi-cycle turn streams (model → tool_call → tool_result → model), it ends
/// with `turn_end` (block exit → `turnState: "idle"`, parked), and a SECOND
/// `user_message` is drained immediately from the parked seam.
#[tokio::test]
async fn dogfood_multi_cycle_human_turn_streams_then_parks_and_drains_next() {
    let tmp = TempDir::new().unwrap();
    let scratch = {
        let p = tmp.path().join("scratch.txt");
        std::fs::write(&p, "hello").unwrap();
        p
    };
    let provider = Arc::new(MockProvider::new());
    // Turn 1: a 2-cycle turn (tool_use → then end_turn).
    provider.push(tool_use_items(
        "tu_dog1",
        "read_file",
        serde_json::json!({ "path": scratch.to_string_lossy() }),
    ));
    provider.push(text_llm_response_event("end_turn"));
    // Turn 2: a single-cycle turn drained from the parked seam.
    provider.push(text_llm_response_event("end_turn"));

    let addr = spawn_server(make_state(
        Arc::clone(&provider),
        tmp.path().join("sessions"),
    ))
    .await;
    let mut ws = connect(addr).await;
    assert_eq!(recv_json(&mut ws).await["type"], "ready");
    reset_and_ready(&mut ws).await;

    // ----- Turn 1: enqueue + stream a multi-cycle turn -----
    send_json(
        &mut ws,
        serde_json::json!({ "type": "user_message", "content": "read it" }),
    )
    .await;
    // The turn must actually advance through a tool cycle.
    let _ = recv_until_type(&mut ws, "tool_call").await;
    let _ = recv_until_type(&mut ws, "tool_result").await;
    let _ = recv_until_type(&mut ws, "turn_end").await;
    // Block exit → parked idle.
    let info = recv_json(&mut ws).await;
    assert_eq!(info["type"], "session_info");
    assert_eq!(
        info["turnState"], "idle",
        "turnState must be idle after turn_end (parked); got {info}"
    );

    // ----- Turn 2: Send while parked → drained immediately -----
    send_json(
        &mut ws,
        serde_json::json!({ "type": "user_message", "content": "again" }),
    )
    .await;
    let _ = recv_until_type(&mut ws, "turn_end").await;
}

// ---------------------------------------------------------------------------
// §15 (U3) Halt + resume-via-queued-steering-message.
// ---------------------------------------------------------------------------

/// The flagship U3 use case: agent heading the wrong way → user halts → it
/// parks at the next seam → user composes a steering message at leisure →
/// queues it → the parked loop resumes WITH that input injected.
///
/// Asserts the queued `user_message` (sent while `halted`) both wakes the
/// park and is injected as a `user_message` event before `turn_resumed`.
#[tokio::test]
async fn halt_then_queued_steering_message_resumes_with_injection() {
    let tmp = TempDir::new().unwrap();
    let scratch = {
        let p = tmp.path().join("scratch.txt");
        std::fs::write(&p, "x").unwrap();
        p
    };
    let provider = Arc::new(MockProvider::new());
    let step_gate = provider.enable_step_gate();
    provider.push(tool_use_items(
        "tu_steer",
        "read_file",
        serde_json::json!({ "path": scratch.to_string_lossy() }),
    ));
    provider.push(text_llm_response_event("end_turn"));

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
    let _ = recv_until_type(&mut ws, "llm_response_started").await;
    send_json(&mut ws, serde_json::json!({ "type": "halt" })).await;
    let _ = recv_until_type(&mut ws, "halt_requested").await;
    step_gate.notify_one();
    let _ = recv_until_type(&mut ws, "turn_halted").await;

    // Compose & queue a steering message while parked.  It must wake the
    // park, be injected as a `user_message` event, then `turn_resumed`.
    send_json(
        &mut ws,
        serde_json::json!({ "type": "user_message", "content": "steer left" }),
    )
    .await;
    let frames = recv_until_type(&mut ws, "turn_resumed").await;
    let injected = frames
        .iter()
        .find(|v| v["type"] == "user_message")
        .unwrap_or_else(|| panic!("no user_message injected before turn_resumed; got {frames:?}"));
    assert_eq!(
        injected["content"], "steer left",
        "injected steering message must carry the queued content; got {injected}"
    );
    // Continue to the end of the resumed block.
    step_gate.notify_one();
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
    provider.push(text_llm_response_event("end_turn"));

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
            index: 0,
            text: "hi".to_owned(),
        })),
        Ok(llm_response_event("end_turn")),
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
    provider.push(text_llm_response_event("end_turn"));

    let addr = spawn_server(make_state(Arc::clone(&provider), sessions_root)).await;
    let mut ws = connect(addr).await;
    assert_eq!(recv_json(&mut ws).await["type"], "ready");
    reset_and_ready(&mut ws).await;

    send_json(
        &mut ws,
        serde_json::json!({ "type": "resume_session", "sessionDir": session_b_name, "allowDirty": true }),
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
// Tool-input streaming signal forwarding (Step 10 / tool-input-streaming.md)
// ---------------------------------------------------------------------------

/// Drive `MockProvider` with a `ToolUseBlockStart` + 2× `ToolInput` +
/// `ToolUseBlockComplete` and assert the three WS frames arrive **in order**
/// with the correct `type` tags.
///
/// This simultaneously validates step-3 agent forwarding and step-4 server
/// transparency: `WsMessage::Item(Box<AgentItem>)` is `#[serde(untagged)]`,
/// so the new `StreamSignal` variants pass through without any production-code
/// change to `omega-server`.
#[tokio::test]
async fn tool_input_streaming_frames_arrive_in_order() {
    let tmp = TempDir::new().unwrap();
    let provider = Arc::new(MockProvider::new());
    provider.push(vec![
        Ok(AgentItem::Signal(StreamSignal::ToolUseBlockStart {
            index: 2,
            tool_use_id: "tu_abc".to_owned(),
            name: "bash".to_owned(),
        })),
        Ok(AgentItem::Signal(StreamSignal::ToolInput {
            index: 2,
            partial_json: r#"{"cmd": "#.to_owned(),
        })),
        Ok(AgentItem::Signal(StreamSignal::ToolInput {
            index: 2,
            partial_json: r#""echo hi"}"#.to_owned(),
        })),
        Ok(AgentItem::Signal(StreamSignal::ToolUseBlockComplete {
            index: 2,
            tool_use_id: "tu_abc".to_owned(),
            name: "bash".to_owned(),
            input: serde_json::json!({"cmd": "echo hi"}),
        })),
        Ok(llm_response_event("tool_use")),
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
        serde_json::json!({ "type": "user_message", "content": "run" }),
    )
    .await;

    // Drain until llm_response_ended (which arrives after the tool_use stop).
    let frames = recv_until_type(&mut ws, "llm_response_ended").await;
    let types: Vec<&str> = frames.iter().filter_map(|v| v["type"].as_str()).collect();

    // Assert the streaming frames are present and in order.
    let pos_start = types.iter().position(|&t| t == "tool_use_block_start");
    let pos_input1 = types.iter().position(|&t| t == "tool_input");
    let pos_input2 = types.iter().rposition(|&t| t == "tool_input");
    // ToolUseBlockComplete is consumed by the agent and re-emitted as
    // the settled OmegaEvent “tool_use_block” (not the raw signal tag).
    let pos_settled = types.iter().position(|&t| t == "tool_use_block");

    assert!(
        pos_start.is_some(),
        "tool_use_block_start frame must be present; got {types:?}"
    );
    assert!(
        pos_input1.is_some() && pos_input2.is_some() && pos_input1 != pos_input2,
        "two tool_input frames must be present; got {types:?}"
    );
    assert!(
        pos_settled.is_some(),
        "tool_use_block (settled) frame must be present; got {types:?}"
    );
    assert!(
        pos_start < pos_input1 && pos_input1 < pos_input2 && pos_input2 < pos_settled,
        "frames must arrive in start → input1 → input2 → settled order; got {types:?}"
    );

    // Spot-check payload fields on the start frame.
    let start_frame = frames
        .iter()
        .find(|v| v["type"] == "tool_use_block_start")
        .unwrap();
    assert_eq!(start_frame["index"], 2);
    assert_eq!(start_frame["tool_use_id"], "tu_abc");
    assert_eq!(start_frame["name"], "bash");

    // Spot-check one of the input frames.
    let input_frame = frames.iter().find(|v| v["type"] == "tool_input").unwrap();
    assert_eq!(input_frame["index"], 2);
    assert!(input_frame["partial_json"].is_string());
}

/// Empty-input case: `ToolUseBlockStart` with no following `ToolInput`
/// deltas settles cleanly via `ToolUseBlockComplete`.
/// Verifies that the buffer drain works when `partial_json` is empty.
#[tokio::test]
async fn tool_input_streaming_empty_input_drains_cleanly() {
    let tmp = TempDir::new().unwrap();
    let provider = Arc::new(MockProvider::new());
    provider.push(vec![
        Ok(AgentItem::Signal(StreamSignal::ToolUseBlockStart {
            index: 0,
            tool_use_id: "tu_empty".to_owned(),
            name: "no_args_tool".to_owned(),
        })),
        // No ToolInput frames — tool was called with `input: {}`.
        Ok(AgentItem::Signal(StreamSignal::ToolUseBlockComplete {
            index: 0,
            tool_use_id: "tu_empty".to_owned(),
            name: "no_args_tool".to_owned(),
            input: serde_json::json!({}),
        })),
        Ok(llm_response_event("tool_use")),
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
        serde_json::json!({ "type": "user_message", "content": "run" }),
    )
    .await;

    let frames = recv_until_type(&mut ws, "llm_response_ended").await;
    let types: Vec<&str> = frames.iter().filter_map(|v| v["type"].as_str()).collect();

    assert!(
        types.contains(&"tool_use_block_start"),
        "tool_use_block_start must arrive; got {types:?}"
    );
    // No tool_input frames expected.
    assert!(
        !types.contains(&"tool_input"),
        "no tool_input frames expected for empty input; got {types:?}"
    );
    // ToolUseBlockComplete becomes the settled "tool_use_block" OmegaEvent.
    assert!(
        types.contains(&"tool_use_block"),
        "tool_use_block (settled) must arrive; got {types:?}"
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
    send_json(
        &mut ws,
        serde_json::json!({ "type": "reset", "allowDirty": true }),
    )
    .await;
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

// ---------------------------------------------------------------------------
// §15 U1 — dog-fooding invariant: one persistent run task per session
// ---------------------------------------------------------------------------

/// Two sequential human messages must BOTH be processed by the single
/// persistent `Agent::run` task spawned at reset.
///
/// Before U1 the server acquired the agent lock per message and held it
/// across parking, so a second `user_message` (arriving while the first
/// turn's stream was parked) deadlocked on that lock.  Under the unified
/// input model `handle_user_message` only pushes to the inbox and the run
/// task — the sole lock holder — drains it, so the second turn runs.
#[tokio::test(flavor = "multi_thread")]
async fn two_sequential_user_messages_share_one_run_task() {
    let tmp = TempDir::new().unwrap();
    let provider = Arc::new(MockProvider::new());
    // Two independent single-cycle turns.
    provider.push(vec![
        Ok(AgentItem::Signal(StreamSignal::Text {
            index: 0,
            text: "one".to_owned(),
        })),
        Ok(llm_response_event("end_turn")),
    ]);
    provider.push(vec![
        Ok(AgentItem::Signal(StreamSignal::Text {
            index: 0,
            text: "two".to_owned(),
        })),
        Ok(llm_response_event("end_turn")),
    ]);
    let addr = spawn_server(make_state(
        Arc::clone(&provider),
        tmp.path().join("sessions"),
    ))
    .await;
    let mut ws = connect(addr).await;
    assert_eq!(recv_json(&mut ws).await["type"], "ready");
    reset_and_ready(&mut ws).await;

    // First message → first turn completes.
    send_json(
        &mut ws,
        serde_json::json!({ "type": "user_message", "content": "first" }),
    )
    .await;
    let first = recv_until_type(&mut ws, "turn_end").await;
    assert!(
        first
            .iter()
            .any(|v| v["type"] == "user_message" && v["content"] == "first"),
        "first turn must echo its user_message; got {first:?}"
    );

    // Second message → MUST be processed by the SAME parked run task. If the
    // old per-message lock survived, this recv would hang (deadlock).
    send_json(
        &mut ws,
        serde_json::json!({ "type": "user_message", "content": "second" }),
    )
    .await;
    let second = recv_until_type(&mut ws, "turn_end").await;
    assert!(
        second
            .iter()
            .any(|v| v["type"] == "user_message" && v["content"] == "second"),
        "second turn must be processed by the same run task; got {second:?}"
    );
}

/// §15 U1 — session-end reaping is preserved.
///
/// Monitor *delivery* is dark in U1, but spawn/reap stay intact.  Resetting
/// while a monitor is live must wind down the prior session's run task and
/// reap the monitor, persisting `MonitorStopped(StoppedBySessionEnd)` to the
/// outgoing session's `events.jsonl`.  This is the server-level proof that
/// `teardown_prior_run` actually fires (the persistent run task otherwise
/// holds the agent lock forever, so reaping cannot happen without it).
#[tokio::test(flavor = "multi_thread")]
async fn reset_reaps_prior_sessions_live_monitor() {
    let tmp = TempDir::new().unwrap();
    let sessions_root = tmp.path().join("sessions");
    let provider = Arc::new(MockProvider::new());
    // Turn cycle 1: spawn a long-lived monitor via the `monitor` tool.
    provider.push(tool_use_items(
        "m1",
        "monitor",
        serde_json::json!({ "description": "watcher", "command": "sleep 60" }),
    ));
    // Turn cycle 2: end the turn so the run loop parks.
    provider.push(text_llm_response_event("end_turn"));
    let addr = spawn_server(make_state(Arc::clone(&provider), sessions_root.clone())).await;
    let mut ws = connect(addr).await;
    assert_eq!(recv_json(&mut ws).await["type"], "ready");
    let dir_a = reset_and_ready(&mut ws).await;

    // Drive a turn that spawns the monitor, then parks.
    send_json(
        &mut ws,
        serde_json::json!({ "type": "user_message", "content": "watch" }),
    )
    .await;
    let _ = recv_until_type(&mut ws, "turn_end").await;

    // Reset again: this must tear down session A's run task and reap its
    // monitor before the new session goes live.
    let _dir_b = reset_and_ready(&mut ws).await;

    // Session A's log must now carry the session-end stop for the monitor.
    let events = std::fs::read_to_string(sessions_root.join(&dir_a).join("events.jsonl"))
        .expect("session A events.jsonl must exist");
    assert!(
        events.contains("monitor_started"),
        "monitor must have been spawned in session A; got:\n{events}"
    );
    assert!(
        events.contains("monitor_stopped"),
        "reset must reap the live monitor and persist MonitorStopped to session A; got:\n{events}"
    );
}

// ===========================================================================
// §15 U1 — InputQueue snapshot tests
// ===========================================================================

/// After enqueueing a human message, an `input_queue` WS frame is sent
/// that contains the pending item.
///
/// Design: `handle_user_message` calls `input_queue.push()` which returns an
/// atomic snapshot (item is guaranteed to be in the queue at snapshot time),
/// then sends `WsMessage::InputQueue` via the WS sender.
#[tokio::test(flavor = "multi_thread")]
async fn input_queue_snapshot_sent_on_enqueue() {
    let tmp = TempDir::new().unwrap();
    let sessions_root = tmp.path().join("sessions");
    let provider = Arc::new(MockProvider::new());
    // One text turn so the agent has something to reply with.
    provider.push(text_llm_response_event("end_turn"));

    let addr = spawn_server(make_state(Arc::clone(&provider), sessions_root.clone())).await;
    let mut ws = connect(addr).await;
    assert_eq!(recv_json(&mut ws).await["type"], "ready");
    let _ = reset_and_ready(&mut ws).await;

    // Send a user message and collect all frames until the turn ends.
    send_json(
        &mut ws,
        serde_json::json!({ "type": "user_message", "content": "hello queue" }),
    )
    .await;
    let frames = recv_until_type(&mut ws, "turn_end").await;

    // At least one `input_queue` frame must have been received.
    let queue_frames: Vec<_> = frames
        .iter()
        .filter(|f| f["type"] == "input_queue")
        .collect();
    assert!(
        !queue_frames.is_empty(),
        "at least one input_queue frame must be sent after enqueue; got frames: {frames:?}"
    );

    // At least one of those frames must contain the pending item.
    let had_item = queue_frames
        .iter()
        .any(|f| f["items"].as_array().is_some_and(|arr| !arr.is_empty()));
    assert!(
        had_item,
        "at least one input_queue frame must show the pending item; got queue frames: {queue_frames:?}"
    );

    // The first item in the first non-empty frame must have source == "human".
    let first_with_item = queue_frames
        .iter()
        .find(|f| f["items"].as_array().is_some_and(|arr| !arr.is_empty()))
        .unwrap();
    assert_eq!(
        first_with_item["items"][0]["source"], "human",
        "queue item source must be 'human'; got: {first_with_item:?}"
    );
}

/// After the agent drains the pending item (visible as the `user_message`
/// event flowing through `spawn_run_task`), an `input_queue` frame is
/// sent showing an empty queue.
#[tokio::test(flavor = "multi_thread")]
async fn input_queue_snapshot_empty_after_drain() {
    let tmp = TempDir::new().unwrap();
    let sessions_root = tmp.path().join("sessions");
    let provider = Arc::new(MockProvider::new());
    provider.push(text_llm_response_event("end_turn"));

    let addr = spawn_server(make_state(Arc::clone(&provider), sessions_root.clone())).await;
    let mut ws = connect(addr).await;
    assert_eq!(recv_json(&mut ws).await["type"], "ready");
    let _ = reset_and_ready(&mut ws).await;

    send_json(
        &mut ws,
        serde_json::json!({ "type": "user_message", "content": "drain me" }),
    )
    .await;
    let frames = recv_until_type(&mut ws, "turn_end").await;

    // There must be an input_queue frame that shows 0 items (queue drained).
    let had_empty = frames
        .iter()
        .any(|f| f["type"] == "input_queue" && f["items"].as_array().is_some_and(Vec::is_empty));
    assert!(
        had_empty,
        "an input_queue frame with 0 items must be sent after drain; got frames: {frames:?}"
    );
}

/// `input_queue` WS frames must NOT be persisted to `events.jsonl`.
///
/// This is a transport-only projection (like `MonitorRoster`): it carries
/// ephemeral UI state, not domain events.
#[tokio::test(flavor = "multi_thread")]
async fn input_queue_frame_not_persisted_to_events_jsonl() {
    let tmp = TempDir::new().unwrap();
    let sessions_root = tmp.path().join("sessions");
    let provider = Arc::new(MockProvider::new());
    provider.push(text_llm_response_event("end_turn"));

    let addr = spawn_server(make_state(Arc::clone(&provider), sessions_root.clone())).await;
    let mut ws = connect(addr).await;
    assert_eq!(recv_json(&mut ws).await["type"], "ready");
    let dir = reset_and_ready(&mut ws).await;

    send_json(
        &mut ws,
        serde_json::json!({ "type": "user_message", "content": "persist check" }),
    )
    .await;
    let _ = recv_until_type(&mut ws, "turn_end").await;

    let events_path = sessions_root.join(&dir).join("events.jsonl");
    let events = std::fs::read_to_string(&events_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", events_path.display()));

    assert!(
        !events.contains("input_queue"),
        "input_queue must NOT appear in events.jsonl; got:\n{events}"
    );
}

// ---------------------------------------------------------------------------
// U2 (§15) — monitor enqueues reach the WS layer
// ---------------------------------------------------------------------------

/// U2 (§15 / §9 always-visible): when a MONITOR enqueues output onto the
/// inbox, the server pushes an `input_queue` WS frame whose item carries a
/// `monitor:<id>` source — not only on human enqueue.  Proves the
/// `InputQueue::on_change` callback registered in `spawn_run_task` reaches
/// the WS layer for manager-side (monitor) pushes too.
#[tokio::test(flavor = "multi_thread")]
async fn input_queue_frame_on_monitor_enqueue() {
    let tmp = TempDir::new().unwrap();
    let sessions_root = tmp.path().join("sessions");
    let provider = Arc::new(MockProvider::new());
    // Turn 1: the model spawns a short-lived monitor via the (hidden) tool.
    provider.push(tool_use_items(
        "tu_mon",
        "monitor",
        serde_json::json!({
            "description": "ws monitor",
            "command": "printf 'wsline\\n'; sleep 2",
        }),
    ));
    // Generous terminal turns: post-tool continuation + the monitor-delivery
    // turn(s) the enqueue wakes.  Surplus responses are harmless.
    for _ in 0..6 {
        provider.push(text_llm_response_event("end_turn"));
    }

    let addr = spawn_server(make_state(Arc::clone(&provider), sessions_root.clone())).await;
    let mut ws = connect(addr).await;
    assert_eq!(recv_json(&mut ws).await["type"], "ready");

    // Reset with the (otherwise hidden) `monitor` tool enabled for this session.
    send_json(
        &mut ws,
        serde_json::json!({
            "type": "reset",
            "allowDirty": true,
            "toolSelection": ["monitor", "run_command"],
        }),
    )
    .await;
    let _ = recv_until_type(&mut ws, "ready").await;

    send_json(
        &mut ws,
        serde_json::json!({ "type": "user_message", "content": "watch it" }),
    )
    .await;

    // Recv frames until an input_queue frame carries a `monitor:<id>` item.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let mut found = false;
    let mut frame_count = 0usize;
    while tokio::time::Instant::now() < deadline {
        let v = match tokio::time::timeout(Duration::from_secs(10), ws.next()).await {
            Ok(Some(Ok(TMessage::Text(t)))) => {
                serde_json::from_str::<serde_json::Value>(&t).unwrap()
            }
            _ => break,
        };
        frame_count += 1;
        if v["type"] == "input_queue"
            && v["items"].as_array().is_some_and(|items| {
                items.iter().any(|it| {
                    it["source"]
                        .as_str()
                        .is_some_and(|s| s.starts_with("monitor:"))
                })
            })
        {
            found = true;
            break;
        }
    }
    assert!(
        found,
        "expected an input_queue WS frame with a monitor:<id> source after monitor enqueue; saw {frame_count} frames",
    );
}
