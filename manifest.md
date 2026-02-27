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

> Obviously, we should be able to abandon pino, since we will roll our own file
> format as a single source of truth.

## Bootstrapping considerations

It is crucial that we structure the refactoring steps for Omega in such a way
that it becomes possible asap that we have a stable version of Omega which we
can use do develop the in-progress version of Omega. Currently, we have use Git
worktree, with a stable but old-fashioned version in `~/omega/main` and the
in-progress version at `~/omega/dev`.

## Step 3 sub-step breakdown

Step 3 is broken into four ordered sub-steps. The ordering prioritises **extending
Omega's practical capacity for long sessions** before completing the full architectural
vision. See `plan/future.md` for detailed acceptance criteria on each.

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

### ~~3d — Non-destructive truncation (structural cache fix)~~ — DONE (commit 997d7f7)
`truncateHistory` renamed to `buildApiMessages` — produces an ephemeral view for a
single API call; the source `llmMessageLog` is never mutated. `Agent.history` →
`Agent.llmMessageLog`; `getHistory()` → `getLlmMessageLog()`. Agentic loop uses
`apiBudget` (halved per prompt-too-long retry); no mutation of the canonical record.
Prompt cache prefix is never invalidated by truncation.
