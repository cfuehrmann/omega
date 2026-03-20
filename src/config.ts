const MAX_OUTPUT_TOKENS = 32768;

export const config = {
  model: "claude-sonnet-4-6",
  maxOutputTokens: MAX_OUTPUT_TOKENS,
  maxContextTokens: 100_000,

  // All tool calls are auto-approved — no allowlist needed.
};
