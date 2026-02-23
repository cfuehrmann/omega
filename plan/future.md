# Future — Issue Tracker

## Open items

### [INFRA] Diagnostic snapshots on fatal API errors — DONE
`src/diagnosis.ts` — `writeDiagnostic()` writes `plan/diagnosis/<timestamp>.json`
on any non-retryable API error (Anthropic or OpenAI). Snapshot contains: verbatim
error message, HTTP status, exact `requestMessages` array sent to the API, full
`this.history` at moment of failure, model, provider, call number, system blocks.
`checkDiagnostics()` checked at startup; `app.ts` prints a yellow warning block
if any files exist, anchoring the next session in hard data rather than speculation.
Files live under source control in `plan/diagnosis/`; delete after resolving.
Commit 61c4ebd.



### [BUG] ~~Line editor cursor stuck on wrapped input~~ — FIXED
Closed. `redrawFromCursor` used `\x1b[nD`/`\x1b[K` which cannot cross
terminal row boundaries. Fix: `redrawLine()` with full-line rewrite
(CUU + CR + CUF + write + `\x1b[J` + reposition), and `moveVisualCol()`
for wrap-aware arrow navigation. `terminalWidth` read from
`process.stdout.columns`; `promptWidth` set by `printPrompt`.
6 new regression tests added. Committed 892cbce.

### [BUG] ~~Bracketed paste garbled display + O(n) append latency~~ — FIXED
Closed. Two problems fixed (commit 7344295):
1. At `[201~` the old code wrote `buf.value` from the current terminal cursor
   position, which garbled the display when the buffer was non-empty before the
   paste point. Fix: record `startVisualCol` + `startCursor` at `[200~`;
   at `[201~` call `redrawLine` (wrap-safe) or emit the pasted slice + tail +
   cursor-back (legacy path).
2. Each printable-char event did `[...buf.value]` (O(n)) even for plain
   end-of-buffer append. Fix: fast path for BMP chars appended at end —
   string concat + increment cursor + one `stdout.write`, no spread.
   This keeps latency O(1) for the dominant typing/wtype-injection path.
6 new tests added (paste correctness + latency guard). 358 tests total.

### [TOPIC] Prompt queuing — interruption, injection, and turn sequencing
**Priority: HIGH — next major design area**

*See "Prompt Queuing" section in world-state.md for context and design notes.*

The core question: how should the user interact with Omega *while a turn is
already in flight*? Today, Esc aborts the turn unconditionally. But there is a
richer space of intents:

1. **Soft interrupt** — "stop what you're doing, here is a correction/redirect"
2. **Hard stop** — "abort unconditionally, don't continue"
3. **Append** — "when you're done with this turn, also do X"
4. **Inject mid-turn** — "before the next tool call, consider this"
5. **Replace** — "discard this turn, start fresh with the following prompt"

There is a "mathematics" to this: each interaction is a sequence of context
states `C₀ → C₁ → … → Cₙ`, where each tool call or agent response advances
the context. A queued prompt is a function `f` that transforms some future
state `Cₖ` rather than the current one. The key questions are:

- At what *granularity* can `f` be injected? (after current tool call, after
  current agentic loop, after full turn compaction)
- How does the *content* of the injected prompt change depending on the
  observed context at injection time?
- Can multiple queued prompts be ordered/merged, and under what algebra?

**Simple use cases to resolve first (design anchors):**

#### UX-Q1: Ideal hard stop
**How should the user cleanly stop an ongoing turn?**

Current: Esc sends an `AbortSignal`. The turn dies mid-stream. But what
happens to half-written files? Partial tool calls? The context is left dirty.

Candidates:
- Esc = abort signal + prompt user "turn aborted — what next?" (current minus
  the "what next" part)
- Esc = queue a synthetic `[STOP]` marker; agent finishes current tool call
  then exits loop cleanly
- Two-key chord: single Esc = soft abort (finish tool, stop); double Esc =
  hard kill

Acceptance criteria: define and implement one semantics; document the choice.

#### UX-Q2: Modifying an ongoing turn
**How should the user redirect a turn that's going in the wrong direction?**

Current: no mechanism other than Esc (destructive) or waiting.

Candidates:
- A "prompt queue" buffer the user types into while a turn runs; delivered as
  a new user message at the *next clean break* (after current tool call
  finishes, before the next API call)
- A visible "pending" line in the UI showing the queued prompt
- Allow the queued prompt to arrive mid-agentic-loop (before the next tool
  use) so the model can course-correct without starting over

**Design questions that need answers before implementation:**
- Where is the queue buffer stored? (in `ui-raw.ts` state? in `agent.ts`?)
- How does the agent loop poll or receive the queued message?
  (shared `AsyncIterable`? a callback? a `Promise` that resolves when enqueued?)
- Does a queued prompt inject into the *current* turn's history (same context
  window) or start the *next* turn (after compaction)?
- What is the UI affordance? (second input line? a mode toggle? a key chord?)

Sub-tasks (to be refined once design is settled):
1. Design doc / decision record — resolve the questions above
2. Queue buffer in `ui-raw.ts` — accept input while turn runs, display pending
3. Agent loop polling — check for queued message between tool calls
4. Soft-abort semantics — finish current tool, then surface the queued prompt
5. Tests: verify queue delivery timing, UI state transitions

---

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
**Priority: high — cuts OpenAI input token cost by ~80% on long sessions; do soon**

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

#### TOOLS-4: `run_command_async` + `await_command`
**Priority: medium — enables parallelism during waits (e.g. read docs while tests run)**

`run_command` is blocking. Adding an async variant lets the agent kick off a
long-running command (e.g. `bun test`, a build, a download) and do useful
preparatory work while it runs — reading files, searching docs, writing a
plan — before awaiting the result.

Goal: two new tools that together replace `run_command` for cases where the
agent can make progress independently of the result.

Sub-tasks:
1. `run_command_async(command, cwd?)` — starts the command, returns a `jobId`
   immediately (similar to `run_background` but waits to be collected, not
   fire-and-forget).
2. `await_command(jobId, timeout_ms?)` — blocks until the command finishes (or
   times out), returns `{ stdout, stderr, exitCode }`.
3. Tests: spawn a real process, do work between the two calls, verify output.
4. Update system prompt tools list.

Note: distinct from `run_background`/`kill_process` which are fire-and-forget
process management. This is about *awaitable* async commands.

---





### [BUG] ~~Stale compaction wipes next-turn tool_use blocks (second race variant)~~ — FIXED
Closed. Commit 8c3d9a3. Root cause: multiple concurrent compactions. When turn N-1's
compaction was slower than turn N's, the older compaction finished last with a stale
`historyLenAtStart` larger than the current history length (which was already shrunk by
the newer compaction). This produced an empty tail, replacing history with just
`[sum_u, sum_a]` and wiping any next-turn messages. Subsequent tool_results push landed
at position 2 with no matching tool_use at position 1 → Anthropic API 400 error.

Fix: `compactionQueue` chain — each compaction is `.then()`-chained onto the previous
one, ensuring they run in turn-order regardless of LLM latency. `historyLenAtStart`
still captured synchronously before queueing. New regression test (4-turn scenario with
latch-controlled slow compaction). 366+1 tests pass.

---

### ~~[TOPIC] WEB-6: World-state fold on web server shutdown~~ — DONE
Commit 737a17d. `performWebShutdown(agent)` exported from `src/web/server.ts`;
drains `foldCurrentSessionIntoWorldState()` to completion (no-op on null/undefined
agent). Hooked into SIGINT/SIGTERM in `runWebApp()` after session-log save.
5 tests in `server-shutdown.test.ts` all pass.

---

### [TOPIC] Web interface e2e tests — expand coverage
**Priority: medium — foundation in place**

Playwright e2e test infrastructure is working (15 tests, commit 9cc964d).
Run with `just e2e`. Uses a lightweight test server (port 3001 + control API
on 3002) so no real Anthropic auth needed.

**Gaps to fill with more tests:**
- Reconnection flow: `.reconnect-banner` appears after 2 failed retries
- Abort button click sends `{type:"abort"}` to server
- Interrupted event renders `⊘ Interrupted` block
- Input clears after send
- Auto-scroll: feed scrolls to bottom on new content
- WEB-4 renderers (api_call_start, api_response, world_state_saved, auth block) —
  add tests before or alongside the renderer implementation

Always go RED first: write the failing test, then implement the feature.

---

### [TOPIC] Web interface
**Priority: medium — in progress**

Replace or supplement the raw terminal UI with a browser-based interface.

**Architecture seam is in place (commit 4183922):**
- `src/terminal/input.ts` — key parsing, line editing (terminal-only)
- `src/terminal/renderer.ts` — all block renderers; the model for a future web renderer that emits JSON/SSE instead of ANSI sequences
- `src/terminal/app.ts` — agent-event loop wired to the terminal renderer; will fork into `terminal/app.ts` vs `web/server.ts`

**Decided direction:** WebSocket bidirectional channel (user input + agent events), static HTML/JS served by Bun's built-in HTTP server, Solid.js frontend. SSH tunnel for deployment (no public port needed).

**State model:** `Turn[]` in `src/web/client/store.ts`; each turn holds an ordered list of `WsEvent`s; streaming text accumulated separately; UI derives all display from this store.

**Stack:** Vite + `vite-plugin-solid` in `src/web/client/`; built output served from `src/web/public/` (gitignored). Usage: `bun run web:build && bun run web`. Dev: `bun run web` in one terminal, `npx vite` in `src/web/` in another (WS proxy to :3000).

**Done:**
- ~~WEB-1~~: `src/web/server.ts` — Bun HTTP + WebSocket; streams `AgentEvent` JSON; accepts `{type:message}` / `{type:abort}`; serves static build output (commit 99a9826)
- ~~WEB-2~~: Solid.js client — `App.tsx`, `main.tsx`, `style.css`, `index.html`; EventBlock/TurnView/InputArea/StatusDot components; auto-scroll; Send/Abort (commit 99a9826)
- ~~WEB-3~~: `store.ts` — `Turn[]` reactive store; `dispatch()` handles all `WsEvent` types (commit 99a9826)

**Remaining:**
4. WEB-4: Parity pass — review all `AgentEvent` types against what the web client renders; add missing renderers (e.g. `api_call_start`/`api_response` debug view, world_state_saved, auth block in feed)
5. ~~WEB-5: Session persistence~~ — DONE (see below)
6. WEB-6: World-state fold on web server shutdown — call `foldCurrentSessionIntoWorldState()` on SIGINT/SIGTERM (mirrors what terminal `app.ts` does on clean exit); currently the server saves the raw event log but doesn't compact into `plan/world-state.md`

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

- **WEB-5: Session persistence** — Done. `src/web/session-store.ts` serialises the in-memory event log to `sessions/current.jsonl` (JSONL, one event per line, atomic rename). `server.ts` loads it on startup (so history replay works after crashes/restarts), saves after every `turn_end` (incremental, fire-and-forget), and saves on SIGINT/SIGTERM. Test server (`e2e/fixtures/test-server.ts`) mirrors the same logic in `sessions-test/`. 4 new Playwright tests in `e2e/persistence.spec.ts` (incremental save, ordering, save+load restart cycle, reset clears disk). Both dirs gitignored. 19 Playwright tests, 365 Bun tests pass.

- **WEB-1/2/3: Bun WebSocket server + Solid.js client + Turn store** — Done (commit 99a9826). `src/web/server.ts` streams `AgentEvent` JSON over WebSocket; `src/web/client/` is a Vite + Solid.js app with `App.tsx`, `store.ts`, `style.css`; `Turn[]` state model with reactive dispatch; structural invariant tests added.

- **WEB-0: Split `ui-raw.ts` into `src/terminal/` modules** — Done (commit 4183922). `input.ts` (key parsing, line editing, shared buffer/paste state), `renderer.ts` (ANSI helpers, all block renderers), `app.ts` (agent-event loop, shutdown, raw-mode setup). `src/ui-raw.ts` is now a 26-line thin re-export shim. Two structural invariant tests added to `src/entry.test.ts`. 360 tests pass.

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

- **TOOLS-3: Background process management** — Done. `run_background(command, cwd?)` spawns detached, returns `{ pid, logFile }` immediately. `kill_process(pid, signal?)` sends signal (default SIGTERM), handles already-dead processes gracefully. 10 tests. Commit 9023c5a. Primary use case: serving a web interface while Omega continues working (web UI planned for foreseeable future).
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
