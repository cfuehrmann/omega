# Future — Issue Tracker

## Open items

### [SESSION] Session storage

#### SESSION-1 Each persisted session gets its own folder ✅ DONE

Each session writes to its own timestamped folder (`sessions/YYYY-MM-DDTHH-MM-SS/`)
containing `context.jsonl` and `events.jsonl`. Old sessions accumulate and are never
touched. The file-rotation machinery (`rotateFile`, `prevPath`, `clearContextStore`,
`clearEvents`) was removed — each fresh folder makes rotation unnecessary.

Implemented in commits 3fa0df4 and 326729a.

#### SESSION-2 Storage location of persisted sessions ✅ DONE

Sessions live in `.omega/sessions/<timestamp>/` relative to the directory
Omega is launched from (the project being worked on).

**Decisions made:**
- **Option 2 (launch directory / cwd)** chosen: simple, predictable, operator
  controls placement by choosing where to launch from. No `.git` walk needed.
- **Nested layout** `.omega/sessions/` over flat `.omega/`: the `.omega/`
  namespace leaves room for future artefacts (config, per-project world-state,
  etc.) without mixing session folders with other files.
- **No automatic `.gitignore`**: committing sessions is the operator's choice —
  the whole point of co-locating sessions with the project.

Implemented in commit caf3aee. `SESSIONS_ROOT` changed from `"sessions"` to
`".omega/sessions"` in `src/session-dir.ts`; `test-guard.ts` updated to match.

#### SESSION-2b Web server persistence parity with terminal

The web server accumulated a parallel persistence layer (`src/web/session-store.ts`
→ `sessions/current.jsonl`) that diverged from the terminal agent's model
(`.omega/sessions/<timestamp>/context.jsonl` + `events.jsonl`). The two paths
drifted out of sync and the e2e test server only simulated half the picture.

**Goal:** The web server is purely a different UI skin — all persistence is
identical to the terminal path. `Agent` writes `context.jsonl` and `events.jsonl`;
the web server just iterates the same async generator and forwards events over
WebSocket.

**Changes:**
- Delete `src/web/session-store.ts` and the `sessions/current.jsonl` mechanism
- Web server event loop mirrors terminal: `for await (event of agent.sendMessage)` → `ws.send`
- History replay on reconnect reads `events.jsonl` from the current session dir
- E2e test server simplified to match: in-memory event log for replay, no `current.jsonl`
- `session_start` / `session_end` / crash-detection handled same as terminal
- `test-guard.ts` updated to protect the new path if needed

#### SESSION-3 Strict session resumption

On startup, offer to resume a previous session. Validate the candidate session's
persisted files against the current Omega schema version: sessions with richer
data (extra fields) are tolerated; sessions with missing required fields are
rejected. If validation fails, Omega should bail out rather than silently
continuing with corrupted state.

Resumption could be offered interactively at startup or triggered via a
command-line flag.

> Command-line flag design is a good occasion to discuss a CLI argument library
> (built-in help, input validation, etc.).

#### SESSION-4 Soft session resumption

When strict resumption fails, offer a fallback: ask the LLM to summarise the
incompatible session and use that summary as a starting point. Soft resumption
should be decoupled from the agent — it is a project-level feature.

_Key advantage:_ soft resumption relaxes the pressure to nail down the schema
once and for all — schema evolution becomes less painful.

#### SESSION-5 Human-readable folder names

Session folders should be renameable to meaningful names (e.g. `implement-login-flow`)
without breaking anything. This is the natural session-labelling mechanism — no
separate tagging concept needed.

### [SCHEMA] Persistence contract (schema lock)

**Status: IN PROGRESS**

Review and lock the exact shape of every JSONL record in
`sessions/context.jsonl` and `sessions/events.jsonl`.

#### SCHEMA-1 — Property names and completeness per event

For every event variant, review: (a) are the existing field names clear and
consistent? (b) are any fields missing that would be needed for post-mortem
diagnosis or session resume?

**Priority: error events first.** `llm_error` and `agent_error` are the events
most likely to be consulted in a post-mortem — if their fields are incomplete or
missing cross-references, the diagnostic value is zero exactly when it matters
most.

Known candidates, in priority order:

- `LlmErrorEvent` / `AgentErrorEvent` — no cross-reference to the `llm_call`
  that triggered the error. Currently linked only by temporal order. Should
  carry a reference (e.g. the `contextHashes` of the failed call, or a call ID)
  so a post-mortem can reconstruct exactly what was sent when the error
  occurred.
- `LlmCallEvent` / `LlmResponseEvent` — linked only by temporal order in the
  JSONL; no explicit cross-reference field. Is ordering sufficient, or should
  `llm_response` carry a reference back to its `llm_call`?
- `TurnEndEvent.toolCalls` — list of tool _names_, not IDs; cannot be correlated
  back to individual `tool_call` events. Consider replacing with `toolUseIds` or
  keeping as a summary alongside the IDs.
- `SessionStartEvent.authMode` — only two live values (`"claude-max"`,
  `"api-key"`); should be a typed union, not a free string.

#### SCHEMA-2 — "All retries exhausted" missing `llm_error`

When every retry attempt is consumed (both Anthropic and OpenAI paths), the
final fallback yields a bare `agent_error` with no `llm_error` event. This is
the worst crash path and has the least diagnostic coverage — exactly backwards
from what we want.

Acceptance criteria:

- "All retries exhausted" path emits `llm_error` (with `lastError` details)
  before `agent_error`
- Both Anthropic and OpenAI paths covered
- Test: mock stream that always throws a retryable error; assert `llm_error`
  event is present after exhaustion

#### SCHEMA-3 — Web server protocol errors not in `events.jsonl`

Three conditions in `web/server.ts` emit `{ type: "error" }` over WebSocket but
write nothing to `events.jsonl`: invalid JSON from client, "turn already in
progress", and uncaught throws in the turn loop. These are intentionally outside
the `OmegaEvent` type (WebSocket-protocol-level errors), but whether they should
be persisted is an open design question.

Decision needed: are these server-internal errors that belong in `events.jsonl`
(perhaps as `agent_error`), or are they purely transport-layer and deliberately
excluded from the session record?

Acceptance criteria:

- Explicit decision recorded in backlog and in the schema doc (SCHEMA-6)
- If persisted: wired via `logEvent()` with an appropriate `OmegaEvent` variant
- If excluded: documented as intentional omission in SCHEMA-4

#### SCHEMA-4 — Persistence completeness audit

Formally verify and document which events/signals are intentionally _not_
persisted, and why. Current known intentional omissions:

- `status` messages — gone; each real signal is now a typed event.
- `metrics` AgentEvent — gone; superseded by `llm_response` usage fields and
  `turn_end` aggregate.
- Streaming `text` fragments — assembled response is in `context.jsonl`; `text`
  is a `StreamSignal`, explicitly outside the persistence boundary by design.

Close the question explicitly so future contributors know these are deliberate,
not oversights.

#### SCHEMA-5 — Forward-compatibility policy

Document the Postel's Law contract for the persistence schema:

- **Tolerant readers:** unknown fields on a known event are silently ignored;
  unknown event types are silently skipped.
- **Additive writers:** adding a new optional field or a new event type is a
  non-breaking change and requires no migration.
- **Breaking changes** (removing or renaming a required field, changing field
  semantics) require a documented migration plan and must not happen silently.

This policy should live in the schema reference document produced by SCHEMA-6.

#### SCHEMA-6 — Schema reference document

After SCHEMA-1 through SCHEMA-5 are resolved, write `plan/schema.md`: the
definitive reference for every JSONL record in `sessions/context.jsonl` and
`sessions/events.jsonl`. Covers field names, types, required vs. optional, all
event variant names, and the forward-compatibility policy from SCHEMA-5. This
document is the stable contract that SCHEMA-7 builds on.

#### SCHEMA-7 — Session resume

**Depends on SCHEMA-6**

On startup, if a `.prev` session exists, offer to resume it. Load
`context.prev.jsonl` and `events.prev.jsonl`, restore `llmContextView` and the
event history, and continue as if the session had not ended.

Acceptance criteria:

- Startup detects a non-empty `context.prev.jsonl`
- User is prompted: resume previous session or start fresh
- On resume: `llmContextView` is restored from the context file; events file is
  appended to (not rotated)
- On fresh start: behaviour unchanged from today
- Test: round-trip — session writes context, restarts, resumes, next API call
  sends the restored history with correct `contextHashes`

---

### [INFRA] Self-protection and structural invariants

#### INFRA-1 — Structural invariant tests for web server entry point

`entry.test.ts` guards `ui-raw.ts` and terminal modules. Same pattern needed for
`src/web/server.ts` exports (`runWebApp`, `closeOpenTurn`, `shouldLogEvent`). If
someone renames or restructures `server.ts`, `bun test` currently won't catch
it.

Acceptance criteria:

- `entry.test.ts` or a new `web-entry.test.ts` imports and asserts callability
  of those exports
- `bun test` catches a rename/deletion of `server.ts`

#### INFRA-2 — Abort-safe agentic loop — soft interrupt at tool boundary

`AbortSignal` can fire mid-tool-execution. The tool result is lost, leaving a
`tool_use` block in history with no matching `tool_result` → 400 on next turn.

Acceptance criteria:

- Esc mid-tool waits for the in-flight tool to complete, then stops
- History is always well-formed (every `tool_use` has a matching `tool_result`)
- Test: abort signal fires during a tool call; next API call succeeds

#### INFRA-3 — History validation before every API call

Cheap sanity check at top of agentic loop: every `tool_use` block must have a
matching `tool_result`. If not, abort the turn rather than sending malformed
history. Circuit-breaker; real fix is INFRA-2.

#### INFRA-4 — Decouple Omega startup from Omega's own repo (world-state)

Currently `projectWorldStatePath()` always resolves to
`<cwd>/plan/world-state.md`. This means Omega's self-knowledge is injected into
_every_ session regardless of which project Omega is pointed at.

**Goal:** When Omega is started on an arbitrary project, it should receive no
Omega-specific world state. When started on itself (`~/omega/dev`), it should
still load its own world state as today.

**Proposed approach:**

1. **World-state opt-in via README** — only read the file if the project's
   `README.md` explicitly references a world state path. If not mentioned, skip
   it.
2. **Remove hardcoded startup coupling** — condition `loadWorldState()` calls on
   the README check, or delegate entirely to the agent after README parsing.
3. **Omega's own README stays as-is** — already references
   `plan/world-state.md`.

Acceptance criteria:

- Starting Omega in an arbitrary project directory injects no Omega-specific
  world state
- Starting Omega in `~/omega/dev` still loads `plan/world-state.md` as Zone 1
  context
- No new config files or command-line flags required
- All existing tests pass; add a test for the "no README world-state reference →
  null" path

---

### [UX] Prompt queuing — interruption, injection, and turn sequencing

**Priority: HIGH — next major design area**

The core question: how should the user interact with Omega _while a turn is in
flight_? Today, Esc aborts unconditionally.

#### UX-1 — Ideal hard stop

Candidates: single Esc = soft abort (finish current tool, stop); double Esc =
hard kill. Acceptance criteria: define and implement one semantics; document the
choice.

#### UX-2 — Modifying an ongoing turn

Candidates: a "prompt queue" buffer delivered at the next clean break (after
current tool call, before next API call); a visible "pending" line in the UI.

Design questions before implementation:

- Where is the queue buffer stored? (in `app.ts` state? in `agent.ts`?)
- How does the agent loop receive it? (callback? `Promise`? shared
  `AsyncIterable`?)
- Does it inject into the _current_ turn's history or start the _next_ turn?
- What is the UI affordance?

---

### [ARCH] Provider feature parity & architecture

#### ARCH-1 — Clean provider boundary in agent.ts

**Priority: do first — unblocks everything below**

`agent.ts` has large `if (useOpenAi) { ... } else { ... }` blocks inside the
agentic loop. Goal: extract `callAnthropicTurn()` and `callOpenAiTurn()` helpers
so each provider's slice is self-contained.

Acceptance criteria:

- Agentic loop body has no large `if (useOpenAi)` branch
- Each provider helper is independently testable
- All existing tests still pass

#### FEAT-1 — Anthropic extended thinking

Pass `thinking: { type: "enabled", budget_tokens: N }` to Anthropic calls.
Requires `anthropic-beta: interleaved-thinking-2025-05-14` header (see FEAT-3).

Sub-tasks: add `thinking` param; handle `thinking` content blocks (don't yield
as text); cost accounting; tests.

#### FEAT-2 — OpenAI `previous_response_id`

**Priority: high — cuts OpenAI input token cost by ~80% on long sessions**

`callOpenAi()` resends full history on every call. Responses API supports
`previous_response_id` to let the server maintain history.

Sub-tasks: accept/return `previousResponseId` in `callOpenAi()`; thread ID
through agentic loop; reset on turn boundary; tests.

#### FEAT-3 — Anthropic beta headers on API-key path

**Priority: low-medium — prerequisite for FEAT-1 on API-key auth**

OAuth client sets `anthropic-beta: claude-code-20250219,oauth-2025-04-20`.
API-key client sends no beta headers. Goal: unify so both paths get the same
betas.

---

### [TOOLS] Tool set expansion

#### TOOLS-1 — `run_command_async` + `await_command`

`run_command` is blocking. Two new tools: `run_command_async(command, cwd?)`
returns a `jobId` immediately; `await_command(jobId, timeout_ms?)` returns
stdout/stderr/exitCode. Distinct from `run_background`/`kill_process`
(fire-and-forget). This is awaitable.

---

### [WEB] Web interface e2e tests — expand coverage

#### WEB-1 — Playwright gap coverage

Playwright infrastructure works (24 tests). Gaps:

- Reconnection flow: `.reconnect-banner` appears after 2 failed retries
- Abort button click sends `{type:"abort"}` to server
- Input clears after send
- Auto-scroll: feed scrolls to bottom on new content

Always go RED first.

---

## Closed items

- **Diagnostics / diagnosis/ dir** — Removed. Replaced by `session_end` event +
  `.prev` file crash detection.
- **Mid-turn context overflow: error-out** — Done. Context overflow is
  non-retryable: `llm_error` + actionable `agent_error`.
- **Event system unification** — Done. `AgentEvent`/`SessionEvent` merged into
  `OmegaEvent`; `status` variant deleted; all stream/wire/UI names match
  persistence; exhaustive switch guards in both UIs.
- **Compaction event overhaul** — Done. `session_compacted` replaced by
  `compact_user_start/done/error`. Three bugs fixed: `context.jsonl` destructive
  mutation, missing error persistence, conditional success event. Hash rebuild
  bug fixed.
- **BUG-1: `max_tokens` mid-tool-call context poison** — Fixed. Dangling
  `tool_use` on `max_tokens` now gets synthetic `tool_result(is_error=true)`
  entries; turn ends cleanly; next turn succeeds.
- **`session_end` event** — Done. `outcome: "clean" | "error"`; terminal startup
  warns on missing/error outcome in `.prev` file.
- **FK/PK contract** — Done. `context.jsonl` records carry `hash` + `ts`;
  `llm_call` carries `contextHashes[]`; `tool_call`/`tool_result`/`llm_response`
  carry `contextHash` FK.
- **Pre-lock field removals** — Done. `LlmResponseEvent.content`,
  `LlmCallEvent.messageCount`, `ToolCallEvent.input`,
  `ToolResultEvent.outputLength` all removed.
- **Auto-compact trigger: token-based** — Done. `lastPromptTokens` check
  replaces message-count. `AUTO_COMPACT_THRESHOLD = 100_000` tokens.
- **Test-pollution prevention** — Done. `OMEGA_TEST=1` preload;
  `assertNotProductionPath()` guard; Agent coercion; `makeTestAgent()` factory;
  pre-commit grep.
- **`/compact` command + tests** — Done. 27 tests covering all event variants,
  state mutations, error path.
- **Tool display truncation** — Done. Both UIs cut at 5 lines / 500 chars.
- **Minimal append-only line editor** — Done. Cursor tracking, arrow keys,
  word-jump, forward-delete all removed.
- **Retire pino** — Done. `src/logger.ts` deleted, `pino` removed.
- **Steps 3a–3e-iii** — Done. Append-only context file, `/compact` command,
  event log dual-write, non-destructive truncation, event/WsEvent renames, FK/PK
  contract.
- **Parallel tool execution** — Done.
- **Anthropic prompt caching + cache savings display** — Done.
- **Rate-limit retry + OAuth auto-relogin** — Done.
- **Background process tools (`run_background`, `kill_process`)** — Done.
- **`grep_files`, `find_files`** — Done.
- **Bun WebSocket server + Solid.js web client** — Done.
- **Pre-commit test gate** — Done.
- **Two-branch model `main`/`develop`** — Done.
