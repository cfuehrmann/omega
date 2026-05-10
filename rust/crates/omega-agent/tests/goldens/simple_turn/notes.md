# Fixture: simple_turn

## Script

A single LLM call that emits one text delta `"Hello, world!"` and an
`LlmResponse` with `stop_reason = "end_turn"`.

## Expected `context.jsonl` (2 lines)

| # | Role      | Content                                                |
| - | --------- | ------------------------------------------------------ |
| 1 | user      | `text` block containing `"say hi"`                     |
| 2 | assistant | `text` block containing `"Hello, world!"`              |

## Plausibility checklist

- [x] Two lines exactly — one user, one assistant.
- [x] Hashes are 16-char lowercase hex (deterministic per HASH-1).
- [x] No `tool_use`, no `thinking`, no `tool_result` blocks.
- [x] User text matches the harness `user_message` argument byte-for-byte.
- [x] Assistant text matches the single `Signal::Text` delta byte-for-byte.
- [x] Both records carry a scrubbed `"time":"<scrubbed>"` placeholder.

## SCHEMA-8 invariant

Byte-equal across all phases. The new schema reshapes streaming events,
not persisted records: a single text-only response must serialise
identically before and after the refactor.
