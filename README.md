# Omega — AI Coding Agent

Omega is a general-purpose coding agent that orients itself by reading a
project's documentation and files. The user interacts via a web UI served
locally or over an SSH tunnel.

## Philosophy

Omega produces append-only JSONL session logs that serve as both operational
state and diagnostics. These logs are inspectable by humans and machines alike,
accelerating the agentic development loop — the same artifact that drives the
session is the post-mortem record when something goes wrong. The web UI reflects
every event in the log, so the operator always has full visibility at runtime.

> In fact, introspectability of any software system — at runtime and after the
> fact — is a first-class design goal in the age of agentic AI.

## Quick start

```bash
bun install
just web-build
just server          # web UI on :3000
```

## Stack

TypeScript + Bun. SolidJS web client in `src/web/client/`. No backend framework
— Bun's built-in HTTP + WebSocket. Agent core in `src/agent.ts`. Config is code
(`src/config.ts`).

## Project layout

- `src/` — all source code
- `backlog.md` — work items and planning docs for specific features
- `docs/` — reference material: architecture, policies, terminology, internals

## For contributors

`.omega/system-prompt-append.md` is injected into the agent's system prompt at
every session start. It also contains operational policies worth reading: test
commands, testing philosophy, the contract authority hierarchy, and
agent-specific constraints.

## Slash commands

| Command    | Effect                                                                 |
| ---------- | ---------------------------------------------------------------------- |
| `/sonnet`  | Anthropic `claude-sonnet-4-6` (default)                                |
| `/opus`    | Anthropic `claude-opus-4-6`                                            |
| `/codex`   | OpenAI `gpt-5.2-codex`                                                 |
| `/compact` | Collapse history head into an LLM summary, keep last 10 turns verbatim |

## Git discipline

- Active branch: `develop`. Merge to `main` when stable.
- **Run `just gate` before every commit.** Gate = full test suite + knip.
- Push to origin at least every 3 commits.
- Never commit red code.

## Task runner

A `Justfile` exists — run `just --list` for available recipes.
