# Session 2026-04-20T06-29-36 — investigation summary

Keep until the user acknowledges; then delete. The pause-refresh bug is
**fixed** by this session's work.

## User-reported symptoms, triaged

1. **`llm_error` + `agent_error` at end of session** — caused by Anthropic
   returning "Unable to parse tool parameter JSON from model" because
   the model's streamed `input_json_delta` chunks for a large `edit_file`
   call had inconsistent newline escaping. Not an Omega bug. Error
   surfaces via `stream.finalMessage()` exception in
   `src/agent.ts:781` → caught → `llm_error` + `agent_error` +
   `turn_interrupted{error}`. The three-event sequence is well-designed.
   **Open follow-up**: the agent gives up rather than retrying. User
   agreed this should auto-retry (or feed-back). Not tackled yet.

2. **"Streaming apparently stopped during the final llm response"** — same
   root cause as (1). Anthropic's stream terminated mid-response when its
   server-side JSON parser rejected the tool input; UI just froze on the
   partial text.

3. **Refresh during Pause flashed "Interrupted"** — **FIXED this session.**

## Bug 3 root cause (the real Omega bug)

`src/web/server.ts :: loadReplayEvents()` was calling `closeOpenTurn()` on
the returned events. The function's own doc comment says *"Does NOT apply
closeOpenTurn — callers decide based on isStreaming state"*. Caller in
`open()` carefully gates with `isStreaming ? raw : closeOpenTurn(raw)`,
but the gate is dead code — the events were already closed by
`loadReplayEvents`.

`closeOpenTurn` walks back looking only for `turn_end` / `turn_interrupted`
terminators — `turn_paused` is NOT a terminator, so during a paused turn
it appended a spurious `turn_interrupted`. That's what rendered as the
"⊘ Interrupted" block on refresh-during-pause.

**Fix**: `return closeOpenTurn(events);` → `return events;` in
`loadReplayEvents` (src/web/server.ts ~235). One character, matches the
doc comment.

## Regression test

`e2e/pause-resume-interject.spec.ts` test 8:
"reload while Paused: no transient ⊘ Interrupted block ever renders".

Installs a MutationObserver + rAF poll via `page.addInitScript()` that
records every appearance of `[data-testid="block-interrupt"]` into
`window.__interruptSightings`. Triggers MULTI_TOOL_TEST, pauses, reloads,
asserts sightings === []. Without the fix, the block is rendered
continuously for the full 1-second settling window (~60 sightings).

## Verified

- New test passes with fix, fails without it.
- All 107 Playwright tests pass after the fix.
- No other call sites of `loadReplayEvents` are affected: the other two
  uses (post-resume, post-reset) read freshly-created sessions with no
  user_message yet, so `closeOpenTurn` was a no-op there anyway.

## Next — explicit user asks

None right now. User said "wait with instrumentation (b)". The invalid-
tool-JSON retry question is open. Discuss before implementing.
