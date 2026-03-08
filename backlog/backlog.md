# Backlog — Issue Tracker

## Open items

### [SYSPROMPT] System prompt architecture

#### SYSPROMPT-1 — Modular system prompt folder `src/system-prompt/`

**Status: DONE** (commit 815c74b)

System prompt extracted from `src/config.ts` into `src/system-prompt/`:
- `identity.ts` — Claude Code prefix (oauth-conditional)
- `core.ts` — main instructions as readable prose template (args: `cwd`, `maxOutputTokens`)
- `append.ts` — absorbed `src/system-prompt-append.ts`; generic opt-in append mechanism
- `index.ts` — assembles all parts via `buildSystemPrompt(args)`
- 39 new tests in `system-prompt.test.ts`
- `config.systemPrompt` removed; `src/system-prompt-append.ts` deleted

#### SYSPROMPT-2 — Review the full system prompt assembly process

**Priority: HIGH — do after SYSPROMPT-1**

**Progress (session 2025-…):**
- Terminology nailed — see `docs/prompt-terminology.md` for the canonical
  definitions of operator, user, agent, model/LLM, and tools, plus pronoun
  conventions and the exo-suit metaphor.
- First terminology pass done on `core.ts` (two changes, not yet committed):
  - `"All tool calls are auto-approved. No confirmation needed."` →
    `"The operator has pre-approved all tool calls. No confirmation is needed."`
  - `"but use real I/O for your own storage"` →
    `"but always use real I/O with the dedicated test output path"`
- **Next:** review `.omega/system-prompt-append.md` (the operator-maintained
  world-state file injected at the bottom of every system prompt) with the
  same terminology lens. Then do a holistic review of the full assembled
  prompt before committing anything.

Now that the structure is in place, conduct a holistic review of the entire
system prompt assembly pipeline — not just the prose in `core.ts`, but every
step from disk read through to the string sent to the API:

- **`loadSystemPromptAppend()`** — when is it called? What happens if it is
  called multiple times, or not at all? Is the timing correct relative to
  `init()`?
- **`buildSystemPrompt()`** — called on every API call; is that the right
  granularity? Should any parts be computed once and cached?
- **`src/system-prompt/index.ts`** — is the assembly order (core → append)
  correct? Are the section headers clear to the model?
- **`formatAppendSection()`** — is `## World State (from previous sessions)`
  the best header? Does it correctly signal to the model how to treat that
  content?
- **`core.ts` prose** — review all instructions for accuracy and completeness.
  Known issue: "If a `.omega/system-prompt-append.md` file exists, it has
  already been injected above — do not re-read it." is misleading; the actual
  injected header is `## World State (from previous sessions)`, not the
  filename.
- **Prompt caching** — the system prompt is wrapped in a single
  `cache_control: ephemeral` block. Is one cache breakpoint enough? Are there
  cases where the cache is invalidated unexpectedly (e.g. `cwd` or
  `maxOutputTokens` changing between calls)?
- **OAuth identity prefix** — previously there was a Claude Code identity
  string prepended for OAuth. Verify whether this is still present, still
  required, and correctly placed.
- **Test coverage** — does `system-prompt.test.ts` cover the assembly
  integration end-to-end, or only individual parts in isolation?

Acceptance criteria:
- Every stage of the pipeline is understood and documented (inline comments
  and/or in `manifest.md`)
- All instructions in `core.ts` are accurate and verified against actual
  behaviour
- Misleading/stale text removed or corrected
- Any structural issues found (ordering, caching, timing) are either fixed
  here or broken out as new backlog items
- `planning-files.test.ts` updated if any sentinel strings change

---

### [DECOUPLE] Omega self-coupling — use on foreign repos

**Status: DONE**

Resolved by renaming `plan/world-state.md` → `.omega/system-prompt-append.md`
and extracting the read/write logic into `src/system-prompt/append.ts` (part
of the SYSPROMPT-1 modular system prompt refactor). The file is opt-in by
existence: if `.omega/system-prompt-append.md` is present, its contents are
appended to the system prompt; if absent, nothing is injected.
Foreign repos will not have this file and are therefore unaffected. System
prompt examples and docstrings updated to be project-neutral. INFRA-4 closed.

---

### [INFRA] `.omega/` namespace organisation

#### INFRA-5 — Move runtime artefacts into `.omega/runtime/`

**Priority: mid**

`.omega/` currently mixes two categories:
- **Authored/source-controlled:** `.omega/system-prompt-append.md` (operator-written context injected into the system prompt).
- **Generated/runtime:** `sessions/` and `test-sessions/` subdirectories written by Omega at runtime.

Move the generated artefacts under `.omega/runtime/` so the distinction is
explicit in the directory layout:

```
.omega/
  system-prompt-append.md   ← authored, source-controlled
  runtime/
    sessions/               ← was .omega/sessions/
    test-sessions/          ← was .omega/test-sessions/
```

Acceptance criteria:
- `SESSIONS_ROOT` in `src/session-dir.ts` updated to `.omega/runtime/sessions`.
- `TEST_SESSIONS_ROOT` updated to `.omega/runtime/test-sessions`.
- `assertNotProductionPath()` in `src/test-guard.ts` updated to match new paths.
- `.gitignore` updated if sessions were excluded there.
- All existing tests pass.
- Any existing `.omega/sessions/` data is noted as needing manual migration
  (one-time rename); no automated migration required.

---

### [SESSION] Session storage

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

Session folders should be renameable to meaningful names (e.g.
`implement-login-flow`) without breaking anything. This is the natural
session-labelling mechanism — no separate tagging concept needed.

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

After SCHEMA-1 through SCHEMA-5 are resolved, write `docs/schema.md`: the
definitive reference for every JSONL record in `sessions/context.jsonl` and
`sessions/events.jsonl`. Covers field names, types, required vs. optional, all
event variant names, and the forward-compatibility policy from SCHEMA-5. This
document is the stable contract that SCHEMA-7 builds on.

#### SCHEMA-7 — Session resume

**Depends on SCHEMA-6**

On startup, offer to resume the most recent previous session. The previous
session directory is found via `findPreviousEventsFile()` in
`src/session-dir.ts`. Load `context.jsonl` and `events.jsonl` from that
directory, restore `llmContextView` and the event history, and continue as if
the session had not ended.

Acceptance criteria:

- Startup detects a non-empty previous session via `findPreviousEventsFile()`
- User is prompted: resume previous session or start fresh
- On resume: `llmContextView` is restored from the context file; a new session
  dir is created but seeded with the restored history
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

**Status: DONE** — Resolved by the DECOUPLE work. `plan/world-state.md` has
been renamed to `.omega/system-prompt-append.md`. Loading is now opt-in by
file existence: present → injected, absent → nothing. Foreign repos are
unaffected.

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

### [TEST] Testing approach

#### TEST-1 — Evaluate snapshot testing for Omega

**Priority: mid**

Investigate whether snapshot testing is a good fit for Omega's output surfaces.
Candidate areas:

- **System prompt assembly** — `buildSystemPrompt()` output is a large string;
  snapshots could catch unintended changes from edits to `core.ts`, `identity.ts`,
  or `append.ts`.
- **Event rendering** — terminal ANSI output (`renderer.ts`) and web client
  HTML/JSX (`App.tsx`); snapshots would catch visual regressions.
- **JSONL record shapes** — `context.jsonl` and `events.jsonl` record formats;
  snapshots complement the schema lock work (SCHEMA-1–SCHEMA-6).
- **Tool output formatting** — `truncateOutput` and related display helpers.

Questions to answer:

- Does Bun's test runner have built-in snapshot support? If not, what library
  fits best (e.g. `jest-snapshot`, a custom serialiser)?
- How are snapshots stored and reviewed in code review? Are they committed to
  source control?
- What is the update workflow when a snapshot intentionally changes?
- Are there surfaces where snapshots would produce too much noise (e.g. outputs
  that embed timestamps or random IDs)?

Acceptance criteria:

- Short written evaluation (can live in `docs/` or inline as a backlog update)
  covering fit, tooling choice, and a recommendation (adopt / defer / skip)
- If adopted: at least one example snapshot test added to the codebase as a
  proof of concept

---

### [WEB] Web interface e2e tests — expand coverage

#### WEB-1 — Auto-scroll

Feed should scroll to bottom on new content. Not yet implemented or tested.
