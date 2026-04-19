# Backlog

Items are grouped by priority. Detailed plans live in `backlog/*.md`.

---

## P1 — High priority

### Advisor tool — Sonnet executor with Opus strategic guidance

**[backlog/advisor-tool.md](backlog/advisor-tool.md)**

Use Anthropic's `advisor_20260301` beta tool to pair Sonnet 4.6 (fast, cheap
executor) with Opus 4.7 (strategic advisor). The model decides when to consult
the advisor — no manual orchestration. Near-Opus quality at near-Sonnet cost.

Requires: beta access (`advisor-tool-2026-03-01`), new content block types in
persistence + UI, usage tracking for advisor sub-inferences.

### Task budget — cost protection for long sessions

**[backlog/task-budget.md](backlog/task-budget.md)**

Add `task_budget` to `output_config` to cap total token spend per session.
Server-side enforcement — when the budget is exhausted, the model stops.
Currently Omega has no cost guardrail.

### Eager input streaming — faster tool call delivery

**[backlog/eager-input-streaming.md](backlog/eager-input-streaming.md)**

Add `eager_input_streaming: true` to `write_file` and `edit_file` tool
definitions. Reduces first-chunk latency from ~15 s to ~3 s for large file
writes. GA, no beta header needed, low implementation effort.

---

## P2 — Medium priority

### UX-1 — Hard stop semantics

Define and implement clean abort semantics. Candidates: single Esc = soft abort
(finish current tool, stop); double Esc = hard kill. Today Esc aborts
unconditionally, but the abort-after-tool-execution guard (already implemented)
ensures history stays well-formed.

### UX-2 — Prompt queue / turn injection

Let the user type a follow-up message while a turn is in flight. Buffer it and
deliver at the next clean break (after current tool, before next API call).
Design questions: where is the buffer stored, how does the agent loop receive
it, does it inject into the current turn or start the next one, what's the UI
affordance.

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
