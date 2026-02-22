# Future — Issue Tracker

Discrete, prioritised, actionable. Close items by moving a one-line outcome
to `past.md`. Keep in priority order.

---

## 1. UI tests for `ui-raw.ts`

No automated tests for the UI layer. Can't use ink-testing-library (Ink was
removed). Options: test the render helpers as pure functions, or spawn a
pty and assert on output. Start with pure-function tests for the block
renderers (renderUserMessage, renderApiRequest, etc.).

## 2. `sudo` handling

Detect when a tool call needs `sudo`, surface it clearly to the operator,
handle the elevated execution. Currently unhandled.

## 3. Context summarisation

When context is truncated, old messages are dropped silently. Better:
summarise dropped content and inject the summary so the agent retains
semantic history even with a full context window.

## 4. Rich command output

`run_command` output is truncated. No scrolling. Improve for long-running
commands (build output, test runs).

## 5. Full-screen TUI or browser UI

Raw terminal can't do collapsible/expandable history. OpenTUI
(Zig+TypeScript) is a promising option — revisit when stable. Browser UI
(Vite + React + local WebSocket) is the most flexible. Neither is urgent.

## 6. Provider abstraction

OpenAI Codex fallback exists, but the provider layer is still Anthropic-
centric. Longer-term: clean provider interface, per-provider settings,
and streaming abstraction. Deferred until the agent is useful enough to
justify multi-provider maintenance.
