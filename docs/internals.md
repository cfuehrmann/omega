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

- **System prompt** — `.omega/system-prompt-append.md` is appended at session
  start. Contains agent-specific constraints and operational policies. Updated
  manually. Lives under source control.
- **History** — `compactedContextHistory` grows verbatim across turns. Sent
  in full to each LLM provider call — no mid-turn trimming. `/compact`
  summarises the head and keeps the last 10 turns verbatim. Auto-compact fires
  when `input_tokens` exceeds `config.autoCompactThreshold = 750_000`.
- **Current turn** — always verbatim, never compacted mid-turn.
- Hard message cap: 100 messages. Token budget: 100k.

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
both streamed from `agent.ts` and persisted to `events.jsonl`.

### OmegaEvent Variants

`session_start`, `session_end`, `user_message`, `llm_call`, `llm_response`,
`tool_call`, `tool_result`, `turn_end`, `llm_error`, `agent_error`,
`turn_interrupted`, `compact_user_start`, `compact_user_done`,
`compact_user_error`, `compact_auto_start`, `compact_auto_done`,
`compact_auto_error`, `oauth_refreshed`, `oauth_token_expired`, `llm_retry`,
`model_changed`, `auth_mode_changed`. All carry ISO `ts` timestamp. No `status` variant.

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
`model_changed`, `auth_mode_changed`, `oauth_url`, `oauth_cancelled`,
`oauth_token_expired`, `oauth_refreshed`, `llm_retry`,
`compact_user_start`, `compact_user_done`, `compact_user_error`,
`compact_auto_start`, `compact_auto_done`, `compact_auto_error`, `llm_call`,
`turn_end`, `llm_error`, `agent_error`, `transport_error`
(WebSocket/server transport errors, persisted best-effort), `turn_interrupted`,
`session_start`, `session_end`.

## context.jsonl Record Shape (ContextRecord)

- `hash` — 8-char lowercase hex SHA-256 of `JSON({ ts, role, content })`.
- `ts` — ISO 8601 timestamp.
- `role` — `"user"` or `"assistant"`.
- `content` — string or content-block array.

## UI Conventions

- **Mobile-first:** every UI element must be usable on a small touch screen.
  Avoid hover-only interactions; prefer tap targets ≥ 44 px; no fixed-width
  layouts that overflow on narrow viewports.
- **Inline legends** use `<details>`/`<summary>` (zero JS, tap-to-expand) — not
  the full-screen modal. The modal is reserved for inspecting large variable-
  length content (tool call bodies, LLM request/response payloads).
- **Modals** are triggered by the `⤢` expand button on feed blocks.

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

- `src/agent.ts` — Agent class, `sendMessage` async generator, `CreateMessageStream`
  type, `PRICING` table; `compactedContextHistory` / `compactedContextHashes[]`
  are the mutable in-memory context window and parallel hash array;
  `appendToHistory()` fire-and-forgets file I/O; `logEvent()` fire-and-forget
  event logger; `emitSessionEnd()` awaits flush; `/compact` replaces context
  view and hashes in memory only.
- `src/events.ts` — `OmegaEvent` discriminated union; `StreamSignal`;
  `exhaustiveCheck(x: never)` guard.
- `src/event-store.ts` — `appendEvent(event, filePath?)` — null-is-no-op.
  UI-only fields stripped by `toPersistedEvent()`.
- `src/context-store.ts` — `ContextRecord`; `buildContextRecord(msg)`;
  `appendContextMessage()` returns hash.
- `src/session-dir.ts` — `makeSessionDir()`; `makeSessionDirName()`;
  `findPreviousEventsFile()`; `SESSIONS_ROOT`; `TEST_SESSIONS_ROOT`.
- `src/compaction.ts` — `compactHistory()`. `KEEP_RECENT_TURNS = 10`.
- `src/system-prompt/` — modular system prompt: `identity.ts` (OAuth prefix),
  `core.ts` (main instructions), `append.ts` (`readSystemPromptAppend()`,
  `writeSystemPromptAppend()`, `systemPromptAppendPath()`,
  ), `index.ts` (`buildSystemPrompt()` assembler).
- `src/tools.ts` — All tool implementations; `MAX_TOOL_OUTPUT_CHARS = 100_000`
  cap.
- `src/web/client/store.ts` — `WsEvent` discriminated union, `dispatch()`,
  `AppState`.
- `src/web/client/App.tsx` — SolidJS UI. Exhaustive switch on `WsEvent`;
  `truncateOutput` for display.
- `src/web/server.ts` — `runWebApp()`, `closeOpenTurn()`, `shouldLogEvent()`.
- `src/test-guard.ts` — `assertNotProductionPath()`. Throws on production path
  writes in test mode.
- `src/test-setup.ts` — Bun preload; sets `OMEGA_TEST=1`.
- `src/test-utils.ts` — `makeTestAgent()` factory; writes to `.omega/test-sessions/`.
