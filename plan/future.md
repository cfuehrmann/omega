# Future — Issue Tracker

Discrete, prioritised, actionable. Close items by moving a one-line outcome
to `past.md`. Keep in priority order.

---

## 1. Spike: pi-tui as Ink replacement

`@mariozechner/pi-tui` (from badlogic/pi-mono, 14.5k stars) is a pure
TypeScript TUI library that owns the full screen, uses differential
rendering, and handles bracketed paste and the Kitty keyboard protocol.
Directly addresses Ink's input flakiness.

Goal: build the simplest possible prototype — scrolling output + input box —
and see if typing feels solid. One to two hours max. Output is a decision,
not a feature. Code gets thrown away.

If good: add migration to future.md as a proper item.
If bad: record why and move on.

## 2. UI tests for `ui.tsx`

`ui.tsx` has zero automated tests. Use `ink-testing-library` to cover the
main states: resume prompt, streaming display, activity indicator, Esc
interrupt, dim prompt while agent acts.

## 2. Dictation truncation bug

`wtype` (Wayland) injects keystrokes one at a time. Text gets truncated on
long dictated inputs despite the `useEffect` fix in `fast-text-input.tsx`.
Root cause not fully pinned. Needs debug logging to find the drop site.

## 3. `sudo` handling

Detect when a tool call needs `sudo`, surface it clearly to the operator,
handle the elevated execution. Currently unhandled.

## 4. Context summarisation

When context is truncated, old messages are dropped silently. Better:
summarise dropped content and inject the summary so the agent retains
semantic history even with a full context window.

## 5. Full-screen TUI or browser UI

Ink can't do collapsible/expandable history. OpenTUI (Zig+TypeScript, 8.8k
stars) is the most promising terminal option — revisit when it reaches 1.0.
Browser UI (Vite + React + local WebSocket) is the most flexible option.
Neither is urgent while the agent is still growing in capability.

## 6. Rich command output

`run_command` output is truncated to 20 lines. ANSI codes stripped. No
scrolling. Improve for long-running commands (build output, test runs).

## 7. Provider abstraction

Support OpenAI and local LLMs alongside Claude. Deferred until the agent
is genuinely useful enough to want alternatives.
