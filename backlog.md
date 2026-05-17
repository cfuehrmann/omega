# Backlog

Items are grouped by priority. Detailed plans live in `backlog/*.md`.

---

## P0 — Top priority

*(No open P0 items.)*

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


### TOKEN — Token-efficiency follow-ups

**[backlog/token-optimizations.md](backlog/token-optimizations.md)**

Tee-on-truncate (footer-always) has shipped across all tools.
Audit deferred until more sessions (incl. Harbor) accumulate.
**Active work item: item 2 — strip `\r`-progress / ANSI escapes in
`run_command` output** (~3.2 M tokens local, ~95 K bench).

Remaining backlog (post-audit):
1. Investigate `git add` 8.6 MB anomaly (pre-commit hook chatter)
4. Prompt nudge against full reads of large planning docs
5. Reduce shell-util usage in favour of native tools

(Item 3 — cap `wait_for_output` — is already done.)

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
| WEB-1 — Scroll tailing + jump-to-bottom button | **Done** | Auto-scroll tailing with ↓ button; `07_scroll.rs` e2e tests. |
| WEB-2 — Live `write_file` / `edit_file` preview | **Done** | Full tool-input streaming pipeline: `ToolUseBlockStart` + `ToolInput` signals emitted by `omega-core`, forwarded by `omega-agent`, rendered as live overlay in `StreamingPlaceholders`; settled view has inline expand toggle. |
| TEST-ARCH-5 — Leptos SSR snapshots | **Done** | Shipped in Phase 3.6. |
| TEST-ARCH-6 — Zero-missed workspace sweep | **Done** | Achieved in Phase 4 Step 5. See `backlog/test-architecture.md`. |
