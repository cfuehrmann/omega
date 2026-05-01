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
    Provider, Role,
};
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
