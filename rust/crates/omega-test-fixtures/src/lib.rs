//! Anthropic-shaped axum SSE fake — shared dev-helper crate.
//!
//! Single source of the LLM HTTP fake used by every binary-level test
//! in the workspace. Plugs into the canonical mocking boundary,
//! [`omega_core::AnthropicProvider::with_base_url`], either directly or
//! via the `ANTHROPIC_BASE_URL` env-var hook in each binary's `main.rs`.
//!
//! Two usage modes:
//!
//! * **One-shot scripted server** ([`MockServer::start`]): start the
//!   server with a pre-loaded queue of [`MockResponse`]s, point the
//!   binary at `mock.base_url`, run the test, drop. Used by the
//!   `omega-cli` and `omega-server` integration tests.
//!
//! * **Long-lived server with mutable script + capture**
//!   ([`router`] + [`Script`] + [`CallHistory`]): the caller hosts the
//!   listener and exposes its own control surface for mutating the
//!   queue / inspecting captured calls. Used by `omega-mock-server` to
//!   bridge the fake to a Playwright-visible HTTP control API.
//!
//! Wire-shape coverage:
//!
//! | Variant         | Stream emitted                                  |
//! |-----------------|-------------------------------------------------|
//! | `Text`          | `text` block → `end_turn`                       |
//! | `SlowText`      | `text` block in N chunks with delay → `end_turn`|
//! | `ToolUse`       | one `tool_use` block → `tool_use`               |
//! | `ToolUseMulti`  | M `tool_use` blocks (different `index`) → `tool_use` |
//! | `HttpError`     | non-200 status with body                        |

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

// ---------------------------------------------------------------------------
// MockResponse
// ---------------------------------------------------------------------------

/// Scripted response for one `/v1/messages` request. Constructible
/// directly in Rust tests (`MockResponse::Text { ... }`) and
/// deserializable from JSON for crates that script the fake over HTTP
/// (e.g. `omega-mock-server`'s `/control/script` endpoint).
///
/// JSON wire format is internally tagged on `kind` with `camelCase`
/// fields; e.g. `{ "kind": "text", "text": "pong" }`.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MockResponse {
    /// Stream a single text block then `end_turn`. Token counts default
    /// to 10/5 if omitted from JSON; when constructed in Rust they must
    /// be specified (the CLI's stderr snapshot tests assert on them).
    #[serde(rename_all = "camelCase")]
    Text {
        text: String,
        #[serde(default = "default_input_tokens")]
        input_tokens: i64,
        #[serde(default = "default_output_tokens")]
        output_tokens: i64,
    },

    /// Stream `text` in `chunks` `text_delta` events separated by
    /// `delay_ms`, then `end_turn`. Used to exercise pause-during-stream
    /// behaviour in real-server e2e tests.
    #[serde(rename_all = "camelCase")]
    SlowText {
        text: String,
        chunks: usize,
        delay_ms: u64,
    },

    /// Stream a single `tool_use` block then `tool_use` stop.
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },

    /// Stream multiple `tool_use` blocks (different `index` per block)
    /// in one response, then `tool_use` stop. Drives the agent into
    /// concurrent-tool dispatch.
    ToolUseMulti { tools: Vec<ToolUseSpec> },

    /// Return a non-200 HTTP status with body. Used for terminal errors
    /// (400) and retryable errors (500).
    HttpError { status: u16, body: String },
}

#[derive(Clone, Debug, Deserialize)]
pub struct ToolUseSpec {
    pub id: String,
    pub name: String,
    pub input: Value,
}

const fn default_input_tokens() -> i64 {
    10
}

const fn default_output_tokens() -> i64 {
    5
}

// ---------------------------------------------------------------------------
// Script (mutable shared queue)
// ---------------------------------------------------------------------------

/// Shared, lock-protected queue of scripted responses. Cloneable
/// `Arc` handle. Each `POST /v1/messages` pops the front entry; an
/// empty queue causes the fake to reply with HTTP 500
/// `"mock: no scripted response left"`.
pub type Script = Arc<Mutex<VecDeque<MockResponse>>>;

#[must_use]
pub fn new_script() -> Script {
    Arc::new(Mutex::new(VecDeque::new()))
}

#[must_use]
pub fn script_from(responses: Vec<MockResponse>) -> Script {
    Arc::new(Mutex::new(VecDeque::from(responses)))
}

// ---------------------------------------------------------------------------
// CallHistory (optional capture surface)
// ---------------------------------------------------------------------------

/// One captured `/v1/messages` request, projected into the shape JS-side
/// tests consume from `omega-mock-server`'s control API.
#[derive(Debug, Clone, Serialize)]
pub struct CapturedCall {
    /// `"task"` for normal turns, `"resumption"` for the synthesised
    /// summary call when a session is resumed (system prompt starts with
    /// `"Summarise the coding session"`).
    #[serde(rename = "systemKind")]
    pub system_kind: &'static str,
    /// Wall-clock millis since the unix epoch.
    pub at: u128,
    /// One entry per message in the request history.
    pub messages: Vec<CapturedMessage>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CapturedMessage {
    pub role: String,
    pub content: String,
}

/// Shared, lock-protected history of every captured call.
#[derive(Clone, Default)]
pub struct CallHistory {
    inner: Arc<Mutex<Vec<CapturedCall>>>,
}

impl CallHistory {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&self, call: CapturedCall) {
        if let Ok(mut g) = self.inner.lock() {
            g.push(call);
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> Vec<CapturedCall> {
        self.inner.lock().map(|g| g.clone()).unwrap_or_default()
    }

    pub fn reset(&self) {
        if let Ok(mut g) = self.inner.lock() {
            g.clear();
        }
    }
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct FakeState {
    script: Script,
    history: Option<CallHistory>,
}

/// Build an axum [`Router`] serving the Anthropic SSE fake.
///
/// Pass `history = None` if you don't need request inspection (the
/// common case for plain unit tests). Pass `Some(history)` to capture
/// every received `/v1/messages` body for later inspection.
pub fn router(script: Script, history: Option<CallHistory>) -> Router {
    Router::new()
        .route("/v1/messages", post(handle_messages))
        .with_state(FakeState { script, history })
}

async fn handle_messages(State(state): State<FakeState>, body: axum::body::Bytes) -> Response {
    // Parse the Anthropic-shaped request body for projection. Failure
    // to parse is non-fatal for fakes that don't capture (we still pop
    // the next response); fatal for fakes that do (the test almost
    // certainly wants to inspect what was sent).
    let parsed = serde_json::from_slice::<AnthropicRequest>(&body);

    if let Some(history) = &state.history {
        match &parsed {
            Ok(req) => history.push(project_call(req)),
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    format!("mock: malformed request body: {e}"),
                )
                    .into_response();
            }
        }
    }

    let next = state.script.lock().ok().and_then(|mut q| q.pop_front());
    let Some(resp) = next else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "mock: no scripted response left",
        )
            .into_response();
    };

    match resp {
        MockResponse::Text {
            text,
            input_tokens,
            output_tokens,
        } => sse_static_response(build_text_sse(&text, input_tokens, output_tokens)),
        MockResponse::SlowText {
            text,
            chunks,
            delay_ms,
        } => sse_slow_text_response(&text, chunks, Duration::from_millis(delay_ms)),
        MockResponse::ToolUse { id, name, input } => {
            sse_static_response(build_tool_use_sse(&[ToolUseSpec { id, name, input }]))
        }
        MockResponse::ToolUseMulti { tools } => sse_static_response(build_tool_use_sse(&tools)),
        MockResponse::HttpError { status, body } => (
            StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            body,
        )
            .into_response(),
    }
}

// ---------------------------------------------------------------------------
// Anthropic request projection
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct AnthropicRequest {
    /// `system` is now an array of blocks (`[{type, text, cache_control?}]`)
    /// since BUG-C stamped cache markers on the system prompt.  Accept
    /// `Value` so we tolerate both the old plain-string shape (legacy tests)
    /// and the new array shape without deserialisation failures.
    #[serde(default)]
    system: Option<Value>,
    #[serde(default)]
    messages: Vec<RawMessage>,
}

#[derive(Deserialize)]
struct RawMessage {
    role: String,
    content: Value,
}

fn project_call(req: &AnthropicRequest) -> CapturedCall {
    // Extract the first text-block content from the system field.
    // Handles both:
    //   - old shape: system = "plain string"
    //   - new shape: system = [{type:"text",text:"billing_header"},{type:"text",text:"actual_prompt",...}]
    // For the resumption-detection heuristic we want the actual prompt text
    // (last text block), not the billing header (first block).
    let system_text: Option<String> = match &req.system {
        None => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Array(blocks)) => {
            // Last text block carries the real system prompt.
            blocks
                .iter()
                .rev()
                .find_map(|b| b.get("text").and_then(Value::as_str).map(str::to_owned))
        }
        Some(other) => other.as_str().map(str::to_owned),
    };
    let system_kind = if system_text
        .as_deref()
        .is_some_and(|s| s.starts_with("Summarise the coding session"))
    {
        "resumption"
    } else {
        "task"
    };

    let messages = req.messages.iter().map(project_message).collect();

    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());

    CapturedCall {
        system_kind,
        at,
        messages,
    }
}

fn project_message(m: &RawMessage) -> CapturedMessage {
    // Single-block plain-text content → expose as a plain string so
    // substring assertions are convenient. Anything else (multi-block,
    // tool_use, tool_result) → JSON-stringify the whole content array.
    let content = match &m.content {
        Value::String(s) => s.clone(),
        Value::Array(arr) => {
            if let [block] = arr.as_slice()
                && block.get("type").and_then(Value::as_str) == Some("text")
                && let Some(t) = block.get("text").and_then(Value::as_str)
            {
                return CapturedMessage {
                    role: m.role.clone(),
                    content: t.to_owned(),
                };
            }
            serde_json::to_string(arr).unwrap_or_default()
        }
        other => other.to_string(),
    };
    CapturedMessage {
        role: m.role.clone(),
        content,
    }
}

// ---------------------------------------------------------------------------
// MockServer (one-shot convenience)
// ---------------------------------------------------------------------------

/// One-shot scripted server. Binds 127.0.0.1 on a random port and
/// serves until dropped. Tests point a binary at [`MockServer::base_url`]
/// via `ANTHROPIC_BASE_URL` (or by constructing the provider directly
/// with `with_base_url`).
pub struct MockServer {
    pub base_url: String,
    /// Remaining scripted responses. Mutable post-start: tests can push
    /// extra responses or peek at how many remain.
    pub script: Script,
    /// Captured request history. `None` unless the server was started
    /// with [`MockServer::start_with_capture`].
    pub history: Option<CallHistory>,
    handle: JoinHandle<()>,
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

impl MockServer {
    /// Start a fake without request capture. The most common test setup.
    pub async fn start(responses: Vec<MockResponse>) -> Self {
        Self::start_inner(script_from(responses), None).await
    }

    /// Start a fake that captures every received request into a
    /// [`CallHistory`] reachable via [`MockServer::history`].
    pub async fn start_with_capture(responses: Vec<MockResponse>) -> Self {
        Self::start_inner(script_from(responses), Some(CallHistory::new())).await
    }

    async fn start_inner(script: Script, history: Option<CallHistory>) -> Self {
        let app = router(script.clone(), history.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let base_url = format!("http://{addr}");
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        Self {
            base_url,
            script,
            history,
            handle,
        }
    }
}

// ---------------------------------------------------------------------------
// SSE construction (also exposed for advanced callers)
// ---------------------------------------------------------------------------

fn sse_static_response(body: String) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .header("cache-control", "no-cache")
        .body(axum::body::Body::from(body))
        .unwrap()
}

fn sse_slow_text_response(text: &str, chunks: usize, delay: Duration) -> Response {
    let chunks_n = chunks.max(1);
    let chunk_size = text.len().div_ceil(chunks_n);

    let mut events: Vec<String> = Vec::new();
    events.push(format_event(
        "message_start",
        &json!({ "message": { "usage": { "input_tokens": default_input_tokens() } } }),
    ));
    events.push(format_event(
        "content_block_start",
        &json!({ "index": 0, "content_block": { "type": "text", "text": "" } }),
    ));
    for chunk in text.as_bytes().chunks(chunk_size) {
        let s = String::from_utf8_lossy(chunk).into_owned();
        events.push(format_event(
            "content_block_delta",
            &json!({ "index": 0, "delta": { "type": "text_delta", "text": s } }),
        ));
    }
    events.push(format_event("content_block_stop", &json!({ "index": 0 })));
    events.push(format_event(
        "message_delta",
        &json!({
            "delta": { "stop_reason": "end_turn" },
            "usage": { "output_tokens": default_output_tokens() }
        }),
    ));
    events.push(format_event("message_stop", &json!({})));

    let s = stream::iter(events).then(move |evt| async move {
        tokio::time::sleep(delay).await;
        Ok::<String, std::convert::Infallible>(evt)
    });

    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .header("cache-control", "no-cache")
        .body(axum::body::Body::from_stream(s))
        .unwrap()
}

/// Build the Anthropic SSE byte sequence for a single `text` block.
/// Public so callers that want to feed the bytes into a different
/// transport can reuse the wire-format encoding.
#[must_use]
pub fn build_text_sse(text: &str, input_tokens: i64, output_tokens: i64) -> String {
    let mut out = String::new();
    push_event(
        &mut out,
        "message_start",
        &json!({ "message": { "usage": { "input_tokens": input_tokens } } }),
    );
    push_event(
        &mut out,
        "content_block_start",
        &json!({ "index": 0, "content_block": { "type": "text", "text": "" } }),
    );
    push_event(
        &mut out,
        "content_block_delta",
        &json!({ "index": 0, "delta": { "type": "text_delta", "text": text } }),
    );
    push_event(&mut out, "content_block_stop", &json!({ "index": 0 }));
    push_event(
        &mut out,
        "message_delta",
        &json!({
            "delta": { "stop_reason": "end_turn" },
            "usage": { "output_tokens": output_tokens }
        }),
    );
    push_event(&mut out, "message_stop", &json!({}));
    out
}

/// Build the Anthropic SSE byte sequence for one or more `tool_use`
/// blocks in a single response. Multi-block responses drive the agent
/// into concurrent-tool dispatch.
#[must_use]
pub fn build_tool_use_sse(tools: &[ToolUseSpec]) -> String {
    let mut out = String::new();
    push_event(
        &mut out,
        "message_start",
        &json!({ "message": { "usage": { "input_tokens": default_input_tokens() } } }),
    );
    for (i, t) in tools.iter().enumerate() {
        push_event(
            &mut out,
            "content_block_start",
            &json!({
                "index": i,
                "content_block": { "type": "tool_use", "id": t.id, "name": t.name, "input": {} }
            }),
        );
        push_event(
            &mut out,
            "content_block_delta",
            &json!({
                "index": i,
                "delta": { "type": "input_json_delta", "partial_json": t.input.to_string() }
            }),
        );
        push_event(&mut out, "content_block_stop", &json!({ "index": i }));
    }
    push_event(
        &mut out,
        "message_delta",
        &json!({
            "delta": { "stop_reason": "tool_use" },
            "usage": { "output_tokens": default_output_tokens() }
        }),
    );
    push_event(&mut out, "message_stop", &json!({}));
    out
}

fn push_event(out: &mut String, event: &str, data: &Value) {
    out.push_str("event: ");
    out.push_str(event);
    out.push('\n');
    out.push_str("data: ");
    out.push_str(&data.to_string());
    out.push_str("\n\n");
}

fn format_event(event: &str, data: &Value) -> String {
    let mut s = String::new();
    push_event(&mut s, event, data);
    s
}
