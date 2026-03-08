## Omega — State of the World

### Purpose

Omega is a general-purpose coding agent. It can be pointed at any project
directory and will orient itself by reading available documentation and project
files. The user interacts via terminal or web UI.

### Stack

- Agent core (`src/agent.ts`) must have no UI imports — UI and agent stay cleanly separated.
- `StreamProvider` is the interface for LLM provider calls; tests inject a mock — the real API is never called in tests. **If `StreamProvider` is renamed, update this file too.**

### Workflow tools

- Use `gh` (not raw `curl`) for GitHub operations — it's authenticated as `cfuehrmann` with `repo` scope. `gh pr create`, `gh issue list`, `gh release create`, `gh auth status`, etc. Still use `git` for push/pull.

### Branch State

All development work goes on `develop`. Merge to `main` when stable.

### Testing

- `just gate` — **run before every commit** — full suite + knip
- `just test` — test-core and test-browser in parallel (outputs printed
  sequentially)
- `just test-fast` — `bun test --bail`, fast feedback during iteration
- `bun test src/foo.test.ts` — single file, preferred while iterating
- `just test-browser` — Playwright suite only (builds web client first)

`just web-build` bundles the Vite/SolidJS web client into `src/web/public/`. It
is not a general project build — backend/agent TypeScript is run directly by
Bun.

Prefer tests that exercise the full stack with real file I/O rather than mocking
away storage. Use a unique output path (e.g. timestamp + random suffix) per test
run so tests can run in parallel without conflicts. Let test artifacts
accumulate — they become inspectable evidence. Mock external services (LLMs,
third-party APIs) but always use real I/O with the dedicated test output path.

### Contract Authority — the most public contract wins

When multiple representations of the same information exist, the most public one
is authoritative and all others conform to it. For Omega:

1. **Persistence** (`events.jsonl`, `context.jsonl`) — most public. Breaking
   changes require explicit migration.
2. **In-memory event type** (`OmegaEvent` in `src/events.ts`) — must match
   persistence.
3. **WebSocket protocol** (`WsEvent`) — transport projection of `OmegaEvent`;
   may carry extra ephemeral fields.
4. **Rendered UI** — least public; can change freely.

Rule: update the UI to match the log — never the log to match the UI.


