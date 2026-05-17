# Omega — AI Coding Agent

Omega is a general-purpose AI coding agent backed by Anthropic's Claude models (Sonnet and Opus). It orients itself by reading a project's documentation and files, then acts through a rich tool set: reading and writing files, running shell commands, searching the web, and more.

## Modes

- **CLI** — run headless from the terminal; suited for scripted or automated
  use. The agent loop runs, prints events to stdout, and exits when done.
- **Web UI** — a browser interface served locally (or over an SSH tunnel) that
  shows the full event stream in real time, lets you compose follow-up
  messages, and gives you context inspection and session history.
- **Benchmark harness** — a [Harbor](https://github.com/the-harbor-project/harbor)
  adapter (`bench/omega_agent.py`) embeds Omega into containerised benchmark
  runs (Terminal-Bench 2.0).

## Configuration

Omega reads API keys from environment variables. The recommended place to
store them is `~/.config/omega/.env` — this file is loaded automatically at
startup regardless of which directory you point Omega at:

```bash
mkdir -p ~/.config/omega
cat > ~/.config/omega/.env <<'EOF'
ANTHROPIC_API_KEY=sk-ant-...
BRAVE_SEARCH_API_KEY=BSA...
EOF
```

A project-level `.env` in the working directory is also supported and takes
precedence over the user config (useful for pointing at a local mock API, for
example). Real environment variables always win over both files.

## Quick start

**Requirements:** Rust stable toolchain (`rustup`), `trunk` for the web UI.

### CLI only

```bash
git clone https://github.com/cfuehrmann/omega
cd omega
cargo build --release -p omega-cli
./target/release/omega --help
```

Run the agent in a project directory:

```bash
cd /path/to/your/project
/path/to/omega run --instruction "Your task here" --effort medium
```

### Web UI

```bash
just server          # builds everything and serves on :3000
```

Then open `http://localhost:3000` in a browser. The server serves the web UI
and a WebSocket API on the same port. To access over SSH, forward port 3000
with `ssh -L 3000:localhost:3000 yourhost`.

To point the server at a different project directory, use `--working-dir`
(no `cd` required):

```bash
omega-server --working-dir /path/to/project
omega-server --working-dir /path/to/project --port 3033
```

The `--leptos-dir` flag is no longer needed in normal use — the server
resolves the frontend bundle automatically from the binary's own location.

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
