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

## AI-friendly software projects

Any software project should be easy to develop further with agentic AI:

- Provide durable, AI-readable diagnostic logs and crash artifacts.
- Structure tests so an agent can run a targeted subset rather than always
  running the full suite. Reserve the full suite for pre-commit gates.
- Keep orientation docs short and accurate. Stale docs are worse than no docs.

Omega applies these principles to itself: session logs are the diagnostic
artifact, `bun test src/foo.test.ts` targets a single file, and `just gate`
is the pre-commit gate.
