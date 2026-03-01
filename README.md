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
| `plan/world-state.md` | LLM-compacted summary of all prior sessions. Loaded into the system prompt automatically. Updated as part of the shutdown ritual (see below). |
| `plan/backlog.md` | Issue tracker. Discrete, prioritised, actionable items. Update after completing work or making decisions worth recording. |
| `plan/dev-policy.md` | Active development-phase policies (temporary invariants and conventions). Read before implementing anything in the event/persistence layer. |
| `manifest.md` | High-level design manifest for Omega's ongoing refactoring. Consult for strategic direction. |
| `docs/lessons-learned.md` | Hard-won lessons about external APIs and protocols. Read before integrating with anything new. |

## Key source files

| File | Role |
|------|------|
| `src/agent.ts` | Agent core — agentic loop, streaming, compaction, tool dispatch |
| `src/config.ts` | Model selection, system prompt, token limits |
| `src/tools.ts` | All tool implementations; `MAX_TOOL_OUTPUT_CHARS = 100_000` cap applied to every result |
| `src/compaction.ts` | LLM compaction: `compactWorldState` (world-state fold) and `compactHistory` (`/compact` command). |
| `src/context-store.ts` | Append-only session context file (`sessions/context.jsonl`). Each record is a `ContextRecord` with `hash`, `ts`, `role`, `content`. `appendContextMessage()` returns the hash. `buildContextRecord()` computes hash without writing. |
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

Any other `/…` input is rejected with an error. Ask the LLM directly for help.

## Git discipline

- Two branches: `main` (stable) and `develop` (working). Omega commits to
  `develop` only. The operator merges `develop → main` when satisfied.
- **Run `just gate` before every commit.** Gate = unit tests + type check + e2e.
  Never commit without a green gate. `just gate` is not automatic — it must be
  run explicitly.
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

Tests must **never** write to `.omega/sessions/` or any other production file.

**Primary mechanism:** `makeTestAgent()` from `src/test-utils.ts` calls `makeSessionDir(now, TEST_SESSIONS_ROOT)` to write real session files under `.omega/test-sessions/` — isolation by path, not deletion. Each call gets a unique timestamped dir; `dispose()` is a no-op (sessions accumulate as inspectable artifacts). Returns `{ agent, sessionDir, contextFile, eventsFile, dispose }`. Tests use real session files — no null-path blind spots.

Belt-and-suspenders secondary layers: `OMEGA_TEST=1` preload via `bunfig.toml`; `assertNotProductionPath()` guard wired into all write functions; Agent constructor coercion; pre-commit grep for bare `new Agent()` in test files.

**If you add a new production side-effect file:** add `filePath: string | null`, wire `assertNotProductionPath()` into the write function, and add an isolation test.

## Git hooks

A pre-commit hook runs `bun test --bail` before every commit, making it
mechanically impossible to commit broken code.

Install after cloning (or if the hook is missing):

```bash
just install-hooks
```

The hook source lives in `scripts/pre-commit` under version control.
To bypass in a genuine emergency: `git commit --no-verify`.

## Shutdown

When the operator asks to wrap up or end the session, do the following — in order:

1. **Update `plan/world-state.md`** — use `compactWorldState()` from `src/compaction.ts`
   (or just rewrite the file directly) to fold this session's work into a concise,
   accurate summary of the current state of the project.
2. **Update `plan/backlog.md`** — mark completed items done, add newly discovered work,
   remove anything that is no longer relevant.

Ctrl-C exits immediately without doing any of this. The shutdown ritual only happens
when the operator explicitly asks for it.

## Testing infrastructure

`StreamProvider` interface in `src/agent.ts` allows mock injection — real API is
never called in tests.
