const MAX_OUTPUT_TOKENS = 32768;

export const config = {
  model: "claude-sonnet-4-6",
  fallbackModel: "gpt-5.2-codex", // no auto-fallback; used for /codex
  maxOutputTokens: MAX_OUTPUT_TOKENS,
  maxContextTokens: 100_000,

  // All tool calls are auto-approved — no allowlist needed.
};
