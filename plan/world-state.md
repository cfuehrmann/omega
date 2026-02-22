## Omega — State of the World

### Purpose
Omega is a self-improving coding agent running in a terminal. It edits its own source code in `src/`, runs `bun test`, commits on green, reverts on red, and restarts itself. The operator interacts via raw terminal input with dictation support.

### Stack
TypeScript + Bun. Raw terminal I/O (`src/ui-raw.ts`). No UI library. Agent core (`src/agent.ts`) has no UI imports. Config is code (`src/config.ts`). `StreamProvider` interface allows mock injection in tests — real API never called in tests.

### Auth
Claude Max via OAuth PKCE through `claude.ai` (sk-ant-oat-… tokens). System prompt must be prefixed with Claude Code identity string for OAuth. Falls back to `ANTHROPIC_API_KEY`. OpenAI Codex fallback via `OPENAI_API_KEY` for `/gpt` command and rate-limit fallback.

### Git Push Cadence
Push to origin at least every 3 commits (enforced via system prompt rule added to `src/config.ts`).

### Context Management (three-zone model)
- **Zone 1** — `plan/world-state.md` (inside the project repo): LLM-compacted summary of all prior sessions. Loaded at session start into system prompt as `## World State (from previous sessions)`. Updated by `foldCurrentSessionIntoWorldState()` on clean shutdown (SIGINT/SIGTERM/Ctrl+C). Lives under source control.
- **Zone 2** — turn summaries: after each `turn_end`, completed turn messages are LLM-compacted into a 2-message synthetic exchange. History is always exactly 2 messages after compaction. Implemented in `src/compaction.ts` via `compactTurn()`.
- **Zone 3** — current turn: always verbatim, never compacted.
- Hard message cap: 100 messages. Token budget: 100k.

No raw session persistence. No "resume session?" prompt. The world file is the only cross-session artifact. Crash mid-session loses conversational context but not work product (files).

### Planning Files
- `plan/world-state.md` — Zone 1 world state; auto-maintained by LLM compaction; under source control.
- `plan/future.md` — discrete actionable backlog items; manually maintained.
- `plan/past.md` and `plan/present.md` — **deleted** (redundant/stale).

The system prompt references only `world-state.md` and `future.md`.

### Key Files
- `src/agent.ts` — Agent class, `sendMessage` async generator, `StreamProvider` type, truncation, compaction wiring, zone tracking, `PRICING` table; `foldCurrentSessionIntoWorldState()` is an async generator yielding `AgentEvent`s including `world_state_saved`; `getActiveFoldProvider()` returns a provider wrapping the currently active provider for use during shutdown fold; builds `systemBlocks` (`TextBlockParam[]` with `cache_control: { type: "ephemeral" }`) and `cachedTools` (last tool gets `cache_control`) for prompt caching; extracts and accumulates `sessionCacheCreationTokens`/`sessionCacheReadTokens`; calls `estimateCostWithCache()` for cost accounting; `estimateCacheSavings(model, cacheReadTokens)` computes savings (0.9 × input rate × read tokens); `sessionSavedUsd` accumulates per-turn savings; `TurnMetrics` and `turn_end` events carry `savedUsd`
- `src/compaction.ts` — `compactTurn()`, `compactWorldState()` — LLM-based compaction; world-state prompt caps last-session section to 1–4 sentences, bans commit hashes and procedural detail
- `src/world-state.ts` — `readWorldState()`, `writeWorldState()`, `projectWorldStatePath()` → `<cwd>/plan/world-state.md`
- `src/ui-raw.ts` — raw terminal UI; `shutdown()` prints a magenta banner, drains `foldCurrentSessionIntoWorldState()`, handles `world_state_saved` with a dim status line; `parseKeys(chunk, callbacks, buf?, options?)` pure function with `options.pasteState` for bracketed paste injection; `setupRawInput` enables bracketed paste mode (`\x1b[?2004h`) on startup; shutdown disables it (`\x1b[?2004l`) before exit; on paste end (`[201~`), echoes full buffer to stdout; exports `renderToolStart(name, input)` and `renderToolResult(result)` for immediate per-event rendering; `renderToolExecution` retained for the shutdown/fold path; exports `displayWidth(ch: string): number` (returns 2 for CJK/wide Unicode, 1 otherwise); backspace uses column-aware erasure; `turn_end` handler passes `savedUsd` (turn and session) to `formatTurnFooter`
- `src/ui-raw.test.ts` — 231 tests for `parseKeys`, `displayWidth`, backspace behavior, bracketed paste, tool rendering
- `src/tool-renderers.test.ts` — 11 tests for `renderToolStart` and `renderToolResult`
- `src/turn-footer.ts` — `formatTurnFooter(turn, session, provider, model)` returns `{ turnLine, sessionLine }` — two ANSI-dimmed labelled lines; appends `cache_write: N` and/or `cache_read: N` when non-zero; shows `cost: $X  saved: $X` with `padEnd` column alignment so digit-count differences don't misalign between turn and session lines; `saved:` only appears when either line has savings > 0; `TurnMetrics` and `SessionTotals` include optional `cacheCreationTokens?`, `cacheReadTokens?`, `savedUsd?`
- `src/session.ts` — session persistence module (no longer imported by production code; kept for independent tests)
- `src/tools.ts` — `read_file`, `write_file`, `edit_file`, `list_files`, `run_command`, `web_search`, `fetch_url`; `executeWebSearch` dispatches to `executeBraveSearch()` (primary, uses `BRAVE_SEARCH_API_KEY`) or `executeDuckDuckGoSearch()` (fallback); Brave returns 10 results with full URLs, titles, and descriptions
- `src/openai.ts` — OpenAI Codex integration; `callOpenAi(prompt, model, provider, options, signal?)` accepts and forwards `AbortSignal` to `fetch`
- `src/config.ts` — model (`claude-sonnet-4-6`), fallback model (`gpt-5.2-codex`), system prompt, token limits; includes git push cadence rule; includes `## Web search` section telling the LLM to use `web_search` freely (Brave Search primary, high-quality index)
- `src/planning-files.test.ts` — structural invariant tests: asserts `future.md` exists, `past.md`/`present.md` do not exist, system prompt references `world-state.md` + `future.md` but not deleted files
- `src/turn-footer.test.ts` — 17+ tests for `formatTurnFooter` including cache fields and savings/cost alignment
- `src/openai.test.ts` — tests for `buildOpenAiRequest`, `parseOpenAiResponse`, and abort signal forwarding
- `src/fold-events.test.ts` — 9 tests covering generator shape, no events for null path/empty history, `api_call_start`, `api_response` with token usage, `world_state_saved` event, file written to disk, absence of `tool_result`, error event on LLM failure, and correct provider used for fold when OpenAI is active
- `src/fold-at-quit.test.ts` — tests for `foldCurrentSessionIntoWorldState()` as a generator (drains with `for await`)
- `src/prompt-caching.test.ts` — 8 tests covering cache_control injection, token extraction, session totals, and cost accounting
- `plan/future.md` — backlog (check file for current items)

### Web Search
`BRAVE_SEARCH_API_KEY` is set in `.env` (