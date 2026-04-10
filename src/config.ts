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
  // Tool result clearing
  // ---------------------------------------------------------------------------

  /**
   * Server-side tool result clearing: begin dropping old tool results when
   * input tokens reach this value.  Acts as a first stage before full
   * compaction (autoCompactThreshold) kicks in.
   *
   * 100 000 is Anthropic's own default and matches real-world Omega session
   * data: 66 % of sessions never exceed ~10 tool calls and will never trigger
   * this; only the heavy coding sessions (top ~10 %) that drive quadratic
   * context growth are affected.
   */
  toolResultClearTrigger: 100_000,

  /**
   * Number of most-recent tool use/result pairs to preserve after each
   * clearing.  Anthropic's default is 3 (aggressive); 10 is conservative —
   * it covers the median turn (4–6 calls) plus the previous turn's results,
   * making it very unlikely that Claude loses context it still needs.
   */
  toolResultClearKeep: 10,

  /**
   * Minimum tokens that must be clearable before the strategy fires.  If
   * fewer tokens would be removed (e.g. only a handful of tiny tool results
   * are older than the keep window), clearing is skipped entirely.  This
   * prevents cache-busting micro-trims where the prompt-cache write cost
   * would exceed the token savings.  15 000 tokens is a comfortable floor
   * for a coding agent whose tool results are typically large.
   */
  toolResultClearAtLeast: 15_000,

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
  // Session resumption
  // ---------------------------------------------------------------------------

  /**
   * Model used for the session-resumption summarisation call.
   *
   * Resumption is a reading-comprehension + writing task: the model reads the
   * previous session's event log and produces a structured summary. It does not
   * require the deeper reasoning that agentic coding turns demand.
   *
   * Sonnet 4.6 is the right choice: fast, cost-effective, and more than capable
   * for summarisation. Switch to "claude-opus-4-6" only if summary quality is
   * noticeably poor on very complex or long sessions.
   */
  resumptionModel: "claude-sonnet-4-6",

  /**
   * Thinking effort for the session-resumption summarisation call.
   * See defaultEffort above for the scale.
   *
   * "low" is intentional: summarisation is reading comprehension and structured
   * writing, not multi-step reasoning. Extended thinking adds negligible quality
   * improvement while making resumption slower and more expensive. Raise to
   * "medium" only if summaries omit important information on complex sessions.
   */
  resumptionEffort: "low" as "low" | "medium" | "high" | "max",

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

// ---------------------------------------------------------------------------
// Compaction instructions
// ---------------------------------------------------------------------------

/**
 * Custom summarisation prompt sent as context_management.edits[0].instructions.
 * Completely replaces Anthropic's default prompt when set.
 *
 * The opening two sentences are Anthropic's own default — kept verbatim because
 * they correctly orient the model to its role as summariser of its own prior
 * transcript. The remainder adds coding-session-specific guidance derived from
 * analysing real Omega compaction summaries: the default prompt produces good
 * output but tends toward activity-log narration rather than state snapshots,
 * and doesn't explicitly ask for learnings or current constant values.
 */
export const COMPACTION_INSTRUCTIONS = `\
You have written a partial transcript for the initial task above. Please write \
a summary of the transcript. The purpose of this summary is to provide \
continuity so you can continue to make progress towards solving the task in a \
future context, where the raw history above may not be accessible and will be \
replaced with this summary.

For a coding session, focus especially on what a developer would need to \
continue the work:

1. **Current state** (snapshot, not narrative): what is true *right now* — \
which files were changed and how they currently stand, what \
constants/config values are currently set to, which plan items are done \
vs. pending.

2. **Next step**: the single most important thing to do next, as specifically \
as possible (e.g. exact file, function, test name).

3. **Key decisions**: conclusions that should not be re-litigated — design \
choices made, approaches confirmed or rejected, and *why*.

4. **Learnings / what not to do**: anything tried that failed and why, so the \
same dead ends are not re-explored.

5. **Technical anchors**: specific file paths, function/type/constant names, \
commit hashes, and test names relevant to continuing the work. Prefer \
current values over historical change narratives.

You must wrap your summary in a <summary></summary> block.\
`;
