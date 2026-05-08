//! omega-core — LLM provider abstraction and retry loop for Omega.
//!
//! This crate owns:
//!
//! - [`Provider`] — the single trait every LLM backend implements.
//! - [`AnthropicProvider`] — the Anthropic Messages API (`/v1/messages`)
//!   over Server-Sent Events.
//! - [`OllamaProvider`] — Ollama's `/api/chat` over newline-delimited JSON.
//! - [`RetryingProvider`] — wraps any [`Provider`] and retries transient
//!   errors with exponential backoff.
//!
//! The contract that crosses crate boundaries is the [`AgentItem`] stream:
//! a sequence of ephemeral [`StreamSignal`]s (text/thinking fragments) and
//! persisted [`omega_types::OmegaEvent`]s (`LlmResponse`, `ToolCall`,
//! `LlmRetry`, `LlmError`).
//!
//! # `context_hash` is filled by the persistence layer
//!
//! Provider implementations construct events with `context_hash:
//! String::new()`. The hash is computed when the assistant record is
//! written to `context.jsonl` — that is the responsibility of `omega-server`
//! (Phase 1c). Treating providers as the keepers of context-hash would
//! conflate streaming and persistence; the boundary kept here matches the
//! TypeScript implementation.
//!
//! [`StreamSignal`]: omega_types::StreamSignal

pub mod anthropic;
pub mod ollama;
pub mod provider;
pub mod retry;
pub mod types;

pub use anthropic::AnthropicProvider;
pub use ollama::OllamaProvider;
pub use provider::{AgentItemStream, Provider};
pub use retry::{RetryConfig, RetryingProvider};
pub use types::{
    AgentItem, ContentBlock, LlmError, LlmRequest, Message, ModelConfig, Role, ToolDefinition,
};
