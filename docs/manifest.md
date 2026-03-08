# Omega — Manifest

## Goals

- Be a general-purpose coding agent that can be pointed at any project
  directory and work effectively without project-specific hardcoding.
- Produce inspectable, durable session artifacts (append-only JSONL logs)
  that serve as both operational state and diagnostics.

## Non-goals

- Omega is not tied to its own repo. It should work equally well on any
  project.
- No mid-turn context trimming or compaction. Send history verbatim; error
  out cleanly if the context is too large.

## Contract authority — the most public contract wins

When multiple representations of the same information exist, the most public
one is authoritative and all others conform to it. For Omega:

1. **Persistence** (`events.jsonl`, `context.jsonl`) — most public. Breaking
   changes require explicit migration.
2. **In-memory event type** (`OmegaEvent` in `src/events.ts`) — must match
   persistence.
3. **WebSocket protocol** (`WsEvent`) — transport projection of `OmegaEvent`;
   may carry extra ephemeral fields.
4. **Rendered UI** — least public; can change freely.

Rule: update the UI to match the log — never the log to match the UI.

## AI-friendly software projects

Any software project should be easy to develop further with agentic AI:

- Provide durable, AI-readable diagnostic logs and crash artifacts.
- Structure tests so an agent can run a targeted subset rather than always
  running the full suite. Reserve the full suite for pre-commit gates.
- Keep orientation docs short and accurate. Stale docs are worse than no docs.

Omega applies these principles to itself: session logs are the diagnostic
artifact, `bun test src/foo.test.ts` targets a single file, and `just gate`
is the pre-commit gate.
