# Fixture: thinking_blocks

## Script

A single LLM call that emits, in order:

1. Thinking delta `"First, let me consider…"` then a
   `ThinkingBlockComplete { signature: "sig-thinking-1" }`.
2. Thinking delta `"Wait — let me double-check."` then a
   `ThinkingBlockComplete { signature: "sig-thinking-2" }`.
3. Text delta `"Here is the answer: 42."`.
4. `LlmResponse` with `stop_reason = "end_turn"`.

The script is **non-interleaved**: thinking blocks come strictly before
the text block. The genuinely interleaved case
(`thinking → text → thinking`) is captured separately as the Phase-3
`interleaved_thinking` fixture, where the persisted block order is
expected to change once SCHEMA-8 preserves the original ordering.

## Expected `context.jsonl` (2 lines)

| # | Role      | Content (block order)                                      |
| - | --------- | ---------------------------------------------------------- |
| 1 | user      | `text("what is the answer?")`                              |
| 2 | assistant | `thinking(sig-thinking-1)`, `thinking(sig-thinking-2)`, `text("Here is the answer: 42.")` |

## Plausibility checklist

- [x] Two lines exactly.
- [x] Assistant `content` is a 3-element array.
- [x] First two blocks are `type: "thinking"` with the matching
      `signature` field — signatures are present and not empty.
- [x] Block order: both thinking blocks come before the text block.
- [x] No `tool_use`, no `tool_result`.

## SCHEMA-8 invariant

Byte-equal across all phases. The script does not interleave content
blocks, so both the legacy flat-accumulator and the new
preserve-order schema must produce the same persisted record.
