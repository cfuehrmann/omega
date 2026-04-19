# Token task budget — cost protection for long sessions

## What

Anthropic's `task_budget` parameter (on `output_config`) sets a total token
budget for a task across multiple turns and compactions. When the budget is
exhausted, the model stops. The client tracks `remaining` across turns.

```typescript
output_config: {
  effort: "high",
  task_budget: { type: "tokens", total: 500_000, remaining: 350_000 }
}
```

The API returns the updated `remaining` in the response, which the client
passes back on the next call.

## Why it matters for Omega

Omega currently has **no cost guardrail**. A runaway session (infinite tool
loop, overly ambitious plan) can burn unlimited tokens. On Opus at $25/MTok
output, that adds up fast.

`task_budget` gives server-side enforcement:
- Set a total budget at session start (e.g. 1M tokens).
- Pass `remaining` back on each turn.
- When exhausted, the model emits a `stop_reason` indicating budget exhaustion
  rather than silently continuing.

## Implementation plan

1. **Add `task_budget` to the API call** in `src/agent.ts`. Source the `total`
   from a new config field (e.g. `taskBudgetTokens: number | null`).
2. **Track remaining.** After each API response, read
   `response.usage.output_tokens` (or whatever the response reports) and update
   `remaining` for the next call. The response's `task_budget` field may
   include the updated remaining directly.
3. **Handle budget exhaustion.** When the model stops due to budget, emit a
   clear `turn_end` event with a reason like `"budget_exhausted"` instead of
   the normal `"end_turn"`.
4. **UI indicator.** Show remaining budget as a progress bar or counter in the
   turn footer, next to the existing token counts.
5. **Configuration.** Expose in the web UI settings panel:
   - Toggle: enable/disable task budget
   - Input: total budget (tokens), with sensible defaults per model
   - Display: remaining tokens, percentage used
6. **Session persistence.** Persist the budget state in `events.jsonl` so
   resumed sessions continue with the correct remaining count.

## Caveats

- The exact response shape for `task_budget` remaining needs verification
  against the live API — the SDK types show `remaining` on the request but
  the response field may differ.
- Budget is in tokens (input + output combined? output only?). Need to verify
  the exact semantics from the API docs when they're published.
- Consider whether compaction tokens count against the budget.

## Effort estimate

Small — the core change is adding one field to the API call and threading
`remaining` through the turn loop. The UI work is optional polish.
