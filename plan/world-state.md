## Omega — State of the World

### Purpose
Omega is a general-purpose coding agent that runs in a terminal. It can be pointed at any project directory and will read the project's `README.md` for orientation. When pointed at its own repo, it can develop itself. The operator interacts via raw terminal input with dictation support.

### Stack
TypeScript + Bun. Raw terminal I/O via `src/terminal/` modules (`input.ts`, `renderer.ts`, `app.ts`); `src/ui-raw.ts` is now a thin shim that re-exports from there and is the entry point. No UI library. Agent core (`src/agent.ts`) has no UI imports. Config is code (`src/config.ts`). `StreamProvider` interface allows mock injection in tests — real API never called in tests.

### Auth
Claude Max via OAuth PKCE through `claude.ai` (sk-ant-oat-… tokens). System prompt must be prefixed with Claude Code identity string for OAuth. Falls back to `ANTHROPIC_API_KEY`. OpenAI Codex fallback via `OPENAI_API_KEY` for `/codex` command and rate-limit fallback.

### Git Push Cadence
Push to origin at least every 3 commits (documented in `README.md`; no longer hardcoded in system prompt).

### Workspace Layout
`~/omega/` is a git workspace with three subdirectories: `main` (stable agent codebase), `dev` (development version), and `plan`. To run the stable agent on the dev project: `cd ~/omega/dev && bun run ~/omega/main/src/ui-raw.ts`. A shell alias `alias omega='bun run ~/omega/main/src/ui-raw.ts'` is a suggested convenience (not yet confirmed added to shell config). `ui-raw.ts` is the CLI entry point; the web server entry point is `src/web/server.ts`.

### Context Management — TRANSITIONAL STATE
- **Zone 1** — `plan/world-state.md`: LLM-compacted summary of all prior sessions. Loaded at session start into system prompt. Updated by `foldCurrentSessionIntoWorldState()` on clean shutdown. Lives under source control.
- **Zone 2** — turn summaries: **REMOVED** (manifest Step 2 complete). `compactTurn()` deleted from `src/compaction.ts`.
- **Zone 3** — current turn: always verbatim, never compacted.
- Hard message cap: 100 messages. Token budget: 100k.

**Known problem:** History grows verbatim. Proactive truncation (`truncateHistory`) silently drops middle messages before each turn, which also invalidates the prompt cache prefix for the entire history — the session pays full token rate after any truncation event. `/compact` (Step 3b, now done) is the near-term operator-triggered fix; Step 3d (non-destructive truncation) is the structural fix.

No raw session persistence. No "resume session?" prompt. The world file is the only cross-session artifact. Each session also writes `sessions/context.jsonl` (append-only JSONL of every `MessageParam`) as a foundation for Step 3b–3d. The file is cleared on startup so it always contains exactly the current session's messages.

### Manifest Refactor Status
`manifest.md` describes a major redesign. Current progress:
- **Step 1** (DONE): System prompt decoupled from Omega's own repo. Project-agnostic prompt reads `README.md` at startup.
- **Step 2** (DONE): Abandoned `compactAfterTurn()`. Removed `compactTurn()` from `src/compaction.ts`. History grows verbatim. Prompt caching handles token efficiency.
- **Step 3** (IN PROGRESS): Replace `MessageParam[]` history with an event-list data structure; persist by appending events to files. Broken into four sub-steps in `plan/future.md`:
  - **3a** (DONE): `src/context-store.ts` — appends each `MessageParam` to `sessions/context.jsonl` as it is pushed. File cleared on `runApp()` startup. Full test isolation: `null` path is a no-op; mock-provider `Agent` defaults `contextFile` to `null`.
  - **3b** (DONE): `/compact` slash command — operator-triggered mid-session compaction. `compactHistory(history, provider, model)` in `src/compaction.ts` summarises the history head via LLM, keeps the last `KEEP_RECENT_TURNS` (10) message-pairs verbatim, returns a new history starting with a synthetic `[Compacted context summary: …]` user message. Handler in `agent.ts` replaces `this.history` in-place and rewrites `sessions/context.jsonl`. 438 tests pass.
  - **3c** (TODO — **highest priority**): `SessionEvent` type + dual-write to `sessions/events.jsonl`. Additive; establishes the canonical event log.
  - **3d** (TODO): Flip the dependency — `this.history` derived from the event log; truncation becomes non-destructive. Fixes cache prefix invalidation on truncation.
- **Step 4** (FUTURE): Retire pino — event-list becomes the single source of truth.

### Planning Files
- `plan/world-state.md` — Zone 1 world state; auto-maintained; under source control.
- `plan/future.md` — discrete actionable backlog items; manually maintained.
- `manifest.md` — high-level design manifest for ongoing refactoring. Strategic direction.
- `README.md` — project orientation for any agent (including Omega itself). References all planning files.

The system prompt is project-agnostic: it tells the agent to read `README.md` for orientation. Project-specific rules (git discipline, testing discipline, planning file locations) live in the README, not the system prompt.

### Slash Commands
| Command | Effect |
|---------|--------|
| `/sonnet` | Anthropic `claude-sonnet-4-6` (default) |
| `/opus` | Anthropic `claude-opus-4-6` |
| `/codex` | OpenAI `gpt-5.2-codex` |
| `/compact` | Collapse history head into LLM summary, keep last 10 turns verbatim |
| `/help` | Compact command list with provider-sensitive footer legend |

Old commands `/gpt`, `/openai`, `/anthropic` are removed and yield "Unknown command". Rate-limit error messages reference `/sonnet`, `/opus`, `/codex`. Startup hint shows `/sonnet /opus /codex /compact /help`.

### Prompt Caching Architecture
Three cache breakpoints: system prompt, last tool definition, last history message. Within a turn's agentic loop, each successive API call gets massive cache hits on all previously-sent messages. Cross-turn, the entire accumulated history is sent verbatim (no compaction since manifest Step 2), so cache hits grow with session length — the system+tools prefix (~5-7k tokens) is always cached, and an increasingly large history prefix cache-hits on successive turns.

**Cache/truncation interaction:** Truncation drops messages from the middle of history, shifting all subsequent message positions. This makes the history byte sequence differ from the cached prefix at the first dropped position, causing a full cache miss on all history tokens. The system+tools cache breakpoints (1 and 2) are unaffected. This is a fundamental tension in the current architecture: caching wants a stable append-only prefix; truncation mutates it. Step 3d resolves this by making truncation produce an ephemeral API-call view rather than mutating stored history.

### Test Isolation — Never Pollute Production Files
Tests must **never** write to `sessions/`, `diagnosis/`, `omega.log`, or any other production file. The mechanism: `Agent` constructor applies a mock-provider heuristic — when a mock `streamProvider` is injected and no explicit path is given, `worldStatePath`, `diagDir`, and `contextFile` all default to `null` (disabled). `appendContextMessage`/`clearContextStore` treat `null` as a no-op. e2e tests use `sessions-test/` not `sessions/`. If a new production side-effect file is added, follow the same pattern: `filePath: string | null` parameter, `null` disables, constructor heuristic sets `null` for mock provider, isolation test added. Violating this contaminates production session data with test noise.

### Event Taxonomy (coordinate-system model)
Events are named as messages between three parties: **agent**, **user**, **llm**. Direction is explicit in the name.

**Taxonomy table:**

| Event name | Meaning | Stream |
|---|---|---|
| `agent_to_llm` | LLM call in main agentic loop | pino debug |
| `agent_to_llm_compact_session` | LLM call to fold session into world-state | pino debug |
| `llm_to_agent` | Response to main loop call | AgentEvent + pino debug |
| `user_to_agent` | User submits a message | pino (future) |
| `agent_to_user` | Agent streams output to user | AgentEvent (future) |
| `agent_to_agent_tool_call` | Tool invocation | AgentEvent + pino debug |
| `agent_to_agent_tool_result` | Tool result | AgentEvent + pino debug |
| `agent_to_agent_compact_session` | Session fold (internal) | pino debug |

**One-sided only** (UI-only or infra-only, no unification needed): `text`, `status`, `interrupted`, `metrics`, `turn_end`, `api_call_start`; `startup`, `oauth_*`, `context_truncated`, `session_compacted`, `api_retry`, `diagnostic_written`.

### Key Files
- `src/agent.ts` — Agent class, `sendMessage` async generator, `StreamProvider` type, truncation, `PRICING` table; history grows **verbatim** (no turn compaction since manifest Step 2); `foldCurrentSessionIntoWorldState()` is an async generator yielding `AgentEvent`s including `world_state_saved`; `getActiveFoldProvider()` returns a provider wrapping the currently active provider for use during shutdown fold; builds `systemBlocks` and `cachedTools` with `cache_control` for prompt caching; extracts and accumulates `sessionCacheCreationTokens`/`sessionCacheReadTokens`; `estimateCostWithCache()` for cost accounting; `estimateCacheSavings()` computes savings; `sessionSavedUsd` accumulates per-turn savings; `TurnMetrics` and `turn_end` events carry `savedUsd`; has `private activeModel: string = config.model` (session-scoped, set by slash commands); `getActiveModel()` exported; agentic loop uses `activeModel` local var (from `this.activeModel`) for all API calls; `provider` stays binary `"anthropic" | "openai"`; `addCacheControlToLastMessage()` helper adds a third cache breakpoint on the last history message without mutating `this.history`; `foldCurrentSessionIntoWorldState` passes `this.activeModel` to `compactWorldState` (both primary and re-auth retry paths); `/help` emits a provider-sensitive footer legend; parallel tool execution: all `agent_to_agent_tool_call` events emitted first, then `Promise.all` executes tools concurrently, then all `agent_to_agent_tool_result` events emitted in original order; both tool events carry `id: string`; on non-retryable fatal errors calls `flushLog()` then `writeDiagnostic()`; `diagDir` field is `string | null | undefined` — when a mock `streamProvider` is injected and no explicit `diagDir` is given, constructor defaults it to `null`; `contextFile` field is likewise `string | null | undefined` — same mock-provider heuristic, defaults to `null` when mock injected; all three path fields (`worldStatePath`, `diagDir`, `contextFile`) default to `null` with mock provider, giving tests automatic isolation from all production files; all logging via `logger.debug/info/warn` from `src/logger.ts`; calls `appendContextMessage()` (fire-and-forget, guarded by `if (this.contextFile !== null)`) after each `this.history.push` in `sendMessage`; `/compact` handler calls `compactHistory()`, replaces `this.history` in-place, rewrites `sessions/context.jsonl` via `clearContextStore` + re-append loop, emits status confirmation.
- `src/context-store.ts` — Step 3a foundation. `appendContextMessage(msg, filePath?)` appends a JSONL line to `sessions/context.jsonl` (creates dirs if needed). `clearContextStore(filePath?)` truncates the file to empty (no-op if missing). Both accept `string | null` as path — `null` is an explicit no-op used for test isolation. `clearContextStore()` (no arg) is called at the top of `runApp()` so each terminal session starts with an empty file containing only the current session's messages.
- `src/compaction.ts` — `compactWorldState()` (LLM-based world-state fold on shutdown) and `compactHistory()` (Step 3b — mid-session history compaction for `/compact`). `compactHistory(history, provider, model)` keeps the last `KEEP_RECENT_TURNS` (10) message-pairs verbatim, summarises the head via LLM, returns `{ history, originalCount, newCount }` with a synthetic `[Compacted context summary: …]` user message at the front. Returns history unchanged (same reference) if already short enough. `KEEP_RECENT_TURNS` exported. Both functions accept a `model` parameter defaulting to `"claude-sonnet-4-6"`.
- `src/world-state.ts` — `readWorldState()`, `writeWorldState()`, `projectWorldStatePath()` → `<cwd>/plan/world-state.md`
- `src/logger.ts` — pino-backed structured logger. Writes JSON-lines synchronously (`sync: true`). Log rotation (`omega.log → omega.prev.log`, exactly 2 sessions retained) triggered explicitly by `initLogger()`, which is idempotent and only rotates when `IS_PRODUCTION_LOG`. **Pino instance is lazy** (`getPino()` factory): file descriptor opened on first actual log write, always after `initLogger()` has rotated. `OMEGA_LOG_FILE` env var overrides the log path. `flushLog()` is a no-op shim. `startup()` convenience wrapper. `makeLogEntry()` factory for taxonomy-compliant discriminated union shapes.
- `src/diagnosis.ts` — `writeDiagnostic(data, diagDir?)` writes a JSON snapshot to `diagnosis/<ISO-timestamp>.json`; passing `null` disables the write; `checkDiagnostics()` returns existing snapshot paths sorted oldest-first.
- `src/ui-raw.ts` — **thin re-export shim** (26 lines). Re-exports `parseKeys`, `displayWidth`, `renderToolStart`, `renderToolResult`, `renderApiRequest`, `runApp` from `src/terminal/` modules. Has `import.meta.main` guard so `bun run src/ui-raw.ts` starts the app. This is the **CLI entry point**.
- `src/terminal/input.ts` — `parseKeys`, `displayWidth`, all line-editing helpers, `sharedBuffer`, `sharedPasteState`, `KeyCallbacks` interface. O(1) BMP-append fast path. Plain Delete key forward-deletes; `Ctrl+Delete` deletes word forward.
- `src/terminal/renderer.ts` — ANSI color helpers, `printBlock`, `println`, `now()`, `truncateOutput`, and all block renderers. Both tool render functions display the last 6 chars of `id` as a dim bracketed suffix. Status hint includes `/compact`.
- `src/terminal/app.ts` — `runApp`, `shutdown`, `setupRawInput`, `printPrompt`, and the full agent-event loop. Calls `initLogger()` then `clearContextStore()` as its first two statements (log rotation, then fresh session context). Shutdown drains `foldCurrentSessionIntoWorldState()`. Bracketed paste mode enabled at startup. Prints diagnostic warnings at startup and after each turn footer.
- `src/turn-footer.ts` — `formatTurnFooter(turn, session, provider, model)` returns `{ turnLine, sessionLine }`; Anthropic format: `new: <non-cached, 1×>  write: <cache-write, 1.25×>  read: <cache-read, 0.1×>  out: <output>  cost: $X  saved: $X`; OpenAI shows `new:` and `out:` only with `cost: