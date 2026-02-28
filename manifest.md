# High-level manifest for the further development of Omega

## AI-friendly software projects

Any modern software project should have a source repository that makes it easy
to develop it further with agentic AI.

A great way of achieving this is providing:

- A folder with with diagnostics.
  - Logs spanning considerable time periods. AI-friendly format!
  - Crash dumps, which cover ony the time near the crash, but with more Provide
    ainformation than the corresponding log entries. AI-friendly format!
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
- Even the in-memory structure might be build in this way: A main "log" of
  events, referencing context messages via a hash table.
- Omege should provide instructions to external agents (and its former stable
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

> Pino has been retired (Step 4, complete). The canonical event log is
> `sessions/events.jsonl` via `src/session-event.ts` — the single source of truth.

## Bootstrapping considerations

It is crucial that we structure the refactoring steps for Omega in such a way
that it becomes possible asap that we have a stable version of Omega which we
can use do develop the in-progress version of Omega. Currently, we have use Git
worktree, with a stable but old-fashioned version in `~/omega/main` and the
in-progress version at `~/omega/dev`.

## Step 3 sub-step breakdown

Step 3 is broken into four ordered sub-steps. The ordering prioritises **extending
Omega's practical capacity for long sessions** before completing the full architectural
vision. See `plan/backlog.md` for detailed acceptance criteria on each.

### 3a — Append-only context file (foundation)
Add `src/context-store.ts`. Append each `MessageParam` to `sessions/context.jsonl`
as it is pushed to `this.history`. No behaviour change; pure foundation for 3b–3d.

### 3b — `/compact` slash command (immediate capacity fix)
Operator-triggered mid-session compaction. Collapses the history head into an LLM
summary, preserves the last 10 message-pairs verbatim, rewrites the context file.
Directly addresses the context ceiling: a 60k-token session collapses to ~3–5k tokens
of summary + recent tail. Cache prefix is preserved from the summary message forward.

### 3c — SessionEvent type + dual-write event log
Define `SessionEvent` union type in `src/session-event.ts`. Append every agent event
to `sessions/events.jsonl`. Additive — establishes the canonical persistent event log
that will replace pino in Step 4.

### 3d — Non-destructive truncation (structural cache fix) — DONE (commit 997d7f7)
`truncateHistory` renamed to `buildApiMessages` — produces an ephemeral view for a
single API call; the source `llmMessageLog` is never mutated. `Agent.history` →
`Agent.llmMessageLog`; `getHistory()` → `getLlmMessageLog()`. Agentic loop uses
`apiBudget` (halved per prompt-too-long retry); no mutation of the canonical record.
Prompt cache prefix is never invalidated by truncation.

### 3e-i/ii/iii — Event renames + FK/PK contract — DONE (commits through b6ef87c)
All `SessionEvent`/`AgentEvent`/`WsEvent` discriminant strings renamed to the
coordinate-system model (`llm_call`, `llm_error`, `agent_error`, `turn_interrupted`,
etc.). `context.jsonl` entries now carry `hash` (SHA-256 8 hex chars of
`{ ts, role, content }`) and `ts`. `LlmCallEvent` carries `contextHashes: string[]`
— the ordered hashes of every message in the `buildApiMessages()` view sent.
Agent maintains a parallel `llmMessageHashes[]` array; `contextHashesForView()`
maps by object-reference identity.

### Pre-lock field removals — DONE (commit b59ba48)
Two breaking changes landed before the schema lock to avoid a post-lock migration:
- `LlmResponseEvent.content` removed — full assistant response was duplicating `context.jsonl`; join via next `llm_call`'s `contextHashes` instead.
- `LlmCallEvent.messageCount` removed — always equalled `contextHashes.length`; use `.length` directly.

### Schema lock — TODO (next)
Review and explicitly document the full shape of every JSONL record in
`sessions/context.jsonl` and `sessions/events.jsonl`. Write a schema reference
that serves as the stable contract for session resume and any future tooling.
No breaking changes after this point without a migration plan.
See `plan/backlog.md` § "Schema lock" for the ordered sub-steps (3e-iv through 3e-viii).

### 3f — Session resume — TODO (depends on schema lock)
On startup, if a `.prev` session exists, offer to resume it.

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
