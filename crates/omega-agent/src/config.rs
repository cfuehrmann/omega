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

/// Models that support the `"max"` effort level (Opus variants).
const OPUS_MODELS: &[&str] = &["claude-opus-4-6", "claude-opus-4-7"];

/// Models that support the `"xhigh"` effort level.
const XHIGH_MODELS: &[&str] = &["claude-opus-4-7"];

/// Cap `effort` to the highest level the given `model` actually supports.
///
/// Mirrors `capEffortForModel` in `src/agent.ts`:
/// - `"xhigh"` → only `claude-opus-4-7`; degrades to `"high"` elsewhere.
/// - `"max"`   → only Opus models; degrades to `"high"` on Sonnet.
/// - All other values are passed through unchanged.
#[must_use]
pub fn cap_effort_for_model<'a>(effort: &'a str, model: &str) -> &'a str {
    if effort == "xhigh" && !XHIGH_MODELS.contains(&model) {
        return "high";
    }
    if effort == "max" && !OPUS_MODELS.contains(&model) {
        return "high";
    }
    effort
}

#[cfg(test)]
mod tests {
    //! Inline carve-out tests for `config.rs`.
    //!
    //! Justification for carve-out: `max_output_tokens_for_model` and
    //! `cap_effort_for_model` are pure functions that look up constants by
    //! model-name string.  Testing them through `Agent::send_message` /
    //! `MockProvider` would require capturing the `max_tokens` field of
    //! `LlmRequest` for each model variant, which adds substantial agent
    //! wiring.  The inline tests are simpler and pin the constants directly.

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

    // --- cap_effort_for_model ---

    #[test]
    fn effort_passthrough_for_common_levels() {
        for model in ["claude-sonnet-4-6", "claude-opus-4-6", "claude-opus-4-7"] {
            for effort in ["low", "medium", "high"] {
                assert_eq!(cap_effort_for_model(effort, model), effort);
            }
        }
    }

    #[test]
    fn xhigh_capped_to_high_on_non_opus47() {
        assert_eq!(cap_effort_for_model("xhigh", "claude-sonnet-4-6"), "high");
        assert_eq!(cap_effort_for_model("xhigh", "claude-opus-4-6"), "high");
        assert_eq!(cap_effort_for_model("xhigh", "claude-opus-4-7"), "xhigh");
    }

    #[test]
    fn max_capped_to_high_on_non_opus() {
        assert_eq!(cap_effort_for_model("max", "claude-sonnet-4-6"), "high");
        assert_eq!(cap_effort_for_model("max", "claude-opus-4-6"), "max");
        assert_eq!(cap_effort_for_model("max", "claude-opus-4-7"), "max");
    }
}
