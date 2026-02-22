# Future — Issue Tracker

## Open items

- **Provider/model architecture** — current design has `provider` (binary: anthropic/openai) + `activeModel` (string). Works for now. Future: consider a unified `{ provider, model }` pair or registry to make adding new providers/models cleaner.

---

Discrete, prioritised, actionable. Keep in priority order.

---

## Closed / dismissed items (for reference)

- **Cache savings display** — Done. Turn footer shows `cost:` (actual paid) and `saved:` (cache read savings = 0.9× input rate × read tokens) when savings > 0. Both fields column-aligned between turn/session lines via `padEnd`. `savedUsd` added to `TurnMetrics`/`SessionTotals` in both `turn-footer.ts` and `agent.ts`. `estimateCacheSavings()` exported from `agent.ts`. `sessionSavedUsd` accumulates across turns. 7 new tests.
- **Anthropic prompt caching** — Done. `cache_control: { type: "ephemeral" }` on system message block, last tool definition, and last message in conversation. Three breakpoints ensure Opus 4.6 (≥4096 token minimum) benefits from caching once conversation grows past first turn. Cache tokens extracted from usage, routed through `estimateCostWithCache()`. `TurnMetrics` and session totals track cache tokens. Turn footer shows `cache_write`/`cache_read` when non-zero. 17 tests.
- **UI tests** — Done. 231+ tests in `ui-raw.test.ts` and `tool-renderers.test.ts`.
- **Rate-limit retry** — Done. Provider-aware retry with `getOpenAiRetryDelayMs` (parses "try again in Ns") and `getAnthropicRetryDelayMs` (exponential backoff). Already at ms precision.
- **OAuth auto-relogin** — Done. `forceRefreshToken()` in auth.ts, `isAuthExpired()` + `reinitAuth()` in agent.ts. 401 in Anthropic stream loop triggers one reauth+retry. 401 in `foldCurrentSessionIntoWorldState` triggers reauth and retries compaction. Clear "run login.ts" error if reauth fails.
- **Tool call batching** — Already works. All `tool_use` blocks from a single response are executed and results collected before the next API call.
- **`run_command` truncation** — 100KB cap per stream is already generous. Truncation is flagged explicitly in output. Not a real pain point.
- **Context health visibility** — Turn footer already shows `in:/out:` token counts. No gap.
- **`sudo` handling** — Wait for a real pain point.
- **Multi-file edit atomicity** — The test-revert discipline (run `bun test`, revert on red) provides the safety net. No code change needed.
- **Interrupt/cancel** — Esc already sends abort signal. No gap.
- **Line editing** — Done. Cursor-aware editing in `parseKeys`: Left/Right arrows (char), Ctrl+Left/Right (word), Ctrl+Backspace / Ctrl+Delete (delete word backward/forward). Insert and backspace work at any cursor position with correct ANSI redraw. 14 new tests.
