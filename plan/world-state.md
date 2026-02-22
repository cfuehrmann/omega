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
| `/help` | Compact command list |

Old commands `/gpt`, `/openai`, `/anthropic` are removed and yield "Unknown command". Rate-limit error messages reference `/sonnet`, `/opus`, `/codex`. Startup hint shows `/sonnet /opus /codex /help`.

### Key Files
- `src/agent.ts` — Agent class, `sendMessage` async generator, `StreamProvider` type, truncation, compaction wiring, zone tracking, `PRICING` table; `foldCurrentSessionIntoWorldState()` is an async generator yielding `AgentEvent`s including `world_state_saved`; `getActiveFoldProvider()` returns a provider wrapping the currently active provider for use during shutdown fold; builds `systemBlocks` and `cachedTools` with `cache_control` for prompt caching; extracts and accumulates `sessionCacheCreationTokens`/`sessionCacheReadTokens`; `estimateCostWithCache()` for cost accounting; `estimateCacheSavings()` computes savings; `sessionSavedUsd` accumulates per-turn savings; `TurnMetrics` and `turn_end` events carry `savedUsd`; has `private activeModel: string = config.model` (session-scoped, set by slash commands); `getActiveModel()` exported; agentic loop uses `activeModel` local var (from `this.activeModel`) for all API calls; `provider` stays binary `"anthropic" | "openai"`
- `src/compaction.ts` — `compactTurn()`, `compactWorldState()` — LLM-based compaction; world-state prompt caps last-session section to 1–4 sentences
- `src/world-state.ts` — `readWorldState()`, `writeWorldState()`, `projectWorldStatePath()` → `<cwd>/plan/world-state.md`
- `src/ui-raw.ts` — raw terminal UI; shutdown drains `foldCurrentSessionIntoWorldState()`; bracketed paste mode; exports `renderToolStart`, `renderToolResult`, `displayWidth`; `formatTurnFooter` called with `savedUsd` (turn and session)
- `src/turn-footer.ts` — `formatTurnFooter(turn, session, provider, model)` returns `{ turnLine, sessionLine }`; shows `cost: $X  saved: $X` with column alignment; appends `cache_write:`/`cache_read:` when non-zero; for `provider === "openai"` shows `cost: <=$X`, omits `saved:` and all cache fields
- `src/tools.ts` — `read_file`, `write_file`, `edit_file`, `list_files`, `run_command`, `web_search`, `fetch_url`; `executeWebSearch` dispatches to `executeBraveSearch()` (primary) or `executeDuckDuckGoSearch()` (fallback)
- `src/openai.ts` — OpenAI Codex integration; `callOpenAi()` accepts and forwards `AbortSignal`
- `src/config.ts` — model (`claude-sonnet-4-6`), fallbackModel (`gpt-5.2-codex`, used for `/codex`), system prompt, token limits
- `src/session.ts` — kept for independent tests; not imported by production code
- `plan/future.md` — backlog; currently notes provider/model architecture as an open item

### Test Files
- `src/agent-integration.test.ts` — full sendMessage loop tests including 8 slash command tests
- `src/agent-rate-limit.test.ts` — rate-limit retry and error message tests (references `/sonnet`, `/opus`, `/codex`)
- `src/fold-events.test.ts` — fold generator tests; uses `/codex` to switch provider before fold
- `src/turn-footer.test.ts` — 32 tests including OpenAI `<=` prefix, Anthropic cache/savings display
- `src/ui-raw.test.ts` — 231 tests for `parseKeys`, `displayWidth`, backspace, bracketed paste, tool rendering
- `src/prompt-caching.test.ts` — 8 tests for cache_control injection, token extraction, cost accounting
- `src/planning-files.test.ts` — structural invariants: `future.md` exists, deleted files don't, system prompt references correct files

### Web Search
`BRAVE_SEARCH_API_KEY` is set in `.env`. `web_search` uses Brave (primary) with DuckDuckGo fallback.

### Recent Session
The OpenAI turn-footer cost display was implemented with `<=` prefix and no cache/savings fields, and old slash commands (`/gpt`, `/openai`, `/anthropic`) were replaced with `/sonnet`, `/opus`, `/codex`, `/help`; the agent gained a session-scoped `activeModel` field so switching to Opus actually routes requests to the correct model. All 280 tests pass.

### Open Issues
- **Provider/model architecture** — current design (`provider` binary + `activeModel` string) works but will need a cleaner registry pattern when adding more providers or models. Tracked in `plan/future.md`.