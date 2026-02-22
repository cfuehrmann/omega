# Future — Issue Tracker

Discrete, prioritised, actionable. Close items by moving a one-line outcome
to `past.md`. Keep in priority order.

---

## 0. Rearchitect context management: fold at quit, not at startup

**Decision made (not yet implemented).** Replace fire-and-forget fold-at-startup with fold-at-quit:

- Fold session history into the world-state file **on clean shutdown** (SIGINT/SIGTERM + normal exit), not at the next startup.
- Treat the world file as project-specific (keyed to working directory) so project-switching is natural.
- Remove the "resume session?" prompt — Omega always resumes, the prompt is pointless.
- Remove raw-history persistence (`persistSession` / `resumeSession`) — the world file is the only cross-session artifact.
- Shutdown path must be **robustly tested** (signal handling, async fold completes before exit).
- Acceptable tradeoff: mid-session crash loses conversational context (not work product, which is saved files).

**Future / deferred:** periodic in-session world folding (e.g. every N turns) to reduce crash-loss window. Do NOT implement now — note only.

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

## 5. Context summarisation ✓ DONE

Three-zone compaction implemented: world state (zone 1), turn summaries (zone 2), verbatim current turn (zone 3). LLM-based. See past.md.

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
