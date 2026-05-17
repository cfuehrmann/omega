# Fixture: compaction

## Script (1 LLM call, with mid-stream compaction)

1. Text delta `"About to be compacted…"`.
2. `Compacted` event with a canned `usage` payload.
3. Text delta `"Picking up after compaction."`.
4. `LlmResponse(stop_reason="end_turn", text="Picking up after compaction.")`.

When `Compacted` arrives, the agent clears its **in-memory** history
and `context_hashes` so subsequent LLM calls only see post-compaction
state. It does NOT reset `text_buf` though — partial text already
streamed within this same response continues to accumulate. As a
result the persisted assistant message in this fixture contains the
concatenation of both pre- and post-compaction text deltas.

This is the documented behaviour of the legacy agent (it mirrors
`src/agent.ts:1432–1453`) and the SCHEMA-8 refactor must preserve it
byte-for-byte: compaction affects the **next** LLM call's context, not
the **current** response's accumulated content.

## Expected `context.jsonl` (2 lines)

| # | Role      | Content                                                              |
| - | --------- | -------------------------------------------------------------------- |
| 1 | user      | `text("please compact")`                                             |
| 2 | assistant | `text("About to be compacted…Picking up after compaction.")` — concatenation |

## Plausibility checklist

- [x] Two lines exactly.
- [x] Assistant content is a single text block.
- [x] Text is the literal concatenation of both pre- and post-compaction
      deltas, with no separator inserted (`"About to be compacted…Picking up after compaction."`).
- [x] No `Compacted` payload appears in `context.jsonl` — the event
      lives only in `events.jsonl` (out of scope for these goldens).

## SCHEMA-8 invariant

Byte-equal across all phases. Compaction's contract concerns the next
turn's LLM-call context, not the current response's accumulated text.
The refactor must not change which deltas land in `text_buf` for the
in-flight response.
