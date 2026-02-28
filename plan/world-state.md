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

No raw session persistence. No "resume session?" prompt. The world file is the only cross-session artifact. Each session also writes `sessions/context.jsonl` (append-only JSONL of every `MessageParam` as a `ContextRecord`) and `sessions/events.jsonl` (append-only JSONL of every `OmegaEvent`) as persistent records. Both files are **rotated** on startup: renamed to `.prev` files before the fresh session starts. `/compact` only replaces the in-memory `llmContextView`; `context.jsonl` is strictly append-only and is never rewritten.

### Rotated File Naming Convention
`rotateFile()` in `src/context-store.ts` inserts `.prev` **before** the last extension, not after:
- `context.jsonl` → `context.prev.jsonl`
- `events.jsonl` → `events.prev.jsonl`
- Files with no extension get `.prev` appended

The exported helper `prevPath(filePath)` encapsulates this logic.

### Manifest Refactor Status
`manifest.md` describes a major redesign. Current progress:
- **Step 1** (DONE): System prompt decoupled from Omega's own repo. Project-agnostic prompt reads `README.md` at startup.
- **Step 2** (DONE): Abandoned `compactAfterTurn()`. History grows verbatim.
- **Step 3** (DONE through 3e-iii): Replace `MessageParam[]` history with an event-list data structure.
  - **3a** (DONE): `src/context-store.ts` — appends each `MessageParam` to `sessions/context.jsonl`. `null` path is a no-op; mock-provider `Agent` defaults `contextFile` to `null`.
  - **3b** (DONE): `/compact` slash command — operator-triggered mid-session compaction. `compactHistory()` in `src/compaction.ts` summarises history head via LLM, keeps last `KEEP_RECENT_TURNS` (10) message-pairs verbatim. Handler in `agent.ts` replaces `this.llmContextView` in memory only; `context.jsonl` is append-only and is never rewritten on compaction.
  - **3c** (DONE): `SessionEvent` type + dual-write to `sessions/events.jsonl`. All events carry ISO `ts`. `logEvent()` private helper in `agent.ts` (fire-and-forget, null-safe). `eventsFile` field with mock-provider heuristic. Wired at every significant site. `clearSessionEvents()` called at startup (rotates to `.prev`).
  - **3d** (DONE): Non-destructive context truncation. `truncateHistory()` renamed to `buildApiMessages()` — purely ephemeral; source array never mutated. `Agent.llmContextView` (formerly `llmMessageLog`) is the mutable in-memory context window; `getLlmContextView()` is the public accessor. Note: `buildApiMessages` / `apiBudget` / prompt-too-long retry halvings subsequently deleted (see commit 13c1f9e). Context overflow now errors out immediately.
  - **3e-i** (DONE): Rename `SessionEvent` and `AgentEvent` discriminant strings. 7 renames applied.
  - **3e-ii** (DONE): Rename `WsEvent` / `AgentEvent` variants to match coordinate-system model. `SessionEvent` variants (`events.jsonl`) remain `tool_call`/`tool_result`/`llm_response` — separate namespace.
  - **3e-iii** (DONE): FK/PK contract — content-addressed context log. `context.jsonl` entries carry `hash` (SHA-256 8 hex chars, computed from `{ ts, role, content }`) and `ts`. `LlmCallEvent` carries `contextHashes: string[]` — ordered hashes of every message in the `buildApiMessages()` view actually sent.
  - **[SCHEMA] pre-lock fixes** (DONE): `LlmResponseEvent.content` removed (duplication); `LlmCallEvent.messageCount` and `llmCallNumber` removed (derivable). `ToolCallEvent.input` removed; `ToolResultEvent.outputLength` removed. All FK pointers use hash-based pattern consistently. `LlmResponseEvent.usage` records all four Anthropic token counts plus `service_tier`.
  - **3e-iv** (TODO): Property names and completeness per event — cross-references on error events, `TurnEndEvent.toolCalls`, `SessionStartEvent.authMode`. See backlog.
  - **3e-v** (TODO): Missing event types — sub-items:
    - **3e-v-1**: ✅ DONE — compaction event overhaul: `compact_user_*` + `compact_auto_*` events; `context.jsonl` append-only invariant fixed; hash rebuild bug fixed.
    - **3e-v-2**: "All retries exhausted" path emits bare `agent_error` with no `llm_error` and no diagnostic.
    - **3e-v-3**: `session_end` event missing — clean shutdown indistinguishable from crash; blocks session resume.
    - **3e-v-4**: Web server protocol errors (`{ type: "error" }`) not in `events.jsonl` — design decision needed.
  - **3e-vi through 3e-viii** (TODO): Persistence audit, forward-compatibility policy, schema reference doc. See backlog.
  - **3f** (TODO): Session resume, depends on schema lock (3e-viii).
- **Step 4** (DONE): Retire pino. `src/logger.ts` deleted, `pino` package removed.
- **EU-1** (DONE): Delete dead weight — `metrics` and `tool_result_message` removed from `AgentEvent`. In-loop `status` yields removed. `/help` slash command removed.
- **EU-2** (DONE): Replace remaining `status` yields with typed events. `model_changed`, `oauth_token_expired`, `oauth_refreshed` added to `AgentEvent`. `status` variant deleted entirely.
- **EU-3** (DONE): Unify `AgentEvent` and `SessionEvent` into one type (`OmegaEvent` in `src/events.ts`). `AgentEvent` kept as backward-compat alias. Stream-facing names now match persisted names: `tool_call`, `tool_result`, `llm_response` are canonical everywhere. `WsEvent` updated to match.
- **EU-4** (DONE): UI sync invariant — every `OmegaEvent` variant has a render case in both UIs. Exhaustive switch + `exhaustiveCheck(x: never)` guard in `terminal/app.ts` and `App.tsx`. `exhaustiveCheck()` exported from `src/events.ts`.

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
Tests must **never** write to `sessions/`, `diagnosis/`, or any other production file.

**Structural guardrails (all five layers implemented):**
- **Layer a:** `bunfig.toml` preloads `src/test-setup.ts`, which sets `OMEGA_TEST=1` before any test runs — unconditionally, for every `bun test` invocation.
- **Layer b:** `src/test-guard.ts` exports `assertNotProductionPath()`, wired into all production write functions (`appendContextMessage`, `clearContextStore`, `appendEvent`, `clearEvents`, `writeDiagnostic`). When `OMEGA_TEST=1`, writing to `sessions/` or `diagnosis/` throws immediately — a loud test failure rather than silent pollution. Temp-dir paths used by file-writing tests are unaffected.
- **Layer c:** `Agent` constructor coerces `undefined` file paths to `null` when `OMEGA_TEST=1`, unconditionally — no longer depends on mock `streamProvider` being injected.
- **Layer d:** `makeTestAgent(streamProvider?, openAiCaller?)` factory in `src/test-utils.ts` — always passes explicit `null` for all path args. Right thing is easy thing.
- **Layer e:** `scripts/pre-commit` greps for bare `new Agent()` in `*.test.ts` files before running tests. Fails with an actionable message pointing at `makeTestAgent`.

The `null`-is-no-op pattern still applies to all write functions. e2e tests use `sessions-test/` not `sessions/` and run via `just e2e` (not `bun test`), so they are unaffected by the preload. If a new production side-effect file is added, wire `assertNotProductionPath()` into its write function.

### Event Taxonomy
`OmegaEvent` (in `src/events.ts`) is the single unified type for all events — both streamed from `agent.ts` and persisted to `sessions/events.jsonl`. `AgentEvent` in `agent.ts` is a backward-compat alias. All names are consistent across all layers.

### OmegaEvent Variants (streamed from agent.ts AND persisted to events.jsonl)
`session_start`, `user_message`, `llm_call`, `llm_response`, `tool_call`, `tool_result`, `turn_end`, `llm_error`, `agent_error`, `turn_interrupted`, `compact_user_start`, `compact_user_done`, `compact_user_error`, `compact_auto_start`, `compact_auto_done`, `compact_auto_error`, `oauth_refreshed`, `oauth_token_expired`, `llm_retry`, `diagnostic_written`, `model_changed`. All carry ISO `ts` timestamp. No `status` variant — all lifecycle signals are typed. `context_view_trimmed` deleted (commit 13c1f9e — no mid-turn trimming). **Pending (3e-v-3):** `session_end` — not yet added; clean shutdown currently indistinguishable from crash.

Streaming text fragments are a `StreamSignal` (`{ type: "text", text: string }`) not an `OmegaEvent` — explicitly outside the persistence boundary by design.

`llm_call` additionally carries `contextHashes: string[]` — the ordered 8-char SHA-256 hashes of every `ContextRecord` in the `buildApiMessages()` view sent with that call.

`tool_call` additionally carries `contextHash: string` — FK to the assistant `context.jsonl` record containing the `tool_use` block. `input` field removed (content is in `context.jsonl`).

`tool_result` additionally carries `contextHash: string` — FK to the user `context.jsonl` record containing the `tool_result` block(s). `outputLength` removed (derivable).

`llm_response` carries metadata only: `stopReason`, `model`, `provider`, `url`, `usage`, and `contextHash` (FK). No `content` field — content is in `context.jsonl`. `usage` includes `input_tokens`, `output_tokens`, `cache_creation_input_tokens?`, `cache_read_input_tokens?`, `service_tier?`.

### WsEvent Variants (WebSocket protocol, src/web/client/store.ts)
`connected`, `disconnected`, `history`, `auth`, `turn_ready`, `reset_done`, `user_message`, `text`, `tool_call`, `tool_result`, `llm_response`, `model_changed`, `oauth_token_expired`, `oauth_refreshed`, `compact_user_start`, `compact_user_done`, `compact_user_error`, `compact_auto_start`, `compact_auto_done`, `compact_auto_error`, `llm_call`, `world_state_saved`, `turn_end`, `llm_error`, `agent_error`, `error` (server-own protocol errors only), `turn_interrupted`. No `status` variant. Note: `session_compacted` is retired — replaced by the three typed compact events above.

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
- `src/agent.ts` — Agent class, `sendMessage` async generator, `StreamProvider` type, `PRICING` table; `llmContextView` / `compactedContextHistory` is the mutable in-memory context window (formerly `llmMessageLog`); `llmContextHashes[]` / `compactedContextHashes[]` parallel array stores the content hash of each stored message; `appendToHistory()` awaits hash computation then fire-and-forgets the file write; builds `systemBlocks` and `cachedTools` with `cache_control`; `estimateCostWithCache()`; `estimateCacheSavings()`; `private activeModel`; `addCacheControlToLastMessage()` helper; parallel tool execution; `logEvent()` private helper (fire-and-forget, null-safe) wired at every significant site; `/compact` handler replaces context view and hashes in memory only — `context.jsonl` is never rewritten; on fatal errors calls `writeDiagnostic()`. Context overflow (prompt-too-long) is non-retryable: emits `llm_error` + actionable `agent_error`, writes diagnostic with `stopReason: "context_overflow"`. No `buildSentContext`, no `apiBudget`, no `contextHashesForView`. No `status` AgentEvent — all lifecycle signals are typed. `AgentEvent` is a backward-compat alias for `OmegaEvent`.
- `src/events.ts` — `OmegaEvent` discriminated union (all variants); `StreamSignal` type (`{ type: "text" }`); `exhaustiveCheck(x: never)` guard exported for exhaustive switches in UIs.
- `src/event-store.ts` — `appendEvent(event, filePath?)` and `clearEvents(filePath?)` — both use `null`-is-no-op pattern. `clearEvents()` rotates via `rotateFile()`. `DEFAULT_EVENTS_FILE = "sessions/events.jsonl"`. UI-only fields stripped by `toPersistedEvent()` before disk write.
- `src/context-store.ts` — `ContextRecord` interface; `buildContextRecord(msg)` (computes hash without writing); `appendContextMessage()` (writes record, returns hash); `clearContextStore()`; `rotateFile()`; `prevPath()`. `sha256hex8()` is internal (not exported). `clearContextStore()` rotates by default; accepts `{ rotate: false }` for in-place truncation.
- `src/compaction.ts` — `compactWorldState()` (LLM-based world-state fold) and `compactHistory()` (Step 3b — mid-session history compaction for `/compact`). `KEEP_RECENT_TURNS` = 10 exported. Returns `{ history, syntheticMessage, tailStartIndex, originalCount, newCount }` — caller uses `tailStartIndex` to reuse existing hashes rather than re-hashing tail messages.
- `src/world-state.ts` — `readWorldState()`, `writeWorldState()`, `projectWorldStatePath()` → `<cwd>/plan/world-state.md`
- `src/diagnosis.ts` — `writeDiagnostic(data, diagDir?)` writes a JSON snapshot to `diagnosis/<ISO-timestamp>.json`; `null` disables; `checkDiagnostics()` returns existing snapshot paths sorted oldest-first.
- `src/ui-raw.ts` — **thin re-export shim** (26 lines). CLI entry point.
- `src/terminal/input.ts` — `parseKeys`, `displayWidth`. Minimal append-only line editor: printable chars append at end, backspace deletes last char, Enter submits, Esc clears buffer (or aborts turn if empty), Ctrl+C exits. No cursor movement, no word-jump, no forward-delete. Bracketed paste accumulates and echoes on close marker.
- `src/terminal/renderer.ts` — ANSI color helpers, `printBlock`, `println`, `now()`, `truncateOutput` (dual-limit: 5 lines / 500 chars, whichever first), and all block renderers. `renderToolStart` truncates input JSON via `truncateOutput`. `renderApiResponse` shows `stop_reason` + `usage` only (no content). Cache/service_tier lines are conditional on non-zero/non-standard values.
- `src/terminal/app.ts` — `runApp`, `shutdown`, `setupRawInput`. Calls `clearContextStore()` then `clearEvents()` at startup (rotates both session files). Exhaustive switch on `OmegaEvent | StreamSignal`; `default` calls `exhaustiveCheck`. Shutdown ritual documented in `README.md ## Shutdown`.
- `src/tools.ts` — All tool implementations. `executeTool()` applies `MAX_TOOL_OUTPUT_CHARS = 100_000` cap to all tool results before they enter history; oversized output is truncated with an actionable note.
- `src/turn-footer.ts` — `formatTurnFooter(turn, session, provider, model)` returns `{ turnLine, sessionLine }`.
- `src/web/client/store.ts` — `WsEvent` discriminated union, `dispatch()`, reactive `AppState`. `turn_interrupted` closes an open turn; server-own protocol errors use `{ type: "error" }`. No `status` variant. `WsEvent` and `Turn` exported.
- `src/web/client/App.tsx` — SolidJS UI renderer. `EventBlock` exhaustive switch on `WsEvent` type; `default` calls `exhaustiveCheck`. `truncateOutput` (5 lines / 500 chars, same as terminal) applied to both `tool_call` input and `tool_result` output display.
- `src/web/server.ts` — `runWebApp()`, `closeOpenTurn()`, `shouldLogEvent()`. `closeOpenTurn` detects open turns on crash and appends `{ type: "turn_interrupted" }`.
- `src/context-hash.test.ts` — integration tests for the FK/PK contract: record shape, hash uniqueness, `contextHashes` cross-referencing, tool-loop growth; `[SCHEMA]` tests asserting field removals. `buildSentContext` and `contextHashesForView` describe blocks deleted (commit 13c1f9e).
- `src/compact-command.test.ts` — 27 tests covering the `/compact` slash command: all three event variants (`compact_user_start`, `compact_user_done`, `compact_user_error`), state mutations, error path, and post-compact continuity.
- `src/compact-auto.test.ts` — 27 tests covering auto-compact: threshold constant (`AUTO_COMPACT_THRESHOLD = 100_000` tokens), fires above/silent below threshold (using `setAboveThreshold()`/`setBelowThreshold()` helpers), error path (fallback continues), 9 BUG-1 scenarios (max_tokens poison → synthetic tool_result + agent_error, well-formed context, next turn succeeds), combined integration scenario.

### Context Overflow Policy (commit 13c1f9e)
Context overflow (API returns 400 "prompt is too long" or 429 "extra usage required for long context") is treated as a **non-retryable terminal error**:
- Emits `llm_error` (carries HTTP status)
- Emits `agent_error` with actionable message: *"Context too large to send. Use /compact to summarise history, or start a fresh focused turn."*
- Writes a diagnostic with `stopReason: "context_overflow"`
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
- `contextHashesForView()` deleted (commit 13c1f9e) — no longer needed since the full history is always sent.

### Key Files (additional)
- `src/test-guard.ts` — `assertNotProductionPath(filePath, fnName)`. Throws when `OMEGA_TEST=1` and `filePath` is under `sessions/` or `diagnosis/`. No-op in production. Wired into all five production write functions.
- `src/test-setup.ts` — Bun test preload (wired via `bunfig.toml`). Sets `OMEGA_TEST=1` before any test file loads.
- `src/test-guard.test.ts` — tests covering throw/no-throw behaviour of `assertNotProductionPath`.
- `src/test-utils.ts` — `makeTestAgent(streamProvider?, openAiCaller?)` factory; always passes `null` for all path args.

### Recent Session Outcomes
Completed **BUG-1 fix** (commit 9682be6): `max_tokens` mid-tool-call context poison. When the LLM hit `max_tokens` while generating a `tool_use` block, the dangling assistant message was appended before the `stop_reason` check, permanently bricking the session. Fix: detect `toolUseBlocks.length > 0 && stop_reason === "max_tokens"` after `appendToHistory`; synthesise `tool_result` blocks with `is_error: true` for every dangling id; append via `appendToHistory`; emit `tool_result` + `agent_error` events. Turn ends cleanly; next turn succeeds.

Completed **`src/compact-auto.test.ts`** (commit 9682be6, later updated to 27 tests via commit 1b560ac): `AUTO_COMPACT_THRESHOLD` constant; auto-compact fires above threshold (start/done events, view shrinks, ordering vs `llm_call`); silent below threshold; error path (fallback continues, view unchanged); 9 BUG-1 scenarios; combined integration scenario.

**Auto-compact trigger design:** Trigger metric is `lastPromptTokens` — total prompt tokens from the last LLM call (`input_tokens + cache_read_input_tokens + cache_creation_input_tokens`). Fires at most once per user turn — after the user message is appended, before the agentic loop starts. Never fires mid-loop. Threshold: `AUTO_COMPACT_THRESHOLD = 100_000` tokens (50% of Claude's 200k window). Constant exported from `src/compaction.ts`.

Completed **context overflow error-out** (commit 13c1f9e): deleted `buildSentContext`, `apiBudget`, `contextHashesForView`, `context_view_trimmed`. Context overflow is now a non-retryable terminal error. 486 tests, gate green.

Completed **BUG-3 fix** (commit 1b560ac): `compact-auto.test.ts` was broken by the token-threshold refactor — tests still used message-count semantics (`seedHistory(agent, AUTO_COMPACT_THRESHOLD + 1)` → 100,001 messages). Fixed by adding `setAboveThreshold()` / `setBelowThreshold()` helpers and rewriting all threshold tests. 27 tests pass.

Completed **synthetic tool_result content fix** (commit fe3c2a9): The `"max_tokens"` literal was absent from the synthetic error content string injected into history on BUG-1 (dangling tool_use). One test was failing. Fix: added `"max_tokens stop — "` prefix to the content. 487 tests, gate green.
