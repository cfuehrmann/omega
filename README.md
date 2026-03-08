# Omega — AI Coding Agent

Omega is a general-purpose coding agent. It can be pointed at any project
directory and will orient itself by reading available documentation and project
files. The user interacts via terminal or web UI.

## Quick start

```bash
bun install
bun run src/ui-raw.ts        # terminal UI
bun run src/web/server.ts    # web UI (build client first: just web-build)
```

## Stack

TypeScript + Bun. Terminal UI in `src/terminal/`. SolidJS web client in
`src/web/client/`. No backend framework — Bun's built-in HTTP + WebSocket.
Agent core in `src/agent.ts`. Config is code (`src/config.ts`).

## Project layout

- `src/` — all source code
- `backlog/` — work items and planning docs for specific features
- `docs/` — reference material: architecture, policies, terminology, internals

## Slash commands

| Command    | Effect                                                               |
| ---------- | -------------------------------------------------------------------- |
| `/sonnet`  | Anthropic `claude-sonnet-4-6` (default)                              |
| `/opus`    | Anthropic `claude-opus-4-6`                                          |
| `/codex`   | OpenAI `gpt-5.2-codex`                                               |
| `/compact` | Collapse history head into an LLM summary, keep last 10 turns verbatim |

## Git discipline

- Active branch: `develop`. Merge to `main` when stable.
- **Run `just gate` before every commit.** Gate = full test suite + knip.
- Push to origin at least every 3 commits.
- Never commit red code.

## Task runner

A `Justfile` exists — run `just --list` for available recipes.
