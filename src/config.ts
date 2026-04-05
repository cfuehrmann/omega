const MAX_OUTPUT_TOKENS = 32768;

export const config = {
  model: "claude-sonnet-4-6",
  maxOutputTokens: MAX_OUTPUT_TOKENS,

  // Server-side compaction trigger. Calibrated for the 1 M-token context window
  // available to API-key users of Sonnet 4.6 / Opus 4.6 (~75 % of 1 M).
  autoCompactThreshold: 750_000,

  // Retry backoff for transient API errors (429, 529, 500, 503).
  // Production uses indefinite retries; tests cap with OMEGA_RETRY_ATTEMPTS.
  retryBaseMs: 1_000,
  retryMaxMs: 60_000,

  // Default thinking effort. "high" = always thinks on Sonnet/Opus 4.6.
  // See Anthropic effort docs for "low" / "medium" / "max" alternatives.
  defaultEffort: "high" as const,

  // Default HTTP port for the web UI (overridable via --port flag or PORT env).
  defaultPort: 3000,

  // All tool calls are auto-approved — no allowlist needed.
};
