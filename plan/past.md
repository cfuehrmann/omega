# Past — Decisions and Outcomes

Compressed record of what shaped the current state. Distill, don't accumulate.

## Stack and architecture

TypeScript + Bun: native TS, fastest startup for self-restart loop. Ink for
the terminal UI. Agent core (`agent.ts`) never imports React/Ink — clean
separation. Config is code (`config.ts`), no YAML/JSON.

The agent is an async generator (`sendMessage`) that emits typed `AgentEvent`
values. The UI consumes the stream; the agent knows nothing about rendering.
`StreamProvider` interface allows test injection of a mock provider — real
Anthropic client never called in tests.

## Auth

Claude Max via OAuth PKCE through `claude.ai` — not the pay-per-token API.
System prompt must be prefixed with the Claude Code identity string when using
OAuth, or the request is rejected. Full details in `docs/oauth-pitfall.md`.
Falls back to `ANTHROPIC_API_KEY` env var for pay-per-token.

## Tools and trust

Tools: `read_file`, `write_file`, `edit_file`, `list_files`, `run_command`,
`web_search`, `fetch_url`. All auto-approved — no allowlist. Dropped the
allowlist (was brittle, parsing unreliable) in favour of full auto-approve
with a tool audit summary shown at the end of each turn.

`web_search` uses DuckDuckGo Instant Answer API (no key needed), falls back
to HTML scrape. `fetch_url` strips HTML to plain text. Both use
`rejectUnauthorized: false` — Bun's TLS doesn't trust the system CA on this
machine.

`gh` CLI is installed and authenticated as `cfuehrmann`
(github.com/cfuehrmann/omega, private). Use it for all GitHub operations.

## Self-modification loop

Agent edits `src/`, runs `bun test`, commits on green, reverts on red,
restarts itself. Red-green discipline is mandatory and enforced in the system
prompt: test must fail first, then fix, then confirm all pass.

## Context management

Truncation (drop oldest messages, keep system + recent) rather than
summarisation. Token budget: 100k. `edit_file` for surgical edits to avoid
rewriting whole files.

## UI (Ink)

Ink owns only the bottom region of the terminal — not the full screen. Content
that scrolls into the terminal's scrollback is outside Ink's control forever.
This makes collapsible/expandable history impossible in Ink.

Log-style layout: time column (HH:MM:SS) on the left of every block.
Three visual prominence levels via color:
- User prompts: bright green bold `───` separator + text (strongest)
- API requests (cyan): pseudo-JSON showing model, system size, tool names,
  message summaries (not full content)
- API responses (blue): pseudo-JSON showing stop_reason, usage, content blocks
- Tool calls (yellow): formatted call + result preview
- Status bar: model, auth, session tokens, cost, Δ tok, Esc hint

`api_response` AgentEvent added (stop_reason, usage, content blocks).
`turn_end` AgentEvent aggregates all tool calls and metrics across a turn.
Tool audit summary shown once at turn end, not per-API-call.

## TUI alternatives researched (2025)

Ink cannot own the full screen. Alternatives:
- **OpenTUI** — Zig core, TypeScript/React bindings, 8.8k stars, powers
  OpenCode. Requires Zig to build. Pre-1.0. Most promising; revisit when stable.
- **unblessed** — TypeScript blessed rewrite, alpha, 6 stars. Too thin community.
- **Ratatui** (Rust) — two languages to maintain.
- **Browser UI** — best long-term flexibility, larger upfront cost.
Decision: stay with Ink. Revisit OpenTUI or browser UI when capability
outgrows the current display model.

## Planning system

Replaced `plan/overview.md` + `plan/ui.md` with `plan/past.md` (this file),
`plan/future.md` (issue tracker), `plan/present.md` (current work).
Maintenance process in system prompt: read all three at session start, update
at session end. Past is compressed not accumulated.
