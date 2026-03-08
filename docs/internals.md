# Omega Internals Reference

Reference material for working on Omega's core infrastructure. Read on demand
when working in the event, session, or WebSocket layers.

## Session Directory Model

Each startup calls `makeSessionDir()` in `src/session-dir.ts`, which creates
`.omega/sessions/YYYY-MM-DDTHH-MM-SS-mmm-<hex8>/` with eager empty
`context.jsonl` and `events.jsonl`. `SESSIONS_ROOT = ".omega/sessions"`.
`TEST_SESSIONS_ROOT = ".omega/test-sessions"`. `findPreviousEventsFile()` finds
the most recent prior session directory (for startup crash detection). No
rotation machinery.

## Context Management

- **Zone 1** — `.omega/system-prompt-append.md`: appended to the system prompt
  at session start. Contains world state summary. Updated manually. Lives under
  source control.
- **Zone 3** — current turn: always verbatim, never compacted.
- Hard message cap: 100 messages. Token budget: 100k.

History grows verbatim. The full `compactedContextHistory` is sent verbatim to
each LLM provider call — no mid-turn trimming. `/compact` is the user-triggered
fix for sessions that grow too long.

## Prompt Caching Architecture

Three cache breakpoints: system prompt, last tool definition, last history
message. Within a turn's agentic loop, each successive LLM provider call gets
massive cache hits on all previously-sent messages. Cross-turn, the entire
accumulated history is sent verbatim, so cache hits grow with session length.

## Context Overflow Policy

Context overflow (400 "prompt too long" or 429 "extra usage required") is
non-retryable: emits `llm_error` then `agent_error` with actionable message. No
mid-turn trimming of any kind — agent sends `compactedContextHistory` verbatim
on every call.

## Tool Output Cap

`executeTool()` caps all tool results at `MAX_TOOL_OUTPUT_CHARS = 100_000`
before they enter history.

## Event Taxonomy

`OmegaEvent` (in `src/events.ts`) is the single unified type for all events —
both streamed from `agent.ts` and persisted to `events.jsonl`. `AgentEvent` in
`agent.ts` is a backward-compat alias.

### OmegaEvent Variants

`session_start`, `session_end`, `user_message`, `llm_call`, `llm_response`,
`tool_call`, `tool_result`, `turn_end`, `llm_error`, `agent_error`,
`turn_interrupted`, `compact_user_start`, `compact_user_done`,
`compact_user_error`, `compact_auto_start`, `compact_auto_done`,
`compact_auto_error`, `oauth_refreshed`, `oauth_token_expired`, `llm_retry`,
`model_changed`. All carry ISO `ts` timestamp. No `status` variant.

Streaming text fragments are a `StreamSignal` (`{ type: "text", text: string }`)
— explicitly outside the persistence boundary by design.

`session_start` carries `systemPrompt: string` and `authMode`.
`session_end` carries `outcome: "clean" | "error"` and optional `reason`.
Absence means the session crashed.
`llm_call` carries `contextHashes: string[]` (ordered 8-char SHA-256 hashes of
every sent message) and `cacheBreakpointIndex: number | null`.
`tool_call` carries `contextHash: string` — FK to the assistant `context.jsonl`
record.
`tool_result` carries `contextHash: string` — FK to the user `context.jsonl`
record.
`llm_response` carries metadata only: `stopReason`, `model`, `provider`, `url`,
`usage`, `contextHash` (FK). No `content` field.

### WsEvent Variants (WebSocket protocol)

`connected`, `disconnected`, `history`, `auth`, `turn_ready`, `reset_done`,
`user_message`, `text`, `tool_call`, `tool_result`, `llm_response`,
`model_changed`, `oauth_token_expired`, `oauth_refreshed`, `compact_user_start`,
`compact_user_done`, `compact_user_error`, `compact_auto_start`,
`compact_auto_done`, `compact_auto_error`, `llm_call`, `world_state_saved`,
`turn_end`, `llm_error`, `agent_error`, `error` (server-own protocol errors
only), `turn_interrupted`, `session_start`, `session_end`.

## context.jsonl Record Shape (ContextRecord)

- `hash` — 8-char lowercase hex SHA-256 of `JSON({ ts, role, content })`.
- `ts` — ISO 8601 timestamp.
- `role` — `"user"` or `"assistant"`.
- `content` — string or content-block array.

## UI Display Policy

Both UIs truncate `tool_result` output and `tool_call` input for display: **5
lines or 500 chars**, whichever fires first. Full content is always in
`context.jsonl` via FK hash. `llm_response` blocks show `stop_reason` and
`usage` only.

## Test Isolation — Never Pollute Production Files

Tests must **never** write to `.omega/sessions/` or any other production file.

**Primary mechanism:** `makeTestAgent()` in `src/test-utils.ts` calls
`makeSessionDir(now, TEST_SESSIONS_ROOT)` to write real session files to
`.omega/test-sessions/`. Isolation is by path, not by deletion. Each call gets a
unique dir; `dispose()` is a no-op — sessions persist as inspectable artifacts.
Returns `{ agent, sessionDir, contextFile, eventsFile, dispose }`.

**Belt-and-suspenders layers:**
- `bunfig.toml` preloads `src/test-setup.ts` → sets `OMEGA_TEST=1` before any
  test runs.
- `assertNotProductionPath()` in `src/test-guard.ts` throws when `OMEGA_TEST=1`
  and path is under `.omega/sessions/`. `.omega/test-sessions/` is explicitly
  allowed.
- `Agent` constructor coerces `undefined` file paths to `null` when `OMEGA_TEST=1`.
- `scripts/pre-commit` greps for bare `new Agent()` in `*.test.ts` files.

All write functions treat `null` path as a no-op. If a new production
side-effect file is added, wire `assertNotProductionPath()` into its write
function.

## Key Files

- `src/agent.ts` — Agent class, `sendMessage` async generator, `StreamProvider`
  type, `PRICING` table; `compactedContextHistory` / `compactedContextHashes[]`
  are the mutable in-memory context window and parallel hash array;
  `appendToHistory()` fire-and-forgets file I/O; `buildSystemPrompt()` builds
  the system prompt; `logEvent()` fire-and-forget event logger;
  `emitSessionEnd()` awaits flush; `/compact` replaces context view and hashes
  in memory only.
- `src/events.ts` — `OmegaEvent` discriminated union; `StreamSignal`;
  `exhaustiveCheck(x: never)` guard.
- `src/event-store.ts` — `appendEvent(event, filePath?)` — null-is-no-op.
  UI-only fields stripped by `toPersistedEvent()`.
- `src/context-store.ts` — `ContextRecord`; `buildContextRecord(msg)`;
  `appendContextMessage()` returns hash.
- `src/session-dir.ts` — `makeSessionDir()`; `makeSessionDirName()`;
  `findPreviousEventsFile()`; `SESSIONS_ROOT`; `TEST_SESSIONS_ROOT`.
- `src/compaction.ts` — `compactWorldState()` and `compactHistory()`.
  `KEEP_RECENT_TURNS = 10`.
- `src/system-prompt/` — modular system prompt: `identity.ts`, `core.ts` (main
  instructions), `append.ts` (`readSystemPromptAppend()`,
  `writeSystemPromptAppend()`, `systemPromptAppendPath()`,
  `formatAppendSection()`), `index.ts` (`buildSystemPrompt()` assembler).
- `src/ui-raw.ts` — thin re-export shim. CLI entry point.
- `src/terminal/input.ts` — `parseKeys`, `displayWidth`. Minimal append-only
  line editor.
- `src/terminal/renderer.ts` — ANSI block renderers; `truncateOutput` (5 lines /
  500 chars).
- `src/terminal/app.ts` — `runApp`, `shutdown`, `setupRawInput`. Exhaustive
  switch on `OmegaEvent | StreamSignal`.
- `src/tools.ts` — All tool implementations; `MAX_TOOL_OUTPUT_CHARS = 100_000`
  cap.
- `src/turn-footer.ts` — `formatTurnFooter(turn, session, provider, model)`.
- `src/web/client/store.ts` — `WsEvent` discriminated union, `dispatch()`,
  `AppState`.
- `src/web/client/App.tsx` — SolidJS UI. Exhaustive switch on `WsEvent`;
  `truncateOutput` for display.
- `src/web/server.ts` — `runWebApp()`, `closeOpenTurn()`, `shouldLogEvent()`.
- `src/test-guard.ts` — `assertNotProductionPath()`. Throws on production path
  writes in test mode.
- `src/test-setup.ts` — Bun preload; sets `OMEGA_TEST=1`.
- `src/test-utils.ts` — `makeTestAgent()` factory; writes to `.omega/test-sessions/`.
