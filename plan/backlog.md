# Future — Issue Tracker

## Open items

### [REFACTOR] Manifest-driven redesign — making Omega project-agnostic
**Priority: HIGHEST — ongoing, guided by `manifest.md`**

#### Step 3e — Stable persistence contract (schema lock)
**Status: IN PROGRESS**

Review and lock the exact shape of every JSONL record in `sessions/context.jsonl`
and `sessions/events.jsonl`. EU-1 through EU-4 are complete; proceed with the
sub-steps below.

**3e-iv — Property names and completeness per event**
For every event variant, review: (a) are the existing field names clear and
consistent? (b) are any fields missing that would be needed for post-mortem
diagnosis or session resume?

**Priority: error events first.** `llm_error` and `agent_error` are the events
most likely to be consulted in a post-mortem — if their fields are incomplete or
missing cross-references, the diagnostic value is zero exactly when it matters most.

Known candidates, in priority order:
- `LlmErrorEvent` / `AgentErrorEvent` — no cross-reference to the `llm_call`
  that triggered the error. Currently linked only by temporal order. Should carry
  a reference (e.g. the `contextHashes` of the failed call, or a call ID) so a
  post-mortem can reconstruct exactly what was sent when the error occurred.
- `LlmCallEvent` / `LlmResponseEvent` — linked only by temporal order in the
  JSONL; no explicit cross-reference field. Is ordering sufficient, or should
  `llm_response` carry a reference back to its `llm_call`?
- `TurnEndEvent.toolCalls` — list of tool *names*, not IDs; cannot be correlated
  back to individual `tool_call` events. Consider replacing with `toolUseIds` or
  keeping as a summary alongside the IDs.
- `SessionStartEvent.authMode` — only two live values (`"claude-max"`,
  `"api-key"`); should be a typed union, not a free string.

**3e-v — Missing event types**

**3e-v-2 — "All retries exhausted" missing `llm_error`**
When every retry attempt is consumed (both Anthropic and OpenAI paths), the final
fallback yields a bare `agent_error` with no `llm_error` event. This is the worst
crash path and has the least diagnostic coverage — exactly backwards from what we want.

Acceptance criteria:
- "All retries exhausted" path emits `llm_error` (with `lastError` details) before
  `agent_error`
- Both Anthropic and OpenAI paths covered
- Test: mock stream that always throws a retryable error; assert `llm_error` event
  is present after exhaustion

**3e-v-4 — Web server protocol errors not in `events.jsonl`**
Three conditions in `web/server.ts` emit `{ type: "error" }` over WebSocket but
write nothing to `events.jsonl`: invalid JSON from client, "turn already in
progress", and uncaught throws in the turn loop. These are intentionally outside
the `OmegaEvent` type (WebSocket-protocol-level errors), but whether they should
be persisted is an open design question.

Decision needed: are these server-internal errors that belong in `events.jsonl`
(perhaps as `agent_error`), or are they purely transport-layer and deliberately
excluded from the session record?

Acceptance criteria:
- Explicit decision recorded in backlog and in the schema doc (3e-viii)
- If persisted: wired via `logEvent()` with an appropriate `OmegaEvent` variant
- If excluded: documented as intentional omission in 3e-vi

**3e-vi — Persistence completeness audit**
Formally verify and document which events/signals are intentionally *not*
persisted, and why. Current known intentional omissions:
- `status` messages — gone (EU-2); each real signal is now a typed event.
- `metrics` AgentEvent — gone (EU-1); superseded by `llm_response` usage fields and `turn_end` aggregate.
- Streaming `text` fragments — assembled response is in `context.jsonl`; `text` is a `StreamSignal`, explicitly outside the persistence boundary by design.

Close the question explicitly so future contributors know these are deliberate,
not oversights.

**3e-vii — Forward-compatibility policy**
Document the Postel's Law contract for the persistence schema:
- **Tolerant readers:** unknown fields on a known event are silently ignored;
  unknown event types are silently skipped.
- **Additive writers:** adding a new optional field or a new event type is a
  non-breaking change and requires no migration.
- **Breaking changes** (removing or renaming a required field, changing field
  semantics) require a documented migration plan and must not happen silently.

This policy should live in the schema reference document produced by 3e-viii.

**3e-viii — Schema reference document**
After 3e-iv through 3e-vii are resolved, write `plan/schema.md`: the definitive
reference for every JSONL record in `sessions/context.jsonl` and
`sessions/events.jsonl`. Covers field names, types, required vs. optional,
all event variant names, and the forward-compatibility policy from 3e-vii.
This document is the stable contract that session-resume (Step 3f) builds on.

#### Step 3f — Session resume
**Status: TODO — depends on schema lock**

On startup, if a `.prev` session exists, offer to resume it. Load
`context.prev.jsonl` and `events.prev.jsonl`, restore `llmContextView` and the
event history, and continue as if the session had not ended.

Acceptance criteria (to be refined after schema lock):
- Startup detects a non-empty `context.prev.jsonl`
- User is prompted: resume previous session or start fresh
- On resume: `llmContextView` is restored from the context file; events file is
  appended to (not rotated)
- On fresh start: behaviour unchanged from today
- Test: round-trip — session writes context, restarts, resumes, next API call
  sends the restored history with correct `contextHashes`

---

### [INFRA] Self-protection — preventing Omega from taking itself down

#### REC-2 (LOW): Structural invariant tests for web server entry point
`entry.test.ts` guards `ui-raw.ts` and terminal modules. Same pattern needed for
`src/web/server.ts` exports (`runWebApp`, `closeOpenTurn`, `shouldLogEvent`). If
someone renames or restructures `server.ts`, `bun test` currently won't catch it.

Acceptance criteria:
- `entry.test.ts` or a new `web-entry.test.ts` imports and asserts callability of
  those exports
- `bun test` catches a rename/deletion of `server.ts`

#### REC-3 (MEDIUM): Abort-safe agentic loop — soft interrupt at tool boundary
`AbortSignal` can fire mid-tool-execution. The tool result is lost, leaving a
`tool_use` block in history with no matching `tool_result` → 400 on next turn.

Acceptance criteria:
- Esc mid-tool waits for the in-flight tool to complete, then stops
- History is always well-formed (every `tool_use` has a matching `tool_result`)
- Test: abort signal fires during a tool call; next API call succeeds

#### REC-4 (LOW): History validation before every API call
Cheap sanity check at top of agentic loop: every `tool_use` block must have a
matching `tool_result`. If not, abort the turn rather than sending malformed
history. Circuit-breaker; real fix is REC-3.

---

### [TOPIC] Prompt queuing — interruption, injection, and turn sequencing
**Priority: HIGH — next major design area**

The core question: how should the user interact with Omega *while a turn is in
flight*? Today, Esc aborts unconditionally.

#### UX-Q1: Ideal hard stop
Candidates: single Esc = soft abort (finish current tool, stop); double Esc = hard
kill. Acceptance criteria: define and implement one semantics; document the choice.

#### UX-Q2: Modifying an ongoing turn
Candidates: a "prompt queue" buffer delivered at the next clean break (after current
tool call, before next API call); a visible "pending" line in the UI.

Design questions before implementation:
- Where is the queue buffer stored? (in `app.ts` state? in `agent.ts`?)
- How does the agent loop receive it? (callback? `Promise`? shared `AsyncIterable`?)
- Does it inject into the *current* turn's history or start the *next* turn?
- What is the UI affordance?

---

### [TOPIC] Provider feature parity & architecture

#### ARCH-1: Clean provider boundary in agent.ts
**Priority: do first — unblocks everything below**

`agent.ts` has large `if (useOpenAi) { ... } else { ... }` blocks inside the agentic
loop. Goal: extract `callAnthropicTurn()` and `callOpenAiTurn()` helpers so each
provider's slice is self-contained.

Acceptance criteria:
- Agentic loop body has no large `if (useOpenAi)` branch
- Each provider helper is independently testable
- All existing tests still pass

#### FEAT-2: Anthropic extended thinking
**Priority: medium**

Pass `thinking: { type: "enabled", budget_tokens: N }` to Anthropic calls. Requires
`anthropic-beta: interleaved-thinking-2025-05-14` header (see FEAT-4).

Sub-tasks: add `thinking` param; handle `thinking` content blocks (don't yield as
text); cost accounting; tests.

#### FEAT-3: OpenAI `previous_response_id`
**Priority: high — cuts OpenAI input token cost by ~80% on long sessions**

`callOpenAi()` resends full history on every call. Responses API supports
`previous_response_id` to let the server maintain history.

Sub-tasks: accept/return `previousResponseId` in `callOpenAi()`; thread ID through
agentic loop; reset on turn boundary; tests.

#### FEAT-4: Anthropic beta headers on API-key path
**Priority: low-medium — prerequisite for FEAT-2 on API-key auth**

OAuth client sets `anthropic-beta: claude-code-20250219,oauth-2025-04-20`. API-key
client sends no beta headers. Goal: unify so both paths get the same betas.

---

### [TOPIC] Tool set expansion

#### TOOLS-4: `run_command_async` + `await_command`
**Priority: medium**

`run_command` is blocking. Two new tools: `run_command_async(command, cwd?)` returns
a `jobId` immediately; `await_command(jobId, timeout_ms?)` returns stdout/stderr/exitCode.
Distinct from `run_background`/`kill_process` (fire-and-forget). This is awaitable.

---

### [TOPIC] Web interface e2e tests — expand coverage
**Priority: medium**

Playwright infrastructure works (24 tests). Gaps:
- Reconnection flow: `.reconnect-banner` appears after 2 failed retries
- Abort button click sends `{type:"abort"}` to server
- Input clears after send
- Auto-scroll: feed scrolls to bottom on new content

Always go RED first.

---

### [REFACTOR] Decouple Omega startup from Omega's own repo (world-state)
**Priority: LOW — do after Steps 3e and 4**

Currently `projectWorldStatePath()` always resolves to `<cwd>/plan/world-state.md`.
This means Omega's self-knowledge (Zone 1 context) is injected into *every* session
regardless of which project Omega is pointed at — coupling the agent to its own repo.

**Goal:** When Omega is started on an arbitrary project, it should receive no
Omega-specific world state. When started on itself (`~/omega/dev`), it should still
load its own world state as today.

**Proposed approach:**

1. **World-state opt-in via README** — `loadWorldState()` should only read the file if
   the project's `README.md` explicitly references a world state path. If the README
   doesn't mention it, no world state is injected.

2. **Remove hardcoded startup coupling** — `terminal/app.ts` and `web/server.ts` both
   call `agent.loadWorldState()` unconditionally. These calls should be conditioned on
   the README check above, or delegated entirely to the agent after README parsing.

3. **Omega's own README stays as-is** — it already references `plan/world-state.md`,
   so Omega pointed at itself continues to work exactly as today.

Acceptance criteria:
- Starting Omega in an arbitrary project directory injects no Omega-specific world state
- Starting Omega in `~/omega/dev` still loads `plan/world-state.md` as Zone 1 context
- No new config files or command-line flags required
- All existing tests pass; add a test for the "no README world-state reference → null" path

---

## Closed items

- **Diagnostics / diagnosis/ dir** — Removed (commit bfd5d0d). Replaced by `session_end` event + `.prev` file crash detection. `writeDiagnostic`, `DiagnosticWrittenEvent`, `diagDir` param, `checkDiagnostics` all gone. `systemPrompt` added to `session_start`; `cacheBreakpointIndex` added to `llm_call`.
- **Mid-turn context overflow: error-out** — Done (commit 13c1f9e). `buildSentContext`, `apiBudget`, `contextHashesForView`, `context_view_trimmed` deleted. Context overflow is non-retryable: `llm_error` + actionable `agent_error`.
- **Event system unification (EU-1–EU-4)** — Done. `AgentEvent`/`SessionEvent` merged into `OmegaEvent`; `status` variant deleted; all stream/wire/UI names match persistence; exhaustive switch guards in both UIs.
- **3e-v-1: Compaction event overhaul** — Done. `session_compacted` replaced by `compact_user_start/done/error`. Three bugs fixed: `context.jsonl` destructive mutation, missing error persistence, conditional success event. Hash rebuild bug fixed (tail hashes reused, not re-computed).
- **BUG-1: `max_tokens` mid-tool-call context poison** — Fixed (commit 9682be6). Dangling `tool_use` on `max_tokens` now gets synthetic `tool_result(is_error=true)` entries; turn ends cleanly; next turn succeeds.
- **3e-v-3: `session_end` event** — Done (commit bfd5d0d). `outcome: "clean" | "error"`; terminal startup warns on missing/error outcome in `.prev` file.
- **FK/PK contract (3e-iii)** — Done (commit b6ef87c). `context.jsonl` records carry `hash` + `ts`; `llm_call` carries `contextHashes[]`; `tool_call`/`tool_result`/`llm_response` carry `contextHash` FK.
- **Pre-lock field removals** — Done. `LlmResponseEvent.content`, `LlmCallEvent.messageCount`, `ToolCallEvent.input`, `ToolResultEvent.outputLength` all removed.
- **Auto-compact trigger: token-based** — Done. `lastPromptTokens` check replaces message-count. `AUTO_COMPACT_THRESHOLD = 100_000` tokens.
- **Test-pollution prevention (layers a–e)** — Done. `OMEGA_TEST=1` preload; `assertNotProductionPath()` guard; Agent coercion; `makeTestAgent()` factory; pre-commit grep.
- **`/compact` command + tests** — Done. 27 tests in `compact-command.test.ts`.
- **Tool display truncation** — Done. Both UIs cut at 5 lines / 500 chars.
- **Minimal append-only line editor** — Done. Cursor tracking, arrow keys, word-jump, forward-delete all removed (~240 lines).
- **Step 4: Retire pino** — Done. `src/logger.ts` deleted, `pino` removed.
- **Steps 3a–3d** — Done. Append-only context file, `/compact` command, event log dual-write, non-destructive truncation.
- **Step 3e-i/ii: Event/WsEvent renames** — Done.
- **Parallel tool execution** — Done.
- **Anthropic prompt caching + cache savings display** — Done.
- **Rate-limit retry + OAuth auto-relogin** — Done.
- **Background process tools (`run_background`, `kill_process`)** — Done.
- **`grep_files`, `find_files`** — Done.
- **Bun WebSocket server + Solid.js web client** — Done.
- **Pre-commit test gate (REC-1)** — Done.
- **Two-branch model `main`/`develop` (REC-0)** — Done.
