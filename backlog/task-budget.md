# Task budgets — evaluated, declined

**Status: declined.** Evaluated against Anthropic's published docs
([task-budgets](https://platform.claude.com/docs/en/build-with-claude/task-budgets)).
The feature does not match the problem it was originally proposed to solve,
and the real problem it does solve is not one Omega currently has. Keeping
this note so future analysis passes don't re-surface the same suggestion.

## What `task_budget` actually is

- **Advisory soft hint, not a hard cap.** The model may exceed the budget if
  interrupting mid-action would be more disruptive than finishing. The real
  hard cap on generated tokens is still `max_tokens` per request.
- **Server-injected countdown marker.** Claude sees a running `remaining`
  counter in its context and uses it to pace itself — prioritize, wrap up
  gracefully, scale down adaptive thinking as the budget depletes.
- **Scoped to one agentic loop** (one user message → tool-use turns →
  `end_turn`), not a session lifetime.
- **Opus 4.7 only**, public beta. Requires header
  `anthropic-beta: task-budgets-2026-03-13`. Not supported on Sonnet 4.6
  or Opus 4.6.
- **Minimum total: 20 000 tokens.** Values below return HTTP 400.
- **Too-small budgets cause refusal-like behavior.** If Claude judges the
  budget insufficient, it may decline, scope down aggressively, or stop early.
- **Prompt-cache interaction.** Mutating `remaining` client-side on every
  request invalidates cache prefixes. Docs recommend setting `total` once
  and letting the server track; only pass `remaining` if you rewrite
  history client-side (Omega doesn't — we use server-side compaction).

## Why the original proposal was wrong

The suggestion was framed as a **cost guardrail**: "set a budget, when
exhausted the model stops." That is not what this feature is. It's a
**behavioral nudge** for long agentic loops to land gracefully. If cost
enforcement is the goal, `task_budget` doesn't deliver it; `max_tokens`
per request plus the user's abort control are still the real ceilings.

## Why it's declined for Omega specifically

1. **Guessing a budget in advance is the wrong direction.** The preferred
   path for cost control is making the agent efficient at its work, not
   pre-committing to a number that's either too low (refusals, premature
   stops) or too high (does nothing).
2. **Opus 4.7 only.** A large fraction of sessions run Sonnet 4.6 and get
   no benefit. Even on Opus 4.7, the user's only `xhigh` use case is
   planning — short, bounded work, not the long agentic loops this feature
   targets.
3. **Cost visibility is already solved.** The expandable per-turn / per-session
   cost display gives the user live feedback, which is the control they want.
4. **The failure mode isn't live.** Runaway loops are theoretically possible
   but rare, visible to the user, and interruptible. No evidence of it being
   a real problem.
5. **Existing guardrails cover the rest.** Per-request `max_tokens` (64k
   Sonnet / 128k Opus), server-side compaction at 750k, tool-result clearing
   at 100k, and user abort.

## When to reconsider

- If a concrete pattern emerges of long Opus 4.7 runs hitting `max_tokens`
  mid-thought and producing truncated output that would have benefited from
  graceful wrap-up.
- If Anthropic extends `task_budget` to Sonnet 4.6 and it becomes cheap
  default-on infrastructure.
- If users start running unattended long-horizon Opus 4.7 sessions where
  live cost visibility isn't enough and a behavioural pacing signal would
  demonstrably help.

None of these apply today.
