# Future — Issue Tracker

Discrete, prioritised, actionable. Keep in priority order.

---

## 1. Provider-specific rate-limit retry policy

Implement provider-aware backoff. For OpenAI, respect "try again in" hints if
present; otherwise exponential backoff with jitter. Anthropic may have different
headers. Must be provider-specific, not generic.

## 2. UI tests for `ui-raw.ts`

No automated tests for the UI layer. Start with pure-function tests for the
block renderers (`renderUserMessage`, `renderApiRequest`, etc.).

## 3. `sudo` handling

Detect when a tool call needs `sudo`, surface it clearly to the operator,
handle the elevated execution.

## 4. Rich command output

`run_command` output is truncated. No scrolling. Improve for long-running
commands (build output, test runs).
