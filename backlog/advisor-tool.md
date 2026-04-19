# Advisor tool — Sonnet executor with Opus strategic guidance

**Status: deferred.** See "Why deferred" below.

## What

Anthropic's **advisor tool** (`advisor_20260301`, beta header
`advisor-tool-2026-03-01`) lets a fast executor (e.g. Sonnet 4.6) consult a
stronger advisor model (e.g. Opus 4.7) mid-generation inside a single
`/v1/messages` call.

The advisor receives the **full transcript** automatically — system prompt,
tool definitions, every prior user/assistant turn, every tool call and result.
The executor cannot prompt it: `server_tool_use.input` is always `{}`. The
server builds the advisor's view from the transcript, the advisor emits
~400–700 tokens of strategic guidance, and the executor continues.

Docs: <https://platform.claude.com/docs/en/agents-and-tools/tool-use/advisor-tool>

## Why it matters for Omega

- Near-Opus quality at near-Sonnet cost: executor output bills at Sonnet rates;
  only the short advisor sub-inference bills at Opus rates.
- Zero orchestration code: the executor decides when to consult.
- Fits Omega's workload (long-horizon coding with many tool calls) — exactly
  the pattern the advisor is built for.

## Why deferred

**`clear_tool_uses` incompatibility.** The Anthropic docs state plainly:
> `clear_tool_uses` is not yet fully compatible with advisor tool blocks;
> full support is planned for a follow-up release.

Omega runs `clear_tool_uses_20250919` in production (triggered at 100k input
tokens) as a first-stage defence before full compaction. Enabling the advisor
today means either:

1. Disabling tool-result clearing when the advisor is on — hurts long coding
   sessions by inflating input tokens and busting the cache earlier, **or**
2. Accepting undefined behaviour on the interaction.

Neither is acceptable. Revisit once Anthropic ships the follow-up.

Secondary concerns (not blockers on their own):

- Beta + possible account-access request.
- Stream pauses up to ~30 s during advisor inference (SSE pings only).
- Two billing rates in one call, reported via `usage.iterations[]`; the turn
  footer needs rework to stay truthful.
- Conversation-level cap requires stripping all prior `advisor_tool_result`
  blocks from history when the cap is hit, which busts the prompt cache.

## When we revisit — implementation sketch

### Phase 1 — core integration

1. Add the tool to the `tools` array behind a flag, plus the beta header.
2. **Round-tripping is nearly free**: `compactedContextHistory` already stores
   `response.content` verbatim, and `context.jsonl` serialises it as-is. New
   block types (`server_tool_use`, `advisor_tool_result`,
   `advisor_tool_result_error`) pass through without schema changes.
3. Extend the streaming loop in `streamLlmCall` to recognise the new block
   types — emit a "consulting advisor" signal on `server_tool_use` open, and
   capture the advisor's advice when `advisor_tool_result` arrives (single
   `content_block_start`, no deltas).
4. Handle `advisor_redacted_result` — opaque `encrypted_content` that must be
   round-tripped but cannot be displayed.
5. Usage tracking: extend `LlmResponseEvent.usage` to carry
   `usage.iterations[]` so advisor-message entries are visible in the footer.

### Phase 2 — UI

6. "Consulting advisor…" stream indicator.
7. Collapsible advice block (mirror the existing thinking-block pattern).
8. Soft-warning rendering for advisor errors.

### Phase 3 — configuration

9. "Sonnet + Opus advisor" composite model choice in the dropdown.
10. Per-request `max_uses` cap.
11. Advisor-side caching (`caching: { type: "ephemeral", ttl: "5m" }`) —
    enable only for conversations with ≥3 expected advisor calls.
12. Conversation-level cap: count client-side; when exceeded, remove the tool
    AND strip all advisor blocks from history.

## Reference — content-block shapes

| Block type | Notes |
|---|---|
| `server_tool_use` | `name: "advisor"`, `input: {}` — executor emits this |
| `advisor_tool_result` with `content.type: "advisor_result"` | Plaintext `text` field |
| `advisor_tool_result` with `content.type: "advisor_redacted_result"` | Opaque `encrypted_content` blob |
| `advisor_tool_result` with `content.type: "advisor_tool_result_error"` | Error codes: `max_uses_exceeded`, `too_many_requests`, `overloaded`, `prompt_too_long`, `execution_time_exceeded`, `unavailable` |

## Valid executor/advisor pairs

| Executor | Advisor |
|---|---|
| Haiku 4.5 | Opus 4.7 |
| Sonnet 4.6 | Opus 4.7 |
| Opus 4.6 | Opus 4.7 |
| Opus 4.7 | Opus 4.7 |
