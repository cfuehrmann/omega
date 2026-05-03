# Backlog

Items are grouped by priority. Detailed plans live in `backlog/*.md`.

---

## P0 — Top priority

### TEST-ARCH — Test architecture & web-surface honesty

**[backlog/test-architecture.md](backlog/test-architecture.md)**

Umbrella plan for bringing every test surface in Omega onto a single, honest
pattern: test through the outermost user-visible surface of each binary; fake
only the LLM (Anthropic-shaped HTTP at `ANTHROPIC_BASE_URL`); let coverage of
orchestration modules flow down from the e2e tier; keep dedicated unit tests
only for leaf utilities (SSE parser, per-tool I/O). Six steps; TEST-ARCH-1
(CLI e2e), TEST-ARCH-2 (server WS-protocol tests), TEST-ARCH-3 (retire
`omega-mock-server`'s `MockProvider`), and TEST-ARCH-4 (retire `omega-agent`
MockProvider suite) are done. TEST-ARCH-5 and TEST-ARCH-6 are gated on the
Leptos rewrite.

---

## P2 — Medium priority

### SCHEMA-1 — Event field audit (error events first)

Review every event variant's fields for completeness and consistency. Priority:
`LlmErrorEvent` / `AgentErrorEvent` — currently no cross-reference to the
`llm_call` that triggered the error. Should carry context hashes or a call ID
for post-mortem diagnosis.

### SCHEMA-2 — "All retries exhausted" missing `llm_error`

When every retry is consumed, the final fallback yields a bare `agent_error`
with no `llm_error` event. The worst crash path has the least diagnostic
coverage. Fix: emit `llm_error` before `agent_error` on exhaustion.

---

## P3 — Low priority / deferred

### Advisor tool — blocked on `clear_tool_uses` compatibility

**[backlog/advisor-tool.md](backlog/advisor-tool.md)**

Anthropic's `advisor_20260301` beta pairs a Sonnet executor with an Opus
advisor for near-Opus quality at near-Sonnet cost. Deferred because the docs
explicitly state `clear_tool_uses` is "not yet fully compatible with advisor
tool blocks; full support is planned for a follow-up release." Omega uses
`clear_tool_uses_20250919` in production, so enabling the advisor today would
mean either disabling tool-result clearing (hurting long sessions) or accepting
undefined behaviour. Revisit when Anthropic ships the follow-up.

### SCHEMA-3 — Web server protocol errors not in `events.jsonl`

Three conditions in `web/server.ts` emit `{ type: "error" }` over WebSocket but
write nothing to `events.jsonl`. Decision needed: persist as `agent_error`, or
document as intentionally excluded.

### SCHEMA-4 — Persistence completeness audit

Document which events/signals are intentionally _not_ persisted, and why.
Known intentional omissions: streaming `text` fragments, old `status`/`metrics`
signals.

### SCHEMA-5 — Forward-compatibility policy

Document the Postel's Law contract: tolerant readers, additive writers, breaking
changes require migration.

### SCHEMA-6 — Schema reference document

After SCHEMA-1–5 are resolved, write `docs/schema.md`: the definitive reference
for every JSONL record.

### SCHEMA-7 — Session resume from persisted state

Load `context.jsonl` and `events.jsonl` from a previous session, restore
`llmContextView`, and continue. Depends on SCHEMA-6.

### TEST-1 — Evaluate snapshot testing

Investigate whether snapshot testing fits Omega's output surfaces (system prompt
assembly, event rendering, JSONL shapes). Write a short evaluation; if adopted,
add a proof-of-concept.

### WEB-1 — Auto-scroll

Feed should scroll to bottom on new content.

### WEB-2 — Live write_file preview

`eager_input_streaming` is already enabled on `write_file` and `edit_file`,
so input chunks arrive as the model generates them. A future enhancement could
stream those `input_json_delta` events to the web UI so the user sees file
content appearing line by line during a large write. This is a UX improvement
only — it does not reduce agent loop latency, since the tool result cannot be
sent until the full stream completes. Requires a streaming JSON parser to
decode partial content-field values, plus UI rendering of in-flight tool
inputs.

---

## Done / removed

| Item | Status | Notes |
|---|---|---|
| SYSPROMPT-2 — System prompt review | **Done** | Reviewed across multiple sessions. Remaining sub-questions (caching, test coverage) are minor and tracked implicitly. |
| INFRA-5 — `.omega/runtime/` namespace | **Removed** | Low value — the current layout is clear enough. |
| SESSION-3/4 — Strict/soft session resumption | **Superseded** by SCHEMA-7 | SCHEMA-7 covers the same ground with a clearer dependency chain. |
| SESSION-5 — Human-readable folder names | **Removed** | Nice-to-have with no urgency. |
| INFRA-1 — Structural invariant tests | **Removed** | The knip linter + gate already catch structural drift. |
| INFRA-2 — Abort-safe agentic loop | **Done** | Abort-after-tool-execution guard is implemented. History is always well-formed. |
| INFRA-3 — History validation | **Done** | Covered by INFRA-2's guard + server-side context management. |
| ARCH-1 — Clean provider boundary | **Removed** | OpenAI provider was removed entirely. Omega is Anthropic-only. |
| FEAT-1 — Extended thinking | **Done** | Adaptive thinking (`type: "adaptive"`) is active on all models. |
| FEAT-2 — OpenAI `previous_response_id` | **Removed** | OpenAI provider was removed. |
| FEAT-3 — Anthropic beta headers | **Done** | Beta headers are passed on all API calls. |
| Eager input streaming | **Done** | `eager_input_streaming: true` added to `write_file` and `edit_file`. Reduces first-chunk latency ~15 s → ~3 s for large file writes. No beta header needed. |
| Task budget (`task_budget` on `output_config`) | **Declined** | See [backlog/task-budget.md](backlog/task-budget.md). Advisory soft hint, not cost enforcement. Opus 4.7 only. Cost visibility already solved by per-turn/session display; user prefers efficiency over pre-committed budgets. Reopen criteria documented. |
| UX-1 / UX-2 — Hard-stop semantics + prompt queue | **Superseded & shipped** | Both replaced by pause/resume/interject. See [backlog/pause-resume-interject.md](backlog/pause-resume-interject.md). |
