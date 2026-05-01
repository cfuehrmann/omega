//! Per-model configuration constants.
//!
//! Mirrors `src/config.ts` in the TypeScript implementation.  Only the
//! fields actually consumed by [`crate::agent::Agent`] in Phase 1d.0a are
//! ported; context-management thresholds and other knobs come in 1d.1.

/// Anthropic API max-tokens ceiling per model.
///
/// Values match the TS source exactly:
/// `claude-sonnet-4-6 → 64 000`, `claude-opus-4-* → 128 000`.
const MODEL_MAX_OUTPUT_TOKENS: &[(&str, u32)] = &[
    ("claude-sonnet-4-6", 64_000),
    ("claude-opus-4-6", 128_000),
    ("claude-opus-4-7", 128_000),
];

/// Fallback when the model name is not recognised — same as Sonnet 4.6.
const MODEL_MAX_OUTPUT_TOKENS_FALLBACK: u32 = 64_000;

/// Return the `max_tokens` ceiling to send for `model`.
///
/// Falls back to the Sonnet 4.6 ceiling for any unknown model id.
#[must_use]
pub fn max_output_tokens_for_model(model: &str) -> u32 {
    for (name, tokens) in MODEL_MAX_OUTPUT_TOKENS {
        if *name == model {
            return *tokens;
        }
    }
    MODEL_MAX_OUTPUT_TOKENS_FALLBACK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_models_return_their_ceiling() {
        assert_eq!(max_output_tokens_for_model("claude-sonnet-4-6"), 64_000);
        assert_eq!(max_output_tokens_for_model("claude-opus-4-6"), 128_000);
        assert_eq!(max_output_tokens_for_model("claude-opus-4-7"), 128_000);
    }

    #[test]
    fn unknown_model_falls_back() {
        assert_eq!(max_output_tokens_for_model("unknown-model"), 64_000);
        assert_eq!(max_output_tokens_for_model(""), 64_000);
    }
}
