//! Ollama `/api/chat` provider.
//!
//! Streams NDJSON chunks (one JSON object per line, terminated by `\n`)
//! from a local or self-hosted Ollama server and translates them into
//! [`AgentItem`] values.  See the wire format reference at
//! <https://github.com/ollama/ollama/blob/main/docs/api.md#generate-a-chat-completion>.

use std::time::Duration;

use async_stream::try_stream;
use omega_types::events::LlmResponseEndedEvent;
use omega_types::{LlmResponseUsage, OmegaEvent, StreamSignal};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::provider::{AgentItemStream, Provider};
use crate::types::{AgentItem, ContentBlock, LlmError, LlmRequest, Message, Role, ToolDefinition};

const DEFAULT_BASE_URL: &str = "http://localhost:11434";

// ---------------------------------------------------------------------------
// Provider
// ---------------------------------------------------------------------------

/// Provider for an Ollama-compatible `/api/chat` endpoint.
pub struct OllamaProvider {
    client: reqwest::Client,
    base_url: String,
}

impl OllamaProvider {
    /// Build a provider pointing at `http://localhost:11434`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: DEFAULT_BASE_URL.to_owned(),
        }
    }

    /// Override the base URL — used by tests and remote deployments.
    #[must_use]
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    /// Replace the underlying HTTP client.
    #[must_use]
    pub fn with_client(mut self, client: reqwest::Client) -> Self {
        self.client = client;
        self
    }
}

impl Default for OllamaProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl Provider for OllamaProvider {
    fn stream(&self, request: LlmRequest) -> AgentItemStream {
        let client = self.client.clone();
        let url = format!("{}/api/chat", self.base_url);
        Box::pin(stream_impl(client, url, request))
    }
}

// ---------------------------------------------------------------------------
// Streaming
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_lines)] // Linear NDJSON state machine reads better in one block.
fn stream_impl(
    client: reqwest::Client,
    url: String,
    request: LlmRequest,
) -> impl futures::Stream<Item = Result<AgentItem, LlmError>> + Send + 'static {
    try_stream! {
        let body = build_request_body(&request);
        let resp = client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::Transport { message: e.to_string() })?;

        let status = resp.status();
        let retry_after = parse_retry_after(resp.headers());
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            Err(LlmError::Http { status: status.as_u16(), body, retry_after })?;
            return;
        }

        let mut bytes = resp.bytes_stream();
        let mut buf: Vec<u8> = Vec::new();
        let mut input_tokens: i64 = 0;
        let mut output_tokens: i64 = 0;
        let mut yielded_tool_calls = false;
        // SCHEMA-8: per-block index counter for the synthetic
        // `ToolUseBlockComplete` signals.  Ollama's wire format has
        // no SSE-style content-block indices, so we mint a monotonic
        // sequence (0, 1, ...) the agent can route by.
        let mut next_tool_use_index: usize = 0;

        loop {
            let next = futures::StreamExt::next(&mut bytes).await;
            let Some(chunk) = next else { break };
            let chunk = chunk.map_err(|e| LlmError::Stream { message: e.to_string() })?;
            buf.extend_from_slice(&chunk);

            // Drain complete \n-terminated lines from `buf`.
            while let Some(nl) = buf.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = buf.drain(..=nl).collect();
                let line_str = std::str::from_utf8(&line).map_err(|e| LlmError::Stream {
                    message: format!("non-UTF8 NDJSON line: {e}"),
                })?;
                let trimmed = line_str.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let parsed: OllamaChunk = serde_json::from_str(trimmed).map_err(|e| LlmError::Stream {
                    message: format!("malformed NDJSON: {e} — body: {trimmed}"),
                })?;

                if let Some(content) = parsed.message.content.as_ref()
                    && !content.is_empty()
                {
                    yield AgentItem::Signal(StreamSignal::Text { index: 0, text: content.clone() });
                }
                if let Some(thinking) = parsed.message.thinking.as_ref()
                    && !thinking.is_empty()
                {
                    yield AgentItem::Signal(StreamSignal::Thinking {
                        index: 0,
                        text: thinking.clone(),
                    });
                }
                if let Some(tool_calls) = parsed.message.tool_calls {
                    for tc in tool_calls {
                        let id = tc.id.unwrap_or_else(synth_tool_id);
                        // SCHEMA-8 Phase 2: providers no longer emit
                        // `OmegaEvent::ToolCall` mid-stream.  Surface
                        // each tool call as a per-block completion
                        // signal carrying a synthetic block index;
                        // the agent dispatches it after `LlmResponse`.
                        yield AgentItem::Signal(StreamSignal::ToolUseBlockComplete {
                            index: next_tool_use_index,
                            id,
                            name: tc.function.name,
                            input: tc.function.arguments,
                        });
                        next_tool_use_index += 1;
                        yielded_tool_calls = true;
                    }
                }

                if parsed.done {
                    let stop_reason = parsed.done_reason.unwrap_or_else(|| {
                        if yielded_tool_calls {
                            "tool_use".into()
                        } else {
                            "end_turn".into()
                        }
                    });
                    if let Some(c) = parsed.prompt_eval_count {
                        input_tokens = c;
                    }
                    if let Some(c) = parsed.eval_count {
                        output_tokens = c;
                    }
                    yield AgentItem::event(OmegaEvent::LlmResponseEnded(LlmResponseEndedEvent {
                        time: now_iso(),
                        stop_reason,
                        cleared_tool_uses: None,
                        cleared_input_tokens: None,
                        usage: LlmResponseUsage {
                            input_tokens,
                            output_tokens,
                            cache_creation_input_tokens: None,
                            cache_read_input_tokens: None,
                            service_tier: None,
                            iterations: None,
                        },
                        context_hash: String::new(),
                        response_summary: None,
                    }));
                    return;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn synth_tool_id() -> String {
    format!("ollama_tool_{:016x}", rand::random::<u64>())
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let raw = headers.get("retry-after")?.to_str().ok()?;
    let secs: f64 = raw.parse().ok()?;
    if !secs.is_finite() || secs < 0.0 {
        return None;
    }
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    let millis = (secs * 1000.0).ceil() as u64;
    Some(Duration::from_millis(millis))
}

// ---------------------------------------------------------------------------
// Wire-format DTOs (private)
// ---------------------------------------------------------------------------

// --- Request body ---

#[derive(Serialize)]
struct OllamaRequestBody<'a> {
    model: &'a str,
    messages: Vec<OllamaMessage>,
    stream: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    think: Option<bool>,
}

#[derive(Serialize)]
struct OllamaMessage {
    role: &'static str,
    content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OllamaToolCallOut>>,
}

#[derive(Serialize)]
struct OllamaToolCallOut {
    function: OllamaFunctionCallOut,
}

#[derive(Serialize)]
struct OllamaFunctionCallOut {
    name: String,
    arguments: Value,
}

fn build_request_body(req: &LlmRequest) -> OllamaRequestBody<'_> {
    let mut messages: Vec<OllamaMessage> = Vec::new();
    if let Some(system) = req.system.as_deref() {
        messages.push(OllamaMessage {
            role: "system",
            content: system.to_owned(),
            thinking: None,
            tool_calls: None,
        });
    }
    for msg in &req.messages {
        flatten_message(msg, &mut messages);
    }

    let tools = req.tools.iter().map(tool_to_ollama).collect::<Vec<_>>();

    OllamaRequestBody {
        model: &req.model,
        messages,
        stream: true,
        tools,
        options: req.config.temperature.map(|t| json!({ "temperature": t })),
        think: req.config.thinking_budget.map(|_| true),
    }
}

/// Flatten one of our [`Message`] values into one or more Ollama messages.
///
/// Any `ToolResult` blocks are split out into separate `role: "tool"`
/// messages because Ollama's wire format keeps them on their own turn.
fn flatten_message(msg: &Message, out: &mut Vec<OllamaMessage>) {
    let role: &'static str = match msg.role {
        Role::User => "user",
        Role::Assistant => "assistant",
    };

    let mut content = String::new();
    let mut thinking = String::new();
    let mut tool_calls: Vec<OllamaToolCallOut> = Vec::new();

    for block in &msg.content {
        match block {
            ContentBlock::Text { text } => content.push_str(text),
            ContentBlock::Thinking { thinking: t, .. } => thinking.push_str(t),
            ContentBlock::ToolUse { name, input, .. } => tool_calls.push(OllamaToolCallOut {
                function: OllamaFunctionCallOut {
                    name: name.clone(),
                    arguments: input.clone(),
                },
            }),
            ContentBlock::ToolResult { content: tr, .. } => {
                // Tool results are emitted as their own `role: tool` message.
                out.push(OllamaMessage {
                    role: "tool",
                    content: tr.clone(),
                    thinking: None,
                    tool_calls: None,
                });
            }
        }
    }

    let has_payload = !content.is_empty() || !thinking.is_empty() || !tool_calls.is_empty();
    if has_payload {
        out.push(OllamaMessage {
            role,
            content,
            thinking: if thinking.is_empty() {
                None
            } else {
                Some(thinking)
            },
            tool_calls: if tool_calls.is_empty() {
                None
            } else {
                Some(tool_calls)
            },
        });
    }
}

fn tool_to_ollama(tool: &ToolDefinition) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": tool.name,
            "description": tool.description,
            "parameters": tool.input_schema,
        },
    })
}

// --- Streaming response ---

#[derive(Deserialize)]
struct OllamaChunk {
    #[serde(default)]
    message: OllamaMessageIn,
    #[serde(default)]
    done: bool,
    done_reason: Option<String>,
    prompt_eval_count: Option<i64>,
    eval_count: Option<i64>,
}

#[derive(Deserialize, Default)]
struct OllamaMessageIn {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    thinking: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<OllamaToolCallIn>>,
}

#[derive(Deserialize)]
struct OllamaToolCallIn {
    #[serde(default)]
    id: Option<String>,
    function: OllamaFunctionCallIn,
}

#[derive(Deserialize)]
struct OllamaFunctionCallIn {
    name: String,
    #[serde(default)]
    arguments: Value,
}
