# Eager input streaming (fine-grained tool streaming)

## What

Anthropic's `eager_input_streaming` flag enables character-by-character
streaming of tool call parameter values *without* server-side JSON
validation/buffering. This means the first chunks of a large tool input
(e.g. `write_file` content) arrive significantly faster — Anthropic's own
examples show first-chunk latency dropping from ~15 s to ~3 s.

Docs: <https://platform.claude.com/docs/en/agents-and-tools/tool-use/fine-grained-tool-streaming>

## How to enable

Add `eager_input_streaming: true` to any tool definition in `src/tools.ts`:

```typescript
{
  name: "write_file",
  description: "...",
  eager_input_streaming: true,   // ← new
  input_schema: toToolInputSchema(WriteFileSchema),
},
```

No beta header required — GA on all models and platforms.

## Which tools benefit

| Tool | Benefit | Notes |
|------|---------|-------|
| `write_file` | **High** — `content` can be thousands of characters | Main win |
| `edit_file` | **Medium** — `replacements` array can be large | |
| `run_command` | Low — `command` strings are short | |
| Others | Negligible | Small JSON inputs |

## Caveats

1. **Partial/invalid JSON.** With eager streaming the API sends chunks before
   validating that the JSON is well-formed. Omega currently waits for the
   complete `tool_use` block via `finalMessage()`, so this is a non-issue
   today — but if we ever process `input_json_delta` events incrementally
   (e.g. to show a live file-write preview in the UI), we'd need to handle
   malformed partials.

2. **Chunking differs.** Chunks are larger and have fewer word breaks compared
   to non-eager mode. Not a problem for Omega since we don't render tool
   input chunks in the UI.

## Implementation plan

1. Add `eager_input_streaming: true` to `write_file` and `edit_file` tool
   definitions in `src/tools.ts`.
2. Verify the SDK type (`BetaTool`) accepts the field — check
   `@anthropic-ai/sdk` types. If not, use a type assertion.
3. Run the full test suite (`just gate`) — the mock streams don't use this
   flag so tests should pass unchanged.
4. Manual verification: run a session, trigger a large `write_file`, and
   compare time-to-first-chunk in the event log.

## Future: incremental tool execution

A more ambitious follow-up would be to *execute* tool calls incrementally as
input chunks arrive — e.g. start writing to the file before the full content
is streamed. This would require:

- Accumulating `input_json_delta` events in the stream loop
- A streaming JSON parser to extract complete fields early
- Fallback/rollback if the final JSON turns out invalid

This is a significant architectural change and should be evaluated separately.
