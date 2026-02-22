# Past — Decisions and Outcomes

Compressed record of what shaped the current state. Distill, don't accumulate.

## Stack and architecture

TypeScript + Bun: native TS, fastest startup for self-restart loop. Raw
terminal I/O for the UI (no library). Agent core (`agent.ts`) has no UI
imports — clean separation. Config is code (`config.ts`), no YAML/JSON.

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

## UI (raw terminal, no library)

Ink was dropped. The operator's dictation tool (Wayland virtual keyboard,
`wrtype` crate) simulates keystrokes — Ink's React render cycle dropped
characters under rapid input. Raw stdin with `setRawMode` handles the full
byte stream per chunk, so dictation works correctly.

Architecture: everything prints to scrollback as it happens. No live zone,
no cursor movement, no line counting. The terminal owns all layout and reflow.
Streaming assistant text printed inline as chunks arrive. Status shown at
turn end. Input echoed character by character with backspace support.
Resize: no SIGWINCH handler needed — no live zone to redraw.

Visual layout: time column (HH:MM:SS) on left, color-coded blocks in
API terminology: user message (green), api call (cyan), api response (blue),
tool execution (yellow), tool result message (magenta), assistant message (white).

`api_response`, `user_message`, `tool_result_message` AgentEvents added.
`turn_end` aggregates metrics across all API calls in a turn.
API call counter resets per user prompt (not per session).

Removed: ink, ink-text-input, react, @types/react, ink-testing-library.

## Core purpose

The UI redesign to API-terminology (messages, roles, content blocks, tool
execution) was identified as a critical breakthrough. The goal is not just a
useful agent — it's a system where the human contributor can see exactly what
flows through every layer, understand the types at every boundary, and make
informed decisions from that basis. Observability of the actual data structures
is a first-class design goal, not a debugging aid.

This is why "types on the table" (the operator's framing, rooted in type
theory) drives UI decisions: the display should reflect the real data model,
not a simplified human-friendly abstraction of it.

Provider design principle: no least-common-denominator API. Each provider is
first-class and uses its native features (caching, usage fields, limits).
Session is a provider-agnostic superset that can be projected into each
provider's request format. UI must show provider-native property names and
actual request URLs (shortened).

## Testing discipline

Red-green for bugs/features. Structural invariant tests for refactors.

After Ink removal, `package.json` start script pointed at a deleted file.
`bun test` didn't catch it — no test covered the entry point. Added
`entry.test.ts`: verifies start/login scripts point to existing files and
that the entry file actually calls `runApp()`. Rule: when renaming or
deleting files, write a test that guards the invariant BEFORE making the
change, not after discovering breakage manually.

## Fallback model

Anthropic rate limits caused outages. Added OpenAI Codex 5.2 fallback:
when Anthropic returns 429, Omega replays the call against OpenAI using
`OPENAI_API_KEY` (optional `OPENAI_BASE_URL`). See `docs/openai-codex.md`.

## Planning system

Replaced `plan/overview.md` + `plan/ui.md` with `plan/past.md` (this file),
`plan/future.md` (issue tracker), `plan/present.md` (current work).
Maintenance process in system prompt: read all three at session start, update
at session end. Past is compressed not accumulated.
