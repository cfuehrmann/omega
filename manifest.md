# High-level manifest for the further development of Omega

## AI-friendly software projects

Any modern software project should have a source repository that makes it easy
to develop it further with agentic AI.

A great way of achieving this is providing:

- A folder with diagnostics.
  - Logs spanning considerable time periods. AI-friendly format!
  - Crash dumps, which cover only the time near the crash, but with more
    information than the corresponding log entries. AI-friendly format!
- AI-friendly instructions how to interpret that folder.

> In the particular case of the omega agent, we are aiming for a complete
> in-memory representation of the session history, which is mirrored in files to
> which we append every new event instantly. That session history can be shown
> in pretty form in the UI _and_, in persisted form, act as the diagnostic log.
> Thus, we have only one single source of truth.

## AI agents

Development on them should be AI-friendly, as for every other project.

On top of that, it should be possible to point an agent to whatever folder that
contains the the project to work on. _This is currently not the case for Omega!_

## Current state of the omega agent

During development so far, we mixed up the AI-friendlyness of omega's source
repo with it's nature as an AI agent. We partially coupled development of omega
to omega itself. This is reflected by the idiosyncratic system prompt, world
compaction, and turn compaction. Omega should be rewritten to separate the two
aspects.

## Contract authority — the most public contract wins

When multiple representations of the same information exist (persisted log, in-memory
event, WebSocket message, rendered UI block), they must all be consistent. When they
diverge — because of a rename, a new field, a schema change — **the most public
contract is authoritative and all others must conform to it**.

"Most public" means: the one that would cause the worst harm if broken. Concretely,
for Omega:

1. **Persistence** (`sessions/events.jsonl`, `sessions/context.jsonl`) — the most
   public contract. Existing log files on disk cannot be retroactively renamed.
   Any tooling, post-mortem script, or future session-resume feature depends on
   these exact field names and event type strings. A breaking change here requires
   explicit migration; an accidental one is silent data corruption.

2. **The in-memory event type** (`OmegaEvent` in `src/events.ts`) — derived from
   and must match the persistence contract. The canonical source of truth for the
   type system.

3. **The WebSocket protocol** (`WsEvent` in `src/web/client/store.ts`) — a
   transport-layer projection of `OmegaEvent`. It may carry extra ephemeral fields
   (e.g. `request` on `llm_call` for UI debugging) but must use the same type
   strings.

4. **The rendered UI** (terminal renderer, `App.tsx`) — the least public contract.
   Display format can change freely; it has no external consumers.

The rule: **when in doubt, update the UI to match the log — never the log to match
the UI.** This principle resolved the EU-3 naming question (`agent_to_agent_tool_call`
vs `tool_call`): the persisted name `tool_call` was already in `events.jsonl`, so
the stream-facing name was changed to match it, not the other way around.

Apply this principle to every future naming or schema decision.

---

## Major aspects of the redesign

- Abandon all compaction for now. Keep relying on prompt caching for token
  efficiency.
- Have the agent maintain a data structure that is primarily an event list, with
  some extras, that represents all interactions in the session so far.
- That data structure should be persisted to disk by appending every new event
  to the files involved in persistence.
- That data structure is for operation, diagnostics, _and_ visualization in the
  UI!
- About the structure of the persistence files: The context messages that go
  into every call to Anthropic should be in a separate file, as a time-ordered
  list. Each message to Anthrhopic should get as unique short hash, which acts
  as a "primary key" that can be referenced from the main event file.
- Even the in-memory structure might be built in this way: A main "log" of
  events, referencing context messages via a hash table.
- Omega should provide instructions to external agents (and its former stable
  self) like: "If you are pointed at this, I (Omega) have probably crashed. A
  high-level of my current state is in this markdown file: ... My future plans
  are in this markdown file: ... The diagnostics files are here: ..."
- Compaction into a world-state file should no longer be automatic, but manual.
- It should not result in context shortening, but in a "bookmark" at which point
  in the event file the compaction occurred.
- It must become possible to point Omega to point to any project to work on.
  - Concretely: world-state injection must be opt-in via the project's `README.md`,
    not hardcoded to `<cwd>/plan/world-state.md`. See `plan/backlog.md` §
    "Decouple Omega startup from Omega's own repo" for the detailed plan.

> The canonical event log is `sessions/events.jsonl` — the single source of truth.
> Pino was retired (Step 4, complete).

## Bootstrapping considerations

It is crucial that we structure the refactoring steps for Omega in such a way
that it becomes possible asap that we have a stable version of Omega which we
can use do develop the in-progress version of Omega. Currently, we have use Git
worktree, with a stable but old-fashioned version in `~/omega/main` and the
in-progress version at `~/omega/dev`.

## Step 3 sub-step breakdown

Step 3 prioritises **extending Omega's practical capacity for long sessions** before
completing the full architectural vision. See `plan/backlog.md` for detailed
acceptance criteria on each sub-step.

### Done: 3a through 3e-iii, EU-1 through EU-4
- **3a** — Append-only context file (`sessions/context.jsonl`). ✅
- **3b** — `/compact` slash command (operator-triggered mid-session compaction). ✅
- **3c** — `SessionEvent` type + dual-write event log (`sessions/events.jsonl`). ✅
- **3d** — Non-destructive truncation (later superseded: agent now sends `compactedContextHistory` verbatim; context overflow errors out immediately). ✅
- **3e-i/ii/iii** — Event renames + FK/PK contract: `context.jsonl` entries carry `hash` (SHA-256 8 hex chars of `{ ts, role, content }`) and `ts`; `llm_call` carries `contextHashes[]`. ✅
- **Pre-lock field removals** — `LlmResponseEvent.content`, `LlmCallEvent.messageCount`, `ToolCallEvent.input`, `ToolResultEvent.outputLength` all removed. ✅
- **EU-1–EU-4** — `AgentEvent` and `SessionEvent` unified into `OmegaEvent`; `status` variant deleted; all stream/wire/UI names match persistence; exhaustive switch guards enforced in both UIs. ✅

### Schema lock — TODO
Review and document the full shape of every JSONL record. Write `plan/schema.md` as
the stable contract for session resume and future tooling. No breaking changes after
this point without a migration plan. See `plan/backlog.md` § "Schema lock" for
ordered sub-steps (3e-iv through 3e-viii).

### 3f — Session resume — TODO (depends on schema lock)
On startup, if a `.prev` session exists, offer to resume it.

## In-turn context management policy

### The problem
Within a single agentic turn, the context window can grow due to accumulated tool
call/result pairs. Tool results are capped at `MAX_TOOL_OUTPUT_CHARS = 100_000`
chars each, but a turn with many tool calls — particularly file reads, grep results,
or command output — can still push the total context beyond the model's limit.

### Three candidate strategies (and why we rejected two)

**Compaction mid-turn — operationally undesirable.**
Compaction involves an LLM round-trip to summarise the history head. This is a
significant latency hit inserted into the middle of an agentic loop that is already
making progress. It also introduces a new failure mode (compaction error) mid-loop.
Compaction is appropriate at turn boundaries — natural pause points — but not
mid-loop. Rejected.

**Trimming mid-turn — complex and dangerous in edge cases.**
Trimming (drop-oldest messages to fit the budget) is cheap and synchronous, but
in the presence of large tool results it can drop so much early-turn context — the
user's original request, the first tool call that established state — that the model
produces garbage on the next API call. Trim-on-append also requires running the
`sanitizeToolPairs` logic on every append to avoid orphaned tool_use/tool_result
pairs. The complexity is real and the failure mode is silent. Rejected.

This applies equally to the **prompt-too-long retry loop** (halving `apiBudget` on
each attempt and recomputing `sentContext`). The halvings produce the same failure
mode: after enough halvings you may be sending only the tail — potentially just one
enormous tool result with no framing — and the model produces garbage silently.
The first prompt-too-long response from the API is already the signal that the
context is unsendable. Retrying with a progressively lobotomised view is no better
than silent trimming. The prompt-too-long retry loop is therefore also rejected and
will be removed.

**Error out — honest, simple, and actionable.**
There are two distinct `max_tokens` failure modes that must be handled differently:

**Mode A — Context overflow** (`stop_reason` comes back as a 400/429 "prompt too long"):
The accumulated history sent to the API exceeds the input context window.
Surface an explicit `agent_error` and stop the turn cleanly. The operator sees a
clear message ("Use /compact or start a focused turn"). The aborted turn's partial
history is already in `compactedContextHistory` and will be summarised by
auto-compact at the start of the next turn — so the session's memory of what was
attempted is preserved. A "more focused" follow-up turn is the correct recovery.
**This is the chosen approach for Mode A.**

**Mode B — Output budget exhaustion** (`stop_reason === "max_tokens"` during tool generation):
The model ran out of output tokens while generating a tool call's arguments —
most commonly a very large `write_file` content block. The context is *not* too
large; the *output* is. This is a task decomposition failure, not a context failure.
Recovery is not "start a more focused turn" — the context fits fine. Recovery is
"use a different strategy": write a skeleton with `write_file` then extend with
`edit_file`, never write a file longer than ~500 lines in one tool call.

Two defences against Mode B:
1. **Prevention** — `maxOutputTokens` is set to 32 768 (Sonnet 4.6 supports up to
   64K) to give generous headroom. The system prompt and `write_file` tool description
   both warn about the budget and recommend incremental strategies for large files.
2. **Recovery** — The BUG-1 guard (commit 9682be6) detects the dangling `tool_use`
   blocks, synthesises `tool_result` entries with `is_error: true` to keep the context
   well-formed, and emits an `agent_error` that explicitly names the budget limit and
   prescribes the incremental approach. The error message references `config.maxOutputTokens`
   so it is always accurate.

**This is the chosen approach.**

### Consequences (implemented — commit 13c1f9e)
- `buildSentContext()` is **deleted**. The agent sends `compactedContextHistory`
  verbatim on every API call. No ephemeral view, no trimming of any kind.
- `apiBudget`, `contextHashesForView()`, and `context_view_trimmed` are all deleted.
- The prompt-too-long retry loop (halving `apiBudget` on each attempt) is removed.
  On the first prompt-too-long response from the API, the turn errors out immediately
  with `llm_error` + actionable `agent_error` ("Use /compact to summarise history,
  or start a fresh focused turn.").
- Transient-error retries (rate limit 429, overload 529, 500/503) are kept — these
  are unrelated to context size; the context is fine and the server is just busy.
- Auto-compact fires at turn boundaries only — after the user message is appended,
  before the agentic loop. It never fires mid-loop.
- If a single turn genuinely exhausts the context window, the system emits
  `agent_error` with an actionable message and exits the loop cleanly.
- **If this approach turns out to be too aggressive in practice**, inspect
  `sessions/events.jsonl` and `sessions/context.jsonl` (join via `contextHashes` FKs)
  to understand the exact syndrome before introducing any trimming complexity.

## Input decoupling

### Immediate feature: Enter-on-empty pastes clipboard
When the user presses Enter with an empty input buffer, Omega reads the system
clipboard (via `wl-paste` on Wayland/Linux) and submits that text instead.
If the clipboard is also empty or whitespace, print a warning. The pasted text
is echoed to the terminal exactly as if the user had typed it. No hint is shown
at the prompt — the behaviour is silent but documented here.

Rationale: the operator prefers composing messages in a dedicated editor. The
workflow is: write in editor → copy → press Enter in Omega. This removes all
friction from that loop.

Platform: Wayland/Linux (`wl-paste`). WSL2 and macOS not supported yet; will
iterate if needed.

### Long-term goal: remove the built-in line editor
`src/terminal/input.ts` is ~400 lines of complex Unicode/ANSI line-editing code
(wide characters, wrapped-line redraws, bracketed paste, word-boundary movement).
Once the clipboard workflow is the primary input path, almost none of this is
exercised for real messages. Only slash commands (≤10 chars) need inline editing.

The long-term direction is to replace the full line editor with a minimal
raw-mode loop — or even Node's built-in `readline` — that handles only:
single-line input, backspace, Enter. ~50 lines instead of ~400.

This is a separate refactor, not part of the immediate feature. Prerequisites:
- Enter-on-empty clipboard paste is in place and working.
- The operator is satisfied that the clipboard workflow fully replaces inline
  editing for real messages.
- A deliberate decision is made to drop multi-line bracketed paste into the
  prompt (currently supported, would be removed).

Until then, `input.ts` remains unchanged. The immediate feature adds only a
clipboard-read code path in `app.ts`, touching nothing in `input.ts`.
