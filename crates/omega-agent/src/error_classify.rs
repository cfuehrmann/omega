//! Error classification for the agentic loop.
//!
//! Two predicates the [`crate::Agent`] uses to decide what to do after a
//! provider error reaches the bottom of the [`omega_core::Provider`]
//! stream — i.e. after every retry already attempted by
//! `RetryingProvider` has been exhausted.
//!
//! The matching strings are stable surfaces of the underlying providers
//! (Anthropic SSE / API error envelopes); they live here so the
//! patterns are documented in one place.

use omega_core::LlmError;

/// Returns `true` if the error indicates the model emitted a tool-use
/// block whose JSON arguments could not be parsed.
///
/// In the Rust pipeline this surfaces as `LlmError::Stream` with a
/// message starting with the literal prefix produced by
/// `omega_core::AnthropicProvider` when its `serde_json::from_str` of the
/// accumulated `partial_json` fails.  The TS agent matches a different
/// message because the TS Anthropic SDK throws its own error first; both
/// converge on this code path.
#[must_use]
pub fn is_invalid_tool_json(err: &LlmError) -> bool {
    matches!(
        err,
        LlmError::Stream { message } if message.starts_with("malformed tool_use JSON")
    )
}

/// Returns `true` if the error is the Anthropic API's "context too long"
/// 429 — payload exceeds the model's input window even with extra usage
/// granted.  Retrying with the same payload is futile, so we surface a
/// dedicated `agent_error` message instead of the generic "rate limit"
/// one.
///
/// Mirrors `isContextTooLong` in `src/agent.ts`.
#[must_use]
pub fn is_context_too_long(err: &LlmError) -> bool {
    matches!(
        err,
        LlmError::Http { status: 429, body, .. }
            if body.contains("Extra usage is required for long context requests")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_tool_json_matches_stream_with_prefix() {
        let err = LlmError::Stream {
            message: "malformed tool_use JSON: expected `,`".to_owned(),
        };
        assert!(is_invalid_tool_json(&err));
    }

    #[test]
    fn invalid_tool_json_rejects_other_stream_errors() {
        let err = LlmError::Stream {
            message: "overloaded_error".to_owned(),
        };
        assert!(!is_invalid_tool_json(&err));
    }

    #[test]
    fn invalid_tool_json_rejects_http() {
        let err = LlmError::Http {
            status: 400,
            body: "malformed tool_use JSON".to_owned(),
            retry_after: None,
        };
        assert!(!is_invalid_tool_json(&err));
    }

    #[test]
    fn context_too_long_matches_429_with_marker() {
        let err = LlmError::Http {
            status: 429,
            body: "Extra usage is required for long context requests on this model".to_owned(),
            retry_after: None,
        };
        assert!(is_context_too_long(&err));
    }

    #[test]
    fn context_too_long_rejects_other_429s() {
        let err = LlmError::Http {
            status: 429,
            body: "rate limit reached".to_owned(),
            retry_after: None,
        };
        assert!(!is_context_too_long(&err));
    }

    #[test]
    fn context_too_long_rejects_other_status() {
        let err = LlmError::Http {
            status: 500,
            body: "Extra usage is required for long context requests".to_owned(),
            retry_after: None,
        };
        assert!(!is_context_too_long(&err));
    }
}
