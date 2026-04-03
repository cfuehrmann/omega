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

## Controls

| Where         | Control                | Effect                                          |
| ------------- | ---------------------- | ----------------------------------------------- |
| Session bar   | Auth mode dropdown     | Switch between Claude Max and API Key           |
| Session bar   | Model dropdown         | Switch between `claude-sonnet-4-6` and `claude-opus-4-6` |
| Slash command | `/compact`             | Collapse history head into an LLM summary, keep last 10 turns verbatim |

## Git discipline

- Active branch: `develop`. Merge to `main` when stable.
- **The gate runs automatically as the pre-commit hook.** Always commit with
  `git add -A && git commit -m "..."` — `git add -A` stages everything (new,
  modified, deleted) so the hook actually runs. Do not use `git commit -a`:
  it silently skips new untracked files. Exit code 0 = committed and gate
  passed. Non-zero = read the tail of the background log first; only open
  `test-output/gate-latest.log` when that tail confirms the gate ran and
  failed (it is stale otherwise).
- Push to origin at least every 3 commits.
- Never commit red code.

## Task runner

A `Justfile` exists — run `just --list` for available recipes.
