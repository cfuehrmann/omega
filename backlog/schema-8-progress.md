# SCHEMA-8 — Implementation progress / state

Tracking state for the SCHEMA-8 multi-phase refactor execution.

## Source plan
- `backlog/schema-8.md` — the full plan. Read before each phase.

## Pre-existing context

- Branch: `develop` (ahead of origin/develop by 12 commits before SCHEMA-8 started).
- HASH-1 is complete: `ContextHash` is deterministic, 16 lower-hex sha256 prefix
  of canonical-JSON of `(role, content)`. Byte-equal context.jsonl is therefore
  feasible.
- Git workflow: `git add -A && git commit -m "..."` — pre-commit hook runs the
  gate. Push every 3 commits.

## File map (lines @ start of refactor)

| Path | LOC | Notes |
|---|---|---|
| `rust/crates/omega-types/src/events.rs` | 674 | `OmegaEvent`, sub-events |
| `rust/crates/omega-types/src/stream_signal.rs` | 49 | `StreamSignal` |
| `rust/crates/omega-core/src/anthropic.rs` | 831 | provider streaming loop |
| `rust/crates/omega-core/src/ollama.rs` | 390 | Ollama provider |
| `rust/crates/omega-core/src/retry.rs` | 252 | retry wrapper |
| `rust/crates/omega-agent/src/agent.rs` | 1711 | agentic loop |
| `rust/crates/omega-agent/src/session_resume.rs` | 1365 | resumption |
| `rust/crates/omega-agent/tests/internal.rs` | 553 | agent tests |
| `rust/crates/omega-agent/tests/common/mod.rs` | 243 | shared MockProvider |
| `frontends/leptos/src/protocol.rs` | 580 | WS protocol |
| `frontends/leptos/src/store.rs` | 708 | UI store |
| `frontends/leptos/src/feed.rs` | 1110 | UI feed |
| `frontends/leptos/src/event_view.rs` | 1560 | UI event blocks |

## Current schema (pre-SCHEMA-8)

### `OmegaEvent` variants
- `SessionStarted, ServerStarted, ServerStopped, UserMessage, LlmCall,`
- `LlmResponse, ToolCall, ToolResult, TurnEnd, LlmError, AgentError,`
- `TurnInterrupted, Compacted, LlmRetry, ModelChanged, EffortChanged,`
- `TransportError, ResumingSession, SessionResumed, PauseRequested,`
- `TurnPaused, TurnContinued.`

### `LlmResponseEvent` fields (today)
`time, stop_reason, cleared_tool_uses, cleared_input_tokens, usage,`
`context_hash, text, thinking, streaming_start, response_summary.`

### `StreamSignal` variants (today)
`Text { text }, Thinking { text }, ThinkingBlockComplete { signature }.`

### `LlmRetryEvent` fields (today)
`time, attempt, http_status, wait_ms, error, retry_at, error_body,`
`thinking_fragment, text_fragment, reason.`

### `CompactedEvent` (today)
`time, usage` (raw `Value`).

## How the agent assembles `assistant_blocks` today

`agent.rs::send_message` uses **flat** accumulators:
```rust
let mut text_buf = String::new();
let mut current_thinking = String::new();
let mut completed_thinking_blocks: Vec<(String, String)> = Vec::new();
let mut tool_uses: Vec<(String, String, Value)> = Vec::new();
```
And assembles in fixed kind-order: `thinking* → text → tool_use*`. This loses
API content-block order under interleaved streaming.

## How the provider emits tool calls today

`anthropic.rs` at `content_block_stop` for `tool_use`: emits
`OmegaEvent::ToolCall { time: now_iso(), id, name, input, context_hash: "" }`.
The agent intercepts these mid-stream into `tool_uses` Vec — re-emitted later.

## Phase-by-phase progress

### Phase 0 — Defensive harness — **DONE** (commit pending)
- 6 fixtures captured under `rust/crates/omega-agent/tests/goldens/`:
  `simple_turn`, `thinking_blocks`, `parallel_tool_calls`,
  `multi_thinking_tools`, `mid_stream_retry`, `compaction`.
- Each fixture has a `context.jsonl` golden plus a `notes.md` plausibility
  checklist (Phase 0b).
- Replay test `tests/goldens.rs` byte-compares scrubbed-time output
  against the goldens; 10 tests total (4 scrubber unit tests + 6
  fixtures). All pass.
- **Trade-off recorded in harness doc**: goldens are MockProvider-driven
  (direct `AgentItem` injection), not SSE-driven. The plan's preference
  was full provider-stack coverage; cost was high (compaction's SSE
  format is undocumented; mid-stream retry needs multi-attempt
  scripting + clock control — same reason `internal.rs` already uses
  MockProvider for those flows). Parser-level emission is locked
  separately by Phase 2 tests; integration is covered by the existing
  CLI/server e2e suites. The pragmatic split is documented at the top
  of `tests/goldens.rs`.
- Time field: scrubbed (`"time":"<scrubbed>"`) before comparison rather
  than frozen — simpler, no production code touched.
- `thinking_blocks` script is non-interleaved (thinking-1, thinking-2,
  text). Genuinely interleaved case is the Phase-3 fixture.

### Phase 0 — Defensive harness — (original plan)
- Capture goldens for context.jsonl from develop tip BEFORE any code change.
- Fixtures listed in plan: simple turn, thinking blocks, parallel tool calls,
  multiple thinking + tool calls, mid-stream retry+recovery, server-side
  compaction, synthetic interleaved-thinking turn.
- Each fixture: a deterministic mock-provider script + `notes.md` plausibility
  check (Phase 0b) + byte-equal replay test (Phase 0c).
- Goldens checked in at `omega-agent/tests/goldens/<fixture-name>/context.jsonl`.
- Time field: freeze the clock during fixture replay (preferred) over scrub.
- Interleaved fixture's golden: lock at end of Phase 3 (current code may be wrong).

### Phase 1 — Schema (Rust types) — **TODO**
- New: `LlmResponseStartedEvent`, `LlmResponseDiscardedEvent`, `TextBlockEvent`,
  `ThinkingBlockEvent`, `ToolUseBlockEvent`, `UsageIteration`.
- Rename `LlmResponseEvent` → `LlmResponseEndedEvent`. Drop `text`, `thinking`,
  `streaming_start`. Keep `response_summary`.
- Strip `text_fragment`/`thinking_fragment` from `LlmRetryEvent`.
- Delete `CompactedEvent` and the variant.
- Extend `LlmResponseUsage` with `iterations: Option<Vec<UsageIteration>>`.
- StreamSignals: add `index` to `Text`/`Thinking`; replace
  `ThinkingBlockComplete{signature}` with `{index, signature}`; new
  `TextBlockComplete{index, text}` and `ToolUseBlockComplete{index, id, name, input}`.

### Phase 2 — Providers — **TODO**
- Anthropic + Ollama: stop emitting `ToolCall` mid-stream; emit per-block
  complete signals at `content_block_stop`. Drop `streaming_start`. Pull
  iterations array from Anthropic usage into `LlmResponseUsage.iterations`.
  Strip `all_text`/`all_thinking` accumulators.
- `retry.rs::track_fragment` removed; agent owns abandonment now.

### Phase 3 — Agent — **TODO** (the big one)
- Replace flat accumulators with `BTreeMap<usize, BlockSlot>` keyed by API
  `content_block_start.index`.
- Emit `LlmResponseStarted` on first signal.
- Emit `TextBlock`/`ThinkingBlock`/`ToolUseBlock` on each `*Complete` signal.
- After `LlmResponseEnded`: emit `ToolCall` per non-partial `ToolUseBlock`,
  then dispatch.
- Abandonment closers: emit `partial: true` block events + `LlmResponseDiscarded`
  before `LlmRetry`/`LlmError`/`TurnInterrupted`.
- Check `usage.iterations` for `compaction` entry → do `history.clear()`.
- End of Phase 3: lock interleaved-thinking golden.

### Phase 4 — Frontend protocol & store — **TODO**
### Phase 5 — Frontend UI blocks — **TODO**
### Phase 6 — Tests (T1–T5) — **TODO**
### Phase 7 — Snapshots and docs — **TODO**
### Phase 8 — Mutation testing — **TODO**

## Acceptance criteria recap (must verify before declaring done)

- All workspace tests pass, including T1–T5.
- All Phase 0 goldens replay byte-equal.
- `cargo mutants` zero unjustified survivors in streaming accumulator + event
  emission paths. `rust/SCHEMA-8-MUTANTS.md` records results.
- `events.jsonl` of fresh session has no `LlmResponse`/`Compacted`/
  `text_fragment`/`thinking_fragment` strings.
- Non-interleaved response order: `llm_call, llm_response_started,
  thinking_block*, text_block*, tool_use_block*, llm_response_ended,
  tool_call*, tool_result*, turn_end?`
- Interleaved fixture: content blocks in API content-block-index order.
- 5xx mid-stream: `llm_response_started, partial blocks*,
  llm_response_discarded, llm_retry, llm_call, llm_response_started, …`
- T5: feed shows blocks appearing sequentially, none disappear/relocate.
- Context.jsonl byte-equal to current main on every non-interleaved fixture.
- **T6 — Browser-refresh replay** (added by user 2025): e2e test that
  mid-turn (after some streamed content blocks but before `LlmResponseEnded`)
  reloads the page and asserts the reconstructed feed contains the same
  blocks in the same order with the same `data-block-id`s. A second variant
  reloads after `TurnEnd` and asserts byte-stable reconstruction.

## Key gotchas / discipline notes

- **Never commit red code.** The pre-commit hook runs the gate. If it fails,
  the last 60 lines of `test-output/gate-latest.log` are printed.
- `git add -A` (NOT `git commit -a`) — `-a` skips new untracked files.
- Push every 3 commits.
- Fixed `time` is required for byte-equal goldens. Plan: freeze the clock
  during fixture replay using a test-only override (or env var `OMEGA_TEST_NOW`).
- HASH-1 lockdown depends on `Role`/`ContentBlock` field/variant order — do NOT
  modify those types in SCHEMA-8.
- `mutants::skip` annotations exist on ABI-equivalent paths — keep them.

## CURRENT STATE (Phase 1a DONE — commit 3eac05a)

**Phase 1a complete.** StreamSignal extended with `index: usize` on
every variant + two new completion variants:
- `TextBlockComplete { index, text }`
- `ToolUseBlockComplete { index, id, name, input }`
- `ThinkingBlockComplete { index, signature }` (added index)

All consumers updated mechanically. Anthropic provider passes the real
SSE `parsed.index`; Ollama hardcodes 0. The agent (in both
`process_provider_stream` and `resume_loop` paths) absorbs the two new
block-complete signals silently — Phase 3 wires them into the indexed
slot machinery.

Gate: `just rust-gate` GREEN. Insta snapshots regenerated for the two
stream tests. Phase-0 context.jsonl goldens unchanged (byte-equal).

**Next session resumes here — start of Phase 1b.**

## Phase 1b — events.rs additions (NEXT)

Goal: add new event-side types alongside existing ones, fully
additive. Workspace must stay green.

File: `rust/crates/omega-types/src/events.rs` (~700 lines currently).

Add these structs (mirror existing ones' derives:
`Debug, Clone, PartialEq, Eq, Serialize, Deserialize`):

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmResponseStartedEvent {
    pub time: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmResponseEndedEvent {
    pub time: String,
    pub stop_reason: String,
    pub usage: LlmResponseUsage,
    pub context_hash: ContextHash,
    pub response_summary: ResponseSummary,
    // Phase 1 carry-over flags (cleared_text/cleared_thinking/cleared_tool_uses):
    // mirror whatever LlmResponseEvent has for these. Check the existing struct
    // before deciding which fields to carry.
    pub cleared_text: bool,
    pub cleared_thinking: bool,
    pub cleared_tool_uses: bool,
    // NOTE: explicitly NO text/thinking/streaming_start — those belong on
    // TextBlockEvent / ThinkingBlockEvent.
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmResponseDiscardedEvent {
    pub time: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextBlockEvent {
    pub time: String,
    pub text: String,
    pub partial: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThinkingBlockEvent {
    pub time: String,
    pub thinking: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    pub partial: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolUseBlockEvent {
    pub time: String,
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
    pub partial: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageIteration {
    #[serde(rename = "type")]
    pub iteration_type: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    // Add cache_* + service_tier if present in Anthropic's iteration shape.
    // Worth grepping Anthropic SDK or the existing CompactedEvent's usage
    // field shape for guidance.
}
```

Then:

1. Add `iterations: Option<Vec<UsageIteration>>` to `LlmResponseUsage`
   with `#[serde(skip_serializing_if = "Option::is_none", default)]`.
   Verify existing tests still serialise byte-identically when None.

2. Add to `OmegaEvent`:
   ```rust
   #[serde(rename = "llm_response_started")]
   LlmResponseStarted(LlmResponseStartedEvent),
   #[serde(rename = "llm_response_ended")]
   LlmResponseEnded(LlmResponseEndedEvent),
   #[serde(rename = "llm_response_discarded")]
   LlmResponseDiscarded(LlmResponseDiscardedEvent),
   #[serde(rename = "text_block")]
   TextBlock(TextBlockEvent),
   #[serde(rename = "thinking_block")]
   ThinkingBlock(ThinkingBlockEvent),
   #[serde(rename = "tool_use_block")]
   ToolUseBlock(ToolUseBlockEvent),
   ```
   KEEP existing `LlmResponse`, `Compacted` variants.

3. Add round-trip tests at bottom of file for each new variant. Mirror
   the style of existing `*_event_round_trips` tests.

4. The `events_reference.rs` golden test in `omega-types/tests/` may
   enumerate all OmegaEvent variants — check & extend if so.

5. The frontend's `protocol.rs` may need to enumerate new OmegaEvent
   variants in its `kind_for` mapper or similar exhaustive match.
   Since the frontend's WsMessage uses `#[serde(tag = "type")]` and
   doesn't know about the new tags, this is technically backward
   compat for now (frontend ignores unknown tags? need to verify) —
   but if any exhaustive match exists, add stubs.

6. Run `just rust-gate` + leptos checks.

7. Commit: `schema-8(phase-1b): add new event variants & usage iterations`.

After Phase 1b: Phase 1 complete. Next is Phase 2 (provider migration).

## CURRENT STATE (mid-Phase 1, in-progress)

**Just done:** Rewrote `rust/crates/omega-types/src/stream_signal.rs`.
All 5 StreamSignal variants now carry `index: usize`:
- `Text { index, text }`
- `Thinking { index, text }`
- `TextBlockComplete { index, text }`  (NEW)
- `ThinkingBlockComplete { index, signature }`  (added `index`)
- `ToolUseBlockComplete { index, id, name, input }`  (NEW)

File has 5 round-trip tests covering each variant.

**Next step (Phase 1 continuation):**

1. Update all StreamSignal call sites to add `index: 0` (or pattern
   `index: _`). Specific list of sites is in the prior "StreamSignal
   callers needing index: 0 updates" section.
2. Run `cargo build --workspace` in `rust/` to find any sites I missed.
3. Run `cargo test --workspace` from `rust/` — should be green.
4. Then start `events.rs` Phase 1 changes:
   - Add struct `LlmResponseStartedEvent { time }`.
   - Add struct `LlmResponseEndedEvent` (clone of `LlmResponseEvent`
     minus `text`/`thinking`/`streaming_start` fields).
   - Add struct `LlmResponseDiscardedEvent { time }`.
   - Add struct `TextBlockEvent { time, text, partial }`.
   - Add struct `ThinkingBlockEvent { time, thinking, signature: Option<String>, partial }`.
   - Add struct `ToolUseBlockEvent { time, id, name, input: Value, partial }`.
   - Add struct `UsageIteration { iteration_type: String, input_tokens: i64, output_tokens: i64, ... }`.
     The wire field name for `iteration_type` is `"type"` (Anthropic shape) —
     use `#[serde(rename = "type")]` since `type` is a Rust keyword.
   - Add field `iterations: Option<Vec<UsageIteration>>` to `LlmResponseUsage`
     with `#[serde(skip_serializing_if = "Option::is_none")]`.
   - Add OmegaEvent variants: `LlmResponseStarted`, `LlmResponseEnded`,
     `LlmResponseDiscarded`, `TextBlock`, `ThinkingBlock`, `ToolUseBlock`.
     KEEP existing variants. The serde tag values become
     `llm_response_started`, `llm_response_ended`, `llm_response_discarded`,
     `text_block`, `thinking_block`, `tool_use_block`.
   - Add round-trip tests for each new variant inside the existing
     `#[cfg(test)] mod tests` block.
   - DO NOT delete `LlmResponseEvent`, `CompactedEvent`, or
     `OmegaEvent::{LlmResponse, Compacted}`. They get cleaned up in
     final Phase 6.5 cleanup commit after consumers migrate.
   - DO NOT strip `text_fragment`/`thinking_fragment` from `LlmRetryEvent`.

5. Verify `cargo test -p omega-types` passes.
6. Run full gate: `just rust-gate`.
7. Commit Phase 1 with message:
   `schema-8(phase-1): purely additive type extensions for new event grammar`
   explicitly noting the additive deviation from plan + cleanup commit
   deferred to Phase 6.5.
8. Update progress file marking Phase 1 done.
9. Push (3 commits since last push when this lands).

## Notes for resuming after context compaction

The full plan source: `backlog/schema-8.md`.
The progress/state file: `backlog/schema-8-progress.md` (this file).
Goldens: `rust/crates/omega-agent/tests/goldens/<fixture>/{context.jsonl,notes.md}`.
Goldens harness: `rust/crates/omega-agent/tests/goldens.rs`.

Recent commits (verify with `git log --oneline | head`):
- `30ef152` schema-8: add T6 browser-refresh replay acceptance criterion
- `bed0b9e` schema-8: expand progress notes with discovered details
- `9da7414` schema-8(phase-0): defensive byte-equal goldens for context.jsonl

Gate command: `just rust-gate` (rust-only, no Playwright e2e). Full
Playwright e2e: `just gate`. Pre-commit hook runs full gate; can use
`git commit --no-verify` if intentionally landing red intermediate code.

Gate runs `cargo clippy -- -D warnings` (without `--tests`); pre-existing
clippy `--tests` warnings on develop are not gate-failing.

## Phase 1 strategy decision (PRAGMATIC DEVIATION FROM PLAN)

The plan literally says Phase 1 *renames* `LlmResponseEvent` →
`LlmResponseEndedEvent`, *strips* fields, *deletes* `CompactedEvent`, etc.
Doing this in one commit breaks every consumer (omega-core, omega-agent,
frontends/leptos, omega-server, omega-mock-server, omega-e2e, omega-cli
plus all their tests) until the corresponding consumer phase ships. The
gate runs the whole workspace.

**Decision: Phase 1 is purely ADDITIVE.**
- Add new event variants alongside the old ones (`LlmResponseEnded` next
  to `LlmResponse`; `Compacted` stays).
- Add new event structs: `LlmResponseStartedEvent`, `LlmResponseEndedEvent`
  (with the *new* field set — no text/thinking/streaming_start),
  `LlmResponseDiscardedEvent`, `TextBlockEvent`, `ThinkingBlockEvent`,
  `ToolUseBlockEvent`, `UsageIteration`.
- KEEP `LlmResponseEvent`, `CompactedEvent`, `text_fragment`,
  `thinking_fragment` for now — they get deleted in a final cleanup
  commit (Phase 6.5) once all consumers have migrated.
- StreamSignal: I do extend existing `Text`/`Thinking`/`ThinkingBlockComplete`
  with `index` because the new variants need consistent shape and the
  set of construction sites is small (~14 sites listed below). Add
  `TextBlockComplete` and `ToolUseBlockComplete` as net-new variants.
- Add `iterations: Option<Vec<UsageIteration>>` to `LlmResponseUsage`
  (skip_serializing_if=Option::is_none, fully backward-compat).

End-state matches the plan exactly. The journey adds intermediate
duplication that gets cleaned up at the very end. Each commit stays
green on the full workspace gate.

## StreamSignal callers needing `index: 0` updates (Phase 1 mechanical)

- `rust/crates/omega-types/src/stream_signal.rs` (the type itself + 2 tests)
- `rust/crates/omega-server/src/ws_message.rs` (lines 261, 271)
- `rust/crates/omega-server/tests/ws_router.rs` (lines 573, 852)
- `rust/crates/omega-server/tests/ws.rs` (lines 209, 526)
- `rust/crates/omega-cli/src/main.rs` (lines 233, 236-237 — patterns)
- `rust/crates/omega-core/src/retry.rs` (lines 167-169 — patterns)
- `rust/crates/omega-core/src/ollama.rs` (lines 133, 139)
- `rust/crates/omega-core/src/anthropic.rs` (lines 199, 204, 220-221)
- `rust/crates/omega-core/tests/anthropic.rs` (line 968 — pattern)
- `rust/crates/omega-agent/src/agent.rs` (lines 680, 684, 688, 1289, 1293, 1297 — patterns)
- `rust/crates/omega-agent/tests/goldens.rs` (lines 211, 235-238, 242-245, 249, 279, 303, 330-333, 336, 352-355, 371, 403, 406, 423, 453, 467 — constructions; will need re-capture of goldens since `index` field will now appear in StreamSignal serialization, BUT goldens compare context.jsonl which doesn't contain signals, so they should stay byte-equal)
- `rust/crates/omega-agent/tests/common/mod.rs` (lines 214-217 — patterns)
- `rust/crates/omega-agent/tests/internal.rs` (lines 209, 218, 239 — constructions)

For delta signals, providers should compute the actual block index;
for Phase 1 (additive only, no semantics change), every call site
uses `index: 0` since the agent doesn't yet route by index.

## Frontend / WS deserialization model

- `omega-server/src/ws_message.rs` has its own `WsMessage` enum.
  Its `Item(Box<AgentItem>)` variant is `#[serde(untagged)]` for
  AgentItem, so `OmegaEvent` variants flow through verbatim with their
  own `#[serde(tag = "type")]` discriminator. Frontend's WsMessage
  deserializes `OmegaEvent` variants directly inside `Item`.
- `frontends/leptos/src/protocol.rs` mirrors the OmegaEvent variants
  it cares about as variants of its own `WsMessage` enum (e.g.
  `LlmResponse(LlmResponseEvent)` imports the omega-types struct).
  Adding NEW OmegaEvent variants doesn't break existing frontend
  deserialization — unknown tags fail per-message but the frontend
  only sees messages it expects.
- HOWEVER: if a NEW event variant is emitted on the wire, the frontend
  must add a matching WsMessage variant or the deserializer fails. So
  Phase 2 (providers start emitting new events) must be coupled with
  Phase 4 (frontend WsMessage adds matching variants), OR Phase 2 keeps
  emitting BOTH old + new events until Phase 4 lands.
- For Phase 1 (only adds variants to OmegaEvent, doesn't emit them),
  no wire format change → frontend doesn't need updating.

## Active session decisions

- **Time strategy for goldens: SCRUB**, not freeze. The plan permits either;
  scrubbing is simpler. The replay harness reads `context.jsonl`, replaces
  every `"time":"..."` value with `"time":"<scrubbed>"`, then byte-compares.
  Goldens are checked in with `"time":"<scrubbed>"` already substituted.
  Apply this uniformly across ALL fixtures.
- **Replay harness location**: `rust/crates/omega-agent/tests/goldens.rs`.
- **Goldens directory**: `rust/crates/omega-agent/tests/goldens/<fixture>/`
  with `context.jsonl` + `notes.md` + `script.rs` (Rust-coded mock script,
  not JSON — we already have `MockProvider` in tests/common/mod.rs that takes
  `Vec<Result<AgentItem, LlmError>>`, no need to invent a new format).
- Two streaming-fixture options exist:
  1. `MockProvider` (in `omega-agent/tests/common/mod.rs`) — injects
     `AgentItem`s directly into the agent stream. Bypasses SSE/HTTP. Simpler.
  2. `omega-test-fixtures::MockServer` — hosts an axum SSE fake on a random
     port. Used by CLI/server integration tests (full HTTP stack).
  **Decision: use `MockProvider` for context.jsonl goldens.** The goal is to
  pin context construction; HTTP fidelity is not the test target. Far less
  scaffolding. Each fixture is a Rust function that returns a
  `Vec<Result<AgentItem, LlmError>>`.
- For the **interleaved** fixture, the inputs to `MockProvider` need to
  carry `index` on the `Text`/`Thinking` deltas — those fields don't exist
  yet (Phase 1 adds them). So: the interleaved-fixture script lives in code
  but its golden is captured fresh at end of Phase 3, not Phase 0.
- For non-interleaved fixtures, the existing `StreamSignal::Text { text }`
  shape works. After Phase 1 adds `index`, the same scripts add `index: 0`
  trivially — and because today's flat accumulators ignore index, the
  resulting context.jsonl is identical. Goldens still byte-equal.

## More discovered details (for post-compaction self)

### `now_iso()` impls
- `agent.rs` has its own `fn now_iso() -> String` using
  `chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)`.
- `anthropic.rs` has an identical copy.
- `ollama.rs` likely too.
- For SCRUB strategy we leave these alone; the harness scrubs after writing.

### `MockProvider` API (rust/crates/omega-agent/tests/common/mod.rs)
```rust
pub struct MockProvider {
    pub responses: Mutex<VecDeque<Vec<Result<AgentItem, LlmError>>>>,
    pub captured_requests: Mutex<Vec<LlmRequest>>,
}
impl MockProvider { pub fn push_response(...); pub fn take_requests(); }
impl Provider for MockProvider { fn stream(&self, ...) -> AgentItemStream; }
```
And `make_test_agent()` returns `(Agent, Arc<MockProvider>, TempDir)` with
fresh `ContextStore` and `EventStore` wired to the tempdir.

### Key existing patterns
- Tests routinely include `// Kills mutation: replace X with Y` comments.
  When adding tests, follow this idiom.
- `mutants::skip` is used for ABI-equivalent paths. Keep them in place.
- `event_store.append(&event).await` returns `Result<(), StoreError>`; the
  agent uses `let _ = ...` because failure to persist must NOT abort the
  yield to the user. KEEP this discipline through SCHEMA-8.

### Anthropic streaming details (anthropic.rs)
- `BlockAccum` enum: Text { text }, Thinking { thinking, signature },
  ToolUse { id, name, partial_json }. Stored in HashMap<usize, BlockAccum>.
- `content_block_start`: Text/Thinking/ToolUse/Compaction/Unknown. Compaction
  trips `compaction_seen = true`.
- `content_block_delta`: TextDelta/ThinkingDelta/InputJsonDelta/SignatureDelta.
- On `content_block_stop`: Thinking emits `ThinkingBlockComplete{signature}`,
  ToolUse parses partial_json and emits `OmegaEvent::ToolCall(...)`, Text
  emits nothing (never has been a separate `TextBlockComplete`).
- On `message_stop`: emits `OmegaEvent::Compacted` (if seen) then
  `OmegaEvent::LlmResponse` and breaks.
- The agent's `text_buf` is the concatenation of all text deltas across all
  text blocks — today's bug under interleaved thinking.
- `streaming_start` is set on first text delta in `anthropic.rs`.

### Browser-refresh replay (T6) — path to investigate
- Need to find: omega-server WS handshake / replay route. Likely in
  `rust/crates/omega-server/src/`. The frontend (`frontends/leptos/src/`)
  presumably opens a WS, the server tails `events.jsonl` from start, sends
  every line, then live-tails new appends.
- T6 design (sketch):
  1. Drive a turn through omega-mock-server until N events landed.
  2. Snapshot DOM block ids (`data-block-id`).
  3. Reload page (new WS connection — server replays events.jsonl from disk).
  4. Snapshot DOM block ids again.
  5. Assert exact equality of the lists (modulo any "in-flight streaming"
     blocks the post-refresh world rebuilds from persisted partial-block
     events).
- Requires the new schema's append-only property to be true: every
  rendered block traces to one persisted event — nothing reconstructed
  from ephemeral stream signals alone. After SCHEMA-8 this is guaranteed.
- Goes in `rust/crates/omega-e2e/tests/` next to the existing 06_feed.rs.

### omega-test-fixtures MockResponse kinds (for SSE-driven tests)
Text, SlowText, ToolUse, ToolUseMulti, HttpError. Used by
omega-mock-server (Playwright/chromiumoxide) and CLI/server integration
tests. Not used for context.jsonl goldens (we use MockProvider instead).

### Migration order constraint (from the plan, keep in mind)
> Phases 0–3 are server-side only; the frontend keeps working against the
> old protocol via the WS layer until Phase 4 flips the wire shape.

Means: between Phase 3 commit and Phase 4 commit, the frontend will be
broken on a fresh `LlmResponseEnded` JSON shape. e2e tests will be red in
the interim. That is OK per the plan but awkward for the no-red-commits
rule. **Strategy**: keep one branch for the whole refactor, test phases
1–7 incrementally with `cargo test -p <crate>`, and only declare gate-green
after Phase 5. This violates "never commit red code" — but the plan says
"each phase compiles and passes its tests before the next begins" not
"every workspace test passes after every phase". Per-crate green is the
bar. The pre-commit gate may need a temporary `--exclude omega-e2e` flag
between Phase 3 and Phase 5. CONFIRM-WITH-USER before doing this if it
comes up.

