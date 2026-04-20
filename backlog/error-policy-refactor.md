# Error-policy refactor and invalid-tool-JSON recovery

**You (the agent reading this in a fresh session) are the intended audience.**
This document is your full handover. Read it end-to-end before acting.

## Progress

| Step | Status |
|---|---|
| Commit 1 — `ErrorPolicy` type, `policyFor(err)`, retry loop uses policy | **DONE** (see `git log src/agent.ts`, commit `feat(agent): ErrorPolicy / policyFor`) |
| Commit 2 — retry-after server-wins + 5 min cap | TODO |
| Commit 3 — invalid-tool-JSON policy + feedback loop | TODO |
| Commit 4 — delete scratchpads | TODO |

## Background

When Claude produces invalid tool-call JSON, Anthropic's SDK throws during
`stream.finalMessage()`. Today the agent gives up: emits `llm_error` +
`agent_error` + `turn_interrupted{error}` and terminates the turn. The user
wants the agent to (a) retry the same request transparently a couple of
times, and (b) if that fails, append a synthetic corrective user message and
let the model retry once or twice more with that nudge in context, and only
then give up.

Simultaneously, we're refactoring the LLM error-handling design. The single
boolean `isRetryable(err)` predicate is being replaced with a policy object
that describes what to do per error kind:

```ts
type ErrorPolicy =
  | { recovery: "none" }
  | {
      recovery: "retry";
      maxAttempts: number | undefined;                   // undefined = infinite
      backoffMs: (attempt: number) => number;
      feedbackOnExhaustion?: string;
    };
```

The retry/recovery loop reads behaviours off the policy. No taxonomy labels,
no "is this a transport error" classification — just direct behavioural
traits.

## Decisions already made (do not re-litigate)

1. **Retry-after wins unconditionally.** If the Anthropic response carries a
   `retry-after` header, we retry regardless of classifier opinion, and we
   keep retrying unboundedly as long as the server keeps sending it. When
   the server stops, the next error falls through to policy-driven handling.
2. **Cap retry-after at 5 minutes** as a sanity limit on absurd durations.
3. **Policy object, not category label.** `isRetryable` is kept as a
   backwards-compatible top-level function; `Agent.policyFor(err)` is the
   new source of truth for the retry loop.
4. **Invalid-tool-JSON policy:**
   ```ts
   {
     recovery: "retry",
     maxAttempts: 2,
     backoffMs: (attempt) => getAnthropicRetryDelayMs(err, attempt, base, max),
     feedbackOnExhaustion: "Your previous response could not be parsed — the tool-call JSON had invalid escaping (likely unescaped newlines or quotes in a string argument). Please retry the same tool call, being extra careful with JSON string escaping.",
   }
   ```
5. **Feedback loop lives in `sendMessage`, not `streamLlmCall`.** When
   `streamLlmCall` returns `{ ok: false, error }` and the policy for that
   error has `feedbackOnExhaustion`, `sendMessage` appends a synthetic user
   message to `this.history` and restarts the turn's outer LLM loop.
   Bounded at 2 feedback cycles per turn; then fall through to
   `agent_error` + `turn_interrupted` as today.
6. **No context pollution from failed turns.** `stream.finalMessage()` throws
   before we build an assistant response, so `appendToHistory` is not
   called. Feedback user messages ARE persisted — they're legitimate
   conversation.

## What exists already (use, don't re-create)

| Symbol | Location | Role |
|---|---|---|
| `ErrorPolicy` type | `src/agent.ts` | discriminated union — see top of file |
| `Agent.policyFor(err)` | `src/agent.ts` | **single source of truth** for error-recovery behaviour; Commits 2/3 extend this |
| `isRetryable(err)` | `src/agent.ts` | top-level, still exported (tests import it). Currently still used in the `rate_limit` event path in `sendMessage` |
| `getRetryAfterMs(err)` | `src/agent.ts` | extracts retry-after header value in ms |
| `getAnthropicRetryDelayMs(err, attempt, base, max)` | `src/agent.ts` | composes retry-after + exponential backoff |
| retry loop in `streamLlmCall` | `src/agent.ts` | now calls `this.policyFor(err)` and branches on `policy.recovery`. Extend this in Commit 2 |
| unit tests | `src/agent.test.ts` | cover `isRetryable` directly — must keep passing unchanged |
| retry-after tests | `src/agent-rate-limit.test.ts` | cover header honour, fractional, cap-interaction, fallback |

## What's missing (the remaining work)

- Server-wins retry-after (today retry-after only applies within
  already-retryable errors, inside `getAnthropicRetryDelayMs`).
- 5-minute cap on retry-after.
- Invalid-tool-JSON detection in `policyFor`.
- Feedback loop in `sendMessage`.

---

## Commit 2 — Retry-after: server-wins + 5-min cap

Change retry-after handling from "authoritative duration within a retryable
error" to "authoritative decision to retry at all", capped at 5 minutes.

Concrete steps:

1. In the retry loop in `streamLlmCall`, BEFORE calling `this.policyFor(err)`,
   check `getRetryAfterMs(err)`. If present:
   - `const waitMs = Math.min(retryAfterMs, 5 * 60 * 1000);`
   - Emit the existing `llm_retry` event (no new event type needed;
     optionally add a `reason: "retry-after"` field if ergonomic).
   - Sleep, `continue` loop. Do NOT call `policyFor`, do NOT count toward
     any attempt cap. Unbounded while server keeps sending retry-after.
2. If no retry-after, fall through to policy-driven handling from Commit 1
   (already in place).
3. Add tests in `src/agent-rate-limit.test.ts`:
   - Error that `isRetryable` returns false for, but carries `retry-after`
     (e.g. a 400 with retry-after) → retries anyway.
   - Retry-after of 600 s → capped to 300 s (5 min).
   - Server sends retry-after 3 times in a row, then succeeds → 3 retries,
     all respecting retry-after cadence, no attempt cap hit.
4. Review existing retry-after test "retry-after header is honoured even
   when it exceeds retryMaxMs cap" — it uses 500 ms, well under 5 min, so
   behaviour unchanged. No test modifications expected.

Acceptance: all new tests pass, all existing tests pass unchanged.

## Commit 3 — Invalid-tool-JSON policy + feedback loop

Detect the invalid-tool-JSON error, give it a policy with a feedback message,
and wire the feedback loop in `sendMessage`.

Concrete steps:

1. Grep `node_modules/@anthropic-ai/sdk/` for the error text "Unable to parse
   tool parameter JSON from model" or "parse tool parameter" to find how
   it's surfaced. Match on whatever stable surface the SDK provides —
   prefer HTTP status + error type over free-text match if available.
   Evidence from the real session:
   `.omega/sessions/2026-04-20T06-29-36-840-3560ab15/events.jsonl` contains
   an actual `llm_error` with this message. Inspect it.
2. Add an `invalid-tool-JSON` branch in `Agent.policyFor(err)` that returns:
   ```ts
   {
     recovery: "retry",
     maxAttempts: 2,
     backoffMs: (attempt) => getAnthropicRetryDelayMs(err, attempt, this.retryBaseMs, this.retryMaxMs),
     feedbackOnExhaustion: "Your previous response could not be parsed — the tool-call JSON had invalid escaping (likely unescaped newlines or quotes in a string argument). Please retry the same tool call, being extra careful with JSON string escaping.",
   }
   ```
3. In `sendMessage`, where `streamLlmCall` returns `{ ok: false, error }`
   (not aborted, not context-overflow), check
   `this.policyFor(err).feedbackOnExhaustion`. If set AND
   `feedbackAttempts < 2`:
   - Append `{ role: "user", content: [{ type: "text", text: feedback }] }`
     to `this.history`.
   - `feedbackAttempts++`.
   - Continue the turn's outer loop → new `llm_call` with the feedback in
     context.
   - Do NOT yield `agent_error` or `turn_interrupted` on this path.
4. If `feedbackAttempts` is exhausted, fall through to today's behaviour:
   yield `agent_error` + `turn_interrupted{error}`.
5. The synthetic user message WILL be written to `context.jsonl` through
   the normal history-persistence path — that's correct and intentional.
6. Tests (in `src/agent.test.ts` or a new file, using the existing
   `CreateMessageStream` mock pattern):
   - **Transparent retry succeeds:** throw invalid-tool-JSON twice, then
     succeed. Expect 2 × `llm_retry`, then success. No feedback message in
     history.
   - **Feedback retry succeeds:** throw 2 times (exhaust transparent),
     feedback message appended, then on the retry with feedback, succeed.
     Expect exactly 1 feedback user message in `this.history`.
   - **Full exhaustion:** throw indefinitely. Expect 2 transparent + 1
     feedback + 2 transparent + 1 feedback + 2 transparent + exhaustion →
     `agent_error` + `turn_interrupted{error}`.

Acceptance: all tests pass; the three recovery modes are individually
exercised.

## Commit 4 — Delete scratchpads

Delete `backlog/error-policy-refactor.md` (this file) and
`backlog/session-2026-04-20-pause-bugs.md`. Both are session scratchpads;
their content is now captured in the git history / commits.

---

## Session conventions (reminder — from AGENTS / CLAUDE / project norms)

- Start with `git status` on HEAD of `develop`.
- `just gate` runs as the pre-commit hook — commit with
  `git add -A && git commit -m "..."`. Never bypass with `--no-verify`.
- Prefer tests that exercise real behaviour with real I/O. Mock only
  external services (the Anthropic stream).
- The pre-existing `CreateMessageStream` mock pattern used throughout
  `src/agent.test.ts` and `src/agent-rate-limit.test.ts` is the right
  template for new tests here.
