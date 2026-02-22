# Future — Issue Tracker

Discrete, prioritised, actionable. Keep in priority order.

---

## 1. Provider-specific caching (Anthropic prompt caching)

Anthropic supports prompt caching via `cache_control` headers. The system
prompt and world-state are stable across turns and would benefit most.
Could meaningfully reduce input token costs for long sessions.
Implement Anthropic-side first; OpenAI caching is automatic.

Infrastructure ready: `estimateCostWithCache()` is implemented and tested.
Remaining: add `cache_control` to system message blocks in API calls,
parse `cache_read_input_tokens` / `cache_creation_input_tokens` from usage,
route through `estimateCostWithCache` in cost accounting.

---

## Closed / dismissed items (for reference)

- **UI tests** — Done. 231+ tests in `ui-raw.test.ts` and `tool-renderers.test.ts`.
- **Rate-limit retry** — Done. Provider-aware retry with `getOpenAiRetryDelayMs` (parses "try again in Ns") and `getAnthropicRetryDelayMs` (exponential backoff). Already at ms precision.
- **OAuth auto-relogin** — Done. `forceRefreshToken()` in auth.ts, `isAuthExpired()` + `reinitAuth()` in agent.ts. 401 in Anthropic stream loop triggers one reauth+retry. 401 in `foldCurrentSessionIntoWorldState` triggers reauth and retries compaction. Clear "run login.ts" error if reauth fails.
- **Tool call batching** — Already works. All `tool_use` blocks from a single response are executed and results collected before the next API call.
- **`run_command` truncation** — 100KB cap per stream is already generous. Truncation is flagged explicitly in output. Not a real pain point.
- **Context health visibility** — Turn footer already shows `in:/out:` token counts. No gap.
- **`sudo` handling** — Wait for a real pain point.
- **Multi-file edit atomicity** — The test-revert discipline (run `bun test`, revert on red) provides the safety net. No code change needed.
- **Interrupt/cancel** — Esc already sends abort signal. No gap.
