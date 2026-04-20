# Error-policy refactor and invalid-tool-JSON recovery

**You (the agent reading this in a fresh session) are the intended audience.**
This document is your full handover. Read it end-to-end before acting.

## Progress

| Step | Status |
|---|---|
| Commit 1 — `ErrorPolicy` type, `policyFor(err)`, retry loop uses policy | **DONE** (commit `feat(agent): ErrorPolicy / policyFor`) |
| Commit 2 — retry-after server-wins + 5 min cap | **DONE** (commit `feat(agent): retry-after server-wins + 5-min cap`) |
| Commit 3 — invalid-tool-JSON policy + feedback loop | **DONE** (commit `feat(agent): invalid-tool-JSON policy + feedback loop`) |
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

1. **Retry-after wins unconditionally.** Handled at the top of
   `streamLlmCall`'s catch block, BEFORE `policyFor` is consulted. Bypasses
   any policy attempt cap. Capped at 5 minutes. **(Implemented in Commit 2.)**
2. **Policy object, not category label.** `isRetryable` is kept as a
   backwards-compatible top-level function; `Agent.policyFor(err)` is the
   new source of truth for the retry loop.
3. **Invalid-tool-JSON policy:**
   ```ts
   {
     recovery: "retry",
     maxAttempts: 2,
     backoffMs: (attempt) =>
       getAnthropicRetryDelayMs(err, attempt, this.retryBaseMs, this.retryMaxMs),
     feedbackOnExhaustion:
       "Your previous response could not be parsed — the tool-call JSON had " +
       "invalid escaping (likely unescaped newlines or quotes in a string " +
       "argument). Please retry the same tool call, being extra careful with " +
       "JSON string escaping.",
   }
   ```
4. **Feedback loop lives in `sendMessage`, not `streamLlmCall`.** When
   `streamLlmCall` returns `{ ok: false, error }` and the policy for that
   error has `feedbackOnExhaustion`, `sendMessage` appends a synthetic user
   message to history and restarts the turn's outer LLM loop. Bounded at 2
   feedback cycles per turn; then fall through to `agent_error` +
   `turn_interrupted` as today.
5. **No context pollution from failed turns.** `stream.finalMessage()` throws
   before we build an assistant response, so `appendToHistory` is not called
   for the failed attempt. Feedback user messages ARE persisted — they're
   legitimate conversation.

## What exists already (use, don't re-create)

| Symbol | Location | Role |
|---|---|---|
| `ErrorPolicy` type | `src/agent.ts` | discriminated union — see top of file |
| `Agent.policyFor(err)` | `src/agent.ts` | **single source of truth** for error-recovery behaviour; Commit 3 extends this with an `invalid-tool-JSON` branch |
| `isRetryable(err)` | `src/agent.ts` | top-level, still exported (tests import it). Still used in the `rate_limit` agent_error branch in `sendMessage` |
| `getRetryAfterMs(err)` | `src/agent.ts` | extracts retry-after header value in ms |
| `getAnthropicRetryDelayMs(err, attempt, base, max)` | `src/agent.ts` | composes retry-after + exponential backoff. (The retry-after branch inside it is now unreachable from `streamLlmCall`'s main loop — the top-level check catches it first — but the function is still used via `policy.backoffMs`.) |
| retry loop in `streamLlmCall` | `src/agent.ts` | top of catch: retry-after (server-wins, unbounded, 5-min cap). Below that: `policyFor` dispatch. Two counters: `attempt` (total, emitted) and `policyAttempts` (gates `maxAttempts` and feeds `backoffMs`). |
| `LlmRetryEvent.reason` | `src/events.ts` | optional `"retry-after"` discriminator introduced in Commit 2. No value means ordinary policy-driven retry. |
| `appendToHistory(msg)` | `src/agent.ts` (private) | canonical way to append to `compactedContextHistory` AND persist to `context.jsonl`. Use this for the feedback user message — not a direct array push. |
| unit tests | `src/agent.test.ts` | cover `isRetryable` directly — must keep passing unchanged |
| retry/retry-after tests | `src/agent-rate-limit.test.ts` | cover header honour, 5-min cap, server-wins over classifier, policy-cap independence |

## What's missing (the remaining work for Commit 3)

- Invalid-tool-JSON detection in `policyFor`.
- Feedback loop in `sendMessage` (bounded per-turn `feedbackAttempts` counter).

---

## Commit 3 — Invalid-tool-JSON policy + feedback loop

Detect the invalid-tool-JSON error, give it a policy with a feedback
message, and wire the feedback loop in `sendMessage`.

### Concrete steps

1. **Find the stable error surface.** Grep `node_modules/@anthropic-ai/sdk/`
   for "Unable to parse tool parameter JSON from model" or "parse tool
   parameter" to find where the SDK throws. Prefer HTTP status + error type
   over free-text message matching if the SDK exposes them.
   Evidence from a real session:
   `.omega/sessions/2026-04-20T06-29-36-840-3560ab15/events.jsonl` contains
   an actual `llm_error` with this message — inspect it to see the exact
   error-body shape we'll match on.

2. **Add an `invalid-tool-JSON` branch in `Agent.policyFor(err)`** that
   returns the policy shown in Decisions §3 above. Place it BEFORE the
   existing `isRetryable` check — invalid-tool-JSON should win even if a
   future classifier change would otherwise route it elsewhere.

3. **Add a per-turn `feedbackAttempts` counter in `sendMessage`.**
   Initialise to 0 at the start of the turn (alongside
   `totalInputTokens` etc.). Reset is automatic since it's a local
   `let`.

4. **Wire the feedback path in `sendMessage`'s `!llmResult.ok` branch.**
   Current structure (after abort check):
   ```
   if (isContextOverflow) { agent_error; turn_interrupted; return; }
   else if (isRetryable)  { agent_error; turn_interrupted; return; }
   else                   { agent_error; turn_interrupted; return; }
   ```
   Insert the feedback branch BEFORE all three classification branches:
   ```ts
   const policy = this.policyFor(llmResult.error);
   const feedback =
     policy.recovery === "retry" ? policy.feedbackOnExhaustion : undefined;
   if (feedback !== undefined && feedbackAttempts < 2) {
     await this.appendToHistory({
       role: "user",
       content: [{ type: "text", text: feedback }],
     });
     // Emit a user_message event so the UI/log shows the synthetic nudge.
     const feedbackEv: OmegaEvent = {
       type: "user_message",
       time: now(),
       content: feedback,
     };
     this.logEvent(feedbackEv);
     yield feedbackEv;
     feedbackAttempts++;
     continueLoop = true;
     continue; // restart the agentic while-loop
   }
   ```
   Do NOT yield `agent_error` or `turn_interrupted` on this path.

5. **If `feedbackAttempts` is exhausted, fall through** to today's three
   classification branches unchanged. (Invalid-tool-JSON will match the
   "else" branch of the isContextOverflow/isRetryable classification and
   produce a generic `API error: …` message. If desired, add a dedicated
   branch for a nicer message — optional.)

6. **Persistence is automatic.** `appendToHistory` writes the feedback
   message to `context.jsonl` through the normal path — that's correct and
   intentional. The user-facing UI will render it the same as any other
   user message.

### Tests

Place in a new file `src/agent-invalid-tool-json.test.ts` (keeps
`agent-rate-limit.test.ts` single-purpose), using the existing
`CreateMessageStream` mock pattern from `src/agent-rate-limit.test.ts`:

- **Transparent retry succeeds.** Throw invalid-tool-JSON twice, then
  succeed. Expect 2 × `llm_retry` (with no `reason` field), then success.
  No feedback `user_message` emitted.
- **Feedback retry succeeds.** Throw 3 times (exhaust the transparent
  retries: attempt 1 + 2 × retry = 3 calls), then on the 4th call succeed.
  Expect 2 × `llm_retry`, then an `llm_error`, then a feedback `user_message`
  event, then success. Exactly 1 feedback message appears in
  `agent.getCompactedContextHistory()`.
- **Full exhaustion.** Throw indefinitely. Expect the pattern
  `llm_retry × 2 + llm_error + user_message(feedback) + llm_retry × 2 +
  llm_error + user_message(feedback) + llm_retry × 2 + llm_error + agent_error
  + turn_interrupted(error)`. Precisely 2 feedback messages end up in
  history.

### Acceptance

All tests pass; the three recovery modes (transparent retry,
feedback-driven retry, full exhaustion) are individually exercised.

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
