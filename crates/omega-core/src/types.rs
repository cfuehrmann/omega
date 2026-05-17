//! Shared types for the LLM provider abstraction.
//!
//! These types are intentionally close to the Anthropic Messages API
//! shape (text / tool-use / tool-result content blocks) because that
//! is the API surface the TypeScript agent already speaks.  The
//! [`OllamaProvider`](crate::OllamaProvider) translates them on the way
//! out and on the way in.

use std::time::Duration;

use omega_types::{OmegaEvent, StreamSignal};
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ContentBlock and Role are defined in omega-types; re-exported here
// so that `omega_core::ContentBlock` and `omega_core::Role` continue to resolve.
pub use omega_types::{ContentBlock, Role};

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
    /// Legacy Anthropic extended-thinking budget (tokens) for the
    /// deprecated `thinking: { type: "enabled", budget_tokens: N }` mode.
    /// Only consulted when [`Self::adaptive_thinking`] is `false`, and
    /// even then `enabled` mode is deprecated on Opus 4.6 / Sonnet 4.6 and
    /// rejected outright on Opus 4.7 — see the Adaptive Thinking docs at
    /// <https://platform.claude.com/docs/en/build-with-claude/adaptive-thinking>.
    /// Production code in `omega-agent` never sets this; it exists for
    /// older models (Sonnet 4.5 / Opus 4.5 / earlier) and external callers.
    /// Ignored by [`OllamaProvider`](crate::OllamaProvider).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_budget: Option<u32>,
    /// Enable Anthropic adaptive thinking — serialised as
    /// `thinking: { "type": "adaptive", "display": "summarized" }`.
    ///
    /// This is the recommended (and on Opus 4.7, the only supported)
    /// thinking mode for all current Claude models. Adaptive mode lets
    /// the model decide when and how much to think, and automatically
    /// enables interleaved thinking between tool calls — no
    /// `anthropic-beta: interleaved-thinking-*` header is required.
    ///
    /// Production code in `omega-agent` sets this to `true` on every
    /// `LlmRequest`; the default is `false` only so that test fixtures
    /// using `..Default::default()` produce a minimal wire body.
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
    /// Optional system prompt, expressed as an ordered list of
    /// cacheable text blocks.
    ///
    /// The Anthropic provider serialises each entry as one element of
    /// the wire-level `system` array (preceded by an uncached billing
    /// header) and stamps `cache_control: ephemeral` on the **last**
    /// block so the whole sequence becomes a single cached prefix.
    /// The Ollama provider concatenates the blocks with blank lines
    /// before submitting as a single `system` message.
    ///
    /// `None` means no system prompt; `Some(vec![])` is normalised the
    /// same way on the wire.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<Vec<String>>,
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
                // Anthropic delivers several error types as payload-only
                // SSE events on an HTTP-200 stream (no status code).
                // Both `overloaded_error` and `api_error` (generic
                // internal server error) are transient and worth retrying.
                message.contains("overloaded_error") || message.contains("api_error")
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
