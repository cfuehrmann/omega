// Test-idiomatic lints we opt out of in this file:
//   items_after_statements: helper `fn`s declared mid-test for locality
//   too_many_lines: long fixture tables (event variants, SSE traces)
//   match_wildcard_for_single_variants: defensive wildcards in `match`
#![allow(
    clippy::items_after_statements,
    clippy::too_many_lines,
    clippy::match_wildcard_for_single_variants
)]

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
            ..Default::default()
        },
        context_management: None,
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
        system: Some(vec!["You are a helpful assistant.".to_owned()]),
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
            ..Default::default()
        },
        context_management: None,
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
// Request body: adaptive thinking + effort
// ---------------------------------------------------------------------------

/// Assert that `adaptive_thinking: true` + `effort` are forwarded to the
/// Anthropic wire body as `thinking: { type: "adaptive", display: "summarized" }`
/// and `output_config: { effort: "high" }`.
#[tokio::test]
async fn request_body_adaptive_thinking() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .append_header("content-type", "text/event-stream")
                .set_body_string(sse_body(&[
                    (
                        "message_start",
                        json!({
                            "type": "message_start",
                            "message": {
                                "id": "msg_01",
                                "model": "claude-sonnet-4-6",
                                "usage": {
                                    "input_tokens": 10,
                                    "output_tokens": 0
                                }
                            }
                        }),
                    ),
                    (
                        "content_block_start",
                        json!({
                            "type": "content_block_start",
                            "index": 0,
                            "content_block": {"type": "text", "text": ""}
                        }),
                    ),
                    (
                        "content_block_delta",
                        json!({
                            "type": "content_block_delta",
                            "index": 0,
                            "delta": {"type": "text_delta", "text": "ok"}
                        }),
                    ),
                    (
                        "content_block_stop",
                        json!({"type": "content_block_stop", "index": 0}),
                    ),
                    (
                        "message_delta",
                        json!({
                            "type": "message_delta",
                            "delta": {"stop_reason": "end_turn"},
                            "usage": {"output_tokens": 1}
                        }),
                    ),
                    ("message_stop", json!({"type": "message_stop"})),
                ])),
        )
        .mount(&server)
        .await;

    let req = LlmRequest {
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
            adaptive_thinking: true,
            effort: Some("high".to_owned()),
            ..Default::default()
        },
        context_management: None,
    };

    let provider = AnthropicProvider::new("test-key").with_base_url(server.uri());
    collect_ok(&provider, req).await;

    let requests = server
        .received_requests()
        .await
        .expect("wiremock recorded requests");
    let wire_body: Value =
        serde_json::from_slice(&requests[0].body).expect("wire body is valid JSON");

    let thinking = &wire_body["thinking"];
    assert_eq!(
        thinking["type"], "adaptive",
        "thinking.type must be adaptive"
    );
    assert_eq!(
        thinking["display"], "summarized",
        "thinking.display must be summarized"
    );
    let output_config = &wire_body["output_config"];
    assert_eq!(
        output_config["effort"], "high",
        "output_config.effort must be high"
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
    assert_eq!(items.len(), 1, "expected only the LlmResponseEnded event");
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

/// `LlmResponseEndedEvent.time` must be a valid RFC3339 timestamp.
/// Catches: `replace now_iso -> String with String::new()` and
/// `replace now_iso -> String with "xyzzy".into()` mutants.
///
/// SCHEMA-8 Phase 2: `streaming_start` is no longer populated by the
/// parser — the agent owns text-block timing now.  This test asserts
/// it is `None` so a regression that re-introduces server-side
/// synthesis is caught immediately.
#[tokio::test]
async fn response_event_time_fields_are_valid_rfc3339() {
    let server = MockServer::start().await;
    // Plain text response; SCHEMA-8 no longer derives `streaming_start`.
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
            Some(omega_types::OmegaEvent::LlmResponseEnded(r)) => Some(r),
            _ => None,
        })
        .expect("expected LlmResponseEnded event");

    chrono::DateTime::parse_from_rfc3339(&resp.time)
        .expect("LlmResponseEnded.time must be valid RFC3339");
    // Phase 6.5: streaming_start removed from LlmResponseEndedEvent.
}

// ---------------------------------------------------------------------------
// Server-side context compaction (Phase 1d.1d)
//
// These tests exercise the request-shape opt-in (`context_management`),
// the SSE compaction-block parser, the `applied_edits` extraction, and the
// compaction-block → trailing-text → `LlmResponseEnded` ordering at the
// provider level.  They mirror the TS reference points at
// `src/agent.ts:1432–1469` and the SSE shape captured in
// `src/compacted.test.ts`.  (SCHEMA-8 Phase 6.5a removed the separate
// `OmegaEvent::Compacted` variant; compaction is now surfaced via
// `LlmResponseEnded.usage.iterations`.)
// ---------------------------------------------------------------------------

/// Build a minimal happy-path SSE envelope.  Used by tests that don't need
/// to vary the wire response.
fn happy_envelope() -> String {
    sse_body(&[
        (
            "message_start",
            json!({
                "type": "message_start",
                "message": {
                    "id": "msg_cm",
                    "model": "claude-sonnet-4-6",
                    "usage": {"input_tokens": 1, "output_tokens": 0}
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
    ])
}

/// Mount a happy-path mock that simply records the request body.
async fn mount_happy(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(happy_envelope())
                .insert_header("content-type", "text/event-stream"),
        )
        .mount(server)
        .await;
}

/// When `LlmRequest.context_management` is `Some(...)`, the wire body
/// must carry it verbatim.  Establishes the agent-to-provider seam for
/// future per-turn context-management wiring.
#[tokio::test]
async fn request_body_emits_context_management_when_set() {
    let server = MockServer::start().await;
    mount_happy(&server).await;

    let cm = json!({
        "edits": [
            {
                "type": "clear_tool_uses_20250919",
                "trigger": {"type": "input_tokens", "value": 750_000},
                "keep": {"type": "tool_uses", "value": 6}
            }
        ]
    });
    let mut req = simple_request();
    req.context_management = Some(cm.clone());

    let provider = AnthropicProvider::new("test-key").with_base_url(server.uri());
    collect_ok(&provider, req).await;

    let received = server
        .received_requests()
        .await
        .expect("wiremock recorded requests");
    let wire: Value = serde_json::from_slice(&received[0].body).expect("wire body JSON");
    assert_eq!(
        wire["context_management"], cm,
        "context_management must be forwarded verbatim into the wire body"
    );
}

/// When `context_management` is `None`, the field must be absent from
/// the wire body — `skip_serializing_if = Option::is_none`.
#[tokio::test]
async fn request_body_omits_context_management_when_none() {
    let server = MockServer::start().await;
    mount_happy(&server).await;

    let provider = AnthropicProvider::new("test-key").with_base_url(server.uri());
    collect_ok(&provider, simple_request()).await;

    let received = server
        .received_requests()
        .await
        .expect("wiremock recorded requests");
    let wire: Value = serde_json::from_slice(&received[0].body).expect("wire body JSON");
    assert!(
        wire.get("context_management").is_none(),
        "context_management must be absent when None — got {wire}"
    );
}

/// SSE shape with a `compaction` content block followed by a regular
/// text block must yield, in order:
///   1. `Signal::Text` for each text delta of the trailing text block,
///   2. `Signal::TextBlockComplete` for the trailing text block,
///   3. `OmegaEvent::LlmResponseEnded`.
///
/// The compaction block itself produces no items — its presence is
/// only observable via the `iterations[]` entries on
/// `LlmResponseUsage`, which Anthropic emits in `message_delta.usage`
/// when server-side compaction fires.
///
/// SCHEMA-8 Phase 6.5a removed the separate `OmegaEvent::Compacted`
/// variant; compaction is now detected via
/// `LlmResponseEnded.usage.iterations`.  Catches mutants that:
///   - re-introduce a `Compacted` event from the parser,
///   - swap the order so `LlmResponseEnded` precedes the trailing text,
///   - drop the trailing `TextBlockComplete` for the post-compaction
///     text block.
#[tokio::test]
async fn compaction_block_followed_by_text_completes_normally() {
    let server = MockServer::start().await;
    let body = sse_body(&[
        (
            "message_start",
            json!({
                "type": "message_start",
                "message": {
                    "id": "msg_compact",
                    "model": "claude-sonnet-4-6",
                    "usage": {"input_tokens": 80_500, "output_tokens": 0}
                }
            }),
        ),
        (
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": {"type": "compaction", "content": null, "encrypted_content": null}
            }),
        ),
        (
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "compaction_delta", "content": "summary text", "encrypted_content": ""}
            }),
        ),
        (
            "content_block_stop",
            json!({"type": "content_block_stop", "index": 0}),
        ),
        (
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": 1,
                "content_block": {"type": "text", "text": ""}
            }),
        ),
        (
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": 1,
                "delta": {"type": "text_delta", "text": "Hello"}
            }),
        ),
        (
            "content_block_stop",
            json!({"type": "content_block_stop", "index": 1}),
        ),
        (
            "message_delta",
            json!({
                "type": "message_delta",
                "delta": {"stop_reason": "end_turn", "stop_sequence": null},
                "usage": {"output_tokens": 50}
            }),
        ),
        ("message_stop", json!({"type": "message_stop"})),
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

    // Expect: Text("Hello"), TextBlockComplete(index=1, text="Hello"),
    // LlmResponseEnded — in that order.  No separate Compacted event.
    use omega_types::OmegaEvent;
    let mut iter = items.iter();

    match iter.next().expect("first item") {
        AgentItem::Signal(omega_types::StreamSignal::Text { text, index }) => {
            assert_eq!(text, "Hello", "text-delta surfaces normally");
            assert_eq!(*index, 1, "text delta carries the SSE index");
        }
        other => panic!("expected text Signal, got {other:?}"),
    }
    match iter.next().expect("second item") {
        AgentItem::Signal(omega_types::StreamSignal::TextBlockComplete { index, text }) => {
            assert_eq!(*index, 1, "TextBlockComplete carries the SSE index");
            assert_eq!(text, "Hello", "TextBlockComplete carries the full text");
        }
        other => panic!("expected TextBlockComplete signal second, got {other:?}"),
    }
    match iter.next().expect("third item").as_event() {
        Some(OmegaEvent::LlmResponseEnded(_)) => {}
        other => panic!("expected LlmResponseEnded event third, got {other:?}"),
    }
    assert!(iter.next().is_none(), "no further items expected");
}

/// `LlmResponseUsage.iterations` must carry every entry Anthropic
/// sends in the `iterations[]` array of the message-delta usage
/// object — verbatim, including all token-count fields and the `type`
/// discriminator.
///
/// SCHEMA-8 Phase 2: replaces the old
/// `compaction_usage_carries_iterations_verbatim` assertion against
/// `OmegaEvent::Compacted.usage["iterations"]`; iterations are now
/// surfaced typed on `LlmResponseEnded.usage.iterations`.  Catches mutants
/// that drop, re-shape, or re-order the array on the way through
/// `extract_iterations`.
#[tokio::test]
async fn iterations_array_is_carried_to_llm_response_usage() {
    let server = MockServer::start().await;
    let body = sse_body(&[
        (
            "message_start",
            json!({
                "type": "message_start",
                "message": {
                    "id": "msg_iter",
                    "model": "claude-sonnet-4-6",
                    "usage": {
                        "input_tokens": 80_500,
                        "output_tokens": 0,
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
                "content_block": {"type": "compaction"}
            }),
        ),
        (
            "content_block_stop",
            json!({"type": "content_block_stop", "index": 0}),
        ),
        (
            "message_delta",
            json!({
                "type": "message_delta",
                "delta": {"stop_reason": "end_turn", "stop_sequence": null},
                "usage": {
                    "output_tokens": 350,
                    "iterations": [
                        {"type": "compaction", "input_tokens": 80_000, "output_tokens": 300},
                        {"type": "message",    "input_tokens": 500,    "output_tokens": 50}
                    ]
                }
            }),
        ),
        ("message_stop", json!({"type": "message_stop"})),
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

    use omega_types::OmegaEvent;
    let lr = items
        .iter()
        .find_map(|i| match i.as_event() {
            Some(OmegaEvent::LlmResponseEnded(r)) => Some(r),
            _ => None,
        })
        .expect("LlmResponseEnded event present");

    // Anthropic-published top-level fields still ride `usage`.
    assert_eq!(lr.usage.input_tokens, 80_500);
    assert_eq!(lr.usage.output_tokens, 350);
    assert_eq!(lr.usage.service_tier.as_deref(), Some("standard"));

    let iters = lr
        .usage
        .iterations
        .as_ref()
        .expect("iterations array preserved on LlmResponseUsage");
    assert_eq!(iters.len(), 2);
    assert_eq!(iters[0].iteration_type, "compaction");
    assert_eq!(iters[0].input_tokens, 80_000);
    assert_eq!(iters[0].output_tokens, 300);
    assert_eq!(iters[1].iteration_type, "message");
    assert_eq!(iters[1].input_tokens, 500);
    assert_eq!(iters[1].output_tokens, 50);
}

/// `applied_edits` containing `clear_tool_uses_20250919` must populate
/// `LlmResponseEnded.cleared_tool_uses` and `cleared_input_tokens`.
/// Mirrors `src/agent.ts:1455–1469`.
#[tokio::test]
async fn applied_edits_populates_cleared_fields() {
    let server = MockServer::start().await;
    let body = sse_body(&[
        (
            "message_start",
            json!({
                "type": "message_start",
                "message": {
                    "id": "msg_ae",
                    "model": "claude-sonnet-4-6",
                    "usage": {"input_tokens": 100, "output_tokens": 0}
                }
            }),
        ),
        (
            "message_delta",
            json!({
                "type": "message_delta",
                "delta": {"stop_reason": "end_turn", "stop_sequence": null},
                "usage": {"output_tokens": 5},
                "context_management": {
                    "applied_edits": [
                        {
                            "type": "clear_tool_uses_20250919",
                            "cleared_tool_uses": 7,
                            "cleared_input_tokens": 42_000
                        }
                    ]
                }
            }),
        ),
        ("message_stop", json!({"type": "message_stop"})),
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

    use omega_types::OmegaEvent;
    let resp = items
        .iter()
        .find_map(|i| match i.as_event() {
            Some(OmegaEvent::LlmResponseEnded(r)) => Some(r),
            _ => None,
        })
        .expect("LlmResponseEnded present");

    assert_eq!(resp.cleared_tool_uses, Some(7));
    assert_eq!(resp.cleared_input_tokens, Some(42_000));
}

/// `applied_edits` containing only edits we don't react to must leave
/// `cleared_*` as `None`.  Catches mutants that flip the type-tag
/// match (e.g. accept any edit, or accept the wrong one).
#[tokio::test]
async fn applied_edits_other_type_leaves_cleared_fields_none() {
    let server = MockServer::start().await;
    let body = sse_body(&[
        (
            "message_start",
            json!({
                "type": "message_start",
                "message": {
                    "id": "msg_other",
                    "model": "claude-sonnet-4-6",
                    "usage": {"input_tokens": 100, "output_tokens": 0}
                }
            }),
        ),
        (
            "message_delta",
            json!({
                "type": "message_delta",
                "delta": {"stop_reason": "end_turn", "stop_sequence": null},
                "usage": {"output_tokens": 5},
                "context_management": {
                    "applied_edits": [
                        {
                            "type": "clear_thinking_20251015",
                            "cleared_tool_uses": 99,
                            "cleared_input_tokens": 99_999
                        }
                    ]
                }
            }),
        ),
        ("message_stop", json!({"type": "message_stop"})),
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

    use omega_types::OmegaEvent;
    let resp = items
        .iter()
        .find_map(|i| match i.as_event() {
            Some(OmegaEvent::LlmResponseEnded(r)) => Some(r),
            _ => None,
        })
        .expect("LlmResponseEnded present");

    assert!(
        resp.cleared_tool_uses.is_none(),
        "non-matching edit must not populate cleared_tool_uses"
    );
    assert!(
        resp.cleared_input_tokens.is_none(),
        "non-matching edit must not populate cleared_input_tokens"
    );
}

// ---------------------------------------------------------------------------
// BUG-C: prompt-cache markers (RED test — proves zero markers in current code)
// ---------------------------------------------------------------------------

/// Assert that the Anthropic wire body carries exactly three
/// `cache_control: {"type":"ephemeral"}` markers:
///
/// 1. The **last system block** — anchors the full system prompt into cache.
/// 2. The **last tool definition** — anchors all tool schemas into cache.
/// 3. The **last block of the last message** — anchors the full conversation
///    prefix so Anthropic can reuse the cached input prefix on subsequent turns.
///
/// This test was written RED (failing) before the BUG-C fix and is kept as
/// a regression guard.  It proves that all three markers survive the round-trip
/// through `build_request_body` → `reqwest` → `wiremock`.
#[tokio::test]
async fn request_body_has_three_cache_control_markers() {
    let server = MockServer::start().await;
    mount_happy(&server).await;

    let req = LlmRequest {
        model: "claude-opus-4-6".to_owned(),
        system: Some(vec!["You are helpful.".to_owned()]),
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
                    id: "tu_01".to_owned(),
                    name: "calculator".to_owned(),
                    input: json!({"a": 2, "b": 2}),
                }],
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "tu_01".to_owned(),
                    content: "4".to_owned(),
                    is_error: false,
                }],
            },
        ],
        tools: vec![
            ToolDefinition {
                name: "calculator".to_owned(),
                description: "Basic arithmetic.".to_owned(),
                input_schema: json!({"type":"object","properties":{}}),
            },
            ToolDefinition {
                name: "read_file".to_owned(),
                description: "Read a file.".to_owned(),
                input_schema: json!({"type":"object","properties":{}}),
            },
        ],
        config: ModelConfig::default(),
        context_management: None,
    };

    let provider = AnthropicProvider::new("test-key").with_base_url(server.uri());
    collect_ok(&provider, req).await;

    let received = server
        .received_requests()
        .await
        .expect("wiremock recorded requests");
    let wire: Value = serde_json::from_slice(&received[0].body).expect("wire body JSON");

    let ephemeral = json!({"type": "ephemeral"});

    // 1. System — last block must have cache_control.
    let system = wire["system"]
        .as_array()
        .expect("system must be an array (not a bare string)");
    assert!(!system.is_empty(), "system array must not be empty");
    assert_eq!(
        system.last().unwrap()["cache_control"],
        ephemeral,
        "last system block missing cache_control: {wire}"
    );

    // 2. Tools — last tool must have cache_control.
    let tools = wire["tools"].as_array().expect("tools array");
    assert!(!tools.is_empty(), "tools array must not be empty");
    assert_eq!(
        tools.last().unwrap()["cache_control"],
        ephemeral,
        "last tool definition missing cache_control: {wire}"
    );

    // 3. Messages — last block of last message must have cache_control.
    let messages = wire["messages"].as_array().expect("messages array");
    assert!(!messages.is_empty(), "messages array must not be empty");
    let last_msg = messages.last().unwrap();
    let last_content = last_msg["content"].as_array().expect("content array");
    assert!(
        !last_content.is_empty(),
        "last message content must not be empty"
    );
    assert_eq!(
        last_content.last().unwrap()["cache_control"],
        ephemeral,
        "last message block missing cache_control: {wire}"
    );

    // Non-last tools and non-last message blocks must NOT have cache_control.
    assert!(
        tools[0].get("cache_control").is_none(),
        "non-last tool must not have cache_control"
    );
}

/// Plain text-only response must report `LlmResponseUsage.iterations`
/// as `None` — there's no compaction marker to surface, and there is no
/// longer any standalone `Compacted` event (removed Phase 6.5a).
/// Counter-test for
/// `compaction_block_followed_by_text_completes_normally` and
/// `iterations_array_is_carried_to_llm_response_usage`.
#[tokio::test]
async fn non_compacting_response_emits_no_compaction_signal() {
    let server = MockServer::start().await;
    mount_happy(&server).await;

    let provider = AnthropicProvider::new("test-key").with_base_url(server.uri());
    let items = collect_ok(&provider, simple_request()).await;

    use omega_types::OmegaEvent;
    // Phase 6.5: OmegaEvent::Compacted removed from the type; provider never emitted it anyway.

    let lr = items
        .iter()
        .find_map(|i| match i.as_event() {
            Some(OmegaEvent::LlmResponseEnded(r)) => Some(r),
            _ => None,
        })
        .expect("LlmResponseEnded event present");
    assert!(
        lr.usage.iterations.is_none(),
        "plain turn must not surface iterations: {:?}",
        lr.usage.iterations,
    );
}

/// `LlmResponseEndedEvent.time` must be a valid RFC3339 timestamp on a
/// compaction-trigger turn too — catches the standard `now_iso → ""`
/// and `now_iso → "xyzzy"` mutants on the post-compaction code path.
///
/// SCHEMA-8 Phase 2: replaces the old `compacted_event_time_is_valid_rfc3339`
/// test (Compacted is no longer emitted by the parser).
#[tokio::test]
async fn llm_response_time_on_compacting_turn_is_valid_rfc3339() {
    let server = MockServer::start().await;
    let body = sse_body(&[
        (
            "message_start",
            json!({
                "type": "message_start",
                "message": {
                    "id": "msg_t",
                    "model": "claude-sonnet-4-6",
                    "usage": {"input_tokens": 1, "output_tokens": 0}
                }
            }),
        ),
        (
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": {"type": "compaction"}
            }),
        ),
        (
            "content_block_stop",
            json!({"type": "content_block_stop", "index": 0}),
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

    use omega_types::OmegaEvent;
    let lr = items
        .iter()
        .find_map(|i| match i.as_event() {
            Some(OmegaEvent::LlmResponseEnded(r)) => Some(r),
            _ => None,
        })
        .expect("LlmResponseEnded event present on compaction turn");

    chrono::DateTime::parse_from_rfc3339(&lr.time)
        .expect("LlmResponseEnded.time must be valid RFC3339");
}

/// A tool-use SSE sequence must yield:
///   `ToolUseBlockStart` (index 0) → `ToolInput` × N → `ToolUseBlockComplete` (index 0)
///
/// This verifies that the new signals are emitted in the correct order and
/// that server-side accumulation (for `ToolUseBlockComplete.input`) is
/// unaffected by the new forwarding.
#[tokio::test]
async fn tool_use_block_yields_start_deltas_and_complete_signals() {
    let server = MockServer::start().await;
    let body = sse_body(&[
        (
            "message_start",
            json!({
                "type": "message_start",
                "message": {
                    "id": "msg_tu",
                    "model": "claude-sonnet-4-6",
                    "usage": {"input_tokens": 10, "output_tokens": 0}
                }
            }),
        ),
        (
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": {
                    "type": "tool_use",
                    "id": "tu_abc",
                    "name": "read_file",
                    "input": {}
                }
            }),
        ),
        (
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "input_json_delta", "partial_json": "{\"pa"}
            }),
        ),
        (
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "input_json_delta", "partial_json": "th\":\"f.txt\"}"}
            }),
        ),
        (
            "content_block_stop",
            json!({"type": "content_block_stop", "index": 0}),
        ),
        (
            "message_delta",
            json!({
                "type": "message_delta",
                "delta": {"stop_reason": "tool_use"},
                "usage": {"output_tokens": 5}
            }),
        ),
        ("message_stop", json!({"type": "message_stop"})),
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

    use omega_types::StreamSignal;

    // Collect only the Signal items for inspection.
    let signals: Vec<&StreamSignal> = items
        .iter()
        .filter_map(|i| match i {
            AgentItem::Signal(s) => Some(s),
            _ => None,
        })
        .collect();

    // Expected signal sequence: ToolUseBlockStart, ToolInput × 2, ToolUseBlockComplete.
    assert_eq!(signals.len(), 4, "expected 4 signals, got: {signals:?}");

    // 1. ToolUseBlockStart
    match signals[0] {
        StreamSignal::ToolUseBlockStart {
            index,
            tool_use_id,
            name,
        } => {
            assert_eq!(*index, 0);
            assert_eq!(tool_use_id, "tu_abc");
            assert_eq!(name, "read_file");
        }
        s => panic!("expected ToolUseBlockStart, got {s:?}"),
    }

    // 2. First ToolInput delta
    match signals[1] {
        StreamSignal::ToolInput {
            index,
            partial_json,
        } => {
            assert_eq!(*index, 0);
            assert_eq!(partial_json, "{\"pa");
        }
        s => panic!("expected ToolInput, got {s:?}"),
    }

    // 3. Second ToolInput delta
    match signals[2] {
        StreamSignal::ToolInput {
            index,
            partial_json,
        } => {
            assert_eq!(*index, 0);
            assert_eq!(partial_json, "th\":\"f.txt\"}");
        }
        s => panic!("expected ToolInput, got {s:?}"),
    }

    // 4. ToolUseBlockComplete with fully assembled input
    match signals[3] {
        StreamSignal::ToolUseBlockComplete {
            index,
            tool_use_id,
            name,
            input,
        } => {
            assert_eq!(*index, 0);
            assert_eq!(tool_use_id, "tu_abc");
            assert_eq!(name, "read_file");
            assert_eq!(input, &serde_json::json!({"path": "f.txt"}));
        }
        s => panic!("expected ToolUseBlockComplete, got {s:?}"),
    }
}

/// An empty-input tool call (no `input_json_delta` events) must still yield
/// `ToolUseBlockStart` followed immediately by `ToolUseBlockComplete` with
/// `input: {}`.
#[tokio::test]
async fn tool_use_block_empty_input_yields_start_and_complete_only() {
    let server = MockServer::start().await;
    let body = sse_body(&[
        (
            "message_start",
            json!({
                "type": "message_start",
                "message": {
                    "id": "msg_ei",
                    "model": "claude-sonnet-4-6",
                    "usage": {"input_tokens": 5, "output_tokens": 0}
                }
            }),
        ),
        (
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": {
                    "type": "tool_use",
                    "id": "tu_empty",
                    "name": "bash",
                    "input": {}
                }
            }),
        ),
        (
            "content_block_stop",
            json!({"type": "content_block_stop", "index": 0}),
        ),
        (
            "message_delta",
            json!({
                "type": "message_delta",
                "delta": {"stop_reason": "tool_use"},
                "usage": {"output_tokens": 2}
            }),
        ),
        ("message_stop", json!({"type": "message_stop"})),
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

    use omega_types::StreamSignal;
    let signals: Vec<&StreamSignal> = items
        .iter()
        .filter_map(|i| match i {
            AgentItem::Signal(s) => Some(s),
            _ => None,
        })
        .collect();

    assert_eq!(
        signals.len(),
        2,
        "expected 2 signals (start + complete), got: {signals:?}"
    );
    assert!(
        matches!(signals[0], StreamSignal::ToolUseBlockStart { .. }),
        "first signal must be ToolUseBlockStart",
    );
    assert!(
        matches!(signals[1], StreamSignal::ToolUseBlockComplete { input, .. } if input == &serde_json::json!({})),
        "second signal must be ToolUseBlockComplete with empty object",
    );
}
