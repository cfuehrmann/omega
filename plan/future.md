# Future — Issue Tracker

## Open items

### [TOPIC] Provider feature parity & architecture
*See "Provider Feature Gaps" section in world-state.md for full analysis.*

The gap between what we send to providers and what they support falls into two
categories: architecture (how the code is structured) and features (what we
use). Work in priority order below.

---

#### ARCH-1: Clean provider boundary in agent.ts
**Priority: do first — unblocks everything below**

`agent.ts` currently has large `if (useOpenAi) { ... } else { ... }` blocks
inside the agentic loop: two separate retry blocks, two separate
`api_call_start` builders, etc. This makes it hard to add provider-specific
features (e.g. Anthropic extended thinking, OpenAI `previous_response_id`)
without touching unrelated code.

Goal: extract a clean `callAnthropicTurn()` and `callOpenAiTurn()` helper
(or similar boundary), so each provider's agentic-loop slice is fully
self-contained. Common plumbing (retry, abort, token accounting) stays shared.

Acceptance criteria:
- `agent.ts` agentic loop body has no large `if (useOpenAi)` branch
- Each provider helper is independently testable
- All existing tests still pass

---

#### FEAT-2: Anthropic extended thinking
**Priority: medium — quality gain on complex tasks, Opus/Sonnet only**

We never pass `thinking: { type: "enabled", budget_tokens: N }` to Anthropic.
Extended thinking would improve reasoning quality on multi-step problems.

Goal: enable thinking on Anthropic calls. Thinking tokens are output tokens
for billing; budget should be a fraction of `max_tokens`.

Sub-tasks:
1. Add `thinking` param to Anthropic stream call (Sonnet: budget ~8k, Opus: ~16k)
2. Handle `thinking` content blocks in the event stream (don't yield as text;
   optionally emit as a separate `thinking` event type for debug logging)
3. Add thinking tokens to turn footer / cost accounting if billed separately
4. Tests: mock stream includes thinking blocks; verify they don't corrupt history

Note: requires `anthropic-beta: interleaved-thinking-2025-05-14` header.
The OAuth client already sets beta headers; the API-key client does not — this
must be fixed as part of this task (add the header to the API-key client path
or unify the client initialisation).

---

#### FEAT-3: OpenAI `previous_response_id`
**Priority: medium — cuts OpenAI input token cost by ~80% on long sessions**

`callOpenAi()` currently resends the full `this.history` on every call inside
the agentic loop. The Responses API supports `previous_response_id` to let the
server maintain history, so we only send the new user message each time.

Goal: store the last response ID and pass it on successive calls within a turn.
History management across turns (compaction) still happens client-side.

Sub-tasks:
1. `callOpenAi()` accepts and returns `previousResponseId`
2. Inside the agentic loop, thread the ID through successive calls
3. On turn boundary (compaction), reset the ID (start fresh next turn)
4. Tests: mock verifies the ID is forwarded on call 2+

---

#### FEAT-4: Anthropic beta headers on API-key path
**Priority: low-medium — prerequisite for FEAT-2 on API-key auth**

The OAuth client sets `anthropic-beta: claude-code-20250219,oauth-2025-04-20`.
The API-key client (`new Anthropic()`) sends no beta headers, which blocks
extended thinking (`interleaved-thinking-2025-05-14`) and other betas.

Goal: unify beta header injection so both auth paths get the same betas
(minus the oauth-specific one on the API-key path).

Acceptance criteria:
- API-key client includes `interleaved-thinking-2025-05-14` (and others as needed)
- Existing auth tests still pass

---

### [TOPIC] Tool set expansion
*Current tools: `read_file`, `write_file`, `edit_file`, `list_files`, `run_command`, `web_search`, `fetch_url`.*

---



#### TOOLS-3: Background process management (`run_background` + `kill_process`)
**Priority: high — enables server/watcher workflows currently impossible**

`run_command` blocks until the process exits (with timeout). There is no way
to start a long-running process, do other work, inspect its output, and then
stop it cleanly. Workarounds (`& echo PID=$!`) leak processes across turns.

Goal: two new tools —
- `run_background(command, cwd?)` → returns `{ pid, logFile }` immediately;
  stdout+stderr are redirected to a temp log file.
- `kill_process(pid, signal?)` → sends signal (default SIGTERM), returns
  exit status.

Practical use cases:
- Start a dev server, test against it, stop it.
- Run a file watcher (`bun --watch test`), confirm it fires, stop it.
- Any "start → inspect → stop" workflow.

Acceptance criteria:
- `run_background` returns pid + log path without blocking
- `kill_process` terminates the process and returns exit info
- Leaked processes are not Omega's responsibility (operator must clean up
  if Omega crashes mid-task), but kill_process should handle "already dead"
  gracefully
- Tests use mock process spawning (no real process in unit tests)

---

### [OTHER] Provider/model architecture
Old item: current design has `provider` (binary: anthropic/openai) + `activeModel`
(string). Works for now. Future: consider a unified `{ provider, model }` pair
or registry to make adding new providers/models cleaner. Low priority until
more providers are added.

---

Discrete, prioritised, actionable. Keep in priority order.

---

## Closed / dismissed items (for reference)

- **TOOLS-INV: Tool set survey** — Done. Decision table:
  | Candidate | Decision | Reason |
  |-----------|----------|--------|
  | `grep_files` | ✅ Added (TOOLS-1) | High value, no good CLI substitute |
  | `find_files` | ✅ Added (TOOLS-2) | High value, `list_files` too blunt |
  | Background processes | ✅ Add (TOOLS-3) | Enables server/watcher workflows |
  | `move_file`/`delete_file`/`copy_file` | ⏭ Skip | `run_command mv/cp/rm` is sufficient |
  | Git structured tools | ⏭ Skip | `run_command git …` already rich |
  | `fetch_url` POST/headers | 🔜 Low priority | Useful for REST; low effort; defer |
  | Diff/patch tools | ⏭ Skip | `edit_file` + `run_command diff` covers it |
  | Clipboard/stdin injection | ⏭ Skip | No clear agentic use case for Omega |
  | Structured data (`jq`) | ⏭ Skip | `run_command jq` is sufficient |
  | Symbol navigation (LSP) | ⏭ Skip | Heavy; `grep_files` covers most needs |

- **FEAT-1: Parallel tool execution** — Done. `Promise.all` in agentic loop; all `tool_call` events emitted before any `tool_result`; results in original order. Test in `agent-integration.test.ts`.

- **TOOLS-1: `grep_files`** — Done. `executeGrepFiles` in `src/tools.ts` wraps `rg` (ripgrep) with `grep -rn` fallback. Accepts `pattern`, `path`, `file_glob`, `context_lines`, `case_sensitive`, `max_results` (default 200). Case-insensitive by default. Returns structured `file:line:text` output, capped with truncation note. 13 tests.
- **TOOLS-2: `find_files`** — Done. `executeFindFiles` in `src/tools.ts` wraps `fd` with `find` fallback. Accepts `pattern` (glob), `path`, `type` (f/d/l), `hidden` (default false), `max_results` (default 200). Ignores hidden/.gitignored by default. Returns one path per line, capped with truncation note. 11 tests.
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
