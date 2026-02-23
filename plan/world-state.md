## Omega — State of the World

### Purpose
Omega is a self-improving coding agent running in a terminal. It edits its own source code in `src/`, runs `bun test`, commits on green, reverts on red, and restarts itself. The operator interacts via raw terminal input with dictation support.

### Stack
TypeScript + Bun. Raw terminal I/O via `src/terminal/` modules (`input.ts`, `renderer.ts`, `app.ts`); `src/ui-raw.ts` is now a thin shim that re-exports from there and is the entry point. No UI library. Agent core (`src/agent.ts`) has no UI imports. Config is code (`src/config.ts`). `StreamProvider` interface allows mock injection in tests — real API never called in tests.

### Auth
Claude Max via OAuth PKCE through `claude.ai` (sk-ant-oat-… tokens). System prompt must be prefixed with Claude Code identity string for OAuth. Falls back to `ANTHROPIC_API_KEY`. OpenAI Codex fallback via `OPENAI_API_KEY` for `/codex` command and rate-limit fallback.

### Git Push Cadence
Push to origin at least every 3 commits (enforced via system prompt rule in `src/config.ts`).

### Context Management (three-zone model)
- **Zone 1** — `plan/world-state.md`: LLM-compacted summary of all prior sessions. Loaded at session start into system prompt. Updated by `foldCurrentSessionIntoWorldState()` on clean shutdown. Lives under source control.
- **Zone 2** — turn summaries: after each `turn_end`, completed turn messages are LLM-compacted into a 2-message synthetic exchange. History is always exactly 2 messages after compaction. Implemented in `src/compaction.ts` via `compactTurn()`.
- **Zone 3** — current turn: always verbatim, never compacted.
- Hard message cap: 100 messages. Token budget: 100k.

No raw session persistence. No "resume session?" prompt. The world file is the only cross-session artifact.

### Planning Files
- `plan/world-state.md` — Zone 1 world state; auto-maintained; under source control.
- `plan/future.md` — discrete actionable backlog items; manually maintained.

The system prompt references only `world-state.md` and `future.md`.

### Slash Commands
| Command | Effect |
|---------|--------|
| `/sonnet` | Anthropic `claude-sonnet-4-6` (default) |
| `/opus` | Anthropic `claude-opus-4-6` |
| `/codex` | OpenAI `gpt-5.2-codex` |
| `/help` | Compact command list with provider-sensitive footer legend |

Old commands `/gpt`, `/openai`, `/anthropic` are removed and yield "Unknown command". Rate-limit error messages reference `/sonnet`, `/opus`, `/codex`. Startup hint shows `/sonnet /opus /codex /help`.

### Prompt Caching Architecture
Three cache breakpoints: system prompt, last tool definition, last history message. Within a turn's agentic loop, each successive API call gets massive cache hits on all previously-sent messages. Cross-turn, only the stable system+tools prefix (~5-7k tokens) cache-hits because `compactAfterTurn` replaces history with a 2-message summary. Anthropic's server-side prefix matching handles staleness via natural non-matching + 5-min TTL — no hash-based invalidation needed.

### Key Files
- `src/agent.ts` — Agent class, `sendMessage` async generator, `StreamProvider` type, truncation, compaction wiring, zone tracking, `PRICING` table; `foldCurrentSessionIntoWorldState()` is an async generator yielding `AgentEvent`s including `world_state_saved`; `getActiveFoldProvider()` returns a provider wrapping the currently active provider for use during shutdown fold; builds `systemBlocks` and `cachedTools` with `cache_control` for prompt caching; extracts and accumulates `sessionCacheCreationTokens`/`sessionCacheReadTokens`; `estimateCostWithCache()` for cost accounting; `estimateCacheSavings()` computes savings; `sessionSavedUsd` accumulates per-turn savings; `TurnMetrics` and `turn_end` events carry `savedUsd`; has `private activeModel: string = config.model` (session-scoped, set by slash commands); `getActiveModel()` exported; agentic loop uses `activeModel` local var (from `this.activeModel`) for all API calls; `provider` stays binary `"anthropic" | "openai"`; `addCacheControlToLastMessage()` helper adds a third cache breakpoint on the last history message without mutating `this.history`; `compactAfterTurn` passes `this.activeModel` to `compactTurn`; `foldCurrentSessionIntoWorldState` passes `this.activeModel` to `compactWorldState` (both primary and re-auth retry paths); `/help` emits a provider-sensitive footer legend (Anthropic shows all fields with multipliers, OpenAI shows only `new:`/`out:`/`cost:`); parallel tool execution: all `tool_call` events emitted first, then `Promise.all` executes tools concurrently, then all `tool_result` events emitted in original order; `tool_call` and `tool_result` events both carry `id: string`; the display-only `compactionRequest` object uses `max_tokens: 4096`
- `src/compaction.ts` — `compactTurn()`, `compactWorldState()` — LLM-based compaction; both accept a `model` parameter (default `"claude-sonnet-4-6"`) forwarded to `callLlm`; `callLlm` accepts a `maxTokens` parameter (default `2048`); `compactTurn` explicitly passes `2048`; `compactWorldState` explicitly passes `4096`; world-state prompt caps last-session section to 1–4 sentences
- `src/world-state.ts` — `readWorldState()`, `writeWorldState()`, `projectWorldStatePath()` → `<cwd>/plan/world-state.md`
- `src/ui-raw.ts` — **thin re-export shim** (26 lines). Re-exports `parseKeys`, `displayWidth`, `renderToolStart`, `renderToolResult`, `renderApiRequest`, `runApp` from the `src/terminal/` modules. Has `import.meta.main` guard so `bun run src/ui-raw.ts` still starts the app. All real logic lives in `src/terminal/`.
- `src/terminal/input.ts` — `parseKeys`, `displayWidth`, all line-editing helpers (`redrawLine`, `moveVisualCol`, word boundary functions, cursor helpers), `sharedBuffer`, `sharedPasteState`, `KeyCallbacks` interface. Paste state objects carry `startVisualCol`/`startCursor` (recorded at `[200~`) so `[201~` redraws correctly. O(1) BMP-append fast path for end-of-buffer inserts.
- `src/terminal/renderer.ts` — ANSI color helpers (`bold`, `dim`, `green`, etc.), `printBlock`, `println`, `now()`, `truncateOutput`, and all block renderers: `renderToolStart(name, input, id)`, `renderToolResult(result, id)`, `renderApiRequest`, `renderApiResponse`, `renderToolResultMessage`, `renderAssistantMessage`, `renderUserMessage`, `renderStatus`. Both tool render functions display the last 6 chars of `id` as a dim bracketed suffix on the header line.
- `src/terminal/app.ts` — `runApp`, `shutdown`, `setupRawInput`, `printPrompt`, and the full agent-event loop (wires `agent.ts` events to renderer calls). Shutdown drains `foldCurrentSessionIntoWorldState()`. `formatTurnFooter` called with `savedUsd` (turn and session). `status` events rendered via `printBlock`. Bracketed paste mode enabled at startup.
- `src/turn-footer.ts` — `formatTurnFooter(turn, session, provider, model)` returns `{ turnLine, sessionLine }`; Anthropic format: `new: <non-cached, 1×>  write: <cache-write, 1.25×>  read: <cache-read, 0.1×>  out: <output>  cost: $X  saved: $X`; all three input buckets and `saved:` always shown for Anthropic even when zero; OpenAI shows `new:` and `out:` only with `cost: <=<ceiling>`
- `src/tools.ts` — `read_file`, `write_file`, `edit_file`, `list_files`, `run_command`, `run_background`, `kill_process`, `web_search`, `fetch_url`, `grep_files`, `find_files`; `executeWebSearch` dispatches to `executeBraveSearch()` (primary) or `executeDuckDuckGoSearch()` (fallback); `executeGrepFiles` probes for `rg` via `which`, falls back to `grep -rn`, caps at `max_results` (default 200) with truncation annotation, treats exit code 1 as no-matches; `executeFindFiles` probes for `fd` via `which`, falls back to `find -name`, supports `pattern`, `path`, `type` (f/d/l), `hidden` (default false), `max_results` (default 200); `executeRunBackground` spawns a detached process and returns its PID; `executeKillProcess` sends a signal (default SIGTERM) to a PID
- `src/openai.ts` — OpenAI Codex integration; `callOpenAi()` accepts and forwards `AbortSignal`; `buildOpenAiRequest()` translates Anthropic-format history to OpenAI Responses API flat `input` array; currently resends full translated history on every agentic-loop iteration (FEAT-3 would fix this)
- `src/config.ts` — model (`claude-sonnet-4-6`), fallbackModel (`gpt-5.2-codex`, used for `/codex`), system prompt, token limits; system prompt includes a `## Tool usage guidance` section nudging use of `grep_files` over speculative `read_file`, `find_files` over repeated `list_files`, `run_background`/`kill_process` for long-lived processes, and parallel tool calls for independent operations
- `src/session.ts` — kept for independent tests; not imported by production code
- `plan/future.md` — backlog; `[TOPIC] Prompt queuing` section at top (highest priority) with full framing; `[TOPIC] Provider feature parity & architecture` section with ARCH-1, FEAT-2–4; FEAT-3 marked high priority — do soon; `[TOPIC] Web interface` open item; TOOLS-1–3 done, TOOLS-4 open

### Provider Feature Gaps
- **ARCH-1** — Extract `callAnthropicTurn()` / `callOpenAiTurn()` helpers to eliminate the large `if (useOpenAi)` branches in the agentic loop. Do first; unblocks everything else.
- **FEAT-2** — Anthropic extended thinking: pass `thinking: { type: "enabled", budget_tokens: N }` + `interleaved-thinking-2025-05-14` beta header. Requires FEAT-4 first on API-key path.
- **FEAT-3 (HIGH — DO SOON)** — OpenAI `previous_response_id`: store and forward last response ID in the agentic loop so we don't resend full history each call. Currently `buildOpenAiRequest()` resends complete translated history on every agentic-loop iteration; `previous_response_id` would cut OpenAI input tokens ~80% on long sessions. Four concrete sub-tasks in `plan/future.md`.
- **FEAT-4** — Beta headers on API-key client path (currently only OAuth path sets beta headers).

### Tool API Architecture Note
Anthropic and OpenAI tool formats are fundamentally incompatible: Anthropic nests tool calls inside `content` blocks in role-based messages; OpenAI Responses API uses a flat `input` array with top-level `function_call`/`function_call_output` items. The translation layer in `buildOpenAiRequest()` is the correct approach. Anthropic-only features: prompt caching, streaming tool input, `is_error` on tool results, extended thinking. OpenAI-only: `previous_response_id` for stateful server-side context.

### Test Files
- `src/agent-integration.test.ts` — full sendMessage loop tests including slash command tests; Anthropic `/help` test asserts presence of `1×`, `1.25×`, `0.1×` multipliers; test asserting parallel dispatch (both `tool_call` events before any `tool_result`)
- `src/agent-rate-limit.test.ts` — rate-limit retry and error message tests (references `/sonnet`, `/opus`, `/codex`)
- `src/fold-events.test.ts` — fold generator tests; includes test asserting that compaction uses the active model (e.g. `claude-opus-4-6` after `/opus`)
- `src/tool-renderers.test.ts` — tests for `renderToolStart` and `renderToolResult`; all calls pass `id` as third/second argument; tests assert short ID suffix appears in rendered output
- `src/tools.test.ts` — 73 tool tests covering all tools including `grep_files` (13), `find_files` (11), `run_background` and `kill_process` (10)
- `src/ui-raw.test.ts` — 358 tests for raw terminal key handling and line editing (imports from `src/terminal/input.ts` via the `ui-raw.ts` shim); includes bracketed paste correctness tests (non-empty buffer, mid-buffer, wrap-safe path, multi-row span) and a latency guard asserting exactly 1 `stdout.write` per char-append on a 5,000-char buffer
- `src/entry.test.ts` — structural invariant tests; now includes two tests asserting that `src/terminal/input.ts`, `renderer.ts`, `app.ts` exist and that `src/ui-raw.ts` re-exports `parseKeys`, `displayWidth`, `renderToolStart`, `renderToolResult` (guards against accidental deletion of the shim or new terminal modules)

### Prompt Queuing
*See `plan/future.md` — `[TOPIC] Prompt queuing` (top priority item) for full analysis.*

The next major design area. Core insight: each turn is a sequence of context states `C₀ → C₁ → … → Cₙ`; a queued prompt is a function that transforms some *future* state rather than the current one. Key unresolved questions: injection granularity (after current tool call, after agentic loop, after turn compaction); how queued prompts are stored and delivered; whether they inject into the current turn's history or start a new turn; UI affordance. Two design-anchor use cases: (UX-Q1) ideal hard stop — what should Esc semantics be? and (UX-Q2) how to redirect a turn mid-flight without destroying it. No code changes yet — design phase.

### Most Recent Session
Split `src/ui-raw.ts` into three focused modules under `src/terminal/`: `input.ts` (key parsing, line editing, shared buffer/paste state), `renderer.ts` (ANSI helpers, all block renderers including `renderToolStart`/`renderToolResult` with short-ID suffix display), and `app.ts` (agent-event loop, shutdown, raw-mode setup). `src/ui-raw.ts` is now a 26-line thin re-export shim; all imports from tests continue to work unchanged. Two structural invariant tests added to `src/entry.test.ts` guard against accidental deletion of the new files or shim exports. All 360 tests pass. Committed `4183922` and pushed. This split establishes the seam needed for the web interface — `renderer.ts` is the model for a future web renderer emitting JSON/SSE instead of ANSI; `app.ts` will fork into `terminal/app.ts` vs `web/server.ts` when the WebSocket layer is added.