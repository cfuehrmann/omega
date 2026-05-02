//! Shared helpers for the `omega-core` integration tests.
//!
//! Each `tests/*.rs` file is compiled as its own binary, so this module
//! lives under `tests/common/` and is included via `mod common;` from
//! the test files that need it.  Items not used by every test crate are
//! marked `#[allow(dead_code)]`.

#![allow(clippy::unwrap_used, clippy::expect_used, dead_code)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use omega_core::{ContentBlock, LlmRequest, Message, ModelConfig, RetryConfig, Role};
use serde_json::Value;

// ---------------------------------------------------------------------------
// id_redactor — stateful placeholder redaction
// ---------------------------------------------------------------------------

/// Stateful redactor that assigns stable `[id_N]` placeholders to unique
/// string values across multiple JSON paths.
///
/// Construct one per test; state is not shared across tests.  Multiple
/// [`redaction`](IdRedactor::redaction) calls on the same `IdRedactor`
/// share the same numbering space, so the same id value receives the same
/// placeholder even when it appears under different JSON keys.
///
/// # Example
///
/// ```ignore
/// let r = id_redactor();
/// insta::assert_json_snapshot!(value, {
///     "**.id"          => r.redaction(),
///     "**.tool_use_id" => r.redaction(),
/// });
/// ```
pub struct IdRedactor(Arc<Mutex<HashMap<String, usize>>>);

impl IdRedactor {
    #[must_use]
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(HashMap::new())))
    }

    /// Returns an [`insta::Redaction`] that maps string values to `[id_N]`
    /// placeholders, sharing the numbering space with every other redaction
    /// produced from this `IdRedactor`.
    #[must_use]
    pub fn redaction(&self) -> insta::internals::Redaction {
        let map = Arc::clone(&self.0);
        insta::dynamic_redaction(move |value, _path| {
            let insta::internals::Content::String(s) = value else {
                return value;
            };
            let mut m = map.lock().expect("id_redactor mutex poisoned");
            let next_n = m.len() + 1;
            let idx = *m.entry(s).or_insert(next_n);
            insta::internals::Content::String(format!("[id_{idx}]"))
        })
    }
}

/// Create a fresh [`IdRedactor`] scoped to one test.
#[must_use]
pub fn id_redactor() -> IdRedactor {
    IdRedactor::new()
}

// ---------------------------------------------------------------------------
// RetryConfig tuned for tests
// ---------------------------------------------------------------------------

/// Tight retry config for tests — 1 ms base, 16 ms cap, no jitter.
///
/// Lives here rather than in production code because it is purely
/// test infrastructure: production callers pick their own
/// [`RetryConfig`] via `Default` or explicit construction.
#[must_use]
pub fn fast_retry_config(max_attempts: u32) -> RetryConfig {
    RetryConfig {
        max_attempts,
        initial_backoff: Duration::from_millis(1),
        max_backoff: Duration::from_millis(16),
        jitter: false,
    }
}

/// Same as [`fast_retry_config`] but with jitter enabled — used to
/// verify the jitter math at a 1 ms base.
#[must_use]
pub fn fast_retry_config_with_jitter(max_attempts: u32) -> RetryConfig {
    RetryConfig {
        max_attempts,
        initial_backoff: Duration::from_millis(1),
        max_backoff: Duration::from_millis(16),
        jitter: true,
    }
}

// ---------------------------------------------------------------------------
// Request builders
// ---------------------------------------------------------------------------

/// A trivial one-turn user request — enough to drive the streaming path.
#[must_use]
pub fn simple_request(model: &str) -> LlmRequest {
    LlmRequest {
        model: model.to_owned(),
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

// ---------------------------------------------------------------------------
// Response body builders
// ---------------------------------------------------------------------------

/// Compose an Anthropic SSE body from `(event, data)` pairs.
#[must_use]
pub fn sse_body(events: &[(&str, Value)]) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    for (event, data) in events {
        writeln!(out, "event: {event}").unwrap();
        writeln!(out, "data: {data}\n").unwrap();
    }
    out
}

/// A minimal Anthropic SSE response that emits a single text token and
/// closes — the smallest "happy path" body for tests that don't care
/// about the full transcript.
#[must_use]
pub fn minimal_anthropic_sse() -> String {
    sse_body(&[
        (
            "message_start",
            serde_json::json!({
                "type": "message_start",
                "message": {
                    "id": "msg_ok",
                    "model": "claude-sonnet-4-6",
                    "usage": { "input_tokens": 1, "output_tokens": 0 }
                }
            }),
        ),
        (
            "content_block_start",
            serde_json::json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": { "type": "text", "text": "" }
            }),
        ),
        (
            "content_block_delta",
            serde_json::json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": { "type": "text_delta", "text": "ok" }
            }),
        ),
        (
            "content_block_stop",
            serde_json::json!({ "type": "content_block_stop", "index": 0 }),
        ),
        (
            "message_delta",
            serde_json::json!({
                "type": "message_delta",
                "delta": { "stop_reason": "end_turn", "stop_sequence": null },
                "usage": { "output_tokens": 1 }
            }),
        ),
        (
            "message_stop",
            serde_json::json!({ "type": "message_stop" }),
        ),
    ])
}

/// A minimal Ollama NDJSON response that closes the stream cleanly.
#[must_use]
pub fn minimal_ollama_ndjson() -> String {
    let line = serde_json::json!({
        "model": "llama3.2",
        "message": { "role": "assistant", "content": "ok" },
        "done": true,
        "prompt_eval_count": 1,
        "eval_count": 1
    });
    format!("{line}\n")
}
