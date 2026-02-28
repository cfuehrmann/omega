# Future — Issue Tracker

## Open items

### [REFACTOR] Event system unification
**Priority: HIGHEST — prerequisite for schema lock and session resume**

Agreed design from session discussion. Four ordered steps:

#### EU-1 — Delete dead weight — DONE (commit 00a8078)
Removed `metrics` and `tool_result_message` from `AgentEvent`. Removed the two
in-loop `status` yields ("thinking…" / "OpenAI provider active"). Removed the
`/help` slash command (now yields `agent_error` like any unknown command — operator
asks the LLM). Removed "generating `<tool>` input…" `status` yield from both
`processStreamEvents()` and the inline streaming loop. Gate green.

#### EU-2 — Replace remaining `status` yields with typed events
Every remaining `status` yield corresponds to a real event that should be
persisted. Replace each:
- `oauth_token_expired` status yields → yield typed `oauth_token_expired` AgentEvent
- `oauth_refreshed` status yields → yield typed `oauth_refreshed` AgentEvent
- `/sonnet`, `/opus`, `/codex` acknowledgements → yield typed `model_changed` AgentEvent (new variant); persist as `SessionEvent`
- `/compact` lifecycle messages → yield typed `session_compacted` AgentEvent (already a `SessionEvent`); drop the intermediate "Compacting…" status
- "generating `<name>` input…" → **delete**; `llm_call` is sufficient signal that a call is in flight; silence until `agent_to_agent_tool_call` is acceptable

After this step, `status` as an AgentEvent variant can be deleted entirely.

Acceptance criteria:
- `AgentEvent` has no `status` variant
- `model_changed` is a new `SessionEvent` variant, persisted and rendered
- All oauth and compaction events are typed, rendered, and persisted
- Gate green

#### EU-3 — Unify AgentEvent and SessionEvent into one event type
One discriminated union — call it `OmegaEvent` — replaces both `AgentEvent`
and `SessionEvent`. Persistence is the default for all variants. A small
separate `StreamSignal` union covers the two genuinely ephemeral rendering
primitives: `text` (streaming token fragments) and nothing else (status is
gone by EU-2).

Design rules agreed:
- **Events** (`OmegaEvent`): discrete things that happened; always persisted;
  always rendered (at minimum: event name + timestamp on one line).
- **Stream signals** (`StreamSignal`): ephemeral rendering primitives; never
  persisted; the UI consumes them for live streaming display.
- Name mismatches disappear: one name per concept, used everywhere.
- The persistence layer writes every `OmegaEvent` to `events.jsonl`. For
  `llm_response`, content is omitted from the event (it's in `context.jsonl`
  via FK) — but the UI rendering uses the FK to display it. This is the only
  field that diverges between in-memory and persisted form, and it is
  principled: streaming is where persistence and UI legitimately diverge.
- `session_start` is rendered by the terminal app from `init()` return value
  (already done today in a different form); make it a proper `OmegaEvent`
  yielded from `init()` or the startup path.

Acceptance criteria:
- `src/session-event.ts` and the `AgentEvent` type in `src/agent.ts` merged
  into a single type (location TBD — probably `src/events.ts`)
- One switch statement per consumer (terminal renderer, web server)
- No name mismatches between persisted and streamed events
- `WsEvent` (web protocol) updated to match
- Gate green; e2e green

#### EU-4 — UI sync invariant: every OmegaEvent is rendered
After EU-3, enforce as a development-phase invariant: every variant in
`OmegaEvent` must have a render case in the terminal renderer and the web UI.
Minimum rendering: event name + timestamp on one line (compact). Some events
warrant more detail (tool calls, errors, turn_end). No event is silently
dropped.

Document this invariant in `plan/dev-policy.md` (ephemeral policy, not
manifest). Add a compile-time guard if feasible (exhaustive switch or lint
rule) so a new event variant without a render case is a build error, not a
silent omission.

Acceptance criteria:
- All `OmegaEvent` variants have a render case in terminal renderer
- All `OmegaEvent` variants have a render case in web UI (`App.tsx`)
- `plan/dev-policy.md` documents the invariant
- Exhaustive switch (TypeScript `never` check) or equivalent guard in place
- Gate green

---

### [REFACTOR] Manifest-driven redesign — making Omega project-agnostic
**Priority: HIGHEST — ongoing, guided by `manifest.md`**

#### Step 3e — Stable persistence contract (schema lock)
**Status: IN PROGRESS — event completeness being addressed in EU-1 through EU-4**

Review and lock the exact shape of every JSONL record in `sessions/context.jsonl`
and `sessions/events.jsonl`. The EU steps above address event completeness and
unification first; schema lock (3e-viii) follows once the unified type is stable.

##### Step 3e-i — Rename SessionEvent and AgentEvent variants
**Status: DONE**

##### Step 3e-ii — Rename WsEvent variants to match (web protocol)
**Status: DONE**

##### Step 3e-iii — FK/PK contract: content-addressed context log
**Status: DONE — commit b6ef87c**

##### Schema lock
**Status: TODO — [SCHEMA] items resolved (commit b59ba48), proceed in order**

The two [SCHEMA] items (`llm_response.content` duplication and redundant
`messageCount`) have been removed (commit b59ba48). Proceed with the sub-steps below.

**3e-iv — Property names and completeness per event**
For every event variant, review: (a) are the existing field names clear and
consistent? (b) are any fields missing that would be needed for post-mortem
diagnosis or session resume?

**Priority: error events first.** `llm_error` and `agent_error` are the events
most likely to be consulted in a post-mortem — if their fields are incomplete or
missing cross-references, the diagnostic value is zero exactly when it matters
most. Address these before other variants.

Known candidates, in priority order:
- `LlmErrorEvent` / `AgentErrorEvent` — no cross-reference to the `llm_call`
  that triggered the error. Currently linked only by temporal order, same weakness
  as `llm_call`/`llm_response`. Should carry a reference (e.g. the `contextHashes`
  of the failed call, or a call ID) so a post-mortem can reconstruct exactly what
  was sent when the error occurred.
- `LlmCallEvent` / `LlmResponseEvent` — linked only by temporal order in the
  JSONL; no explicit cross-reference field. Is ordering sufficient, or should
  `llm_response` carry a reference back to its `llm_call`?
- `TurnEndEvent.toolCalls` — list of tool *names*, not IDs; cannot be correlated
  back to individual `tool_call` events. Consider replacing with `toolUseIds` or
  keeping as a summary alongside the IDs.
- `SessionStartEvent.authMode` — only two live values (`"claude-max"`,
  `"api-key"`); should be a typed union, not a free string.
- `ToolCallEvent` / `ToolResultEvent` — both now carry `contextHash: string` (FK to
  the relevant `context.jsonl` record). `ToolCallEvent.input` and `ToolResultEvent.outputLength`
  removed (both derivable from `context.jsonl`). ✅ Done (commit 34f7708).

**3e-v — Missing event types**
Decide whether any important lifecycle events are absent.

**Priority: `session_end` first.** Without it there is no way to distinguish a
clean shutdown from a crash in the event log — the same gap that makes error
events incomplete also makes the overall session record ambiguous. Session resume
(Step 3f) cannot reliably detect a previously-crashed session without this.

Known candidates, in priority order:
- `session_end` — symmetric with `session_start`; allows distinguishing a clean
  shutdown from a crash. Needed for session resume to know whether the previous
  session completed normally.
- `model_changed` — when the operator uses `/sonnet`, `/opus`, or `/codex`
  mid-session the active model switches. Currently invisible in the event log;
  a replay or audit cannot determine when or why the model changed.
  Note: `model_changed` is also scheduled in EU-2 above — these two items converge.

**3e-v-bug-A — `user_message` event appears after `llm_call` in events.jsonl** ✅ FIXED (commit 25078f3)

Observed in a live session: the `llm_call` event was written to `events.jsonl`
*before* the `user_message` event that triggered it. Root cause: `logEvent()` was
fire-and-forget everywhere; the `user_message` async write lost the race to the
`llm_call` write. Fix: `logEvent()` now returns `Promise<void>`; the `user_message`
site awaits it before entering the agentic loop. All other `logEvent` sites remain
fire-and-forget — only `user_message` needs the ordering guarantee.

**3e-v-bug-B — `llm_call.contextHashes` FKs not yet flushed to context.jsonl** ✅ NOT A REAL BUG (investigated commit 25078f3)

Same session, same apparent symptom. Investigated: `appendToHistory()` fully awaits
`appendContextMessage()` which awaits the file write; all `appendToHistory()` calls
in each loop iteration are awaited before `continueLoop` is set; so `context.jsonl`
is always fully flushed before the next `llm_call` fires. The apparent out-of-order
appearance in `events.jsonl` was entirely caused by bug-A (the `user_message` event
race). No further fix needed.

**3e-vi — Persistence completeness audit**
Formally verify and document which events/signals are intentionally *not*
persisted, and why. Current known intentional omissions (to be updated after EU-1/EU-2):
- `status` messages — being removed entirely by EU-2; each real signal becomes a typed event.
- `metrics` AgentEvent — being removed by EU-1; superseded by `llm_response` usage fields and `turn_end` aggregate.
- Streaming `text` fragments — assembled response is captured in `context.jsonl`
  assistant message (`llm_response` intentionally carries no `content` — resolved in commit b59ba48).
  `text` becomes a `StreamSignal`, not an event — explicitly outside the persistence boundary by design.
Close the question explicitly so future contributors know these are deliberate,
not oversights.

**3e-vii — Forward-compatibility policy**
Document the Postel's Law contract for the persistence schema:
- **Tolerant readers:** unknown fields on a known event are silently ignored;
  unknown event types are silently skipped. This rule applies uniformly — both
  to new event variants and to new fields on existing variants.
- **Additive writers:** adding a new optional field to an existing event, or
  adding a new event type, is a non-breaking change and requires no migration.
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
`context.prev.jsonl` and `events.prev.jsonl`, restore `llmMessageLog` and the
event history, and continue as if the session had not ended.

Acceptance criteria (to be refined after schema lock):
- Startup detects a non-empty `context.prev.jsonl`
- User is prompted: resume previous session or start fresh
- On resume: `llmMessageLog` is restored from the context file; events file is
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
matching `tool_result`. If not, write a diagnostic and abort the turn rather than
sending malformed history. Circuit-breaker; real fix is REC-3.

Deprioritised: will be superseded by Step 3's event-list model.

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

### [OTHER] Provider/model architecture
`provider` (binary: anthropic/openai) + `activeModel` (string). Low priority until
more providers are added.

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
   the project's `README.md` (already read at startup) explicitly references a world
   state path (e.g. `plan/world-state.md`). If the README doesn't mention it, no world
   state is injected. This requires no new config format — the README is already the
   project orientation document.

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

- **Test-pollution prevention (layers a–e)** — Done. All five structural layers
  implemented: `bunfig.toml` preload sets `OMEGA_TEST=1` (layer a); `assertNotProductionPath()`
  hard-errors on production writes in tests (layer b); Agent constructor coerces
  `undefined` paths to `null` when `OMEGA_TEST=1` (layer c); `makeTestAgent()` factory
  in `src/test-utils.ts` (layer d); pre-commit grep for bare `new Agent()` in test
  files (layer e).
- **Shutdown decoupling** — Done. All fold-on-exit code removed from `app.ts` and
  `web/server.ts` (`foldCurrentSessionIntoWorldState`, `performWebShutdown`). Ctrl-C
  exits immediately. Shutdown ritual documented in `README.md ## Shutdown`.
- **Step 4: Retire pino** — Done. `src/logger.ts` deleted, `pino` package removed,
  `omega.log`/`omega.prev.log` removed from `.gitignore`. All infra-only events were
  already in `SessionEvent`.
- **Step 3e-i: Rename SessionEvent/AgentEvent variants** — Done. All 7 renames applied.
- **Step 3e-iii: FK/PK content-addressed context log** — Done (commit b6ef87c).
- **Step 3e-ii: Rename WsEvent variants** — Done. Pushed to `origin/develop`.
- **Merge dev → main (Steps 3a–3d)** — Done. `develop` merged into `main`; both branches now in sync.
- **Step 3d: Non-destructive context truncation** — Done (commit 997d7f7).
- **Step 3c: SessionEvent + dual-write event log** — Done (commit 357ec23).
- **Step 3b: `/compact` slash command** — Done (commit f2d5631).
- **Step 3a: Append-only context file** — Done (commit 551d676).
- **Step 2: Abandon automatic compaction** — Done. `compactAfterTurn()` removed.
- **Step 1: System prompt decoupling + README** — Done.
- **REC-1: Pre-commit test gate** — Done (commit b33ecff).
- **REC-0: Git-based known-good anchor** — Done. Two-branch model (`main`/`develop`).
- **WEB-5: Session persistence** — Done. `sessions/current.jsonl`.
- **WEB-4: Renderer parity** — Done (commit 538b717).
- **WEB-1/2/3: Bun WebSocket server + Solid.js client + Turn store** — Done (commit 99a9826).
- **WEB-0: Split `ui-raw.ts` into `src/terminal/` modules** — Done (commit 4183922).
- **TOOLS-3: Background process management** — Done (commit 9023c5a).
- **TOOLS-2: `find_files`** — Done.
- **TOOLS-1: `grep_files`** — Done.
- **FEAT-1: Parallel tool execution** — Done.
- **LOG-1: Redesign diagnostic/logging subsystem** — Done (commit 71e7dfc).
- **LOG-2: Complete event taxonomy renaming** — Done (commit f137610).
- **Diagnostic snapshots on fatal API errors** — Done (commit 61c4ebd).
- **Anthropic prompt caching** — Done.
- **Cache savings display** — Done.
- **Rate-limit retry** — Done.
- **OAuth auto-relogin** — Done.
- **Line editor cursor stuck on wrapped input** — Fixed (commit 892cbce).
- **Bracketed paste garbled display + O(n) append latency** — Fixed (commit 7344295).
- **Web UI stuck streaming after interrupted session** — Fixed (commit 87bca6d).
- **Terminal UI breakage not caught by tests** — Fixed (commit 467cdb8).
- **Stale compaction race** — Fixed (commit 8c3d9a3).
- **Tool output cap** — Fixed. `MAX_TOOL_OUTPUT_CHARS = 100_000` in `executeTool()`.
- **`truncateHistory` no-op on short-but-fat history** — Fixed.
- **Graceful handling of context-too-long 429** — Fixed.
