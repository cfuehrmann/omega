export const config = {
  // ---------------------------------------------------------------------------
  // Model
  // ---------------------------------------------------------------------------

  /** Default model used when no explicit model override is active. */
  model: "claude-sonnet-4-6",

  // ---------------------------------------------------------------------------
  // Context compaction
  // ---------------------------------------------------------------------------

  /**
   * Server-side compaction trigger: begin summarising when input tokens reach
   * this value. Set to 75 % of the 1 M-token context window for Sonnet/Opus 4.6.
   *
   * Why 750 000 and not earlier?
   * In a coding agent, accumulated context (earlier decisions, code state, prior
   * tool results) is highly valuable. Premature compaction discards that value
   * for a marginal focus-quality gain. At 750 k the model still has 250 k tokens
   * of headroom — plenty for the compaction summary (typically 5–20 k tokens)
   * plus a full continuation turn.
   *
   * Trade-off: Anthropic notes that model focus degrades before the hard limit.
   * If very long sessions show quality problems, consider lowering this toward
   * 500 k–600 k. Must be ≥ 50 000 (API minimum; Anthropic's own default is
   * 150 000, which is far more aggressive than needed for a coding agent).
   */
  autoCompactThreshold: 750_000,

  // ---------------------------------------------------------------------------
  // Retry backoff
  // ---------------------------------------------------------------------------

  /**
   * Initial wait before the first retry on a transient API error (429, 529,
   * 500, 503). Doubles on each subsequent attempt with ±10 % jitter, up to
   * retryMaxMs. 1 s gives fast recovery on brief quota-window bursts without
   * hammering the API before the window resets.
   */
  retryBaseMs: 1_000,

  /**
   * Upper bound on the per-attempt retry wait. Caps the exponential backoff so
   * that even after many consecutive failures the agent never stalls longer than
   * 60 s between attempts — long enough for a transient server overload to
   * subside, short enough to remain responsive.
   */
  retryMaxMs: 60_000,

  // ---------------------------------------------------------------------------
  // Thinking effort
  // ---------------------------------------------------------------------------

  /**
   * Default adaptive-thinking effort level sent as output_config.effort.
   *
   *   "low"    — minimises thinking; fastest and cheapest.
   *   "medium" — may skip thinking on simple queries. Anthropic's recommended
   *              default for agentic coding workflows on Sonnet 4.6.
   *   "high"   — Claude almost always thinks; equivalent to omitting the
   *              parameter (Anthropic's built-in default).
   *   "max"    — unconstrained thinking depth; Opus 4.6 only.
   *
   * "medium" is chosen because Anthropic explicitly recommends it as the
   * starting point for agentic coding on Sonnet 4.6: it is the best balance
   * of speed, cost, and quality for tool-heavy workflows. Claude still thinks
   * on complex tasks at medium effort — it only skips thinking for simple
   * queries where extended reasoning adds no value. Users can raise this to
   * "high" or "max" when maximum reasoning depth is needed.
   */
  defaultEffort: "medium" as const,

  // ---------------------------------------------------------------------------
  // Server
  // ---------------------------------------------------------------------------

  /**
   * HTTP port for the web UI. Overridable via --port flag or PORT env var.
   * 3000 is the standard local-dev convention and avoids conflicts with common
   * services on 8080 or privileged ports below 1024.
   */
  defaultPort: 3000,
};

// ---------------------------------------------------------------------------
// Per-model output token limits
// ---------------------------------------------------------------------------

/**
 * Maximum output tokens (including thinking tokens) per API call, keyed by
 * model ID.
 *
 * Set to each model's documented Anthropic API maximum so the model has the
 * full headroom it needs for thinking + response text + tool call arguments.
 * max_tokens is a ceiling, not a target: the model stops as soon as its
 * response is complete, so setting it to the maximum does not inflate cost.
 * Restricting it below the model maximum risks stop_reason: max_tokens on
 * legitimately long responses (large write_file content, extended thinking,
 * multi-tool loops with verbose output).
 *
 *   claude-sonnet-4-6 →  64 000  (Anthropic API maximum for this model)
 *   claude-opus-4-6   → 128 000  (Anthropic API maximum for this model)
 */
const MODEL_MAX_OUTPUT_TOKENS: Record<string, number> = {
  "claude-sonnet-4-6":  64_000,
  "claude-opus-4-6":   128_000,
};

/** Fallback used when the active model is not in the map (should not occur). */
const MODEL_MAX_OUTPUT_TOKENS_FALLBACK = 64_000;

/**
 * Return the max_tokens ceiling to send for the given model.
 * Falls back to the Sonnet 4.6 ceiling for unrecognised model IDs.
 */
export function maxOutputTokensForModel(model: string): number {
  return MODEL_MAX_OUTPUT_TOKENS[model] ?? MODEL_MAX_OUTPUT_TOKENS_FALLBACK;
}
