# Fixture: interleaved_thinking

## Script

A single LLM call that genuinely interleaves thinking and text blocks
by API content-block `index`:

1. Thinking delta `"Step 1: think."` @ `index=0`, then
   `ThinkingBlockComplete { index: 0, signature: "sig-step1" }`.
2. Text delta `"Then I answer:"` @ `index=1`, then
   `TextBlockComplete { index: 1, text: "Then I answer:" }`.
3. Thinking delta `"Wait — reconsider."` @ `index=2`, then
   `ThinkingBlockComplete { index: 2, signature: "sig-step3" }`.
4. Text delta `"Final: yes."` @ `index=3`, then
   `TextBlockComplete { index: 3, text: "Final: yes." }`.
5. `LlmResponse` with `stop_reason = "end_turn"`.

The script is **genuinely interleaved**: thinking and text alternate.
Phase 0 deferred locking this golden because the pre-SCHEMA-8 flat
accumulators reordered blocks to a fixed `thinking* → text` shape.
Phase 3 (specifically commit 3e) restored API index order by building
`assistant_blocks` from `BTreeMap<usize, BlockSlot>` in key order;
this golden locks that behaviour.

## Expected `context.jsonl` (2 lines)

| # | Role      | Content (block order)                                                 |
| - | --------- | --------------------------------------------------------------------- |
| 1 | user      | `text("think step by step")`                                          |
| 2 | assistant | `thinking(sig-step1)`, `text("Then I answer:")`, `thinking(sig-step3)`, `text("Final: yes.")` |

## Plausibility checklist

- [x] Two lines exactly.
- [x] Assistant `content` is a 4-element array.
- [x] Block order matches API index order: thinking, text, thinking, text.
- [x] Signatures present and distinct on both thinking blocks.
- [x] No `tool_use`, no `tool_result`.

## SCHEMA-8 invariant

Locked at end of Phase 3 (commit 3e). Pre-3e flat-accumulator builds
would have produced `[thinking, thinking, text]` (both texts merged
into one, both thinkings ordered first). The new slot-based assembly
preserves the actual API content-block order.
