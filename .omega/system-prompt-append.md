## Omega — State of the World

### Purpose

Omega is a general-purpose coding agent. It can be pointed at any project
directory and will orient itself by reading available documentation and project
files. The user interacts via terminal or web UI.

### Stack

TypeScript + Bun. The agent core (`src/agent.ts`) should have no UI imports — UI
and agent must stay cleanly separated. `StreamProvider` is the interface for LLM
provider calls (Anthropic, OpenAI); tests inject a mock — the real LLM provider
API is never called in tests. Config is code (`src/config.ts`).

NOTE: If `StreamProvider` is renamed, update this file too.

### Branch State

All development work goes on `develop`. See `docs/dev-policy.md` for branching
strategy.

### Git & Gate Policy

**Run `just gate` before every commit.**

### Testing

- `just gate` — full suite + knip, run before every commit
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

### Open Work

See `backlog/backlog.md`.

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

### Reference Docs

Detailed reference material lives in `docs/`. Read on demand:

- `docs/internals.md` — event schemas, session model, test isolation, key files
- `docs/manifest.md` — design philosophy and strategic direction
- `docs/dev-policy.md` — active development-phase policies
- `docs/prompt-terminology.md` — operator/user/agent/LLM terminology
