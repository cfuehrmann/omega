//! Integration tests for `OllamaProvider`.
//!
//! Stands up a `wiremock` server that emits NDJSON chunks shaped like
//! Ollama's `/api/chat` responses and asserts the provider lifts them
//! into the right `AgentItem` sequence.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use futures::StreamExt;
use omega_core::{
    AgentItem, ContentBlock, LlmError, LlmRequest, Message, ModelConfig, OllamaProvider, Provider,
    Role,
};
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

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
            temperature: None,
            thinking_budget: None,
        },
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
            if let Some(omega_protocol::OmegaEvent::ToolCall(tc)) = i.as_event() {
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
