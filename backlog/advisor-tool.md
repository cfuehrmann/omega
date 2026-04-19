# Advisor tool — Sonnet executor with Opus strategic guidance

## What

Anthropic's **advisor tool** (`advisor_20260301`) lets a fast executor model
(e.g. Sonnet 4.6) consult a more capable advisor model (e.g. Opus 4.7)
mid-generation. The advisor sees the full transcript, produces strategic
guidance (~400–700 tokens of advice, ~1400–1800 total including thinking),
and the executor continues — all inside a single API call.

Beta header: `advisor-tool-2026-03-01`
Docs: <https://platform.claude.com/docs/en/agents-and-tools/tool-use/advisor-tool>

## Why it matters for Omega

This is Anthropic's answer to "I want Opus quality at Sonnet speed/cost":

- **Near-Opus quality at Sonnet rates.** The bulk of token generation happens
  at Sonnet pricing ($0.80/$4 per MTok). Only the advisor sub-inference bills
  at Opus rates ($5/$25 per MTok), and those calls are short (~1800 tokens).
- **Model decides when to consult.** The executor invokes the advisor like any
  other tool — no manual orchestration needed.
- **Zero extra round trips.** Everything happens server-side inside one
  `/v1/messages` request.

## How it works

1. Add the advisor tool to the `tools` array:
   ```typescript
   { type: "advisor_20260301", name: "advisor", model: "claude-opus-4-7" }
   ```
2. The executor emits a `server_tool_use` block (empty `input`).
3. Anthropic runs a separate inference on the advisor model, passing the full
   transcript (system prompt, tools, all turns).
4. The advisor's response comes back as an `advisor_tool_result` block.
5. The executor continues, informed by the advice.

## New content block types to handle

| Block type | Where | Notes |
|---|---|---|
| `server_tool_use` | Assistant content | `name: "advisor"`, `input: {}` |
| `advisor_tool_result` | Assistant content | Contains `advisor_result` (text) or `advisor_redacted_result` (encrypted) |
| `advisor_tool_result_error` | Assistant content | Error codes: `max_uses_exceeded`, `too_many_requests`, `overloaded`, `prompt_too_long`, `execution_time_exceeded`, `unavailable` |

## Implementation plan

### Phase 1 — Core integration

1. **Add advisor tool to the tools array** in `src/agent.ts` when a new config
   flag is enabled (e.g. `useAdvisor: true` + `advisorModel: "claude-opus-4-7"`).
2. **Add beta header** `advisor-tool-2026-03-01` to the `betas` array.
3. **Pass through new content blocks.** The advisor blocks are part of the
   assistant message and must be round-tripped verbatim in subsequent turns.
   Omega's `context.jsonl` persistence must serialize them as-is.
4. **Handle `advisor_redacted_result`** — opaque `encrypted_content` blob that
   must be round-tripped but cannot be displayed. On the next turn the server
   decrypts it for the executor.
5. **Usage tracking.** Advisor usage appears in `usage.iterations[]` as
   `{ type: "advisor_message", model: "...", input_tokens, output_tokens }`.
   Surface this in `llm_response` events and the turn footer.

### Phase 2 — UI

6. **Stream pause indicator.** The executor's stream pauses during advisor
   inference. Show a "consulting advisor…" indicator in the web UI.
7. **Advisor result display.** Show the advisor's text advice in a collapsible
   block (like thinking blocks) — useful for debugging but not primary content.
8. **Error display.** Show advisor errors as a soft warning, not a hard failure.

### Phase 3 — Configuration

9. **Model dropdown integration.** Add a "Sonnet + Opus advisor" composite
   option alongside the existing single-model choices.
10. **`max_uses` control.** Let the user cap advisor calls per request to
    control cost.
11. **Advisor caching.** Enable `caching: { type: "ephemeral", ttl: "5m" }` to
    cache the advisor's transcript across calls within a conversation.

## Caveats

- **Beta, requires account access.** May need to request access from Anthropic.
- **Streaming pause.** The advisor sub-inference does not stream — the
  executor's stream goes quiet (with SSE keepalives) until the advisor
  finishes. Short advisor calls may show no pings.
- **Multi-turn round-trip.** All `advisor_tool_result` blocks must be passed
  back on subsequent turns. If the advisor tool is removed from `tools`
  mid-conversation, all advisor result blocks must also be stripped from
  history — otherwise the API returns 400.
- **No conversation-level cap.** `max_uses` is per-request. To limit across a
  conversation, count client-side and remove the tool when the ceiling is
  reached.

## Valid executor/advisor pairs

| Executor | Advisor |
|---|---|
| Haiku 4.5 | Opus 4.7 |
| Sonnet 4.6 | Opus 4.7 |
| Opus 4.6 | Opus 4.7 |
| Opus 4.7 | Opus 4.7 |
