# Fixture: multi_thinking_tools

## Script (3 LLM calls)

**Call 1**

1. Thinking `"Plan: list, then read."` + signature `"sig-plan"`.
2. Text `"Looking at the workspace."`.
3. `ToolCall(tu_a, list_files, ".")`.
4. `LlmResponse(stop_reason="tool_use")`.

Note: this call's deltas arrive in `thinking → text → tool_use` order,
which is not "interleaved" in the SCHEMA-8 sense (each block kind
appears at most once in a single contiguous run), so byte-equality
holds.

**Call 2**

1. Thinking `"Now I will pick a file."` + signature `"sig-pick"`.
2. `ToolCall(tu_b, read_file, "README.md")`.
3. `LlmResponse(stop_reason="tool_use")`.

**Call 3**

1. Text `"All done."`.
2. `LlmResponse(stop_reason="end_turn")`.

## Expected `context.jsonl` (6 lines)

| # | Role      | Content (block order)                                                                    |
| - | --------- | ---------------------------------------------------------------------------------------- |
| 1 | user      | `text("explore and summarise")`                                                          |
| 2 | assistant | `thinking(sig-plan)`, `text("Looking at the workspace.")`, `tool_use(tu_a, list_files, .)` |
| 3 | user      | `tool_result(tu_a, …)`                                                                   |
| 4 | assistant | `thinking(sig-pick)`, `tool_use(tu_b, read_file, README.md)`                             |
| 5 | user      | `tool_result(tu_b, is_error=true, "No such file or directory")`                           |
| 6 | assistant | `text("All done.")`                                                                      |

## Plausibility checklist

- [x] Six lines exactly.
- [x] Assistant #2 has three blocks in order: thinking, text, tool_use.
- [x] Assistant #4 has two blocks: thinking, tool_use.
- [x] Tool-result #5 carries `is_error: true` and a recognisable
      "No such file" message — the agent ran `read_file` against the
      real cwd, where `README.md` does not exist.
- [x] Final assistant #6 is a single text block.

## SCHEMA-8 invariant

Byte-equal across all phases. Each assistant turn places its blocks in
their natural emission order with no kind appearing twice
non-contiguously.
