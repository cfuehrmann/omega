## Omega — State of the World

### Purpose

Omega is a general-purpose coding agent. It can be pointed at any project
directory and will orient itself by reading available documentation and project
files. The user interacts via terminal or web UI.

### Stack

TypeScript + Bun. The agent core (`src/agent.ts`) should have no UI imports
— UI and agent must stay cleanly separated. `StreamProvider` is the interface
for LLM provider calls (Anthropic, OpenAI); tests inject a mock — the real
LLM provider API is never called in tests. Config is code (`src/config.ts`).

NOTE: If `StreamProvider` is renamed, update this file too.

### Workspace Layout

`~/omega/` is a git workspace with three subdirectories: `main` (stable agent
codebase), `dev` (development version), and `plan`. To run the stable agent on
the dev project: `cd ~/omega/dev && bun run ~/omega/main/src/ui-raw.ts`.
`ui-raw.ts` is the CLI entry point; the web server entry point is
`src/web/server.ts`.

### Branch State

`develop` is the active branch. `main` lags behind and needs merging
periodically.

### Git & Gate Policy

Push to origin at least every 3 commits. **Run `just gate` before every
commit.** Gate = `test-core` + `test-browser` (run in parallel) + knip.
`just gate` is user-triggered — only run it when the user asks, not autonomously.

### Testing

- `just gate` — full suite + knip, run before every commit
- `just test` — test-core and test-browser in parallel
- `just test-fast` — `bun test --bail`, fast feedback during iteration
- `bun test src/foo.test.ts` — single file, preferred while iterating
- `just test-browser` — Playwright suite only (builds web client first)

`just web-build` bundles the Vite/SolidJS web client into `src/web/public/`.
It is not a general project build — backend/agent TypeScript is run directly
by Bun.

Prefer tests that exercise the full stack with real file I/O rather than
mocking away storage. Use a unique name per test run so tests can run in
parallel without conflicts. Let test artifacts accumulate — they become
inspectable evidence. Mock external services (LLMs, third-party APIs) but
always use real I/O with the dedicated test output path.

### Open Work

See `backlog/backlog.md`. Priority areas: SYSPROMPT-2 (system prompt pipeline
review, HIGH), SCHEMA-1–SCHEMA-7 (schema lock + session resume), ARCH-1 (clean
provider boundary), UX-1/UX-2 (abort/prompt queuing).

### Slash Commands

| Command    | Effect                                                               |
| ---------- | -------------------------------------------------------------------- |
| `/sonnet`  | Anthropic `claude-sonnet-4-6` (default)                              |
| `/opus`    | Anthropic `claude-opus-4-6`                                          |
| `/codex`   | OpenAI `gpt-5.2-codex`                                               |
| `/compact` | Collapse history head into LLM summary, keep last 10 turns verbatim |

Any other `/…` input is rejected with `agent_error`. Startup hint shows
`/sonnet /opus /codex /compact`.

### Reference Docs

Detailed reference material lives in `docs/`. Read on demand:
- `docs/internals.md` — event schemas, session model, test isolation, key files
- `docs/manifest.md` — design philosophy and strategic direction
- `docs/dev-policy.md` — active development-phase policies
- `docs/prompt-terminology.md` — operator/user/agent/LLM terminology
