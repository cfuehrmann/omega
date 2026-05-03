//! Anthropic-shaped axum SSE fixture.
//!
//! Per-test scripted responses on a 127.0.0.1 random port. Tests point
//! the CLI at it via `ANTHROPIC_BASE_URL`. Each POST `/v1/messages` pops
//! one [`MockResponse`] from the script in order.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use serde_json::json;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

/// Scripted response for one `/v1/messages` request.
#[derive(Clone, Debug)]
pub enum MockResponse {
    /// Stream a single text block then `end_turn`.
    Text {
        text: String,
        input_tokens: i64,
        output_tokens: i64,
    },
    /// Stream a single `tool_use` block then `tool_use` stop.
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// Return a non-200 HTTP status with body. Used for terminal errors
    /// (400) and retryable errors (500).
    HttpError { status: u16, body: String },
}

type Script = Arc<Mutex<VecDeque<MockResponse>>>;

pub struct MockServer {
    pub base_url: String,
    handle: JoinHandle<()>,
    /// Number of requests served. Useful for retry-test assertions.
    pub script: Script,
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

impl MockServer {
    pub async fn start(responses: Vec<MockResponse>) -> Self {
        let script: Script = Arc::new(Mutex::new(VecDeque::from(responses)));
        let app = Router::new()
            .route("/v1/messages", post(handle_messages))
            .with_state(script.clone());

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let base_url = format!("http://{addr}");
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        Self {
            base_url,
            handle,
            script,
        }
    }
}

async fn handle_messages(
    State(script): State<Script>,
    _headers: HeaderMap,
    _body: axum::body::Bytes,
) -> Response {
    let next = script.lock().unwrap().pop_front();
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
        } => sse_text(&text, input_tokens, output_tokens).into_response(),
        MockResponse::ToolUse { id, name, input } => {
            sse_tool_use(&id, &name, &input).into_response()
        }
        MockResponse::HttpError { status, body } => (
            StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            body,
        )
            .into_response(),
    }
}

/// Build a streaming response that emits an Anthropic-shaped SSE
/// sequence ending in a single text block.
fn sse_text(text: &str, input_tokens: i64, output_tokens: i64) -> Response {
    let body = build_text_sse(text, input_tokens, output_tokens);
    sse_response(body)
}

fn sse_tool_use(id: &str, name: &str, input: &serde_json::Value) -> Response {
    let body = build_tool_use_sse(id, name, input);
    sse_response(body)
}

fn sse_response(body: String) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .header("cache-control", "no-cache")
        .body(axum::body::Body::from(body))
        .unwrap()
}

fn build_text_sse(text: &str, input_tokens: i64, output_tokens: i64) -> String {
    let mut out = String::new();
    sse_event(
        &mut out,
        "message_start",
        &json!({
            "message": { "usage": { "input_tokens": input_tokens } }
        }),
    );
    sse_event(
        &mut out,
        "content_block_start",
        &json!({ "index": 0, "content_block": { "type": "text", "text": "" } }),
    );
    sse_event(
        &mut out,
        "content_block_delta",
        &json!({ "index": 0, "delta": { "type": "text_delta", "text": text } }),
    );
    sse_event(&mut out, "content_block_stop", &json!({ "index": 0 }));
    sse_event(
        &mut out,
        "message_delta",
        &json!({
            "delta": { "stop_reason": "end_turn" },
            "usage": { "output_tokens": output_tokens }
        }),
    );
    sse_event(&mut out, "message_stop", &json!({}));
    out
}

fn build_tool_use_sse(id: &str, name: &str, input: &serde_json::Value) -> String {
    let mut out = String::new();
    sse_event(
        &mut out,
        "message_start",
        &json!({ "message": { "usage": { "input_tokens": 1 } } }),
    );
    sse_event(
        &mut out,
        "content_block_start",
        &json!({
            "index": 0,
            "content_block": { "type": "tool_use", "id": id, "name": name, "input": {} }
        }),
    );
    sse_event(
        &mut out,
        "content_block_delta",
        &json!({
            "index": 0,
            "delta": { "type": "input_json_delta", "partial_json": input.to_string() }
        }),
    );
    sse_event(&mut out, "content_block_stop", &json!({ "index": 0 }));
    sse_event(
        &mut out,
        "message_delta",
        &json!({
            "delta": { "stop_reason": "tool_use" },
            "usage": { "output_tokens": 1 }
        }),
    );
    sse_event(&mut out, "message_stop", &json!({}));
    out
}

fn sse_event(out: &mut String, event: &str, data: &serde_json::Value) {
    out.push_str("event: ");
    out.push_str(event);
    out.push('\n');
    out.push_str("data: ");
    out.push_str(&data.to_string());
    out.push_str("\n\n");
}

/// Replace temp-dir paths with a stable placeholder for snapshots.
pub fn normalize_temp_paths(s: &str, temp_dir: &Path) -> String {
    let p = temp_dir.to_string_lossy().into_owned();
    s.replace(&p, "[TEMP_DIR]")
}

/// Replace the session-dir line `Session: <root>/<timestamp>-<hex>` with
/// a stable placeholder. The session-dir name embeds wallclock + random
/// bytes, so the literal path is never reproducible across runs.
pub fn normalize_session_line(s: &str) -> String {
    let mut out = String::new();
    for line in s.split_inclusive('\n') {
        if let Some(rest) = line.strip_prefix("Session: ") {
            let _ = rest;
            out.push_str("Session: [SESSION_DIR]\n");
        } else {
            out.push_str(line);
        }
    }
    out
}
