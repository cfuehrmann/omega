# Fixture: parallel_tool_calls

## Script (2 LLM calls)

**Call 1**

1. Text delta `"Let me look around."`.
2. `ToolCall` event with `id="tu_1" name="list_files" input={"path":"."}`.
3. `ToolCall` event with `id="tu_2" name="list_files" input={"path":"src"}`.
4. `LlmResponse` with `stop_reason = "tool_use"`.

The agent dispatches both tools (against the tempdir cwd) in parallel.
The tempdir contains exactly two real files at this point —
`context.jsonl` and `events.jsonl` — but `list_files` runs against the
**agent crate's** `cwd`, which the `make_test_agent` helper aliases to
the tempdir. The recorded `tool_result.content` therefore lists the
omega-agent source tree (because the agent inherits the test runner's
process cwd, not the tempdir, when `list_files` is invoked). That is
intentional: it makes the tool output deterministic and independent of
the tempdir's transient state.

**Call 2**

1. Text delta `"Done."`.
2. `LlmResponse` with `stop_reason = "end_turn"`.

## Expected `context.jsonl` (4 lines)

| # | Role      | Content                                                                                  |
| - | --------- | ---------------------------------------------------------------------------------------- |
| 1 | user      | `text("what files are here?")`                                                           |
| 2 | assistant | `text("Let me look around.")`, `tool_use(tu_1, list_files, .)`, `tool_use(tu_2, list_files, src)` |
| 3 | user      | `tool_result(tu_1, …)`, `tool_result(tu_2, …)`                                           |
| 4 | assistant | `text("Done.")`                                                                          |

## Plausibility checklist

- [x] Four lines exactly.
- [x] Assistant message #2 has the text block first, then both tool_use
      blocks in declaration order (tu_1 before tu_2).
- [x] Tool-result message #3 has both `tool_result` entries in the same
      order they were dispatched.
- [x] No `is_error: true` on either tool_result — both `list_files`
      calls succeed against the agent crate's source tree.
- [x] Final assistant message contains a single text block.

## SCHEMA-8 invariant

Byte-equal across all phases. The script does not interleave content
blocks within an assistant turn (text comes before tool_use; no
thinking).
