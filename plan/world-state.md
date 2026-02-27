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

### Branch State
**`dev` is ahead of `main`**; Steps 3a–3d are complete and stable in `dev`. Merging `dev → main` is the correct next action before proceeding with Steps 3e or 4. Run `just gate` first, then merge.

### Context Management
- **Zone 1** — `plan/world-state.md`: LLM-compacted summary of all prior sessions. Loaded at session start into system prompt. Updated by `foldCurrentSessionIntoWorldState()` on clean shutdown. Lives under source control.
- **Zone 2** — turn summaries: **REMOVED** (manifest Step 2 complete).
- **Zone 3** — current turn: always verbatim, never compacted.
- Hard message cap: 100 messages. Token budget: 100k.

History grows verbatim. `buildApiMessages()` produces an ephemeral trimmed view for each API call without mutating `llmMessageLog` — the cache prefix is never invalidated by truncation. `/compact` (Step 3b) is the operator-triggered fix for sessions that grow too long.

No raw session persistence. No "resume session?" prompt. The world file is the only cross-session artifact. Each session also writes `sessions/context.jsonl` (append-only JSONL of every `MessageParam`) and `sessions/events.jsonl` (append-only JSONL of every `SessionEvent`) as persistent records. Both files are **rotated** on startup: renamed to `.prev` files before the fresh session starts. The `/compact` rewrite truncates `context.jsonl` in-place (no rotation).

### Rotated File Naming Convention
`rotateFile()` in `src/context-store.ts` inserts `.prev` **before** the last extension, not after:
- `context.jsonl` → `context.prev.jsonl`
- `events.jsonl` → `events.prev.jsonl`
- Files with no extension get `.prev` appended

The exported helper `prevPath(filePath)` encapsulates this logic. The logger follows the same convention: `omega.prev.log`.

### Manifest Refactor Status
`manifest.md` describes a major redesign. Current progress:
- **Step 1** (DONE): System prompt decoupled from Omega's own repo. Project-agnostic prompt reads `README.md` at startup.
- **Step 2** (DONE): Abandoned `compactAfterTurn()`. History grows verbatim.
- **Step 3** (IN PROGRESS): Replace `MessageParam[]` history with an event-list data structure.
  - **3a** (DONE): `src/context-store.ts` — appends each `MessageParam` to `sessions/context.jsonl`. `null` path is a no-op; mock-provider `Agent` defaults `contextFile` to `null`.
  - **3b** (DONE): `/compact` slash command — operator-triggered mid-session compaction. `compactHistory()` in `src/compaction.ts` summarises history head via LLM, keeps last `KEEP_RECENT_TURNS` (10) message-pairs verbatim. Handler in `agent.ts` replaces `this.llmMessageLog` in-place and rewrites `sessions/context.jsonl`.
  - **3c** (DONE): `SessionEvent` type + dual-write to `sessions/events.jsonl`. 12-variant discriminated union; all events carry ISO `ts`. `logEvent()` private helper in `agent.ts` (fire-and-forget, null-safe). `eventsFile` field with mock-provider heuristic. Wired at every significant site. `clearSessionEvents()` called at startup (rotates to `.prev`).
  - **3d** (DONE): Non-destructive context truncation. `truncateHistory()` renamed to `buildApiMessages()` — purely ephemeral; produces a trimmed view for each API call without ever mutating `llmMessageLog`. `Agent.history` renamed to `Agent.llmMessageLog`; `getHistory()` → `getLlmMessageLog()`. Prompt-too-long retries reduce `apiBudget` (halved per attempt); the next iteration's `buildApiMessages()` picks up the tighter budget automatically. Cache prefix is never invalidated by truncation.
- **Step 3e** (TODO — discuss before acting): Review event completeness and UI reflection alignment. Currently not persisted: `status` (intentional — ephemeral UI noise), per-API-call `metrics` (aggregate captured in `turn_end`), `tool_result_message`. Decide guiding principle before acting.
- **Step 4** (TODO — can proceed independently of 3e): Retire pino. Pino still provides uniquely: ~6 infra-only events (`oauth_reauthed`, `oauth_token_expired`, `context_truncated`, `api_retry`, `diagnostic_written`, `world_state_updated`). Migration: add those as `SessionEvent` variants, then delete `src/logger.ts` and all call sites. `omega.log`/`omega.prev.log` removed from `.gitignore`.

### Planning Files
- `plan/world-state.md` — Zone 1 world state; auto-maintained; under source control.
- `plan/future.md` — discrete actionable backlog items; manually maintained.
- `manifest.md` — high-level design manifest for ongoing refactoring. Strategic direction.
- `README.md` — project orientation for any agent (including Omega itself). References all planning files.

### Slash Commands
| Command | Effect |
|---------|--------|
| `/sonnet` | Anthropic `claude-sonnet-4-6` (default) |
| `/opus` | Anthropic `claude-opus-4-6` |
| `/codex` | OpenAI `gpt-5.2-codex` |
| `/compact` | Collapse history head into LLM summary, keep last 10 turns verbatim |
| `/help` | Compact command list with provider-sensitive footer legend |

Old commands `/gpt`, `/openai`, `/anthropic` are removed and yield "Unknown command". Startup hint shows `/sonnet /opus /codex /compact /help`.

### Prompt Caching Architecture
Three cache breakpoints: system prompt, last tool definition, last history message. Within a turn's agentic loop, each successive API call gets massive cache hits on all previously-sent messages. Cross-turn, the entire accumulated history is sent verbatim, so cache hits grow with session length.

**Cache/truncation interaction (resolved):** `buildApiMessages()` produces an ephemeral API-call view from `llmMessageLog` without mutating it. The cache-control breakpoint on the last message always refers to the same stored message, so the prompt cache prefix is never invalidated by a truncation event.

### Test Isolation — Never Pollute Production Files
Tests must **never** write to `sessions/`, `diagnosis/`, `omega.log`, or any other production file. The mechanism: `Agent` constructor applies a mock-provider heuristic — when a mock `streamProvider` is injected and no explicit path is given, `worldStatePath`, `diagDir`, `contextFile`, and `eventsFile` all default to `null` (disabled). All file-writing functions treat `null` as a no-op. e2e tests use `sessions-test/` not `sessions/`. If a new production side-effect file is added, follow the same pattern.

### Event Taxonomy (coordinate-system model)
Events are named as messages between three parties: **agent**, **user**, **llm**. Direction is explicit in the name.

| Event name | Meaning |
|---|---|
| `agent_to_llm` | LLM call in main agentic loop |
| `agent_to_llm_compact_session` | LLM call to fold session into world-state |
| `llm_to_agent` | Response to main loop call |
| `user_to_agent` | User submits a message |
| `agent_to_agent_tool_call` | Tool invocation |
| `agent_to_agent_tool_result` | Tool result |
| `agent_to_agent_compact_session` | Session fold (internal) |

**One-sided only** (UI-only or infra-only): `text`, `status`, `interrupted`, `metrics`, `turn_end`, `api_call_start`; `startup`, `oauth_*`, `context_truncated`, `session_compacted`, `api_retry`, `diagnostic_written`.

### SessionEvent Variants (sessions/events.jsonl)
`session_start`, `user_message`, `api_call_start`, `llm_response`, `tool_call`, `tool_result`, `turn_end`, `api_error`, `error`, `interrupted`, `world_state_saved`, `session_compacted`. All carry ISO `ts` timestamp.

### Key Files
- `src/agent.ts` — Agent class, `sendMessage` async generator, `StreamProvider` type, `buildApiMessages()` (ephemeral API-call view from `llmMessageLog`; never mutates), `PRICING` table; `llmMessageLog` grows **verbatim**; `foldCurrentSessionIntoWorldState()` async generator; `getActiveFoldProvider()`; builds `systemBlocks` and `cachedTools` with `cache_control`; `estimateCostWithCache()`; `estimateCacheSavings()`; `private activeModel`; `addCacheControlToLastMessage()` helper; parallel tool execution; `logEvent()` private helper (fire-and-forget, null-safe) wired at every significant site; `eventsFile` field with mock-provider heuristic; `/compact` handler passes `{ rotate: false }` to `clearContextStore`; on fatal errors calls `flushLog()` then `writeDiagnostic()`.
- `src/session-event.ts` — `SessionEvent` discriminated union (12 variants). `appendSessionEvent(event, filePath?)` and `clearSessionEvents(filePath?)` — both use `null`-is-no-op pattern. `clearSessionEvents()` rotates via `rotateFile()`. `DEFAULT_EVENTS_FILE = "sessions/events.jsonl"`.
- `src/context-store.ts` — `appendContextMessage()`, `clearContextStore()`, `rotateFile()`, `prevPath()`. `clearContextStore()` rotates by default; accepts `{ rotate: false }` for in-place truncation (used by `/compact`). `rotateFile()` renames file to `.prev` variant (via `prevPath()`) then creates fresh empty file — shared by both context and events stores.
- `src/compaction.ts` — `compactWorldState()` (LLM-based world-state fold on shutdown) and `compactHistory()` (Step 3b — mid-session history compaction for `/compact`). `KEEP_RECENT_TURNS` = 10 exported.
- `src/world-state.ts` — `readWorldState()`, `writeWorldState()`, `projectWorldStatePath()` → `<cwd>/plan/world-state.md`
- `src/logger.ts` — pino-backed structured logger. Log rotation (`omega.log → omega.prev.log`). **To be retired in Step 4.**
- `src/diagnosis.ts` — `writeDiagnostic(data, diagDir?)` writes a JSON snapshot to `diagnosis/<ISO-timestamp>.json`; `null` disables; `checkDiagnostics()` returns existing snapshot paths sorted oldest-first.
- `src/ui-raw.ts` — **thin re-export shim** (26 lines). CLI entry point.
- `src/terminal/input.ts` — `parseKeys`, `displayWidth`, all line-editing helpers.
- `src/terminal/renderer.ts` — ANSI color helpers, `printBlock`, `println`, `now()`, `truncateOutput`, and all block renderers.
- `src/terminal/app.ts` — `runApp`, `shutdown`, `setupRawInput`. Calls `initLogger()` then `clearContextStore()` then `clearSessionEvents()` as first three statements (log rotation, then rotate both session files). Shutdown drains `foldCurrentSessionIntoWorldState()`.
- `src/tools.ts` — All tool implementations. `executeTool()` applies `MAX_TOOL_OUTPUT_CHARS = 100_000` cap to all tool results before they enter history; oversized output is truncated with an actionable note.
- `src/turn-footer.ts` — `formatTurnFooter(turn, session, provider, model)` returns `{ turnLine, sessionLine }`.

### Context Poison Prevention
Two bugs fixed (2026-02-25), both now subsumed by the Step 3d architecture:

1. **`buildApiMessages` short-but-fat handling** (`src/agent.ts`): When all messages fall within the "always keep" tail (≤ `KEEP_RECENT_TURNS*2` messages), instead of returning unchanged, the function drops from the oldest end of the tail, keeping at minimum the last message. This prevented all 5 retries sending the same oversized payload.

2. **Tool output cap** (`src/tools.ts`): `executeTool()` caps all tool results at `MAX_TOOL_OUTPUT_CHARS = 100_000` before they enter history. Oversized output is truncated with a note: `[truncated: tool output was N chars; showing first 100000. Use offset/limit or a more specific query to see other parts.]`

### Non-Destructive Truncation (Step 3d — DONE, commit 997d7f7)
- `buildApiMessages(history, budget)` — exported from `agent.ts`. Produces an ephemeral trimmed view; the source array is never mutated.
- `Agent.llmMessageLog` — the canonical, append-only record of all LLM messages. Never shortened by truncation.
- `Agent.getLlmMessageLog()` — public read-only accessor.
- Agentic loop: `apiView = buildApiMessages(llmMessageLog, apiBudget)` at top of each iteration. `apiBudget` starts at `config.maxContextTokens`; halved on each prompt-too-long retry. `llmMessageLog` is never touched.
- Anthropic retry sub-loop: `attemptApiView` / `attemptCachedMessages` recomputed per attempt to pick up the tighter budget — also fixes a pre-existing stale-`cachedMessages` bug.
- Diagnostic snapshots include both `requestMessages` (the view sent) and `history` (the full `llmMessageLog`).

### Current Test Count
471 tests across 27 files. All pass.

### Recent Session Outcomes
Completed **Step 3d** (non-destructive context truncation) and updated all planning documents (`plan/world-state.md`, `plan/future.md`, `manifest.md`) to reflect the completed step. All changes pushed to `develop`; `dev` is now ahead of `main` by Steps 3a–3d. The immediate next action is merging `develop → main` (run `just gate` first).