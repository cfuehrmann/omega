//! Anthropic Messages API provider.
//!
//! Speaks the streaming `/v1/messages` SSE protocol described at
//! <https://docs.claude.com/en/api/messages-streaming>.  Each SSE event
//! is parsed and translated into either a [`StreamSignal`] (for raw
//! text/thinking deltas) or a persisted [`OmegaEvent`] (for tool calls
//! and the final response envelope).

use std::collections::HashMap;
use std::time::Duration;

use eventsource_stream::Eventsource;
use futures::stream::TryStreamExt;
use omega_protocol::events::{CompactedEvent, LlmResponseEvent, ToolCallEvent};
use omega_protocol::{LlmResponseUsage, OmegaEvent, StreamSignal};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::provider::{AgentItemStream, Provider};
use crate::types::{AgentItem, LlmError, LlmRequest, Message, ToolDefinition};

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const ANTHROPIC_VERSION: &str = "2023-06-01";

// ---------------------------------------------------------------------------
// Provider
// ---------------------------------------------------------------------------

/// Configuration / handle for the Anthropic Messages API.
pub struct AnthropicProvider {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    beta_features: Vec<String>,
}

impl AnthropicProvider {
    /// Build a provider with the public Anthropic endpoint.
    #[must_use]
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: DEFAULT_BASE_URL.to_owned(),
            api_key: api_key.into(),
            beta_features: Vec::new(),
        }
    }

    /// Override the base URL — useful in tests against `wiremock` and
    /// for routing through corporate proxies.
    #[must_use]
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    /// Add an `anthropic-beta` feature flag (e.g.
    /// `"interleaved-thinking-2025-05-14"`).
    #[must_use]
    pub fn with_beta(mut self, feature: impl Into<String>) -> Self {
        self.beta_features.push(feature.into());
        self
    }

    /// Replace the underlying HTTP client (e.g. with a custom timeout).
    #[must_use]
    pub fn with_client(mut self, client: reqwest::Client) -> Self {
        self.client = client;
        self
    }
}

impl Provider for AnthropicProvider {
    fn stream(&self, request: LlmRequest) -> AgentItemStream {
        let client = self.client.clone();
        let url = format!("{}/v1/messages", self.base_url);
        let api_key = self.api_key.clone();
        let beta = if self.beta_features.is_empty() {
            None
        } else {
            Some(self.beta_features.join(","))
        };
        Box::pin(stream_impl(client, url, api_key, beta, request))
    }
}

// ---------------------------------------------------------------------------
// Streaming
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_lines)] // Linear SSE state machine reads better in one block.
fn stream_impl(
    client: reqwest::Client,
    url: String,
    api_key: String,
    beta: Option<String>,
    request: LlmRequest,
) -> impl futures::Stream<Item = Result<AgentItem, LlmError>> + Send + 'static {
    async_stream::try_stream! {
        let body = build_request_body(&request);
        let mut req = client
            .post(&url)
            .header("x-api-key", &api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .header("accept", "text/event-stream");
        if let Some(beta) = beta.as_deref() {
            req = req.header("anthropic-beta", beta);
        }
        let resp = req
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::Transport { message: e.to_string() })?;

        let status = resp.status();
        let retry_after = parse_retry_after(resp.headers());
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            Err(LlmError::Http { status: status.as_u16(), body, retry_after })?;
            return; // Unreachable; the `?` above already exits the generator.
        }

        // Adapt reqwest's bytes stream (errors `reqwest::Error`) to the byte
        // stream eventsource-stream expects (errors must impl `std::error::Error`).
        let bytes = resp.bytes_stream().map_err(std::io::Error::other);
        let mut sse = bytes.eventsource();

        let mut blocks: HashMap<usize, BlockAccum> = HashMap::new();
        let mut all_text = String::new();
        let mut all_thinking = String::new();
        let mut input_tokens: i64 = 0;
        let mut output_tokens: i64 = 0;
        let mut cache_creation: Option<i64> = None;
        let mut cache_read: Option<i64> = None;
        let mut service_tier: Option<String> = None;
        let mut stop_reason = String::from("unknown");
        let mut streaming_start: Option<String> = None;
        // Server-side compaction tracking (mirrors src/agent.ts:1432–1469).
        let mut compaction_seen = false;
        let mut usage_value: serde_json::Map<String, Value> = serde_json::Map::new();
        let mut cleared_tool_uses: Option<i64> = None;
        let mut cleared_input_tokens: Option<i64> = None;

        while let Some(ev) = futures::StreamExt::next(&mut sse).await {
            let ev = ev.map_err(|e| LlmError::Stream { message: e.to_string() })?;
            if ev.data.is_empty() || ev.event == "ping" {
                continue;
            }
            match ev.event.as_str() {
                "message_start" => {
                    let parsed: MessageStartData = parse_data(&ev.data)?;
                    input_tokens = parsed.message.usage.input_tokens;
                    cache_creation = parsed.message.usage.cache_creation_input_tokens.or(cache_creation);
                    cache_read = parsed.message.usage.cache_read_input_tokens.or(cache_read);
                    service_tier = parsed.message.usage.service_tier.or(service_tier);
                    // Capture the raw usage object verbatim so the
                    // Compacted event can carry every field Anthropic
                    // sends (e.g. `iterations[]`, `service_tier`).  The
                    // typed parse above already proved the data is
                    // valid JSON, so this second parse cannot fail.
                    let raw: Value = parse_data(&ev.data)?;
                    if let Some(obj) = raw
                        .get("message")
                        .and_then(|m| m.get("usage"))
                        .and_then(Value::as_object)
                    {
                        usage_value.clone_from(obj);
                    }
                }
                "content_block_start" => {
                    let parsed: ContentBlockStartData = parse_data(&ev.data)?;
                    if matches!(&parsed.content_block, ContentBlockStart::Compaction) {
                        compaction_seen = true;
                    }
                    if let Some(accum) = BlockAccum::from_start(parsed.content_block) {
                        blocks.insert(parsed.index, accum);
                    }
                }
                "content_block_delta" => {
                    let parsed: ContentBlockDeltaData = parse_data(&ev.data)?;
                    if let Some(accum) = blocks.get_mut(&parsed.index) {
                        match (parsed.delta, accum) {
                            (ContentBlockDelta::TextDelta { text }, BlockAccum::Text { text: t }) => {
                                t.push_str(&text);
                                all_text.push_str(&text);
                                if streaming_start.is_none() {
                                    streaming_start = Some(now_iso());
                                }
                                yield AgentItem::Signal(StreamSignal::Text { text });
                            }
                            (ContentBlockDelta::ThinkingDelta { thinking }, BlockAccum::Thinking { thinking: t, .. }) => {
                                t.push_str(&thinking);
                                all_thinking.push_str(&thinking);
                                yield AgentItem::Signal(StreamSignal::Thinking { text: thinking });
                            }
                            (ContentBlockDelta::InputJsonDelta { partial_json }, BlockAccum::ToolUse { partial_json: pj, .. }) => {
                                pj.push_str(&partial_json);
                            }
                            (ContentBlockDelta::SignatureDelta { signature }, BlockAccum::Thinking { signature: sig, .. }) => {
                                sig.push_str(&signature);
                            }
                            _ => { /* mismatched — ignore */ }
                        }
                    }
                }
                "content_block_stop" => {
                    let parsed: ContentBlockStopData = parse_data(&ev.data)?;
                    match blocks.remove(&parsed.index) {
                        Some(BlockAccum::Thinking { signature, .. }) => {
                            yield AgentItem::Signal(
                                StreamSignal::ThinkingBlockComplete { signature },
                            );
                        }
                        Some(BlockAccum::ToolUse { id, name, partial_json }) => {
                            let input: Value = if partial_json.is_empty() {
                                Value::Object(serde_json::Map::new())
                            } else {
                                serde_json::from_str(&partial_json).map_err(|e| LlmError::Stream {
                                    message: format!("malformed tool_use JSON: {e}"),
                                })?
                            };
                            yield AgentItem::event(OmegaEvent::ToolCall(ToolCallEvent {
                                time: now_iso(),
                                id,
                                name,
                                input,
                                context_hash: String::new(),
                            }));
                        }
                        _ => {}
                    }
                }
                "message_delta" => {
                    let parsed: MessageDeltaData = parse_data(&ev.data)?;
                    if let Some(sr) = parsed.delta.stop_reason {
                        stop_reason = sr;
                    }
                    output_tokens = parsed.usage.output_tokens;
                    if parsed.usage.cache_creation_input_tokens.is_some() {
                        cache_creation = parsed.usage.cache_creation_input_tokens;
                    }
                    if parsed.usage.cache_read_input_tokens.is_some() {
                        cache_read = parsed.usage.cache_read_input_tokens;
                    }
                    // Merge the raw usage object so the Compacted event
                    // (if any) carries the final iteration breakdown.
                    let raw: Value = parse_data(&ev.data)?;
                    if let Some(obj) = raw.get("usage").and_then(Value::as_object) {
                        for (k, v) in obj {
                            usage_value.insert(k.clone(), v.clone());
                        }
                    }
                    // Extract applied_edits for clear_tool_uses_20250919
                    // — mirrors src/agent.ts:1455–1469.  First matching
                    // edit wins.
                    if let Some(cm) = parsed.context_management {
                        for edit in cm.applied_edits {
                            if let AppliedEdit::ClearToolUses {
                                cleared_tool_uses: tu,
                                cleared_input_tokens: it,
                            } = edit
                            {
                                cleared_tool_uses = tu;
                                cleared_input_tokens = it;
                                break;
                            }
                        }
                    }
                }
                "message_stop" => {
                    if compaction_seen {
                        yield AgentItem::event(OmegaEvent::Compacted(CompactedEvent {
                            time: now_iso(),
                            usage: Value::Object(usage_value.clone()),
                        }));
                    }
                    yield AgentItem::event(OmegaEvent::LlmResponse(LlmResponseEvent {
                        time: now_iso(),
                        stop_reason: stop_reason.clone(),
                        cleared_tool_uses,
                        cleared_input_tokens,
                        usage: LlmResponseUsage {
                            input_tokens,
                            output_tokens,
                            cache_creation_input_tokens: cache_creation,
                            cache_read_input_tokens: cache_read,
                            service_tier: service_tier.clone(),
                        },
                        context_hash: String::new(),
                        text: if all_text.is_empty() { None } else { Some(all_text.clone()) },
                        thinking: if all_thinking.is_empty() { None } else { Some(all_thinking.clone()) },
                        streaming_start: streaming_start.clone(),
                        response_summary: None,
                    }));
                    break;
                }
                "error" => {
                    let parsed: ErrorPayload = parse_data(&ev.data)?;
                    let kind = parsed.error.error_type.unwrap_or_else(|| "error".into());
                    let msg = parsed.error.message.unwrap_or_default();
                    Err(LlmError::Stream {
                        message: format!("{}: {} (raw: {})", kind, msg, ev.data),
                    })?;
                }
                _ => { /* unknown event — ignore for forward-compat */ }
            }
        }
    }
}

fn parse_data<T: for<'de> Deserialize<'de>>(data: &str) -> Result<T, LlmError> {
    serde_json::from_str(data).map_err(|e| LlmError::Stream {
        message: format!("malformed SSE data: {e} — body: {data}"),
    })
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

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

// ---------------------------------------------------------------------------
// Per-block accumulator
// ---------------------------------------------------------------------------

enum BlockAccum {
    Text {
        text: String,
    },
    Thinking {
        thinking: String,
        signature: String,
    },
    ToolUse {
        id: String,
        name: String,
        partial_json: String,
    },
}

impl BlockAccum {
    fn from_start(start: ContentBlockStart) -> Option<Self> {
        match start {
            ContentBlockStart::Text { text } => Some(Self::Text { text }),
            ContentBlockStart::Thinking { thinking } => Some(Self::Thinking {
                thinking,
                signature: String::new(),
            }),
            ContentBlockStart::ToolUse { id, name, .. } => Some(Self::ToolUse {
                id,
                name,
                partial_json: String::new(),
            }),
            ContentBlockStart::Compaction | ContentBlockStart::Unknown => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Wire-format DTOs (private)
// ---------------------------------------------------------------------------

// --- Request body ---

#[derive(Serialize)]
struct AnthropicRequestBody<'a> {
    model: &'a str,
    max_tokens: u32,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<&'a str>,
    messages: &'a [Message],
    #[serde(skip_serializing_if = "<[ToolDefinition]>::is_empty")]
    tools: &'a [ToolDefinition],
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<ThinkingConfig>,
    /// Adaptive-thinking effort level.  Serialises to
    /// `{ "effort": "..." }` inside the `output_config` key.
    #[serde(skip_serializing_if = "Option::is_none")]
    output_config: Option<OutputConfig<'a>>,
    /// Forwarded verbatim into the Anthropic request body.  See
    /// `LlmRequest.context_management` for the rationale behind opaque
    /// pass-through.
    #[serde(skip_serializing_if = "Option::is_none")]
    context_management: Option<&'a Value>,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ThinkingConfig {
    Enabled { budget_tokens: u32 },
    Adaptive { display: String },
}

/// Serialises to `{ "effort": "..." }` inside `output_config`.
#[derive(Serialize)]
struct OutputConfig<'a> {
    effort: &'a str,
}

fn build_request_body(req: &LlmRequest) -> AnthropicRequestBody<'_> {
    let thinking = if req.config.adaptive_thinking {
        Some(ThinkingConfig::Adaptive {
            display: "summarized".to_owned(),
        })
    } else {
        req.config
            .thinking_budget
            .map(|budget_tokens| ThinkingConfig::Enabled { budget_tokens })
    };
    let output_config = req
        .config
        .effort
        .as_deref()
        .map(|effort| OutputConfig { effort });
    AnthropicRequestBody {
        model: &req.model,
        max_tokens: req.config.max_tokens,
        stream: true,
        system: req.system.as_deref(),
        messages: &req.messages,
        tools: &req.tools,
        temperature: req.config.temperature,
        thinking,
        output_config,
        context_management: req.context_management.as_ref(),
    }
}

// `ContentBlock`'s default Serialize already matches Anthropic's wire format
// — see the snake-case round-trip test below.

// --- SSE events ---

#[derive(Deserialize)]
struct MessageStartData {
    message: MessageStartInner,
}

#[derive(Deserialize)]
struct MessageStartInner {
    usage: AnthropicStartUsage,
}

#[derive(Deserialize)]
struct AnthropicStartUsage {
    input_tokens: i64,
    cache_creation_input_tokens: Option<i64>,
    cache_read_input_tokens: Option<i64>,
    service_tier: Option<String>,
}

#[derive(Deserialize)]
struct ContentBlockStartData {
    index: usize,
    content_block: ContentBlockStart,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ContentBlockStart {
    Text {
        #[serde(default)]
        text: String,
    },
    Thinking {
        #[serde(default)]
        thinking: String,
    },
    ToolUse {
        id: String,
        name: String,
        #[allow(dead_code)]
        #[serde(default)]
        input: Value,
    },
    /// Server-side compaction summary block.  Carries `content` /
    /// `encrypted_content` we don't read — the agent reacts to its
    /// presence, not its payload (see `src/agent.ts:1432–1453`).
    Compaction,
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
struct ContentBlockDeltaData {
    index: usize,
    delta: ContentBlockDelta,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ContentBlockDelta {
    TextDelta {
        text: String,
    },
    ThinkingDelta {
        thinking: String,
    },
    InputJsonDelta {
        partial_json: String,
    },
    SignatureDelta {
        signature: String,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
struct ContentBlockStopData {
    index: usize,
}

#[derive(Deserialize)]
struct MessageDeltaData {
    delta: MessageDeltaInner,
    usage: AnthropicDeltaUsage,
    /// Anthropic's per-turn record of any `context_management` edits
    /// it applied server-side.  Optional — absent on plain turns.
    #[serde(default)]
    context_management: Option<MessageDeltaContextMgmt>,
}

#[derive(Deserialize)]
struct MessageDeltaContextMgmt {
    #[serde(default)]
    applied_edits: Vec<AppliedEdit>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AppliedEdit {
    /// The only edit type we react to today — see
    /// `src/agent.ts:1455–1469`.
    #[serde(rename = "clear_tool_uses_20250919")]
    ClearToolUses {
        #[serde(default)]
        cleared_tool_uses: Option<i64>,
        #[serde(default)]
        cleared_input_tokens: Option<i64>,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
struct MessageDeltaInner {
    stop_reason: Option<String>,
}

#[derive(Deserialize)]
#[allow(clippy::struct_field_names)] // Field names mirror the Anthropic wire format.
struct AnthropicDeltaUsage {
    output_tokens: i64,
    cache_creation_input_tokens: Option<i64>,
    cache_read_input_tokens: Option<i64>,
}

#[derive(Deserialize)]
struct ErrorPayload {
    error: ErrorInner,
}

#[derive(Deserialize)]
struct ErrorInner {
    #[serde(rename = "type")]
    error_type: Option<String>,
    message: Option<String>,
}
