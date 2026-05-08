# Omega — AI Coding Agent

Omega is a general-purpose AI coding agent. It supports multiple LLM backends (Anthropic, Ollama) and is primarily developed and benchmarked against Sonnet and Opus. It orients itself
by reading a project's documentation and files, then acts through a rich tool
set: reading and writing files, running shell commands, searching the web, and
more.

## Modes

- **CLI** — run headless from the terminal; suited for scripted or automated
  use. The agent loop runs, prints events to stdout, and exits when done.
- **Web UI** — a browser interface served locally (or over an SSH tunnel) that
  shows the full event stream in real time, lets you compose follow-up
  messages, and gives you context inspection and session history.
- **Benchmark harness** — a [Harbor](https://github.com/the-harbor-project/harbor)
  adapter (`bench/omega_agent.py`) embeds Omega into containerised benchmark
  runs (Terminal-Bench 2.0, SWE-Bench Verified, etc.).

## Quick start

**Requirements:** Rust stable toolchain (`rustup`), `trunk` for the web UI.

### CLI only

```bash
git clone https://github.com/cfuehrmann/omega
cd omega
cargo build --release -p omega-cli
./rust/target/release/omega --help
```

Run the agent in a project directory:

```bash
cd /path/to/your/project
/path/to/omega --max-turns 50 --effort medium
```

### Web UI

```bash
just server          # builds everything and serves on :3000
```

Then open `http://localhost:3000` in a browser. The server serves the web UI
and a WebSocket API on the same port. To access over SSH, forward port 3000
with `ssh -L 3000:localhost:3000 yourhost`.

Run `just --list` to see all available recipes.

## Philosophy

Omega produces append-only JSONL session logs that serve as both operational
state and diagnostics. These logs are inspectable by humans and machines alike,
accelerating the agentic development loop — the same artifact that drives the
session is the post-mortem record when something goes wrong. The web UI
reflects every event in the log, so the operator always has full visibility at
runtime.

> Introspectability of any software system — at runtime and after the fact —
> is a first-class design goal in the age of agentic AI.

## Project layout

| Path | Contents |
|---|---|
| `rust/crates/omega-cli/` | CLI binary |
| `rust/crates/omega-server/` | HTTP + WebSocket server |
| `rust/crates/omega-agent/` | Core agent loop |
| `rust/crates/omega-core/` | LLM provider abstraction (Anthropic, Ollama) |
| `rust/crates/omega-tools/` | Tool implementations |
| `rust/crates/omega-store/` | Session persistence |
| `rust/crates/omega-protocol/` | Shared event/message types |
| `frontends/leptos/` | Web UI (Rust → WASM via Trunk) |
| `bench/` | Terminal-Bench harness and results |
| `docs/` | Architecture, policies, internals |
| `backlog/` | Work items and planning docs |

## Git discipline

- Active branch: `develop`. Merge to `main` when stable.
- **The gate runs automatically as the pre-commit hook.** Always commit with
  `git add -A && git commit -m "..."` — `git add -A` stages everything (new,
  modified, deleted) so the hook actually runs. Do not use `git commit -a`:
  it silently skips new untracked files. Exit code 0 = committed and gate
  passed. Non-zero = the hook prints the last 60 lines of
  `test-output/gate-latest.log`; open the full log only if that isn't enough.
- Push to origin at least every 3 commits.
- Never commit red code.
- Install the hook once with `just install-hooks`.
