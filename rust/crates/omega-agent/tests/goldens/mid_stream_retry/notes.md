# Fixture: mid_stream_retry

## Script (1 LLM call, with mid-stream retry)

1. Text delta `"Partial answer that will be retried…"`.
2. Thinking delta `"Half-baked thought"`.
3. `LlmRetry` event (attempt 1, http_status=529, wait_ms=1000) carrying
   the partial fragments above.
4. Text delta `"Final answer."`.
5. `LlmResponse(stop_reason="end_turn")`.

The `LlmRetry` event simulates `RetryingProvider` having slept and
re-issued the call. The agent's contract on receiving it is to **drop
all partial content buffers** so the eventually persisted assistant
message reflects only post-retry deltas.

## Expected `context.jsonl` (2 lines)

| # | Role      | Content                                  |
| - | --------- | ---------------------------------------- |
| 1 | user      | `text("please retry")`                   |
| 2 | assistant | `text("Final answer.")` — only post-retry content |

## Plausibility checklist

- [x] Two lines exactly.
- [x] Assistant content has a single text block.
- [x] Text reads `"Final answer."` — the pre-retry
      `"Partial answer that will be retried…"` and `"Half-baked thought"`
      fragments must NOT appear in `context.jsonl`.
- [x] No `thinking` block in the persisted assistant message — the
      pre-retry thinking fragment was discarded with the buffer reset.

## SCHEMA-8 invariant

Byte-equal across all phases. The buffer-clearing semantics of
`LlmRetry` is independent of the streaming-event schema: pre-retry
deltas must remain unpersisted whether the agent uses flat or
preserve-order accumulators.
