## Omega — State of the World

### Purpose
Omega is a self-improving coding agent running in a terminal. It edits its own source code in `src/`, runs `bun test`, commits on green, reverts on red, and restarts itself. The operator interacts via raw terminal input with dictation support.

### Stack
TypeScript + Bun. Raw terminal I/O (`src/ui-raw.ts`). No UI library. Agent core (`src/agent.ts`) has no UI imports. Config is code (`src/config.ts`). `StreamProvider` interface allows mock injection in tests — real API never called in tests.

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
Three cache breakpoints: system prompt, last tool definition, last history message. Within a turn's agentic loop, each successive API call gets massive cache hits on all previously-sent messages. Cross-turn, only the stable system+tools prefix (~5-7k tokens) cache-hits because `compactAfterTurn` replaces history with a 2-message summary. Anthropic's server-side prefix matching handles staleness via natural non-matching + 5-min TTL — no hash-based invalidation needed. The identity `cost + saved === hypothetical_no_cache_cost` holds exactly.

### Key Files
- `src/agent.ts` — Agent class, `sendMessage` async generator, `StreamProvider` type, truncation, compaction wiring, zone tracking, `PRICING` table; `foldCurrentSessionIntoWorldState()` is an async generator yielding `AgentEvent`s including `world_state_saved`; `getActiveFoldProvider()` returns a provider wrapping the currently active provider for use during shutdown fold; builds `systemBlocks` and `cachedTools` with `cache_control` for prompt caching; extracts and accumulates `sessionCacheCreationTokens`/`sessionCacheReadTokens`; `estimateCostWithCache()` for cost accounting; `estimateCacheSavings()` computes savings; `sessionSavedUsd` accumulates per-turn savings; `TurnMetrics` and `turn_end` events carry `savedUsd`; has `private activeModel: string = config.model` (session-scoped, set by slash commands); `getActiveModel()` exported; agentic loop uses `activeModel` local var (from `this.activeModel`) for all API calls; `provider` stays binary `"anthropic" | "openai"`; `addCacheControlToLastMessage()` helper adds a third cache breakpoint on the last history message without mutating `this.history`; `compactAfterTurn` passes `this.activeModel` to `compactTurn`; `foldCurrentSessionIntoWorldState` passes `this.activeModel` to `compactWorldState` (both primary and re-auth retry paths); `/help` emits a provider-sensitive footer legend (Anthropic shows all fields with multipliers, OpenAI shows only `new:`/`out:`/`cost:`); parallel tool execution: all `tool_call` events emitted first, then `Promise.all` executes tools concurrently, then all `tool_result` events emitted in original order; `tool_call` and `tool_result` events both carry `id: string`; the display-only `compactionRequest` object uses `max_tokens: 4096`
- `src/compaction.ts` — `compactTurn()`, `compactWorldState()` — LLM-based compaction; both accept a `model` parameter (default `"claude-sonnet-4-6"`) forwarded to `callLlm`; `callLlm` accepts a `maxTokens` parameter (default `2048`); `compactTurn` explicitly passes `2048`; `compactWorldState` explicitly passes `4096`; world-state prompt caps last-session section to 1–4 sentences
- `src/world-state.ts` — `readWorldState()`, `writeWorldState()`, `projectWorldStatePath()` → `<cwd>/plan/world-state.md`
- `src/ui-raw.ts` — raw terminal UI; shutdown drains `foldCurrentSessionIntoWorldState()`; bracketed paste mode; cursor-aware line editing with Left/Right arrows (character movement), Ctrl+Left/Right (word movement), Ctrl+Backspace (word-backward delete), Ctrl+Delete (word-forward delete); exports `renderToolStart(name, input, id)`, `renderToolResult(result, id)`, `displayWidth`; both render functions display the last 6 chars of `id` as a dim bracketed suffix (e.g. `[XYZ789]`) on the header line, enabling visual matching of parallel tool calls; `formatTurnFooter` called with `savedUsd` (turn and session); `status` events rendered via `printBlock(now(), event.message.split("\n"))`
- `src/turn-footer.ts` — `formatTurnFooter(turn, session, provider, model)` returns `{ turnLine, sessionLine }`; Anthropic format: `new: <non-cached, 1×>  write: <cache-write, 1.25×>  read: <cache-read, 0.1×>  out: <output>  cost: $X  saved: $X`; all three input buckets and `saved:` always shown for Anthropic even when zero; OpenAI shows `new:` and `out:` only with `cost: <=<ceiling>`
- `src/tools.ts` — `read_file`, `write_file`, `edit_file`, `list_files`, `run_command`, `web_search`, `fetch_url`, `grep_files`, `find_files`; `executeWebSearch` dispatches to `executeBraveSearch()` (primary) or `executeDuckDuckGoSearch()` (fallback); `executeGrepFiles` probes for `rg` via `which`, falls back to `grep -rn`, caps at `max_results` (default 200) with truncation annotation, treats exit code 1 as no-matches; `executeFindFiles` probes for `fd` via `which`, falls back to `find -name`, supports `pattern`, `path`, `type` (f/d/l), `hidden` (default false), `max_results` (default 200)
- `src/openai.ts` — OpenAI Codex integration; `callOpenAi()` accepts and forwards `AbortSignal`
- `src/config.ts` — model (`claude-sonnet-4-6`), fallbackModel (`gpt-5.2-codex`, used for `/codex`), system prompt, token limits
- `src/session.ts` — kept for independent tests; not imported by production code
- `plan/future.md` — backlog; `[TOPIC] Provider feature parity & architecture` section with four prioritised items (ARCH-1, FEAT-2–4); FEAT-1, TOOLS-1, TOOLS-2 done

### Provider Feature Gaps
Identified in analysis (see `plan/future.md` [TOPIC] section for full detail and priority order):
- **ARCH-1** — Extract `callAnthropicTurn()` / `callOpenAiTurn()` helpers to eliminate the large `if (useOpenAi)` branches in the agentic loop. Do first; unblocks everything else.
- **FEAT-2** — Anthropic extended thinking: pass `thinking: { type: "enabled", budget_tokens: N }` + `interleaved-thinking-2025-05-14` beta header. Requires FEAT-4 first on API-key path.
- **FEAT-3** — OpenAI `previous_response_id`: store and forward last response ID in the agentic loop so we don't resend full history each call.
- **FEAT-4** — Beta headers on API-key client path (currently only OAuth path sets beta headers).

### Test Files
- `src/agent-integration.test.ts` — full sendMessage loop tests including slash command tests; Anthropic `/help` test asserts presence of `1×`, `1.25×`, `0.1×` multipliers; test asserting parallel dispatch (both `tool_call` events before any `tool_result`)
- `src/agent-rate-limit.test.ts` — rate-limit retry and error message tests (references `/sonnet`, `/opus`, `/codex`)
- `src/fold-events.test.ts` — fold generator tests; includes test asserting that compaction uses the active model (e.g. `claude-opus-4-6` after `/opus`)
- `src/tool-renderers.test.ts` — tests for `renderToolStart` and `renderToolResult`; all calls pass `id` as third/second argument; tests assert short ID suffix appears in rendered output
- `src/tools.test.ts` — includes tests for `grep_files` (13) and `find_files` (11) covering primary backend, fallback, glob filtering, type filtering, hidden files, max_results cap, no-matches, error cases, and `formatToolCall` formatting; 63 tool tests total
- `src/ui-raw.test.ts` — 231+ tests for raw terminal key handling and line editing

### Most Recent Session
Added the `find_files` tool (`fd` primary, `find` fallback, mirroring `grep_files`'s pattern) with parameters `pattern`, `path`, `type`, `hidden`, and `max_results`; updated the system prompt and marked TOOLS-2 done. Also fixed a cosmetic `max_tokens: 2048` in the display-only `compactionRequest` object in `src/agent.ts` to correctly show `4096`. Full suite now passes 336 tests.