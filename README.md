# Omega — AI Coding Agent

Omega is a coding agent that runs in a terminal. It uses Claude (Anthropic) and
optionally OpenAI models to read, write, and edit code, run commands, search the
web, and manage background processes — all driven by natural-language
conversation with a human operator.

## Quick start

```bash
bun install
bun run src/ui-raw.ts        # terminal UI
bun run src/web/server.ts    # web UI (build client first: bun run web:build)
```

## Stack

TypeScript + Bun. Raw terminal I/O (`src/terminal/`). Solid.js web client
(`src/web/client/`). No framework on the backend — just Bun's built-in HTTP +
WebSocket. Config is code (`src/config.ts`).

## Planning documents — read these at session start

| File | Purpose |
|------|---------|
| `plan/world-state.md` | LLM-compacted summary of all prior sessions. Loaded into the system prompt automatically. **Do not edit manually** — it is updated on clean shutdown. |
| `plan/future.md` | Issue tracker. Discrete, prioritised, actionable items. Update after completing work or making decisions worth recording. |
| `manifest.md` | High-level design manifest for Omega's ongoing refactoring. Consult for strategic direction. |
| `docs/lessons-learned.md` | Hard-won lessons about external APIs and protocols. Read before integrating with anything new. |

## Diagnosis

`diagnosis/` (gitignored) holds fatal API error snapshots written automatically.
Each file contains the exact request payload, history state, and error body.
If any files exist at session start, read them before doing anything else.

## Key source files

| File | Role |
|------|------|
| `src/agent.ts` | Agent core — agentic loop, streaming, compaction, tool dispatch |
| `src/config.ts` | Model selection, system prompt, token limits |
| `src/tools.ts` | All tool implementations; `MAX_TOOL_OUTPUT_CHARS = 100_000` cap applied to every result |
| `src/compaction.ts` | LLM compaction: `compactWorldState` (world-state fold on shutdown) and `compactHistory` (`/compact` command). |
| `src/context-store.ts` | Append-only session context file (`sessions/context.jsonl`) |
| `src/world-state.ts` | Read/write `plan/world-state.md` |
| `src/terminal/app.ts` | Terminal UI entry point |
| `src/terminal/input.ts` | Key parsing, line editing |
| `src/terminal/renderer.ts` | ANSI block renderers |
| `src/web/server.ts` | Web UI server |
| `src/web/client/` | Solid.js web client |

## Auth

Claude Max via OAuth PKCE (`claude.ai`). Falls back to `ANTHROPIC_API_KEY`.
OpenAI Codex fallback via `OPENAI_API_KEY` for `/codex` command.

## Slash commands

| Command | Effect |
|---------|--------|
| `/sonnet` | Anthropic `claude-sonnet-4-6` (default) |
| `/opus` | Anthropic `claude-opus-4-6` |
| `/codex` | OpenAI `gpt-5.2-codex` |
| `/compact` | Collapse history head into an LLM summary, keep last 10 turns verbatim |
| `/help` | Command list |

## Git discipline

- Two branches: `main` (stable) and `develop` (working). Omega commits to
  `develop` only. The operator merges `develop → main` when satisfied.
- `just gate` is the operator-run test gate — never invoke it automatically.
- Push to origin at least every 3 commits.
- If tests go red: stop, do not commit, fix before proceeding.
- Never commit or push red code.
- The `gh` CLI is installed and authenticated (`github.com/cfuehrmann/omega`,
  private). Use it for all GitHub operations.

## Testing discipline

- **Red-green** (bugs and features): Write a failing test first. Fix production
  code. Commit. Never write test + fix together.
- **Structural invariants** (refactors): Write a test that guards the invariant
  *before* making the change. See `entry.test.ts` for the pattern.
- Run tests: `bun test`. Run e2e: `just e2e`.
- A `Justfile` exists — run `just --list` for available recipes.

### Test isolation — never pollute production files

Tests must **never** write to `sessions/`, `diagnosis/`, `omega.log`, or any
other production file. The rule and the mechanism:

- `Agent` constructor: when a mock `streamProvider` is injected and no explicit
  path is given, `worldStatePath`, `diagDir`, and `contextFile` all default to
  `null` (disabled). Tests get isolation automatically — just pass a mock
  provider and omit the path arguments.
- `OMEGA_LOG_FILE` env var: redirect the pino log in tests that exercise the
  logger directly. The test infra sets this to a tmp path automatically for
  any test that imports `src/logger.ts`.
- e2e tests: use `sessions-test/` (not `sessions/`) via the fixture server in
  `e2e/fixtures/test-server.ts`.

**If you add a new production side-effect file** (any append/write that
`agent.ts` or other core code performs): follow the same pattern — add a
`filePath: string | null` parameter, disable on `null`, and apply the same
constructor heuristic (mock provider → null). Add an isolation test like the
ones in `agent-integration.test.ts` ("does not write to ... when ... is not given").

## Git hooks

A pre-commit hook runs `bun test --bail` before every commit, making it
mechanically impossible to commit broken code.

Install after cloning (or if the hook is missing):

```bash
just install-hooks
```

The hook source lives in `scripts/pre-commit` under version control.
To bypass in a genuine emergency: `git commit --no-verify`.

## Testing infrastructure

`StreamProvider` interface in `src/agent.ts` allows mock injection — real API is
never called in tests. `OMEGA_LOG_FILE` env var redirects logs in test mode.
