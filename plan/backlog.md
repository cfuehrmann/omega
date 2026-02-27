# Future — Issue Tracker

## Open items

### [REFACTOR] Manifest-driven redesign — making Omega project-agnostic
**Priority: HIGHEST — ongoing, guided by `manifest.md`**

#### Step 3e — Stable persistence contract (event completeness + schema lock)
**Status: IN PROGRESS**

Review the full persistence layer and reach explicit agreement on every aspect
before building session-resume on top of it. Covers:

1. **Event completeness:** Currently not persisted: `status` (intentionally — ephemeral
   UI noise), `metrics` (per-API-call; `turn_end` captures aggregate), `tool_result_message`
   (individual `tool_result` events are persisted). Decide whether any should be added.

2. **UI reflection:** Terminal renders `status`, `text`, `tool_result_message`, `metrics`
   (not persisted). Event log has `session_start` (not rendered). Guiding principle:
   "anything that could matter for a post-mortem should be persisted; pure streaming
   scaffolding need not be."

3. **Schema lock:** Review and agree on the exact shape of every JSONL record in
   `sessions/context.jsonl` and `sessions/events.jsonl` — field names, types, required
   vs. optional fields, all event variant names. The goal is a stable contract that
   won't need breaking changes when session-resume is built on top.

##### Step 3e-i — Rename SessionEvent and AgentEvent variants
**Status: DONE**

Rename the following `SessionEvent` discriminant strings (and the matching
`AgentEvent` yield sites in `agent.ts`) to their agreed names:

| Old name | New name |
|---|---|
| `api_call_start` | `llm_call` |
| `api_error` | `llm_error` |
| `error` | `agent_error` |
| `interrupted` | `turn_interrupted` |
| `oauth_reauthed` | `oauth_refreshed` |
| `api_retry` | `llm_retry` |
| `context_truncated` | `context_view_trimmed` |

Scope: `src/session-event.ts`, `src/agent.ts`, `src/terminal/app.ts`,
and all test files. Does **not** touch the `WsEvent` union in
`src/web/client/store.ts` or the server's own `{ type: "error" }` sends —
those are a separate WebSocket protocol layer (see Step 3e-ii).

Acceptance criteria:
- All 7 renames applied consistently across every call site
- `bun test` passes
- No `api_call_start`, `api_error`, `"error"` (as SessionEvent), `"interrupted"`,
  `oauth_reauthed`, `api_retry`, or `context_truncated` strings remain in
  `src/session-event.ts` or `src/agent.ts`

##### Step 3e-ii — Rename WsEvent variants to match (web protocol)
**Status: TODO — depends on Step 3e-i**

After 3e-i is done, apply the same renames to the WebSocket protocol layer:
the `WsEvent` union in `src/web/client/store.ts`, the switch cases that
consume it, the e2e tests in `e2e/web-ui.spec.ts`, and the server sends in
`src/web/server.ts` that originate from `agent.sendMessage()`.

The server's *own* protocol error sends (invalid JSON, turn-already-in-progress,
catch blocks) use `{ type: "error" }` as a server→client signal — decide
whether those should also become `agent_error` or stay as `error`.

Acceptance criteria:
- `WsEvent` union uses the new names
- All switch cases and e2e assertions updated
- `bun test` and `just e2e` pass

##### Step 3e-iii — FK/PK contract: content-addressed context log
**Status: TODO — depends on Step 3e-ii**

Each `MessageParam` written to `context.jsonl` gets a content hash as its
primary key. Each `llm_call` event in `events.jsonl` carries a `contextHashes`
array — the ordered list of hashes of every message actually sent with that
API call. This makes the exact prompt sent to the LLM auditable and recoverable
for any call, including calls where `buildApiMessages()` produced a truncated
view that dropped older messages from `llmMessageLog`.

**Design decisions (agreed):**

- **Hash input:** the full JSON-serialised `MessageParam` record *plus* its `ts`
  timestamp (written at append time). Including `ts` prevents hash collisions
  between identical messages sent at different times (e.g. "ok" twice in one
  session). The `ts` is written first; the hash is computed from the stored
  record — no flakiness from re-computing timestamps.
- **Hash algorithm:** SHA-256 truncated to 8 hex chars. Collision risk is
  negligible for a session log.
- **Hash computed from the view, not `llmMessageLog`:** `contextHashes` must
  reflect the messages as passed to `buildApiMessages()` output (the truncated
  view actually sent), not the full canonical history. This is the critical
  correctness constraint.
- **Natural IDs elsewhere:** `tool_call`/`tool_result` already have `tool_use_id`
  (Anthropic UUID — reliable natural key). `session_start` has `sessionId`.
  `callNumber` on `llm_call` is NOT a reliable unique key — retries within the
  same outer loop iteration reuse the same `callNumber` and emit duplicate
  `llm_call` events. `contextHashes` is the correct cross-reference mechanism.
- **Tool result content:** hashed *after* the 100k truncation cap applied by
  `executeTool()` — the hash reflects what was actually in the `MessageParam`
  sent to the API.

**Changes required:**
- `context.jsonl` entries: add `hash: string` and `ts: string` fields alongside
  the existing `MessageParam` fields
- `llm_call` SessionEvent: add `contextHashes: string[]` field
- `appendContextMessage()` in `context-store.ts`: compute and store hash at
  write time
- Agentic loop: after `buildApiMessages()` produces the view, compute
  `contextHashes` from that view and include in the `llm_call` event/logEvent
- `LlmCallEvent` type in `session-event.ts`: add `contextHashes` field

**Testing discipline — chaotic scenarios required:**
- Truncation fires on retry 2 but not retry 1: verify `contextHashes` arrays differ
- Tool loop where the view shrinks mid-loop: verify each `llm_call` hashes match
  its actual view
- Identical message content sent twice: verify different hashes (due to `ts`)
- Retry within outer loop iteration: verify duplicate `llm_call` events have
  identical `contextHashes` (same view was sent)

Acceptance criteria:
- `context.jsonl` entries have `hash` and `ts` fields
- `llm_call` events have `contextHashes: string[]`
- Hashes computed from the view sent, not from `llmMessageLog`
- All chaotic test scenarios pass
- `bun test` passes

##### Schema lock
**Status: TODO — follows Step 3e-iii**

Review and explicitly document the full shape of every JSONL record in
`sessions/context.jsonl` and `sessions/events.jsonl`. Write a schema reference
(in `plan/` or inline in source) that serves as the stable contract for
session resume and any future tooling. No breaking changes after this point
without a migration plan.

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

- **Shutdown decoupling** — Done. All fold-on-exit code removed from `app.ts` and
  `web/server.ts` (`foldCurrentSessionIntoWorldState`, `performWebShutdown`). Ctrl-C
  exits immediately. Shutdown ritual documented in `README.md ## Shutdown`.
- **Step 4: Retire pino** — Done. `src/logger.ts` deleted, `pino` package removed, `omega.log`/`omega.prev.log` removed from `.gitignore`. All infra-only events (`oauth_reauthed`, `oauth_token_expired`, `context_truncated`, `api_retry`, `diagnostic_written`) were already in `SessionEvent`. 422 tests pass.
- **Step 3e-i: Rename SessionEvent/AgentEvent variants** — Done. All 7 renames applied (`api_call_start`→`llm_call`, `api_error`→`llm_error`, `error`→`agent_error`, `interrupted`→`turn_interrupted`, `oauth_reauthed`→`oauth_refreshed`, `api_retry`→`llm_retry`, `context_truncated`→`context_view_trimmed`). 422 tests pass.
- **Merge dev → main (Steps 3a–3d)** — Done. `develop` merged into `main`; both branches now in sync.
- **Step 3d: Non-destructive context truncation** — Done (commit 997d7f7).
  `buildApiMessages()` is purely ephemeral; `llmMessageLog` never mutated.
- **Step 3c: SessionEvent + dual-write event log** — Done (commit 357ec23). 12-variant
  discriminated union; `sessions/events.jsonl`.
- **Step 3b: `/compact` slash command** — Done (commit f2d5631). `compactHistory()` in
  `src/compaction.ts`; handler in `agent.ts`.
- **Step 3a: Append-only context file** — Done (commit 551d676). `sessions/context.jsonl`.
- **Step 2: Abandon automatic compaction** — Done. `compactAfterTurn()` removed.
- **Step 1: System prompt decoupling + README** — Done. Project-agnostic system prompt;
  `README.md` created.
- **REC-1: Pre-commit test gate** — Done (commit b33ecff). `scripts/pre-commit` + `just
  install-hooks`.
- **REC-0: Git-based known-good anchor** — Done. Two-branch model (`main`/`develop`).
- **WEB-5: Session persistence** — Done. `sessions/current.jsonl`.
- **WEB-4: Renderer parity** — Done (commit 538b717).
- **WEB-1/2/3: Bun WebSocket server + Solid.js client + Turn store** — Done (commit
  99a9826).
- **WEB-0: Split `ui-raw.ts` into `src/terminal/` modules** — Done (commit 4183922).
- **TOOLS-3: Background process management** — Done (commit 9023c5a).
- **TOOLS-2: `find_files`** — Done.
- **TOOLS-1: `grep_files`** — Done.
- **FEAT-1: Parallel tool execution** — Done.
- **LOG-1: Redesign diagnostic/logging subsystem** — Done (commit 71e7dfc). Pino +
  simplified snapshots.
- **LOG-2: Complete event taxonomy renaming** — Done (commit f137610).
- **Diagnostic snapshots on fatal API errors** — Done (commit 61c4ebd).
- **Anthropic prompt caching** — Done. Three cache breakpoints; `estimateCostWithCache()`.
- **Cache savings display** — Done. Turn footer shows `cost:` and `saved:`.
- **Rate-limit retry** — Done. Provider-aware exponential backoff.
- **OAuth auto-relogin** — Done.
- **Line editor cursor stuck on wrapped input** — Fixed (commit 892cbce).
- **Bracketed paste garbled display + O(n) append latency** — Fixed (commit 7344295).
- **Web UI stuck streaming after interrupted session** — Fixed (commit 87bca6d).
- **Terminal UI breakage not caught by tests** — Fixed (commit 467cdb8).
- **Stale compaction race** — Fixed (commit 8c3d9a3). `compactionQueue` chain.
- **Tool output cap** — Fixed. `MAX_TOOL_OUTPUT_CHARS = 100_000` in `executeTool()`.
- **`truncateHistory` no-op on short-but-fat history** — Fixed. `buildApiMessages`
  drops from oldest end when all messages fall within the "always keep" tail.
- **Graceful handling of context-too-long 429** — Fixed. `isContextTooLong()` helper;
  excluded from retry; clear actionable message shown.
