//! Shared types for the LLM provider abstraction.
//!
//! These types are intentionally close to the Anthropic Messages API
//! shape (text / tool-use / tool-result content blocks) because that
//! is the API surface the TypeScript agent already speaks.  The
//! [`OllamaProvider`](crate::OllamaProvider) translates them on the way
//! out and on the way in.

use std::time::Duration;

use omega_protocol::{OmegaEvent, StreamSignal};
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Conversation primitives
// ---------------------------------------------------------------------------

/// Role of a [`Message`] in the conversation history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts-bindings", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-bindings", ts(export))]
pub enum Role {
    User,
    Assistant,
}

/// A single content block inside a [`Message`].
///
/// Mirrors the Anthropic Messages API shape — the union of every block
/// type the agent sends or receives.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[cfg_attr(feature = "ts-bindings", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-bindings", ts(export))]
pub enum ContentBlock {
    /// A text block.
    Text { text: String },
    /// A thinking block (extended reasoning, returned by Anthropic
    /// when the model has thinking enabled).  The `signature` is the
    /// opaque token Anthropic requires when echoing the block back in
    /// a follow-up turn.
    Thinking {
        thinking: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts-bindings", ts(optional))]
        signature: Option<String>,
    },
    /// A tool invocation by the assistant.
    ToolUse {
        id: String,
        name: String,
        /// Arbitrary JSON input parameters supplied by the LLM.
        #[cfg_attr(feature = "ts-bindings", ts(type = "unknown"))]
        input: Value,
    },
    /// The result of a tool invocation, sent back as a user message.
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        is_error: bool,
    },
}

/// A single message in the conversation history sent to the provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

/// A tool the assistant may choose to call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    /// JSON Schema describing the tool's input parameters.
    pub input_schema: Value,
}

/// Per-call model configuration knobs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelConfig {
    /// Hard cap on output tokens.
    pub max_tokens: u32,
    /// Sampling temperature.  `None` lets the provider apply its default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Anthropic extended-thinking budget (tokens).  `None` disables
    /// explicit-budget thinking.  Ignored by
    /// [`OllamaProvider`](crate::OllamaProvider).
    /// When [`Self::adaptive_thinking`] is `true` this field is ignored.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_budget: Option<u32>,
    /// Enable Anthropic adaptive thinking
    /// (`{ "type": "adaptive", "display": "summarized" }`).
    /// Takes precedence over [`Self::thinking_budget`] when `true`.
    /// Ignored by [`OllamaProvider`](crate::OllamaProvider).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub adaptive_thinking: bool,
    /// Thinking-effort level forwarded as `output_config.effort` in the
    /// Anthropic request body.  `None` omits the field entirely.
    /// Ignored by [`OllamaProvider`](crate::OllamaProvider).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            max_tokens: 4096,
            temperature: None,
            thinking_budget: None,
            adaptive_thinking: false,
            effort: None,
        }
    }
}

/// A single LLM call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmRequest {
    /// Provider-specific model identifier (e.g. `"claude-sonnet-4-6"`,
    /// `"llama3.1:8b"`).
    pub model: String,
    /// Conversation history.
    pub messages: Vec<Message>,
    /// Optional system prompt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    /// Tool definitions visible to the model on this call.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDefinition>,
    pub config: ModelConfig,
    /// Provider-specific context-management configuration (Anthropic
    /// `context_management` request field). Opaque pass-through: the
    /// shape of `edits[]` evolves on Anthropic's side, and forcing
    /// strong typing here would require frequent updates without
    /// catching real bugs. Forwarded verbatim into the request body
    /// when `Some`.
    ///
    /// Mirrors `src/agent.ts:1280–1306` in the TypeScript reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_management: Option<Value>,
}

// ---------------------------------------------------------------------------
// Stream item
// ---------------------------------------------------------------------------

/// A single item yielded by [`Provider::stream`](crate::Provider::stream).
///
/// Either an ephemeral [`StreamSignal`] (text/thinking token fragment) or
/// a persisted [`OmegaEvent`] (`LlmResponse`, `ToolCall`, `LlmRetry`,
/// `LlmError`).
///
/// # `context_hash` is left empty
///
/// Provider implementations construct `LlmResponse` and `ToolCall`
/// events with `context_hash: String::new()`.  The hash is computed
/// when the assistant context.jsonl record is written by `omega-server`.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(untagged)]
pub enum AgentItem {
    Signal(StreamSignal),
    /// `OmegaEvent` is significantly larger than `StreamSignal` (~280 vs
    /// ~32 bytes); we box it to keep the enum small for the hot path.
    Event(Box<OmegaEvent>),
}

impl AgentItem {
    /// Convenience constructor that boxes the event for callers.
    #[must_use]
    pub fn event(event: OmegaEvent) -> Self {
        Self::Event(Box::new(event))
    }

    /// Borrow the inner `OmegaEvent` if this item carries one.
    #[must_use]
    pub fn as_event(&self) -> Option<&OmegaEvent> {
        match self {
            Self::Event(b) => Some(b.as_ref()),
            Self::Signal(_) => None,
        }
    }
}

impl From<StreamSignal> for AgentItem {
    fn from(signal: StreamSignal) -> Self {
        Self::Signal(signal)
    }
}

impl From<OmegaEvent> for AgentItem {
    fn from(event: OmegaEvent) -> Self {
        Self::Event(Box::new(event))
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors raised by a provider's stream.
///
/// The [`is_retryable`](Self::is_retryable) and
/// [`retry_after`](Self::retry_after) accessors drive the
/// [`RetryingProvider`](crate::RetryingProvider) policy.
#[derive(Debug, Clone, thiserror::Error)]
pub enum LlmError {
    /// Non-2xx HTTP response.  `body` is the raw response body (may be
    /// truncated by the provider for very large bodies).
    #[error("HTTP {status}: {body}")]
    Http {
        status: u16,
        body: String,
        #[doc(hidden)]
        retry_after: Option<Duration>,
    },
    /// Mid-stream parse / decode failure (malformed SSE event, NDJSON
    /// line that didn't parse, missing required field).
    #[error("stream error: {message}")]
    Stream { message: String },
    /// Connection / IO failure before or during the stream.
    #[error("transport error: {message}")]
    Transport { message: String },
    /// Catch-all for everything else.
    #[error("{message}")]
    Other { message: String },
}

impl LlmError {
    /// HTTP status code, if any.
    #[must_use]
    pub fn status(&self) -> Option<u16> {
        match self {
            Self::Http { status, .. } => Some(*status),
            _ => None,
        }
    }

    /// `Retry-After` header parsed by the provider, if any.
    #[must_use]
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::Http { retry_after, .. } => *retry_after,
            _ => None,
        }
    }

    /// Body text for HTTP errors, or the message for everything else.
    #[must_use]
    pub fn body(&self) -> &str {
        match self {
            Self::Http { body, .. } => body.as_str(),
            Self::Stream { message } | Self::Transport { message } | Self::Other { message } => {
                message.as_str()
            }
        }
    }

    /// Whether this error is worth retrying.
    ///
    /// Mirrors the policy in the TypeScript agent
    /// (`src/agent.ts::isRetryable`):
    ///
    /// - Status 429, 500, 503, 529 → retry — except a 429 whose body
    ///   says `"Extra usage is required for long context requests"`,
    ///   which is non-transient.
    /// - Body containing `"overloaded_error"` (Anthropic SSE error
    ///   event delivered without a status code) → retry.
    /// - Transport errors → retry (idle-connection close, ECONNRESET,
    ///   server keepalive timeout).
    /// - Everything else → terminal.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Http { status, body, .. } => {
                // "Prompt too long" 429s are not transient — same payload
                // will fail again.
                if *status == 429
                    && body.contains("Extra usage is required for long context requests")
                {
                    return false;
                }
                matches!(*status, 429 | 500 | 503 | 529)
            }
            Self::Stream { message } | Self::Other { message } => {
                // Anthropic's SDK delivers `overloaded_error` as a
                // payload-only event without a status code.  Surface it
                // as Stream/Other and retry.
                message.contains("overloaded_error")
            }
            Self::Transport { .. } => true,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// `body()` must return the raw HTTP response body for Http errors.
    #[test]
    fn body_returns_http_body_text() {
        let e = LlmError::Http {
            status: 400,
            body: "bad input".to_owned(),
            retry_after: None,
        };
        assert_eq!(e.body(), "bad input");
    }

    /// `body()` must return the message for Stream errors.
    #[test]
    fn body_returns_stream_message() {
        let e = LlmError::Stream {
            message: "eof mid-stream".to_owned(),
        };
        assert_eq!(e.body(), "eof mid-stream");
    }

    /// `body()` must return the message for Transport errors.
    #[test]
    fn body_returns_transport_message() {
        let e = LlmError::Transport {
            message: "ECONNRESET".to_owned(),
        };
        assert_eq!(e.body(), "ECONNRESET");
    }

    /// `body()` must return the message for Other errors.
    #[test]
    fn body_returns_other_message() {
        let e = LlmError::Other {
            message: "unknown error".to_owned(),
        };
        assert_eq!(e.body(), "unknown error");
    }
}
