## Omega — State of the World

### Purpose
Omega is a general-purpose coding agent that runs in a terminal. It can be pointed at any project directory and will read the project's `README.md` for orientation. When pointed at its own repo, it can develop itself. The operator interacts via raw terminal input with dictation support.

### Stack
TypeScript + Bun. Raw terminal I/O via `src/terminal/` modules (`input.ts`, `renderer.ts`, `app.ts`); `src/ui-raw.ts` is now a thin shim that re-exports from there and is the entry point. No UI library. Agent core (`src/agent.ts`) has no UI imports. Config is code (`src/config.ts`). `StreamProvider` interface allows mock injection in tests — real API never called in tests.

### Auth
Claude Max via OAuth PKCE through `claude.ai` (sk-ant-oat-… tokens). System prompt must be prefixed with Claude Code identity string for OAuth. Falls back to `ANTHROPIC_API_KEY`. OpenAI Codex fallback via `OPENAI_API_KEY` for `/codex` command and rate-limit fallback.

### Git Push Cadence
Push to origin at least every 3 commits (documented in `README.md`; no longer hardcoded in system prompt).

### Context Management (three-zone model)
- **Zone 1** — `plan/world-state.md`: LLM-compacted summary of all prior sessions. Loaded at session start into system prompt. Updated by `foldCurrentSessionIntoWorldState()` on clean shutdown. Lives under source control.
- **Zone 2** — turn summaries: after each `turn_end`, completed turn messages are LLM-compacted into a 2-message synthetic exchange. History is always exactly 2 messages after compaction. Implemented in `src/compaction.ts` via `compactTurn()`.
- **Zone 3** — current turn: always verbatim, never compacted.
- Hard message cap: 100 messages. Token budget: 100k.

No raw session persistence. No "resume session?" prompt. The world file is the only cross-session artifact.

### Planning Files
- `plan/world-state.md` — Zone 1 world state; auto-maintained; under source control.
- `plan/future.md` — discrete actionable backlog items; manually maintained.
- `manifest.md` — high-level design manifest for ongoing refactoring. Strategic direction.
- `README.md` — project orientation for any agent (including Omega itself). References all planning files.

The system prompt is project-agnostic: it tells the agent to read `README.md` for orientation. Project-specific rules (git discipline, testing discipline, planning file locations) live in the README, not the system prompt. This decoupling allows Omega to work on any project.

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

### Event Taxonomy (coordinate-system model)
Events are named as messages between three parties: **agent**, **user**, **llm**. Direction is explicit in the name. Frequency determines log level (per-session=info, per-turn=info if files touched or history changes, per-LLM-iteration=debug, transient=trace).

**Taxonomy table:**

| Event name | Meaning | Stream |
|---|---|---|
| `agent_to_llm` | LLM call in main agentic loop | pino debug |
| `agent_to_llm_compact_turn` | LLM call to compact a turn | pino debug |
| `agent_to_llm_compact_session` | LLM call to fold session into world-state | pino debug |
| `llm_to_agent` | Response to main loop call | AgentEvent + pino debug |
| `user_to_agent` | User submits a message | pino (future) |
| `agent_to_user` | Agent streams output to user | AgentEvent (future) |
| `agent_to_agent_tool_call` | Tool invocation | AgentEvent + pino debug |
| `agent_to_agent_tool_result` | Tool result | AgentEvent + pino debug |
| `agent_to_agent_compact_turn` | Turn compaction (internal) | pino debug |
| `agent_to_agent_compact_session` | Session fold (internal) | pino debug |

**True duals** (same moment, both AgentEvent and pino log, same name): `llm_to_agent`, `agent_to_agent_tool_call`, `agent_to_agent_tool_result`. Wire-format uses of `"tool_result"` in Anthropic history blocks are left untouched — those are API protocol, not event names.

**One-sided only** (UI-only or infra-only, no unification needed): `text`, `status`, `interrupted`, `metrics`, `turn_end`, `api_call_start`; `startup`, `oauth_*`, `context_truncated`, `session_compacted`, `api_retry`, `diagnostic_written`.

**Remaining renaming** (LOG-2 complete — see future.md): all pino wrappers and per-iteration event names migrated to taxonomy-compliant names.

### Key Files
- `src/agent.ts` — Agent class, `sendMessage` async generator, `StreamProvider` type, truncation, compaction wiring, zone tracking, `PRICING` table; `foldCurrentSessionIntoWorldState()` is an async generator yielding `AgentEvent`s including `world_state_saved`; `getActiveFoldProvider()` returns a provider wrapping the currently active provider for use during shutdown fold; builds `systemBlocks` and `cachedTools` with `cache_control` for prompt caching; extracts and accumulates `sessionCacheCreationTokens`/`sessionCacheReadTokens`; `estimateCostWithCache()` for cost accounting; `estimateCacheSavings()` computes savings; `sessionSavedUsd` accumulates per-turn savings; `TurnMetrics` and `turn_end` events carry `savedUsd`; has `private activeModel: string = config.model` (session-scoped, set by slash commands); `getActiveModel()` exported; agentic loop uses `activeModel` local var (from `this.activeModel`) for all API calls; `provider` stays binary `"anthropic" | "openai"`; `addCacheControlToLastMessage()` helper adds a third cache breakpoint on the last history message without mutating `this.history`; `compactAfterTurn` passes `this.activeModel` to `compactTurn`; `foldCurrentSessionIntoWorldState` passes `this.activeModel` to `compactWorldState` (both primary and re-auth retry paths); `/help` emits a provider-sensitive footer legend (Anthropic shows all fields with multipliers, OpenAI shows only `new:`/`out:`/`cost:`); parallel tool execution: all `agent_to_agent_tool_call` events emitted first, then `Promise.all` executes tools concurrently, then all `agent_to_agent_tool_result` events emitted in original order; both tool events carry `id: string`; the display-only `compactionRequest` object uses `max_tokens: 4096`; `compactAfterTurn()` snapshots `this.history.length` before awaiting, then merges `[...newSummaryMsgs, ...this.history.slice(historyLenAtStart)]` to preserve any next-turn messages pushed during async compaction; on non-retryable fatal errors calls `flushLog()` then `writeDiagnostic()` from `src/diagnosis.ts`; `diagDir` field is `string | null | undefined` — when a mock `streamProvider` is injected and no explicit `diagDir` is given, constructor defaults it to `null`; all four diagnostic write sites forward `this.diagDir`; all logging via `logger.debug/info/warn` from `src/logger.ts`
- `src/compaction.ts` — `compactTurn()`, `compactWorldState()` — LLM-based compaction; both accept a `model` parameter (default `"claude-sonnet-4-6"`) forwarded to `callLlm`; `callLlm` accepts a `maxTokens` parameter (default `2048`); `compactTurn` explicitly passes `2048`; `compactWorldState` explicitly passes `4096`; world-state prompt caps last-session section to 1–4 sentences
- `src/world-state.ts` — `readWorldState()`, `writeWorldState()`, `projectWorldStatePath()` → `<cwd>/plan/world-state.md`
- `src/logger.ts` — pino-backed structured logger. Writes JSON-lines synchronously (`sync: true`). Log rotation (`omega.log → omega.prev.log`, exactly 2 sessions retained) is **not** automatic on import; instead it is triggered explicitly by `initLogger()`, which is idempotent (no-op after first call) and only rotates when `IS_PRODUCTION_LOG` (i.e. `OMEGA_LOG_FILE` is not set). `initLogger()` must be called as the very first statement in each app entry point: `runApp()` in `src/terminal/app.ts` and `runWebApp()` in `src/web/server.ts`. **Pino instance is lazy** (`getPino()` factory): the file descriptor is opened on the first actual log write, which always occurs after `initLogger()` has rotated — this prevents the bug where pino would open `omega.log` before rotation, then write to `omega.prev.log` after the rename. `OMEGA_LOG_FILE` env var overrides the log path — used by `src/test-setup.ts` preload to redirect test output to `/dev/null`. `flushLog()` is a no-op shim (kept so call sites still compile). `startup()` convenience wrapper. `makeLogEntry()` factory for taxonomy-compliant `MessageEntry`/`InfraEntry` discriminated union shapes. `omega.log` and `omega.prev.log` in `.gitignore`.
- `src/diagnosis.ts` — `writeDiagnostic(data, diagDir?)` writes a JSON snapshot to `diagnosis/<ISO-timestamp>.json`; `diagDir` parameter is `string | null | undefined` — passing `null` disables the write entirely (returns `null` early); snapshots contain `logFile: "omega.log"` pointer; `checkDiagnostics()` returns existing snapshot paths sorted oldest-first; all I/O errors silently swallowed; `diagnosis/` is in `.gitignore`.
- `src/ui-raw.ts` — **thin re-export shim** (26 lines). Re-exports `parseKeys`, `displayWidth`, `renderToolStart`, `renderToolResult`, `renderApiRequest`, `runApp` from the `src/terminal/` modules. Has `import.meta.main` guard so `bun run src/ui-raw.ts` still starts the app.
- `src/terminal/input.ts` — `parseKeys`, `displayWidth`, all line-editing helpers (`redrawLine`, `moveVisualCol`, word boundary functions, cursor helpers), `sharedBuffer`, `sharedPasteState`, `KeyCallbacks` interface. Paste state objects carry `startVisualCol`/`startCursor`. O(1) BMP-append fast path for end-of-buffer inserts. Plain Delete key (`\x1b[3~`) forward-deletes the char under the cursor; `Ctrl+Delete` (`\x1b[3;5~`) deletes word forward.
- `src/terminal/renderer.ts` — ANSI color helpers, `printBlock`, `println`, `now()`, `truncateOutput`, and all block renderers: `renderToolStart(name, input, id)`, `renderToolResult(result, id)`, `renderApiRequest`, `renderApiResponse`, `renderToolResultMessage`, `renderAssistantMessage`, `renderUserMessage`, `renderStatus`. Both tool render functions display the last 6 chars of `id` as a dim bracketed suffix on the header line.
- `src/terminal/app.ts` — `runApp`, `shutdown`, `setupRawInput`, `printPrompt`, and the full agent-event loop. Calls `initLogger()` as its very first statement. Shutdown drains `foldCurrentSessionIntoWorldState()`. `formatTurnFooter` called with `savedUsd` (turn and session). `status` events rendered via `printBlock`. Bracketed paste mode enabled at startup. At startup calls `checkDiagnostics()` and prints a yellow warning block listing all snapshot paths if any exist. After each turn's footer lines, calls `checkDiagnostics()` again and prints a red `⚠ N diagnostic snapshot(s): <names>` line if any exist.
- `src/turn-footer.ts` — `formatTurnFooter(turn, session, provider, model)` returns `{ turnLine, sessionLine }`; Anthropic format: `new: <non-cached, 1×>  write: <cache-write, 1.25×>  read: <cache-read, 0.1×>  out: <output>  cost: $X  saved: $X`; OpenAI shows `new:` and `out:` only with `cost: <=<ceiling>`
- `src/tools.ts` — `read_file`, `write_file`, `edit_file`, `list_files`, `run_command`, `run_background`, `kill_process`, `web_search`, `fetch_url`, `grep_files`, `find_files`; `executeWebSearch` dispatches to `executeBraveSearch()` (primary) or `executeDuckDuckGoSearch()` (fallback); `executeGrepFiles` probes for `rg` via `which`, falls back to `grep -rn`, caps at `max_results` (default 200); `executeFindFiles` probes for `fd` via `which`, falls back to `find -name`, supports `pattern`, `path`, `type`, `hidden`, `max_results`
- `src/openai.ts` — OpenAI Codex integration; `callOpenAi()` accepts and forwards `AbortSignal`; `buildOpenAiRequest()` translates Anthropic-format history to OpenAI Responses API flat `input` array
- `src/config.ts` — model (`claude-sonnet-4-6`), fallbackModel (`gpt-5.2-codex`), system prompt (project-agnostic — directs agent to read README.md), token limits
- `src/session.ts` — kept for independent tests; not imported by production code
- `plan/future.md` — backlog; `[REFACTOR] Manifest-driven redesign` at top (highest priority, step 1 done); `[TOPIC] Prompt queuing`; `[TOPIC] Provider feature parity & architecture` with ARCH-1, FEAT-2–4; `[INFRA] Self-protection` with REC-1–4; `[TOPIC] Web interface` WEB-1–6 all done; TOOLS-1–3 done, TOOLS-4 open

### Web UI
The project has a web interface under `src/web/`. Client code lives in `src/web/client/` (`App.tsx`, `main.tsx`, `style.css`). The layout order is: `ReconnectBanner → feed (scrollable, flex:1) → StatusDot (status bar) → InputArea`. The status bar was moved from the top to a sticky position between the feed and input area; `.status-row` has `border-top` and `border-bottom` with reduced padding `4px 0`. Frontend is built via `bun run web:build`. `src/web/server.ts` calls `initLogger()` as its very first statement in `runWebApp()`.

### Diagnostic Snapshots & Logging
`diagnosis/` at repo root (gitignored) holds fatal API error snapshots written by `src/diagnosis.ts`. Passing `null` as `diagDir` disables writes — the `Agent` constructor automatically sets `diagDir = null` when a mock `streamProvider` is injected and no explicit `diagDir` is given.

### Recent Sessions
- **System prompt decoupling (manifest step 1):** Removed all omega-specific content from `src/config.ts` system prompt. System prompt is now project-agnostic — tells the agent to read `README.md` for orientation. Created `README.md` at project root with all omega-specific context: planning files, stack, auth, git discipline, testing discipline, key source files, diagnosis folder. Updated `src/planning-files.test.ts` structural invariants to guard the new architecture (README exists, system prompt mentions README, README references world-state/future/manifest). All 430 unit + 27 e2e tests pass. This is the first step toward the manifest.md goal of making Omega usable on any project.
- Fixed logging bug: pino was opening `omega.log` at module import time, before `initLogger()` rotated it to `omega.prev.log`; after rotation the open fd still pointed at the renamed file so