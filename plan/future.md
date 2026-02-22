# Future — Issue Tracker

Discrete, prioritised, actionable. Close items by moving a one-line outcome
to `past.md`. Keep in priority order.

---

## 1. Token efficiency + OpenAI-first provider design

Make token efficiency top priority. Integrate OpenAI as a first-class
provider (no least-common-denominator API). Use provider-specific features
(prompt caching, usage fields, model-specific limits). Session should be a
provider-agnostic superset that can be projected into provider request
formats. UI must display provider-native property names and the actual URL
called (shortened).

## 2. Provider-specific rate-limit retry policy

Implement provider-aware backoff. For OpenAI, respect "try again in" hints if present; otherwise use exponential backoff with jitter. Anthropic may have different headers. Must be provider-specific, not generic.

## 3. UI tests for `ui-raw.ts`

No automated tests for the UI layer. Can't use ink-testing-library (Ink was
removed). Options: test the render helpers as pure functions, or spawn a
pty and assert on output. Start with pure-function tests for the block
renderers (renderUserMessage, renderApiRequest, etc.).

## 4. `sudo` handling

Detect when a tool call needs `sudo`, surface it clearly to the operator,
handle the elevated execution. Currently unhandled.

## 5. Context summarisation

When context is truncated, old messages are dropped silently. Better:
summarise dropped content and inject the summary so the agent retains
semantic history even with a full context window.

## 6. Rich command output

`run_command` output is truncated. No scrolling. Improve for long-running
commands (build output, test runs).

## 7. Full-screen TUI or browser UI

Raw terminal can't do collapsible/expandable history. OpenTUI
(Zig+TypeScript) is a promising option — revisit when stable. Browser UI
(Vite + React + local WebSocket) is the most flexible. Neither is urgent.

## 8. Provider abstraction

OpenAI Codex fallback exists, but the provider layer is still Anthropic-
centric. Longer-term: clean provider interface, per-provider settings,
and streaming abstraction. Deferred until the agent is useful enough to
justify multi-provider maintenance.
