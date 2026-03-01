## Omega — State of the World

### Purpose
Omega is a general-purpose coding agent that runs in a terminal. It can be pointed at any project directory and will read the project's `README.md` for orientation. When pointed at its own repo, it can develop itself. The operator interacts via raw terminal input with dictation support.

### Stack
TypeScript + Bun. Raw terminal I/O via `src/terminal/` modules (`input.ts`, `renderer.ts`, `app.ts`); `src/ui-raw.ts` is now a thin shim that re-exports from there and is the entry point. No UI library. Agent core (`src/agent.ts`) has no UI imports. Config is code (`src/config.ts`). `StreamProvider` interface allows mock injection in tests — real API never called in tests.

### Auth
Claude Max via OAuth PKCE through `claude.ai` (sk-ant-oat-… tokens). System prompt must be prefixed with Claude Code identity string for OAuth. Falls back to `ANTHROPIC_API_KEY`. OpenAI Codex fallback via `OPENAI_API_KEY` for `/codex` command and rate-limit fallback.

### Git Push Cadence
Push to origin at least every 3 commits (documented in `README.md`; no longer hardcoded in system prompt).

### Gate Before Every Commit
**Run `just gate` before every commit.** Gate = unit tests + type check + e2e tests. Never commit without a green gate. `just gate` is operator-only (not run automatically), but Omega must run it explicitly before committing.

### Workspace Layout
`~/omega/` is a git workspace with three subdirectories: `main` (stable agent codebase), `dev` (development version), and `plan`. To run the stable agent on the dev project: `cd ~/omega/dev && bun run ~/omega/main/src/ui-raw.ts`. A shell alias `alias omega='bun run ~/omega/main/src/ui-raw.ts'` is a suggested convenience (not yet confirmed added to shell config). `ui-raw.ts` is the CLI entry point; the web server entry point is `src/web/server.ts`.

### Branch State
`develop` is the active branch. All manifest steps through EU-4 are complete. `main` was previously synced at Steps 3a–3d; it needs merges to pick up Step 4, Steps 3e-i/ii/iii, EU-1 through EU-4, and recent UI cleanups.

### Context Management
- **Zone 1** — `plan/world-state.md`: LLM-compacted summary of all prior sessions. Loaded at session start into system prompt. Updated manually at session end. Lives under source control.
- **Zone 2** — turn summaries: **REMOVED** (manifest Step 2 complete).
- **Zone 3** — current turn: always verbatim, never compacted.
- Hard message cap: 100 messages. Token budget: 100k.

History grows verbatim. The full `compactedContextHistory` is sent verbatim to each API call — no mid-turn trimming. `/compact` (Step 3b) is the operator-triggered fix for sessions that grow too long.

No raw session persistence. No "resume session?" prompt. The world file is the only cross-session artifact. Each session writes to its own timestamped directory `.omega/sessions/YYYY-MM-DDTHH-MM-SS/` containing `context.jsonl` (append-only JSONL of every `MessageParam` as a `ContextRecord`) and `events.jsonl` (append-only JSONL of every `OmegaEvent`). Old session directories accumulate and are never touched. There is no file rotation and no `.prev` files. `/compact` only replaces the in-memory `llmContextView`; `context.jsonl` is strictly append-only and is never rewritten.

### Session Directory Model
Each startup calls `makeSessionDir()` in `src/session-dir.ts`, which creates `.omega/sessions/YYYY-MM-DDTHH-MM-SS-mmm-<hex8>/` (millisecond precision + 8-char random hex suffix for global uniqueness; colons/dots replaced with hyphens for filesystem safety) with eager empty `context.jsonl` and `events.jsonl` (created immediately by `makeSessionDir`, not lazily on first write). `SESSIONS_ROOT = ".omega/sessions"`. `TEST_SESSIONS_ROOT = ".omega/test-sessions"`. `findPreviousEventsFile()` finds the most recent prior session directory (for startup crash detection). There is no rotation machinery — no `rotateFile()`, no `prevPath()`, no `.prev` files.

### Manifest Refactor Status
`manifest.md` describes a major redesign. All steps through schema pre-lock are done.

**Done:**
- Steps 1–4: system prompt decoupling, abandon auto-compaction, event-list history, retire pino.
- Append-only `context.jsonl` + `events.jsonl`; `/compact` command; FK/PK contract (`hash`+`ts` on every record, `contextHashes[]` on `llm_call`); pre-lock field removals.
- Event system unification: `OmegaEvent` is the single type; `status` variant gone; all names match persistence; exhaustive switch guards in both UIs.
- Compaction event overhaul; BUG-1 fix; `session_end` event; diagnostics removed.
- SESSION-1/2/2b: per-session timestamped directories; sessions in `.omega/sessions/`; web server persistence parity.

**TODO (see `plan/backlog.md`):**
- **SCHEMA-1–SCHEMA-6**: property completeness, missing events, persistence audit, forward-compat policy, schema reference doc (`plan/schema.md`).
- **SCHEMA-7**: session resume (depends on schema lock).
- **INFRA-4**: decouple world-state injection from Omega's own repo.

### Planning Files
- `plan/world-state.md` — Zone 1 world state; manually maintained; under source control.
- `plan/backlog.md` — discrete actionable backlog items; manually maintained.
- `manifest.md` — high-level design manifest for ongoing refactoring. Strategic direction.
- `README.md` — project orientation for any agent (including Omega itself). References all planning files.

### Slash Commands
| Command | Effect |
|---------|--------|
| `/sonnet` | Anthropic `claude-sonnet-4-6` (default) |
| `/opus` | Anthropic `claude-opus-4-6` |
| `/codex` | OpenAI `gpt-5.2-codex` |
| `/compact` | Collapse history head into LLM summary, keep last 10 turns verbatim |

Any other `/…` input is rejected with `agent_error`. Startup hint shows `/sonnet /opus /codex /compact`.

### Prompt Caching Architecture
Three cache breakpoints: system prompt, last tool definition, last history message. Within a turn's agentic loop, each successive API call gets massive cache hits on all previously-sent messages. Cross-turn, the entire accumulated history is sent verbatim, so cache hits grow with session length.

**Cache/truncation interaction (resolved):** `buildApiMessages()` produces an ephemeral API-call view from `llmContextView` without mutating it. The cache-control breakpoint on the last message always refers to the same stored message, so the prompt cache prefix is never invalidated by a truncation event.

### Test Isolation — Never Pollute Production Files
Tests must **never** write to `.omega/sessions/` or any other production file.

**Primary mechanism:** `makeTestAgent()` in `src/test-utils.ts` calls `makeSessionDir(now, TEST_SESSIONS_ROOT)` to write real session files to `.omega/test-sessions/` (not `/tmp`, not `.omega/sessions/`). Isolation is by path, not by deletion. Each call gets a unique dir (timestamp + counter); `dispose()` is a no-op — sessions persist as inspectable artifacts. Returns `{ agent, sessionDir, contextFile, eventsFile, dispose }`. No null-path blind spots.

**Belt-and-suspenders layers (secondary):**
- **Layer a:** `bunfig.toml` preloads `src/test-setup.ts` → sets `OMEGA_TEST=1` before any test runs.
- **Layer b:** `assertNotProductionPath()` in `src/test-guard.ts` guards `.omega/sessions/` only (`.omega/test-sessions/` is explicitly allowed). Writing to `.omega/sessions/` when `OMEGA_TEST=1` throws immediately.
- **Layer c:** `Agent` constructor coerces `undefined` file paths to `null` when `OMEGA_TEST=1`.
- **Layer e:** `scripts/pre-commit` greps for bare `new Agent()` in `*.test.ts` files. Fails with an actionable message pointing at `makeTestAgent`.

All write functions treat `null` path as a no-op. e2e tests also write to `.omega/test-sessions/` via `makeSessionDir(…, TEST_SESSIONS_ROOT)`. If a new production side-effect file is added, wire `assertNotProductionPath()` into its write function.

### Event Taxonomy
`OmegaEvent` (in `src/events.ts`) is the single unified type for all events — both streamed from `agent.ts` and persisted to `events.jsonl`. `AgentEvent` in `agent.ts` is a backward-compat alias. All names are consistent across all layers.

### OmegaEvent Variants (streamed from agent.ts AND persisted to events.jsonl)
`session_start`, `session_end`, `user_message`, `llm_call`, `llm_response`, `tool_call`, `tool_result`, `turn_end`, `llm_error`, `agent_error`, `turn_interrupted`, `compact_user_start`, `compact_user_done`, `compact_user_error`, `compact_auto_start`, `compact_auto_done`, `compact_auto_error`, `oauth_refreshed`, `oauth_token_expired`, `llm_retry`, `model_changed`. All carry ISO `ts` timestamp. No `status` variant — all lifecycle signals are typed. `context_view_trimmed` deleted (commit 13c1f9e — no mid-turn trimming). `diagnostic_written` deleted (commit bfd5d0d — diagnostics removed).

Streaming text fragments are a `StreamSignal` (`{ type: "text", text: string }`) not an `OmegaEvent` — explicitly outside the persistence boundary by design.

`session_start` carries `systemPrompt: string` (full system prompt at session start) and `authMode`.

`session_end` carries `outcome: "clean" | "error"` and optional `reason`. Absence of `session_end` in the previous session's `events.jsonl` means the session crashed.

`llm_call` additionally carries `contextHashes: string[]` — the ordered 8-char SHA-256 hashes of every `ContextRecord` in the `buildApiMessages()` view sent with that call — and `cacheBreakpointIndex: number | null` (0-based index of the last message that received `cache_control: { type: "ephemeral" }`; `null` for OpenAI).

`tool_call` additionally carries `contextHash: string` — FK to the assistant `context.jsonl` record containing the `tool_use` block. `input` field removed (content is in `context.jsonl`).

`tool_result` additionally carries `contextHash: string` — FK to the user `context.jsonl` record containing the `tool_result` block(s). `outputLength` removed (derivable).

`llm_response` carries metadata only: `stopReason`, `model`, `provider`, `url`, `usage`, and `contextHash` (FK). No `content` field — content is in `context.jsonl`. `usage` includes `input_tokens`, `output_tokens`, `cache_creation_input_tokens?`, `cache_read_input_tokens?`, `service_tier?`.

### WsEvent Variants (WebSocket protocol, src/web/client/store.ts)
`connected`, `disconnected`, `history`, `auth`, `turn_ready`, `reset_done`, `user_message`, `text`, `tool_call`, `tool_result`, `llm_response`, `model_changed`, `oauth_token_expired`, `oauth_refreshed`, `compact_user_start`, `compact_user_done`, `compact_user_error`, `compact_auto_start`, `compact_auto_done`, `compact_auto_error`, `llm_call`, `world_state_saved`, `turn_end`, `llm_error`, `agent_error`, `error` (server-own protocol errors only), `turn_interrupted`, `session_start`, `session_end`. No `status` variant. `diagnostic_written` removed (commit bfd5d0d).

### context.jsonl Record Shape (ContextRecord)
Each line is a JSON object with fields:
- `hash` — 8-char lowercase hex SHA-256, computed from `JSON({ ts, role, content })`. Including `ts` prevents collisions between identical messages sent at different times.
- `ts` — ISO 8601 timestamp of when the message was appended.
- `role` — `"user"` or `"assistant"`.
- `content` — string or content-block array (same as `Anthropic.MessageParam.content`).

### UI Display Policy
Both terminal and web UIs apply presentation-only truncation to both `tool_result` output and `tool_call` input — **5 lines or 500 chars**, whichever fires first. The truncation note states both the total line count and total char count. Full content is always in `context.jsonl` via the relevant FK hash. The `result` field on `ToolResultEvent` is UI-only and is stripped before writing to `events.jsonl`.

`llm_response` blocks show `stop_reason` and `usage` only — no content section. Text was already streamed token-by-token; tool calls are shown by the subsequent `tool_call` block. Cache tokens (`cache_write`, `cache_read`) shown only when non-zero. `service_tier` shown only when non-null and not `"standard"`.

### Key Files
- `src/agent.ts` — Agent class, `sendMessage` async generator, `StreamProvider` type, `PRICING` table; `llmContextView` / `compactedContextHistory` is the mutable in-memory context window (formerly `llmMessageLog`); `llmContextHashes[]` / `compactedContextHashes[]` parallel array stores the content hash of each stored message; `appendToHistory()` awaits hash computation then fire-and-forgets the file write; `buildSystemPrompt()` builds the system prompt (OAuth prefix + world state); builds `systemBlocks` and `cachedTools` with `cache_control`; `estimateCostWithCache()`; `estimateCacheSavings()`; `private activeModel`; `addCacheControlToLastMessage()` helper; parallel tool execution; `logEvent()` private helper (fire-and-forget, null-safe) wired at every significant site; `emitSessionEnd(outcome, reason?)` public method awaits flush; `/compact` handler replaces context view and hashes in memory only — `context.jsonl` is never rewritten. Context overflow (prompt-too-long) is non-retryable: emits `llm_error` + actionable `agent_error`. No `writeDiagnostic`, no `diagDir`. No `buildSentContext`, no `apiBudget`, no `contextHashesForView`. No `status` AgentEvent — all lifecycle signals are typed. `AgentEvent` is a backward-compat alias for `OmegaEvent`.
- `src/events.ts` — `OmegaEvent` discriminated union (all variants); `StreamSignal` type (`{ type: "text" }`); `exhaustiveCheck(x: never)` guard exported for exhaustive switches in UIs.
- `src/event-store.ts` — `appendEvent(event, filePath?)` — uses `null`-is-no-op pattern. UI-only fields stripped by `toPersistedEvent()` before disk write.
- `src/context-store.ts` — `ContextRecord` interface; `buildContextRecord(msg)` (computes hash without writing); `appendContextMessage()` (writes record, returns hash). `sha256hex8()` is internal (not exported). No rotation machinery.
- `src/session-dir.ts` — `makeSessionDir()` creates `.omega/sessions/YYYY-MM-DDTHH-MM-SS-mmm-<hex8>/` and eagerly writes empty `context.jsonl` + `events.jsonl`; `makeSessionDirName()`; `findPreviousEventsFile()` (finds most recent prior session dir for crash detection); `SESSIONS_ROOT = ".omega/sessions"`; `TEST_SESSIONS_ROOT = ".omega/test-sessions"`.
- `src/compaction.ts` — `compactWorldState()` (LLM-based world-state fold) and `compactHistory()` (Step 3b — mid-session history compaction for `/compact`). `KEEP_RECENT_TURNS` = 10 exported. Returns `{ history, syntheticMessage, tailStartIndex, originalCount, newCount }` — caller uses `tailStartIndex` to reuse existing hashes rather than re-hashing tail messages.
- `src/world-state.ts` — `readWorldState()`, `writeWorldState()`, `projectWorldStatePath()` → `<cwd>/plan/world-state.md`
- `src/ui-raw.ts` — **thin re-export shim** (26 lines). CLI entry point.
- `src/terminal/input.ts` — `parseKeys`, `displayWidth`. Minimal append-only line editor: printable chars append at end, backspace deletes last char, Enter submits, Esc clears buffer (or aborts turn if empty), Ctrl+C exits. No cursor movement, no word-jump, no forward-delete. Bracketed paste accumulates and echoes on close marker.
- `src/terminal/renderer.ts` — ANSI color helpers, `printBlock`, `println`, `now()`, `truncateOutput` (dual-limit: 5 lines / 500 chars, whichever first), and all block renderers. `renderToolStart` truncates input JSON via `truncateOutput`. `renderApiResponse` shows `stop_reason` + `usage` only (no content). Cache/service_tier lines are conditional on non-zero/non-standard values.
- `src/terminal/app.ts` — `runApp`, `shutdown`, `setupRawInput`. Calls `makeSessionDir()` at startup (creates fresh session dir). Reads previous session's `events.jsonl` via `findPreviousEventsFile()` to detect prior session errors. `shutdown()` awaits `agent.emitSessionEnd("clean")` before `process.exit()`. Exhaustive switch on `OmegaEvent | StreamSignal`; `default` calls `exhaustiveCheck`.
- `src/tools.ts` — All tool implementations. `executeTool()` applies `MAX_TOOL_OUTPUT_CHARS = 100_000` cap to all tool results before they enter history; oversized output is truncated with an actionable note.
- `src/turn-footer.ts` — `formatTurnFooter(turn, session, provider, model)` returns `{ turnLine, sessionLine }`.
- `src/web/client/store.ts` — `WsEvent` discriminated union, `dispatch()`, reactive `AppState`. `turn_interrupted` closes an open turn; server-own protocol errors use `{ type: "error" }`. No `status` variant. `WsEvent` and `Turn` exported.
- `src/web/client/App.tsx` — SolidJS UI renderer. `EventBlock` exhaustive switch on `WsEvent` type; `default` calls `exhaustiveCheck`. `truncateOutput` (5 lines / 500 chars, same as terminal) applied to both `tool_call` input and `tool_result` output display. Textarea disabled when `!state.connected || state.streaming`. Exposes `__omegaDispatch` on `window` for e2e tests.
- `src/web/server.ts` — `runWebApp()`, `closeOpenTurn()`, `shouldLogEvent()`. History replay reads `events.jsonl` from the Agent's session dir (no separate store). Graceful shutdown mirrors terminal: `emitSessionEnd("clean")` then exit. `closeOpenTurn` detects open turns on crash and appends `{ type: "turn_interrupted" }`.
- `src/context-hash.test.ts` — integration tests for the FK/PK contract: record shape, hash uniqueness, `contextHashes` cross-referencing, tool-loop growth; `[SCHEMA]` tests asserting field removals.
- `src/compact-command.test.ts` — 27 tests covering the `/compact` slash command: all three event variants (`compact_user_start`, `compact_user_done`, `compact_user_error`), state mutations, error path, and post-compact continuity.
- `src/compact-auto.test.ts` — 34 tests covering auto-compact: threshold constant (`AUTO_COMPACT_THRESHOLD = 100_000` tokens), fires above/silent below threshold (using `setAboveThreshold()`/`setBelowThreshold()` helpers), error path (fallback continues), 9 BUG-1 scenarios (max_tokens poison → synthetic tool_result + agent_error, well-formed context, next turn succeeds), combined integration scenario.

### Context Overflow Policy (commit 13c1f9e)
Context overflow (API returns 400 "prompt is too long" or 429 "extra usage required for long context") is treated as a **non-retryable terminal error**:
- Emits `llm_error` (carries HTTP status)
- Emits `agent_error` with actionable message: *"Context too large to send. Use /compact to summarise history, or start a fresh focused turn."*
- Does **not** retry, does **not** halve a budget — the first overflow response is the signal

`buildSentContext()`, `apiBudget`, `contextHashesForView()`, and `context_view_trimmed` are all deleted. The agent sends `compactedContextHistory` verbatim on every call. No mid-turn trimming of any kind.

### Tool Output Cap (still active)
`executeTool()` caps all tool results at `MAX_TOOL_OUTPUT_CHARS = 100_000` before they enter history. Oversized output is truncated with a note: `[truncated: tool output was N chars; showing first 100000. Use offset/limit or a more specific query to see other parts.]`

### FK/PK Contract (Step 3e-iii — DONE)
Each `MessageParam` written to `context.jsonl` carries a `hash` field (SHA-256 of `JSON({ ts, role, content })`, truncated to 8 hex chars) and a `ts` field. Each `llm_call` event carries `contextHashes: string[]` — the ordered hashes of every message in `compactedContextHistory` sent. Key design decisions:
- Hash computed from `{ ts, role, content }` (not including `hash` itself); `ts` prevents collisions between identical messages.
- `contextHashes` equals `compactedContextHashes` (the full history — no trimming, no absent messages).
- SHA-256 via Web Crypto (`crypto.subtle.digest`) benchmarks at ~11 µs per hash — negligible vs. API/tool latency.
- `appendContextMessage()` returns the hash; `buildContextRecord()` computes hash without writing.
- Agent maintains `compactedContextHashes[]` parallel to `compactedContextHistory`; `appendToHistory()` awaits hash then fire-and-forgets file I/O.
- On `/compact`: tail hashes are sliced from existing `compactedContextHashes` (no re-hash, no re-write); only the new synthetic message is appended to `context.jsonl` via `appendContextMessage()`.

### Key Files (additional)
- `src/test-guard.ts` — `assertNotProductionPath(filePath, fnName)`. Throws when `OMEGA_TEST=1` and `filePath` is under `.omega/sessions/` or `diagnosis/`. `.omega/test-sessions/` is explicitly allowed. No-op in production. Wired into all production write functions.
- `src/test-setup.ts` — Bun test preload (wired via `bunfig.toml`). Sets `OMEGA_TEST=1` before any test file loads.
- `src/test-guard.test.ts` — tests covering throw/no-throw behaviour of `assertNotProductionPath`.
- `src/test-utils.ts` — `makeTestAgent(streamProvider?, openAiCaller?)` factory. Calls `makeSessionDir(now, TEST_SESSIONS_ROOT)` → real session files in `.omega/test-sessions/`. `dispose()` is a no-op (sessions preserved). Returns `{ agent, sessionDir, contextFile, eventsFile, dispose }`.
- `src/session-dir.test.ts` — real-I/O tests for `makeSessionDir`: verifies both files are created eagerly (even for sessions with no messages).

### Recent Work
- **TEST-1** (commit 9feb285): Migrated all agent tests to real temp-dir (`/tmp`) session files. `makeTestAgent()` returns `{ agent, sessionDir, contextFile, eventsFile, dispose }`. The `sessions-test/` directory eliminated. 498 unit + 41 e2e tests green.
- **TEST-2** (commit 06909d3): Moved test sessions from `/tmp` to `.omega/test-sessions/`. `makeTestAgent()` now uses `makeSessionDir(now, TEST_SESSIONS_ROOT)`. `dispose()` is a no-op — sessions accumulate as inspectable artifacts. `assertNotProductionPath()` updated to allow `.omega/test-sessions/`. e2e tests also write to `.omega/test-sessions/`. 499 unit + 41 e2e green.
- **Eager session files** (commit d9f9f08): `makeSessionDir()` now creates both `context.jsonl` and `events.jsonl` immediately (flag `wx`), so sessions without any messages are never incomplete. 3 new tests in `session-dir.test.ts`. 502 unit + 41 e2e green.
