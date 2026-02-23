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

**True duals** (same moment, both AgentEvent and pino log, same name): `llm_to_agent`, `agent_to_agent_tool_call`, `agent_to_agent_tool_result`. These were unified in commit 899f136 (previously `api_response`, `tool_call`, `tool_result`). Wire-format uses of `"tool_result"` in Anthropic history blocks are left untouched — those are API protocol, not event names.

**One-sided only** (UI-only or infra-only, no unification needed): `text`, `status`, `interrupted`, `metrics`, `turn_end`, `api_call_start`; `startup`, `oauth_*`, `context_truncated`, `session_compacted`, `api_retry`, `diagnostic_written`.

**Remaining renaming** (not yet done — see future.md LOG-2): pino wrappers `toolExec`→`debug`, `apiCall`→`debug`; per-iteration pino events currently named `api_request`/`api_response` should become `agent_to_llm`/`llm_to_agent`; per-turn aggregate `api_call` should become something turn-scoped.

### Key Files
- `src/agent.ts` — Agent class, `sendMessage` async generator, `StreamProvider` type, truncation, compaction wiring, zone tracking, `PRICING` table; `foldCurrentSessionIntoWorldState()` is an async generator yielding `AgentEvent`s including `world_state_saved`; `getActiveFoldProvider()` returns a provider wrapping the currently active provider for use during shutdown fold; builds `systemBlocks` and `cachedTools` with `cache_control` for prompt caching; extracts and accumulates `sessionCacheCreationTokens`/`sessionCacheReadTokens`; `estimateCostWithCache()` for cost accounting; `estimateCacheSavings()` computes savings; `sessionSavedUsd` accumulates per-turn savings; `TurnMetrics` and `turn_end` events carry `savedUsd`; has `private activeModel: string = config.model` (session-scoped, set by slash commands); `getActiveModel()` exported; agentic loop uses `activeModel` local var (from `this.activeModel`) for all API calls; `provider` stays binary `"anthropic" | "openai"`; `addCacheControlToLastMessage()` helper adds a third cache breakpoint on the last history message without mutating `this.history`; `compactAfterTurn` passes `this.activeModel` to `compactTurn`; `foldCurrentSessionIntoWorldState` passes `this.activeModel` to `compactWorldState` (both primary and re-auth retry paths); `/help` emits a provider-sensitive footer legend (Anthropic shows all fields with multipliers, OpenAI shows only `new:`/`out:`/`cost:`); parallel tool execution: all `agent_to_agent_tool_call` events emitted first, then `Promise.all` executes tools concurrently, then all `agent_to_agent_tool_result` events emitted in original order; both tool events carry `id: string`; the display-only `compactionRequest` object uses `max_tokens: 4096`; `compactAfterTurn()` snapshots `this.history.length` before awaiting, then merges `[...newSummaryMsgs, ...this.history.slice(historyLenAtStart)]` to preserve any next-turn messages pushed during async compaction; on non-retryable fatal errors calls `flushLog()` then `writeDiagnostic()` from `src/diagnosis.ts`; `diagDir` field is `string | null | undefined` — when a mock `streamProvider` is injected and no explicit `diagDir` is given, constructor defaults it to `null`; all four diagnostic write sites (prompt-too-long 400, generic Anthropic error, OpenAI error, fold shutdown failure) forward `this.diagDir`; `eventBuffer` field removed; all logging via `logger.debug/info/warn` from `src/logger.ts`
- `src/compaction.ts` — `compactTurn()`, `compactWorldState()` — LLM-based compaction; both accept a `model` parameter (default `"claude-sonnet-4-6"`) forwarded to `callLlm`; `callLlm` accepts a `maxTokens` parameter (default `2048`); `compactTurn` explicitly passes `2048`; `compactWorldState` explicitly passes `4096`; world-state prompt caps last-session section to 1–4 sentences
- `src/world-state.ts` — `readWorldState()`, `writeWorldState()`, `projectWorldStatePath()` → `<cwd>/plan/world-state.md`
- `src/logger.ts` — pino-backed structured logger. On module load rotates `omega.log → omega.prev.log` (exactly 2 sessions retained). Writes JSON-lines to `omega.log` at repo root; async buffered (zero hot-path latency). Thin wrapper converts `logger.info("event_name", { fields })` call-site API to pino's object-first form. Named exports: `logger`, `flushLog()` (synchronous flush), `startup()`, `toolExec()` (info, tool name+args), `apiCall()` (info, model+tokens) convenience wrappers. `omega.log` and `omega.prev.log` in `.gitignore`. minLength=1024 with 5s periodicFlush.
- `src/diagnosis.ts` — `writeDiagnostic(data, diagDir?)` writes a JSON snapshot to `diagnosis/<ISO-timestamp>.json`; `diagDir` parameter is `string | null | undefined` — passing `null` disables the write entirely (returns `null` early); snapshots contain `logFile: "omega.log"` pointer (not inline event buffer — the log IS the event timeline); `checkDiagnostics()` returns existing snapshot paths sorted oldest-first; all I/O errors silently swallowed; `diagnosis/` is in `.gitignore`. `RollingEventBuffer`, `BufferedEvent`, `writeDiagnosticWithBuffer` all removed.
- `src/ui-raw.ts` — **thin re-export shim** (26 lines). Re-exports `parseKeys`, `displayWidth`, `renderToolStart`, `renderToolResult`, `renderApiRequest`, `runApp` from the `src/terminal/` modules. Has `import.meta.main` guard so `bun run src/ui-raw.ts` still starts the app. All real logic lives in `src/terminal/`.
- `src/terminal/input.ts` — `parseKeys`, `displayWidth`, all line-editing helpers (`redrawLine`, `moveVisualCol`, word boundary functions, cursor helpers), `sharedBuffer`, `sharedPasteState`, `KeyCallbacks` interface. Paste state objects carry `startVisualCol`/`startCursor` (recorded at `[200~`) so `[201~` redraws correctly. O(1) BMP-append fast path for end-of-buffer inserts. Plain Delete key (`\x1b[3~`) forward-deletes the char under the cursor; `Ctrl+Delete` (`\x1b[3;5~`) deletes word forward.
- `src/terminal/renderer.ts` — ANSI color helpers (`bold`, `dim`, `green`, etc.), `printBlock`, `println`, `now()`, `truncateOutput`, and all block renderers: `renderToolStart(name, input, id)`, `renderToolResult(result, id)`, `renderApiRequest`, `renderApiResponse`, `renderToolResultMessage`, `renderAssistantMessage`, `renderUserMessage`, `renderStatus`. Both tool render functions display the last 6 chars of `id` as a dim bracketed suffix on the header line.
- `src/terminal/app.ts` — `runApp`, `shutdown`, `setupRawInput`, `printPrompt`, and the full agent-event loop (wires `agent.ts` events to renderer calls). Shutdown drains `foldCurrentSessionIntoWorldState()`. `formatTurnFooter` called with `savedUsd` (turn and session). `status` events rendered via `printBlock`. Bracketed paste mode enabled at startup. At startup calls `checkDiagnostics()` and prints a yellow warning block listing all snapshot paths if any exist. After each turn's footer lines, calls `checkDiagnostics()` again and prints a red `⚠ N diagnostic snapshot(s): <names>` line if any exist — making hard API errors unmissable even if the original error scrolled off screen. Handles `agent_to_agent_tool_call`, `agent_to_agent_tool_result`, `llm_to_agent` event types.
- `src/turn-footer.ts` — `formatTurnFooter(turn, session, provider, model)` returns `{ turnLine, sessionLine }`; Anthropic format: `new: <non-cached, 1×>  write: <cache-write, 1.25×>  read: <cache-read, 0.1×>  out: <output>  cost: $X  saved: $X`; all three input buckets and `saved:` always shown for Anthropic even when zero; OpenAI shows `new:` and `out:` only with `cost: <=<ceiling>`
- `src/tools.ts` — `read_file`, `write_file`, `edit_file`, `list_files`, `run_command`, `run_background`, `kill_process`, `web_search`, `fetch_url`, `grep_files`, `find_files`; `executeWebSearch` dispatches to `executeBraveSearch()` (primary) or `executeDuckDuckGoSearch()` (fallback); `executeGrepFiles` probes for `rg` via `which`, falls back to `grep -rn`, caps at `max_results` (default 200) with truncation annotation, treats exit code 1 as no-matches; `executeFindFiles` probes for `fd` via `which`, falls back to `find -name`, supports `pattern`, `path`, `type` (f/d/l), `hidden` (default false), `max_results` (default 200); `executeRunBackground` spawns a detached process and returns its PID; `executeKillProcess` sends a signal (default SIGTERM) to a PID
- `src/openai.ts` — OpenAI Codex integration; `callOpenAi()` accepts and forwards `AbortSignal`; `buildOpenAiRequest()` translates Anthropic-format history to OpenAI Responses API flat `input` array; currently resends full translated history on every agentic-loop iteration (FEAT-3 would fix this)
- `src/config.ts` — model (`claude-sonnet-4-6`), fallbackModel (`gpt-5.2-codex`, used for `/codex`), system prompt, token limits; system prompt includes `## Tool usage guidance` section; references `diagnosis/` (repo root) for fatal API error snapshots
- `src/session.ts` — kept for independent tests; not imported by production code
- `plan/future.md` — backlog; `[TOPIC] Prompt queuing` section at top (highest priority); `[TOPIC] Provider feature parity & architecture` with ARCH-1, FEAT-2–4; FEAT-3 marked high priority; `[TOPIC] Web interface` with WEB-1 through WEB-5; TOOLS-1–3 done, TOOLS-4 open; `[INFRA] Diagnostic snapshots — DONE`

### Diagnostic Snapshots & Logging
`diagnosis/` at repo root (gitignored) holds fatal API error snapshots written by `src/diagnosis.ts`. Passing `null` as `diagDir` disables writes — the `Agent` constructor automatically sets `diagDir = null` when a mock `streamProvider` is injected and no