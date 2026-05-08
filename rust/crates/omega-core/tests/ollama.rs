//! Integration tests for `OllamaProvider`.
//!
//! Stands up a `wiremock` server that emits NDJSON chunks shaped like
//! Ollama's `/api/chat` responses and asserts the provider lifts them
//! into the right `AgentItem` sequence.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use futures::StreamExt;
use omega_core::{
    AgentItem, ContentBlock, LlmError, LlmRequest, Message, ModelConfig, OllamaProvider, Provider,
    Role, ToolDefinition,
};
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

mod common;

fn simple_request() -> LlmRequest {
    LlmRequest {
        model: "llama3.2".to_owned(),
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

/// Concatenate NDJSON lines (one object per line, terminated by `\n`).
fn ndjson(lines: &[Value]) -> String {
    let mut out = String::new();
    for line in lines {
        out.push_str(&line.to_string());
        out.push('\n');
    }
    out
}

async fn collect_ok(provider: &OllamaProvider, req: LlmRequest) -> Vec<AgentItem> {
    let mut stream = provider.stream(req);
    let mut out = Vec::new();
    while let Some(item) = stream.next().await {
        out.push(item.expect("stream yielded unexpected error"));
    }
    out
}

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

/// Snapshot the full `LlmRequest → Ollama wire body` transformation.
///
/// Includes: multi-turn conversation with a tool-use / tool-result pair
/// (id-correlated), system prompt, two tool definitions, non-default
/// `ModelConfig`.  Both the input projection and the captured wire body
/// are included so the transformation is self-explanatory.
///
/// The Ollama provider strips tool ids from the wire body
/// (`flatten_message` does not forward `tool_use.id` to the
/// `tool_calls` array).  The redaction therefore fires only in the input
/// portion — demonstrating that `[id_1]` present in the input is absent
/// from the wire body, which is the expected lossy transformation.
#[tokio::test]
async fn request_body_kitchen_sink() {
    let server = MockServer::start().await;

    // Mount a minimal happy-path response so the provider completes.
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(200).set_body_string(ndjson(&[json!({
            "model": "llama3.2",
            "message": {"role": "assistant", "content": "ok"},
            "done": true,
            "prompt_eval_count": 10,
            "eval_count": 1
        })])))
        .mount(&server)
        .await;

    let tool_id = "toolu_ks_01";
    let req = LlmRequest {
        model: "llama3.2".to_owned(),
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
            ..Default::default()
        },
        context_management: None,
    };

    // Serialise input *before* consuming `req`.
    let input = serde_json::to_value(&req).expect("LlmRequest serialises");

    let provider = OllamaProvider::new().with_base_url(server.uri());
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
// Happy path: thinking + text + tool_calls in final chunk
// ---------------------------------------------------------------------------

#[tokio::test]
async fn streams_thinking_text_and_tool_calls() {
    let server = MockServer::start().await;

    let body = ndjson(&[
        json!({
            "model": "qwen3",
            "created_at": "2025-01-01T00:00:00Z",
            "message": {
                "role": "assistant",
                "content": "",
                "thinking": "Let me think."
            },
            "done": false
        }),
        json!({
            "model": "qwen3",
            "created_at": "2025-01-01T00:00:01Z",
            "message": { "role": "assistant", "content": "Hello" },
            "done": false
        }),
        json!({
            "model": "qwen3",
            "created_at": "2025-01-01T00:00:02Z",
            "message": { "role": "assistant", "content": ", world!" },
            "done": false
        }),
        json!({
            "model": "qwen3",
            "created_at": "2025-01-01T00:00:03Z",
            "message": {
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "id": "tc_1",
                    "function": { "name": "calc", "arguments": { "a": 1, "b": 2 } }
                }]
            },
            "done": true,
            "done_reason": "tool_calls",
            "prompt_eval_count": 25,
            "eval_count": 87
        }),
    ]);

    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(body)
                .insert_header("content-type", "application/x-ndjson"),
        )
        .mount(&server)
        .await;

    let provider = OllamaProvider::new().with_base_url(server.uri());
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
// Synthesises a tool ID when Ollama doesn't supply one
// ---------------------------------------------------------------------------

#[tokio::test]
async fn synthesises_tool_call_id_when_missing() {
    let server = MockServer::start().await;

    let body = ndjson(&[json!({
        "model": "llama3.2",
        "message": {
            "role": "assistant",
            "content": "",
            "tool_calls": [{
                "function": { "name": "calc", "arguments": { "x": 1 } }
            }]
        },
        "done": true,
        "prompt_eval_count": 1,
        "eval_count": 1
    })]);

    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(body)
                .insert_header("content-type", "application/x-ndjson"),
        )
        .mount(&server)
        .await;

    let provider = OllamaProvider::new().with_base_url(server.uri());
    let items = collect_ok(&provider, simple_request()).await;

    let tool_call = items
        .iter()
        .find_map(|i| {
            if let Some(omega_types::OmegaEvent::ToolCall(tc)) = i.as_event() {
                Some(tc)
            } else {
                None
            }
        })
        .expect("expected a tool_call event");
    assert!(
        tool_call.id.starts_with("ollama_tool_"),
        "id should be synthesised, got {:?}",
        tool_call.id
    );
}

// ---------------------------------------------------------------------------
// HTTP error → LlmError::Http
// ---------------------------------------------------------------------------

#[tokio::test]
async fn maps_500_to_http_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(500).set_body_string("ollama down"))
        .mount(&server)
        .await;

    let provider = OllamaProvider::new().with_base_url(server.uri());
    let mut stream = provider.stream(simple_request());

    match stream.next().await.expect("expected an item") {
        Err(LlmError::Http { status, body, .. }) => {
            assert_eq!(status, 500);
            assert_eq!(body, "ollama down");
        }
        other => panic!("expected LlmError::Http, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Malformed NDJSON → LlmError::Stream
// ---------------------------------------------------------------------------

#[tokio::test]
async fn maps_malformed_line_to_stream_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("not-json\n")
                .insert_header("content-type", "application/x-ndjson"),
        )
        .mount(&server)
        .await;

    let provider = OllamaProvider::new().with_base_url(server.uri());
    let mut stream = provider.stream(simple_request());

    match stream.next().await.expect("expected an item") {
        Err(LlmError::Stream { message }) => {
            assert!(message.contains("malformed NDJSON"), "got {message:?}");
        }
        other => panic!("expected LlmError::Stream, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Shared helpers for new tests
// ---------------------------------------------------------------------------

/// Minimal Ollama NDJSON response that terminates the stream cleanly.
fn minimal_success_ndjson() -> String {
    ndjson(&[json!({
        "model": "llama3.2",
        "message": { "role": "assistant", "content": "" },
        "done": true,
        "prompt_eval_count": 1,
        "eval_count": 1
    })])
}

/// Build a mock that accepts any POST and returns a minimal success NDJSON.
async fn success_mock(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(minimal_success_ndjson())
                .insert_header("content-type", "application/x-ndjson"),
        )
        .mount(server)
        .await;
}

// ---------------------------------------------------------------------------
// parse_retry_after
// ---------------------------------------------------------------------------

/// `retry-after: 3` → `Some(Duration::from_secs(3))`.
/// Catches: whole-function-→-None and `replace < with >`.
#[tokio::test]
async fn maps_429_to_http_error_with_retry_after() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "3")
                .set_body_string("too many"),
        )
        .mount(&server)
        .await;

    let provider = OllamaProvider::new().with_base_url(server.uri());
    let mut stream = provider.stream(simple_request());
    match stream.next().await.expect("expected item") {
        Err(LlmError::Http {
            status,
            retry_after,
            ..
        }) => {
            assert_eq!(status, 429);
            assert_eq!(retry_after, Some(Duration::from_secs(3)));
        }
        other => panic!("expected LlmError::Http, got {other:?}"),
    }
}

/// `retry-after: 0` → `Some(Duration::ZERO)` (zero is valid).
/// Catches: `replace < with <=` and `replace < with ==`.
#[tokio::test]
async fn parse_retry_after_zero_is_some_zero() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "0")
                .set_body_string("{}"),
        )
        .mount(&server)
        .await;

    let provider = OllamaProvider::new().with_base_url(server.uri());
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
/// Catches: `replace || with &&`, `replace < with ==`.
#[tokio::test]
async fn parse_retry_after_negative_is_none() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "-1")
                .set_body_string("{}"),
        )
        .mount(&server)
        .await;

    let provider = OllamaProvider::new().with_base_url(server.uri());
    let mut stream = provider.stream(simple_request());
    match stream.next().await.expect("expected item") {
        Err(LlmError::Http { retry_after, .. }) => {
            assert_eq!(retry_after, None, "retry-after:-1 must give None");
        }
        other => panic!("expected LlmError::Http, got {other:?}"),
    }
}

/// `retry-after: inf` and friends → `None` (non-finite float is invalid).
/// Catches: `delete !` (which inverts the `is_finite` check).
#[tokio::test]
async fn parse_retry_after_nonfinite_is_none() {
    for bad_value in &["inf", "nan", "-inf"] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(
                ResponseTemplate::new(429)
                    .insert_header("retry-after", *bad_value)
                    .set_body_string("{}"),
            )
            .mount(&server)
            .await;

        let provider = OllamaProvider::new().with_base_url(server.uri());
        let mut stream = provider.stream(simple_request());
        match stream.next().await.expect("expected item") {
            Err(LlmError::Http { retry_after, .. }) => {
                assert_eq!(retry_after, None, "retry-after:{bad_value} must give None");
            }
            other => panic!("expected LlmError::Http, got {other:?}"),
        }
    }
}

/// `retry-after: 1.5` → `Some(1500ms)` (sub-second precision).
/// Catches: `replace * with +` (gives 1002ms) and `replace * with /` (gives 1ms).
#[tokio::test]
async fn parse_retry_after_subsecond_is_millis() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "1.5")
                .set_body_string("{}"),
        )
        .mount(&server)
        .await;

    let provider = OllamaProvider::new().with_base_url(server.uri());
    let mut stream = provider.stream(simple_request());
    match stream.next().await.expect("expected item") {
        Err(LlmError::Http { retry_after, .. }) => {
            assert_eq!(
                retry_after,
                Some(Duration::from_millis(1500)),
                "retry-after:1.5 must give 1500ms"
            );
        }
        other => panic!("expected LlmError::Http, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// with_client builder
// ---------------------------------------------------------------------------

/// `with_client` must actually install the provided client.
/// If it returned `Default::default()`, the custom default headers
/// would be lost, the mock requiring them would not fire, and the
/// request would get a 404 → test fails.
/// Catches: `replace OllamaProvider::with_client -> Self with Default::default()`.
#[tokio::test]
async fn with_client_custom_header_is_propagated() {
    let server = MockServer::start().await;

    let mut default_headers = reqwest::header::HeaderMap::new();
    default_headers.insert("x-omega-test", "custom".parse().unwrap());
    let custom_client = reqwest::Client::builder()
        .default_headers(default_headers)
        .build()
        .unwrap();

    // This mock only fires if the custom header is present.
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .and(header("x-omega-test", "custom"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(minimal_success_ndjson())
                .insert_header("content-type", "application/x-ndjson"),
        )
        .mount(&server)
        .await;

    let provider = OllamaProvider::new()
        .with_client(custom_client)
        .with_base_url(server.uri());
    // `collect_ok` panics if any item is Err (e.g. 404 when header absent).
    let items = collect_ok(&provider, simple_request()).await;
    assert_eq!(items.len(), 1, "expected LlmResponse event");
}

// ---------------------------------------------------------------------------
// now_iso — LlmResponse time fields must be valid RFC3339
// ---------------------------------------------------------------------------

/// `LlmResponse.time` must be a valid RFC3339 timestamp.
/// Catches: `replace now_iso -> String with String::new()` and
/// `replace now_iso -> String with "xyzzy".into()`.
#[tokio::test]
async fn response_event_time_is_valid_rfc3339() {
    let server = MockServer::start().await;
    success_mock(&server).await;

    let provider = OllamaProvider::new().with_base_url(server.uri());
    let items = collect_ok(&provider, simple_request()).await;

    let resp = items
        .iter()
        .find_map(|i| match i.as_event() {
            Some(omega_types::OmegaEvent::LlmResponse(r)) => Some(r),
            _ => None,
        })
        .expect("expected LlmResponse event");

    chrono::DateTime::parse_from_rfc3339(&resp.time)
        .expect("LlmResponse.time must be valid RFC3339");
}

// ---------------------------------------------------------------------------
// flatten_message — request body composition
// ---------------------------------------------------------------------------

/// A user text message must appear in the `messages` array sent to Ollama.
/// Catches: `replace flatten_message with ()` (whole-function no-op),
/// and `delete ! in flatten_message` at the `!content.is_empty()` operand.
#[tokio::test]
async fn request_body_contains_user_text_message() {
    let server = MockServer::start().await;
    success_mock(&server).await;

    let req = LlmRequest {
        model: "llama3.2".to_owned(),
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "hello from test".to_owned(),
            }],
        }],
        system: None,
        tools: vec![],
        config: ModelConfig::default(),
        context_management: None,
    };

    let provider = OllamaProvider::new().with_base_url(server.uri());
    let _ = provider.stream(req).collect::<Vec<_>>().await;

    let received = server
        .received_requests()
        .await
        .expect("request recording enabled");
    assert_eq!(received.len(), 1);
    let body: Value = serde_json::from_slice(&received[0].body).unwrap();
    let messages = body["messages"].as_array().expect("messages array");
    let user_msg = messages
        .iter()
        .find(|m| m["role"] == "user")
        .expect("user role message present");
    assert_eq!(
        user_msg["content"], "hello from test",
        "user message content must be in request body"
    );
}

/// An assistant message with ONLY a thinking block must still be pushed.
/// Catches: `replace || with && at the first || in has_payload`
/// (which would require BOTH content AND thinking to push).
#[tokio::test]
async fn request_body_thinking_only_message_is_included() {
    let server = MockServer::start().await;
    success_mock(&server).await;

    let req = LlmRequest {
        model: "llama3.2".to_owned(),
        messages: vec![Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Thinking {
                thinking: "let me think".to_owned(),
                signature: None,
            }],
        }],
        system: None,
        tools: vec![],
        config: ModelConfig::default(),
        context_management: None,
    };

    let provider = OllamaProvider::new().with_base_url(server.uri());
    let _ = provider.stream(req).collect::<Vec<_>>().await;

    let received = server
        .received_requests()
        .await
        .expect("request recording enabled");
    assert_eq!(received.len(), 1);
    let body: Value = serde_json::from_slice(&received[0].body).unwrap();
    let messages = body["messages"].as_array().expect("messages array");
    let asst_msg = messages
        .iter()
        .find(|m| m["role"] == "assistant")
        .expect("assistant message with thinking must be present");
    assert_eq!(
        asst_msg["thinking"], "let me think",
        "thinking field must be populated"
    );
}

/// An assistant message with ONLY a tool-use block must still be pushed.
/// Catches: `replace || with && at the second || in has_payload`
/// (which would require both thinking AND tool_calls to push).
#[tokio::test]
async fn request_body_tool_use_only_message_is_included() {
    let server = MockServer::start().await;
    success_mock(&server).await;

    let req = LlmRequest {
        model: "llama3.2".to_owned(),
        messages: vec![Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "tc_1".to_owned(),
                name: "calc".to_owned(),
                input: json!({ "a": 1 }),
            }],
        }],
        system: None,
        tools: vec![],
        config: ModelConfig::default(),
        context_management: None,
    };

    let provider = OllamaProvider::new().with_base_url(server.uri());
    let _ = provider.stream(req).collect::<Vec<_>>().await;

    let received = server
        .received_requests()
        .await
        .expect("request recording enabled");
    assert_eq!(received.len(), 1);
    let body: Value = serde_json::from_slice(&received[0].body).unwrap();
    let messages = body["messages"].as_array().expect("messages array");
    let asst_msg = messages
        .iter()
        .find(|m| m["role"] == "assistant")
        .expect("assistant message with tool_calls must be present");
    let tool_calls = asst_msg["tool_calls"]
        .as_array()
        .expect("tool_calls array present");
    assert!(!tool_calls.is_empty(), "tool_calls must not be empty");
    assert_eq!(tool_calls[0]["function"]["name"], "calc");
}

/// A user message with ONLY ToolResult blocks must NOT produce a spurious
/// empty `role:user` message alongside the `role:tool` messages.
/// Catches: `delete ! before thinking.is_empty()` and
/// `delete ! before tool_calls.is_empty()` in `has_payload`.
#[tokio::test]
async fn request_body_tool_result_has_no_extra_empty_message() {
    let server = MockServer::start().await;
    success_mock(&server).await;

    let req = LlmRequest {
        model: "llama3.2".to_owned(),
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "tc_1".to_owned(),
                content: "42".to_owned(),
                is_error: false,
            }],
        }],
        system: None,
        tools: vec![],
        config: ModelConfig::default(),
        context_management: None,
    };

    let provider = OllamaProvider::new().with_base_url(server.uri());
    let _ = provider.stream(req).collect::<Vec<_>>().await;

    let received = server
        .received_requests()
        .await
        .expect("request recording enabled");
    assert_eq!(received.len(), 1);
    let body: Value = serde_json::from_slice(&received[0].body).unwrap();
    let messages = body["messages"].as_array().expect("messages array");

    // There should be exactly one message: the role:tool message.
    assert_eq!(
        messages.len(),
        1,
        "only the tool-result message should appear"
    );
    assert_eq!(messages[0]["role"], "tool");
    assert_eq!(messages[0]["content"], "42");
}

// ---------------------------------------------------------------------------
// tool_to_ollama — tool definitions in the request body
// ---------------------------------------------------------------------------

/// Tool definitions must be serialised with the Ollama function-call schema.
/// Catches: `replace tool_to_ollama -> Value with Default::default()`
/// (which would produce `null` for every tool entry).
#[tokio::test]
async fn request_body_contains_tool_definitions() {
    let server = MockServer::start().await;
    success_mock(&server).await;

    let req = LlmRequest {
        model: "llama3.2".to_owned(),
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "use a tool".to_owned(),
            }],
        }],
        system: None,
        tools: vec![ToolDefinition {
            name: "calculator".to_owned(),
            description: "does math".to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": { "x": { "type": "number" } }
            }),
        }],
        config: ModelConfig::default(),
        context_management: None,
    };

    let provider = OllamaProvider::new().with_base_url(server.uri());
    let _ = provider.stream(req).collect::<Vec<_>>().await;

    let received = server
        .received_requests()
        .await
        .expect("request recording enabled");
    assert_eq!(received.len(), 1);
    let body: Value = serde_json::from_slice(&received[0].body).unwrap();
    let tools = body["tools"].as_array().expect("tools array in request");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["type"], "function", "tool type must be 'function'");
    assert_eq!(
        tools[0]["function"]["name"], "calculator",
        "tool name must be preserved"
    );
    assert_eq!(
        tools[0]["function"]["description"], "does math",
        "tool description must be preserved"
    );
}
