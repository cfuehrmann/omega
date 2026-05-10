# Backlog

Items are grouped by priority. Detailed plans live in `backlog/*.md`.

---

## P0 — Top priority

### WEB-1 — Scroll tailing + jump-to-bottom button ✅ DONE

**Migration defect.** The feed has no visible tailing / non-tailing indicator
and no way for the user to jump back to the bottom other than manually scrolling.

#### Desired behaviour

| Mode | Trigger | Effect |
|---|---|---|
| **Tailing** (default) | App start; reaching bottom; button click | Every new event and every streamed fragment scrolls the sentinel into view. No button shown. |
| **Non-tailing** | User scrolls up (leaves the `AUTOSCROLL_THRESHOLD_PX = 40 px` grace zone) | Content does **not** move as new events arrive. A ↓ button appears. |
| Return to tailing | Scroll back to bottom (within threshold) **or** click ↓ button | `auto_scroll` flips to `true`; sentinel scrolled into view immediately; button disappears. |

The ↓ button is a **1:1 indicator**: visible ↔ non-tailing. There is no other
mode indicator.

#### What already exists (don't re-implement)

- `auto_scroll: RwSignal<bool>` in `ConversationFeed` — already toggled
  correctly by the `on_scroll` handler via `should_autoscroll()`.
- `sentinel_ref: NodeRef<html::Div>` — the scroll target.
- `data-auto-scroll="true"|"false"` on `<section class="leptos-feed">` —
  already reflects the mode; the e2e harness can use it.
- `should_autoscroll()` pure function with full unit-test coverage.

#### What to build

1. **Button** — add inside the same `view!` block, conditionally rendered
   with `<Show when=move || !auto_scroll.get()>`. Clicking it sets
   `auto_scroll.set(true)` then calls `sentinel_ref.scroll_into_view()`.
   Give it `data-testid="scroll-to-bottom"` and `aria-label="Scroll to
   bottom"`.

2. **Layout** — the button must float over the feed. Wrap `<section
   class="leptos-feed">` and the button together in a `<div
   class="feed-wrapper">` with `position: relative; flex: 1; min-height:
   0; overflow: hidden`. Position the button `position: absolute; bottom:
   1rem; right: 1rem` inside the wrapper. The feed section keeps its
   existing `flex: 1; overflow-y: auto` but the `flex: 1` moves to the
   wrapper.

3. **CSS** — `.scroll-to-bottom-btn` styled as a circular icon button
   (Catppuccin surface1 background, overlay2 border, text colour, 2.2 rem
   diameter, `border-radius: 50%`, mild `box-shadow`, `font-size: 1.1rem`).
   Use the Unicode ↓ (`↓`) as the label; no SVG dependency.

4. **e2e test** — add `scroll_tailing` spec to
   `rust/crates/omega-e2e/tests/06_feed.rs` (or a new `07_scroll.rs`):
   - Load a session with ≥ 10 events (reuse mock-server scripting).
   - Assert `data-auto-scroll="true"` and button absent.
   - Programmatically scroll up via `page.evaluate("el.scrollTop = 0")`
     on the feed section.
   - Assert `data-auto-scroll="false"` and button present.
   - Dispatch a new event (send another mock turn).
   - Assert feed did **not** scroll (button still present).
   - Click the button.
   - Assert `data-auto-scroll="true"` and button absent.
   - Stream a new turn; assert feed follows (sentinel visible).

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

### HASH-1 — Deterministic content-derived `ContextHash`

**[backlog/hash-1.md](backlog/hash-1.md)**

Replace the current 6-byte random `ContextHash` with a deterministic
16-hex-char content hash (sha256 prefix of canonical-JSON of
`(role, content)`). Restores replay determinism, enables byte-equal
goldens for `context.jsonl`, and removes a long-standing misnomer.
No old-session compatibility — hard cutover. Includes lockdown tests
that freeze the canonical hash for a small set of fixtures and
mutation testing on `omega-store`. **SCHEMA-8 depends on this.**

### SCHEMA-8 — Append-only event grammar

**[backlog/schema-8.md](backlog/schema-8.md)**

Major refactor of the event schema to make `events.jsonl` strictly
append-only at the UI level. Replaces `LlmResponse` (interval-summary) with
an `LlmResponseStarted` / `LlmResponseEnded` pair plus per-content-block
events (`TextBlock`, `ThinkingBlock`, `ToolUseBlock`). Drops `Compacted`
(folded into `LlmResponseEnded.usage.iterations`). Re-purposes `ToolCall` to
agent-dispatch time. Folds in CTX-ORDER: replaces flat per-kind streaming
accumulators with an order-preserving block-index-keyed accumulator so
context.jsonl is correct under interleaved thinking. Hard cutover; no
backward compatibility. Gated on Phase 0 golden context.jsonl tests for
safety. **Depends on HASH-1.**

---

## P3 — Low priority / deferred

### TEST-ARCH-5 / TEST-ARCH-6 — Done

Both completed. TEST-ARCH-5 (Leptos SSR snapshots) shipped in Phase 3.6;
TEST-ARCH-6 (zero-missed workspace sweep) achieved in Phase 4 Step 5.
See [backlog/test-architecture.md](backlog/test-architecture.md).

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

### WEB-1 — Auto-scroll *(promoted to P0; see above)*

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
