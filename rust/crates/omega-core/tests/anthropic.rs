//! Integration tests for `AnthropicProvider`.
//!
//! These tests stand up a `wiremock` server that speaks the Anthropic
//! `/v1/messages` SSE protocol and assert that the provider translates
//! it into the correct `AgentItem` sequence.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use futures::StreamExt;
use omega_core::{
    AgentItem, AnthropicProvider, ContentBlock, LlmError, LlmRequest, Message, ModelConfig,
    Provider, Role, ToolDefinition,
};

mod common;
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a simple `LlmRequest` with one user turn — enough to exercise
/// the streaming path.
fn simple_request() -> LlmRequest {
    LlmRequest {
        model: "claude-sonnet-4-6".to_owned(),
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Hi".to_owned(),
            }],
        }],
        system: None,
        tools: vec![],
        config: ModelConfig {
            max_tokens: 1024,
            temperature: None,
            thinking_budget: None,
        },
    }
}

/// Compose an SSE response body from `(event, data)` pairs.
fn sse_body(events: &[(&str, Value)]) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    for (event, data) in events {
        writeln!(out, "event: {event}").unwrap();
        writeln!(out, "data: {data}\n").unwrap();
    }
    out
}

/// Drain a stream into a vector — fails the test on an unexpected error.
async fn collect_ok(provider: &AnthropicProvider, req: LlmRequest) -> Vec<AgentItem> {
    let mut stream = provider.stream(req);
    let mut out = Vec::new();
    while let Some(item) = stream.next().await {
        out.push(item.expect("stream yielded unexpected error"));
    }
    out
}

/// Project an `AgentItem` to a JSON `Value` for snapshotting.  Time fields
/// are redacted via `insta` at the assertion site below.
fn project(items: &[AgentItem]) -> Value {
    Value::Array(
        items
            .iter()
            .map(|i| serde_json::to_value(i).expect("AgentItem serializes"))
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// Request-body kitchen-sink snapshot
// ---------------------------------------------------------------------------

/// Snapshot the full `LlmRequest → Anthropic wire body` transformation.
///
/// Includes: multi-turn conversation with a tool-use / tool-result pair
/// (id-correlated), system prompt, two tool definitions, non-default
/// `ModelConfig`.  Both the input projection and the captured wire body
/// are included in the snapshot so the transformation is self-explanatory.
///
/// `[id_1]` appears in both the `tool_use.id` and `tool_result.tool_use_id`
/// positions, proving they carry the same value end-to-end.
#[tokio::test]
async fn request_body_kitchen_sink() {
    let server = MockServer::start().await;

    // Mount a minimal happy-path response so the provider completes.
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(sse_body(&[
                    (
                        "message_start",
                        json!({
                            "type": "message_start",
                            "message": {
                                "id": "msg_ks",
                                "model": "claude-opus-4-6",
                                "usage": {"input_tokens": 10, "output_tokens": 0}
                            }
                        }),
                    ),
                    (
                        "message_delta",
                        json!({
                            "type": "message_delta",
                            "delta": {"stop_reason": "end_turn", "stop_sequence": null},
                            "usage": {"output_tokens": 1}
                        }),
                    ),
                    ("message_stop", json!({"type": "message_stop"})),
                ]))
                .insert_header("content-type", "text/event-stream"),
        )
        .mount(&server)
        .await;

    let tool_id = "toolu_ks_01";
    let req = LlmRequest {
        model: "claude-opus-4-6".to_owned(),
        system: Some("You are a helpful assistant.".to_owned()),
        messages: vec![
            Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: "What is 2+2?".to_owned(),
                }],
            },
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: tool_id.to_owned(),
                    name: "calculator".to_owned(),
                    input: json!({"a": 2, "b": 2}),
                }],
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: tool_id.to_owned(),
                    content: "4".to_owned(),
                    is_error: false,
                }],
            },
        ],
        tools: vec![
            ToolDefinition {
                name: "calculator".to_owned(),
                description: "Performs basic arithmetic.".to_owned(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "a": {"type": "number"},
                        "b": {"type": "number"}
                    },
                    "required": ["a", "b"]
                }),
            },
            ToolDefinition {
                name: "read_file".to_owned(),
                description: "Reads a file from disk.".to_owned(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"}
                    },
                    "required": ["path"]
                }),
            },
        ],
        config: ModelConfig {
            max_tokens: 2_048,
            temperature: Some(0.5),
            thinking_budget: None,
        },
    };

    // Serialise input *before* consuming `req`.
    let input = serde_json::to_value(&req).expect("LlmRequest serialises");

    let provider = AnthropicProvider::new("test-key").with_base_url(server.uri());
    collect_ok(&provider, req).await;

    let requests = server
        .received_requests()
        .await
        .expect("wiremock recorded requests");
    let wire_body: Value =
        serde_json::from_slice(&requests[0].body).expect("wire body is valid JSON");

    let r = common::id_redactor();
    insta::assert_json_snapshot!(
        json!({"input": input, "wire_body": wire_body}),
        {
            ".**.id"          => r.redaction(),
            ".**.tool_use_id" => r.redaction(),
        }
    );
}

// ---------------------------------------------------------------------------
// Happy path: text + thinking + tool_use
// ---------------------------------------------------------------------------

#[tokio::test]
#[allow(clippy::too_many_lines)] // The fixture body is long but linear.
async fn streams_text_thinking_and_tool_use() {
    let server = MockServer::start().await;

    let body = sse_body(&[
        (
            "message_start",
            json!({
                "type": "message_start",
                "message": {
                    "id": "msg_01",
                    "model": "claude-sonnet-4-6",
                    "usage": {
                        "input_tokens": 25,
                        "output_tokens": 1,
                        "cache_creation_input_tokens": 10,
                        "cache_read_input_tokens": 5,
                        "service_tier": "standard"
                    }
                }
            }),
        ),
        (
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": { "type": "thinking", "thinking": "" }
            }),
        ),
        (
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": { "type": "thinking_delta", "thinking": "Let me think." }
            }),
        ),
        (
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": { "type": "signature_delta", "signature": "sig123" }
            }),
        ),
        (
            "content_block_stop",
            json!({ "type": "content_block_stop", "index": 0 }),
        ),
        (
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": 1,
                "content_block": { "type": "text", "text": "" }
            }),
        ),
        (
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": 1,
                "delta": { "type": "text_delta", "text": "Hello" }
            }),
        ),
        (
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": 1,
                "delta": { "type": "text_delta", "text": ", world!" }
            }),
        ),
        (
            "content_block_stop",
            json!({ "type": "content_block_stop", "index": 1 }),
        ),
        (
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": 2,
                "content_block": { "type": "tool_use", "id": "toolu_42", "name": "calc", "input": {} }
            }),
        ),
        (
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": 2,
                "delta": { "type": "input_json_delta", "partial_json": "{\"a\":" }
            }),
        ),
        (
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": 2,
                "delta": { "type": "input_json_delta", "partial_json": "1,\"b\":2}" }
            }),
        ),
        (
            "content_block_stop",
            json!({ "type": "content_block_stop", "index": 2 }),
        ),
        (
            "message_delta",
            json!({
                "type": "message_delta",
                "delta": { "stop_reason": "tool_use", "stop_sequence": null },
                "usage": {
                    "output_tokens": 87,
                    "cache_creation_input_tokens": 10,
                    "cache_read_input_tokens": 5
                }
            }),
        ),
        ("message_stop", json!({ "type": "message_stop" })),
    ]);

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "test-key"))
        .and(header("anthropic-version", "2023-06-01"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(body)
                .insert_header("content-type", "text/event-stream"),
        )
        .mount(&server)
        .await;

    let provider = AnthropicProvider::new("test-key").with_base_url(server.uri());
    let items = collect_ok(&provider, simple_request()).await;

    insta::assert_json_snapshot!(
        project(&items),
        {
            "[].time" => "[time]",
            "[].streamingStart" => "[time]",
        }
    );
}

// ---------------------------------------------------------------------------
// HTTP error → LlmError::Http with retry-after parsed
// ---------------------------------------------------------------------------

#[tokio::test]
async fn maps_429_to_http_error_with_retry_after() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "3")
                .set_body_string(
                    r#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#,
                ),
        )
        .mount(&server)
        .await;

    let provider = AnthropicProvider::new("test-key").with_base_url(server.uri());
    let mut stream = provider.stream(simple_request());

    let first = stream.next().await.expect("expected one error item");
    match first {
        Err(LlmError::Http {
            status,
            retry_after,
            body,
        }) => {
            assert_eq!(status, 429);
            assert_eq!(retry_after, Some(Duration::from_secs(3)));
            assert!(body.contains("rate_limit_error"), "body was {body:?}");
        }
        other => panic!("expected LlmError::Http, got {other:?}"),
    }
    assert!(stream.next().await.is_none(), "stream must end after error");
}

// ---------------------------------------------------------------------------
// SSE error event mid-stream → LlmError::Stream
// ---------------------------------------------------------------------------

#[tokio::test]
async fn maps_sse_error_event_to_stream_error() {
    let server = MockServer::start().await;
    let body = sse_body(&[(
        "error",
        json!({
            "type": "error",
            "error": { "type": "overloaded_error", "message": "server overloaded" }
        }),
    )]);

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(body)
                .insert_header("content-type", "text/event-stream"),
        )
        .mount(&server)
        .await;

    let provider = AnthropicProvider::new("test-key").with_base_url(server.uri());
    let mut stream = provider.stream(simple_request());

    let first = stream.next().await.expect("expected one item");
    match first {
        Err(LlmError::Stream { message }) => {
            assert!(
                message.contains("overloaded_error"),
                "message was {message:?}"
            );
        }
        other => panic!("expected LlmError::Stream, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Beta header propagated when configured
// ---------------------------------------------------------------------------

#[tokio::test]
async fn propagates_beta_header() {
    let server = MockServer::start().await;
    let body = sse_body(&[
        (
            "message_start",
            json!({
                "type": "message_start",
                "message": {
                    "id": "msg_x",
                    "model": "claude-opus-4-7",
                    "usage": { "input_tokens": 1, "output_tokens": 1 }
                }
            }),
        ),
        (
            "message_delta",
            json!({
                "type": "message_delta",
                "delta": { "stop_reason": "end_turn", "stop_sequence": null },
                "usage": { "output_tokens": 1 }
            }),
        ),
        ("message_stop", json!({ "type": "message_stop" })),
    ]);

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("anthropic-beta", "interleaved-thinking-2025-05-14"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(body)
                .insert_header("content-type", "text/event-stream"),
        )
        .mount(&server)
        .await;

    let provider = AnthropicProvider::new("test-key")
        .with_base_url(server.uri())
        .with_beta("interleaved-thinking-2025-05-14");

    let items = collect_ok(&provider, simple_request()).await;
    assert_eq!(items.len(), 1, "expected only the LlmResponse event");
}

// ---------------------------------------------------------------------------
// parse_retry_after edge cases
// ---------------------------------------------------------------------------

/// `retry-after: 0` → `Some(Duration::ZERO)` — not None.
/// Catches: `replace < with <=` and `replace < with ==` mutants.
#[tokio::test]
async fn parse_retry_after_zero_is_some_zero() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "0")
                .set_body_string("{}"),
        )
        .mount(&server)
        .await;

    let provider = AnthropicProvider::new("test-key").with_base_url(server.uri());
    let mut stream = provider.stream(simple_request());
    match stream.next().await.expect("expected item") {
        Err(LlmError::Http { retry_after, .. }) => {
            assert_eq!(
                retry_after,
                Some(Duration::ZERO),
                "retry-after:0 must give Some(ZERO)"
            );
        }
        other => panic!("expected LlmError::Http, got {other:?}"),
    }
}

/// `retry-after: -1` → `None` (negative delay is invalid).
/// Catches: `replace || with &&` and `replace < with ==` mutants.
#[tokio::test]
async fn parse_retry_after_negative_is_none() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "-1")
                .set_body_string("{}"),
        )
        .mount(&server)
        .await;

    let provider = AnthropicProvider::new("test-key").with_base_url(server.uri());
    let mut stream = provider.stream(simple_request());
    match stream.next().await.expect("expected item") {
        Err(LlmError::Http { retry_after, .. }) => {
            assert_eq!(retry_after, None, "retry-after:-1 must give None");
        }
        other => panic!("expected LlmError::Http, got {other:?}"),
    }
}

/// `retry-after: inf` and `retry-after: nan` → `None` (non-finite is invalid).
/// Catches: `delete ! in parse_retry_after` (which inverts the `is_finite` check).
#[tokio::test]
async fn parse_retry_after_nonfinite_is_none() {
    for bad_value in &["inf", "nan", "-inf"] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(
                ResponseTemplate::new(429)
                    .insert_header("retry-after", *bad_value)
                    .set_body_string("{}"),
            )
            .mount(&server)
            .await;

        let provider = AnthropicProvider::new("test-key").with_base_url(server.uri());
        let mut stream = provider.stream(simple_request());
        match stream.next().await.expect("expected item") {
            Err(LlmError::Http { retry_after, .. }) => {
                assert_eq!(retry_after, None, "retry-after:{bad_value} must give None");
            }
            other => panic!("expected LlmError::Http, got {other:?}"),
        }
    }
}

// ---------------------------------------------------------------------------
// now_iso produces valid RFC3339
// ---------------------------------------------------------------------------

/// `LlmResponse.time` and `LlmResponse.streaming_start` must be valid
/// RFC3339 timestamps, not empty strings or nonsense.
/// Catches: `replace now_iso -> String with String::new()` and
/// `replace now_iso -> String with "xyzzy".into()` mutants.
#[tokio::test]
async fn response_event_time_fields_are_valid_rfc3339() {
    let server = MockServer::start().await;
    // Include a text block so `streaming_start` is populated.
    let body = sse_body(&[
        (
            "message_start",
            json!({
                "type": "message_start",
                "message": {
                    "id": "msg_ts",
                    "model": "claude-sonnet-4-6",
                    "usage": { "input_tokens": 1, "output_tokens": 0 }
                }
            }),
        ),
        (
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": { "type": "text", "text": "" }
            }),
        ),
        (
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": { "type": "text_delta", "text": "Hi" }
            }),
        ),
        (
            "content_block_stop",
            json!({ "type": "content_block_stop", "index": 0 }),
        ),
        (
            "message_delta",
            json!({
                "type": "message_delta",
                "delta": { "stop_reason": "end_turn", "stop_sequence": null },
                "usage": { "output_tokens": 1 }
            }),
        ),
        ("message_stop", json!({ "type": "message_stop" })),
    ]);

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(body)
                .insert_header("content-type", "text/event-stream"),
        )
        .mount(&server)
        .await;

    let provider = AnthropicProvider::new("test-key").with_base_url(server.uri());
    let items = collect_ok(&provider, simple_request()).await;

    let resp = items
        .iter()
        .find_map(|i| match i.as_event() {
            Some(omega_protocol::OmegaEvent::LlmResponse(r)) => Some(r),
            _ => None,
        })
        .expect("expected LlmResponse event");

    chrono::DateTime::parse_from_rfc3339(&resp.time)
        .expect("LlmResponse.time must be valid RFC3339");

    if let Some(ss) = &resp.streaming_start {
        chrono::DateTime::parse_from_rfc3339(ss)
            .expect("LlmResponse.streaming_start must be valid RFC3339");
    }
}
