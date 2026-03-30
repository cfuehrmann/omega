const MAX_OUTPUT_TOKENS = 32768;

export const config = {
  model: "claude-sonnet-4-6",
  maxOutputTokens: MAX_OUTPUT_TOKENS,

  // Server-side compaction trigger. Calibrated for the 1 M-token context window
  // available to API-key users of Sonnet 4.6 / Opus 4.6 (~75 % of 1 M).
  autoCompactThreshold: 750_000,

  // All tool calls are auto-approved — no allowlist needed.
};
