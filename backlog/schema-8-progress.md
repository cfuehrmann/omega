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

### Phase 2 — Providers — **DONE** (2026-05-10)
- Anthropic + Ollama: stop emitting `ToolCall` mid-stream; emit per-block
  complete signals at `content_block_stop`. Drop `streaming_start`. Pull
  iterations array from Anthropic usage into `LlmResponseUsage.iterations`.
  Strip `all_text`/`all_thinking` accumulators.
- `retry.rs::track_fragment` removed; agent owns abandonment now.

### Phase 3 — Agent — **DONE** (2026-05-12, commits c10b72c..6e16ce9)
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

### Phase 4 — Frontend protocol & store — **DONE** (2026-05-13, commits 4c657a5..a3484a4)
### Phase 5 — Frontend UI blocks — **DONE** (2026-05-14, commits 7c74d51..6ed6044)
### Phase 6.5 — Legacy band-aid removal — **DONE** (commits 21ce5d8..08d9791)
### Phase 6 — Tests (T1–T5) — **DONE** (commits 370d66a..59b2b25)
### Phase 7 — Snapshots and docs — **DONE**
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

## CURRENT STATE (Phase 7 DONE — next: Phase 8 Mutation testing)

**Phase 7 complete.** Single commit on `develop`, all gates green:

- `schema-8(phase-7): SSR snapshot audit + docs/schema.md`
  - **Item 54 (SSR snapshot audit):** grep of
    `frontends/leptos/tests/snapshots/` for `\bllm_response\b`,
    `LlmResponseEvent\b`, and `\bcompacted\b` returned one match —
    `snapshots__snap_event_llm_response_ended_compacted.snap`. Inspection
    confirmed it is a false positive: the word `compacted` appears as the
    text content and CSS class of the Phase-5f `[compacted]` badge inside
    a correct `data-event-type="llm_response_ended"` block. No stale
    references to the removed `OmegaEvent::Compacted` or
    `LlmResponseEvent` types anywhere in the snapshot tree. No snapshot
    update needed; host-snapshots gate stayed at 39/39 green.
  - **Item 55 (`docs/schema.md`):** new file written from scratch at
    `docs/schema.md` (499 lines). Covers: all 26 `OmegaEvent` variants
    with wire-format tables and annotated JSON examples; camelCase-outer /
    snake_case-nested-usage gotcha on `LlmResponseEndedEvent`; `Option`
    fields without `#[serde(default)]` gotcha; `LlmCallEvent.cache_breakpoint_index`
    always-serialized gotcha; `usage.iterations` compaction-detection
    contract (replacing `OmegaEvent::Compacted`); `LlmRetryEvent` shape
    after 6.5c (no text_fragment/thinking_fragment); `StreamSignal`
    grammar; slot-assembly invariants (BTreeMap + empty-text-slot skip +
    abandonment flush); HASH-1 contract (sha256 first 8 bytes, 16
    lower-hex, canonical-form ABI); append-only DOM invariant +
    abandonment semantics (cross-links to `10_append_only.rs` and
    `09_refresh.rs`). Style matches `docs/internals.md`.

Gate counts post-Phase-7 (unchanged from Phase 6):

  - goldens: 11
  - defensive: 3
  - wasm tests: 376
  - host snapshots: 39
  - `just rust-gate`: green
  (e2e not re-run: docs-only change with no snapshot touches)

### Phase 8 resumption plan (Mutation testing)

Run `cargo mutants` against the two focal crates:

```
cd rust && cargo mutants -p omega-core --timeout 60
cd rust && cargo mutants -p omega-agent --timeout 600
```

Triage every survivor:
- **Real gap:** write a catching test, re-run.
- **Acceptable miss:** document in `rust/SCHEMA-8-MUTANTS.md` with
  explicit justification.

Target: zero unjustified survivors in:
- `omega-core/src/anthropic.rs` — streaming accumulator, signal emission
- `omega-agent/src/agent.rs` — event emission, abandonment closers,
  ToolCall dispatch

High-value mutation targets (per `backlog/schema-8.md` § Phase 8):
- Off-by-one on streaming block index assembly → must fail T2.
- Swap `LlmResponseEnded` / `ToolCall` emission order → must fail e2e.
- Replace `signature: Some(_)` with `None` on completed thinking block
  → must fail T1 or a golden.
- Skip `LlmResponseDiscarded` emission → must fail T5.
- Concatenate two thinking blocks → must fail T1 + T2.

Record results in `rust/SCHEMA-8-MUTANTS.md`:
total mutants per crate, caught, unviable, missed, per-miss justification.

--- end Phase-7 summary ---

**Phase 6 complete.** Three commits on `develop`, all gates green:

- `370d66a` schema-8(phase-6a): T1-T4 defensive tests for the slot-assembly
  contract.  New file `rust/crates/omega-agent/tests/defensive.rs` with
  three host-level tests (T1 signature preservation, T2 block order in
  context.jsonl, T3 events.jsonl ↔ context.jsonl cross-check).  T4 added
  as a module-doc annotation in `tests/goldens.rs` making the byte-level
  context.jsonl comparison contract explicit (HASH-1 determinism +
  frozen-time-via-scrubber).
- `09699d2` schema-8(phase-6b): tidy stale `LlmResponse`/`Compacted`
  doc-comments and `.expect(...)` messages left over from 6.5 in:
  omega-core/tests/{anthropic,ollama,retry}.rs +
  omega-e2e/tests/{03_markdown,06_feed}.rs.  Comment-only; no functional
  change.  Historical "Phase N.M: ... removed" notes preserved.  Also
  reviewed mock-server `MockResponse` handling (item 51): `Text` →
  `build_text_sse`, `ToolUse` → `build_tool_use_sse`, `SlowText` →
  `sse_slow_text_response`, `ToolUseMulti`, `HttpError` — all emit the
  current SCHEMA-8 SSE shapes; no legacy fields anywhere.
- `59b2b25` schema-8(phase-6c): T5 append-only DOM invariant in
  `omega-e2e/tests/10_append_only.rs`.  Uses `launch_with_ws_spy +
  inject_ws_frame` to synthesise a multi-block stream + mid-stream
  abandon + retry sequence directly at the WS level, snapshots the set
  of `data-block-id` attrs after every injection, and asserts the sets
  are monotonically non-decreasing.  Design rationale captured in the
  test file's module doc: the mock server has no `MockResponse` variant
  for "partial stream then abort" (HttpError aborts before any SSE
  streams), so inject-frame is the only path that exercises the
  partial-flagged TextBlock/ThinkingBlock + LlmResponseDiscarded
  sequence end-to-end.  Same pattern as `08_modal_esc.rs`.

Gate counts post-Phase-6:

  - goldens: 11 (unchanged — defensive tests are additive)
  - defensive: **3** (NEW — T1 signature preservation, T2 block order,
    T3 events↔context cross-check)
  - wasm tests: 376 (unchanged)
  - host snapshots: 39 (unchanged)
  - e2e: **52** (was 51, +1 for T5 in `10_append_only.rs`)
  - `just rust-gate`: green throughout

### What T1–T5 actually pin

| Test | Scope | What it would catch |
|---|---|---|
| T1 | host (defensive.rs) | Signature merging or swapping across two distinct ThinkingBlock slots — fixture uses sentinel signatures `SIG-ALPHA-...`/`SIG-BETA-...`, both bytes asserted byte-equal to input. |
| T2 | host (defensive.rs) | Reorder of assistant content blocks in `context.jsonl` away from emission order; fixture interleaves `thinking(0) → text(1) → thinking(2) → text(3) → tool_use(4)` with `stop_reason="end_turn"` so the tool slot lands in context but is not dispatched. |
| T3 | host (defensive.rs) | `events.jsonl` block events drifting from `context.jsonl` ContentBlocks — same fixture as T2; loops through both lists in lockstep and asserts text/signature/id/name/input bytes match. |
| T4 | host (goldens.rs) | Quiet byte-shifts in any of the 11 captured fixtures — Phase 0 already enforces this; T4 is a module-doc annotation tying byte-equality to HASH-1 + scrubbed-time so the contract is discoverable. |
| T5 | e2e (10_append_only.rs) | Any DOM rendering path that removes/reorders an event block under abandon+retry; partial-flagged blocks from the abandoned stream remain visible after the retry's blocks land. |

### Cleanup pass (Phase 6 item 50–51) — landed in `09699d2`

All edits are comment-only; no code paths or selectors changed.  Files
touched:

- `rust/crates/omega-core/tests/anthropic.rs` — ~10 doc-comments and
  `.expect(...)` messages re-pointed from `LlmResponse` to
  `LlmResponseEnded`, one Phase-6.5a comment de-mangled for readability.
  Historical `// Phase 6.5: OmegaEvent::Compacted removed...` notes kept.
- `rust/crates/omega-core/tests/ollama.rs` — same treatment, 3 sites.
- `rust/crates/omega-core/tests/retry.rs` — 1 comment.
- `rust/crates/omega-e2e/tests/03_markdown.rs` — file-header doc + test
  doc-comment re-pointed from `llm_response` to `text_block`.
- `rust/crates/omega-e2e/tests/06_feed.rs` — same treatment, 3 sites.

### Phase 7 plan (Snapshots & docs — 2 items from `backlog/schema-8.md`)

54. **Refresh Leptos SSR snapshots** if any reference old block shapes.
    Likely a no-op given the host-snapshots gate stayed at 39/39 through
    Phases 4–6, but worth a one-shot grep through
    `frontends/leptos/tests/snapshots/__snapshots__/` for `llm_response`
    (without `_started`/`_ended`/`_discarded` suffix), `compacted`, or
    `LlmResponse` (the old struct name) before declaring done.
55. **Write `docs/schema.md`** — the file does **not** exist (only
    `docs/internals.md` is present).  Document the post-SCHEMA-8 wire
    shape:
    - `OmegaEvent` variants (26 total — list in §"What the type surface
      looks like now after Phase 6.5" below).
    - `LlmResponseEndedEvent` (camelCase on wire; snake_case in Rust),
      including the `usage.iterations` compaction-detection contract.
    - `LlmRetryEvent` shape after 6.5c (no text_fragment/thinking_fragment).
    - `TextBlockEvent` / `ThinkingBlockEvent` / `ToolUseBlockEvent` shapes
      with `partial: bool`.
    - StreamSignal grammar (Text{index}, Thinking{index},
      TextBlockComplete{index, text}, ThinkingBlockComplete{index, signature},
      ToolUseBlockComplete{index, id, name, input}).
    - Slot-assembly invariants (BTreeMap<usize, BlockSlot> keyed by API
      content_block_start.index; empty Text slots skipped at flush;
      assembly order = key order = emission order).
    - HASH-1: `ContextHash = first 16 lower-hex of sha256(canonical_json(
      (role, content)))`, why byte-equal context.jsonl is feasible.
    - Append-only DOM invariant + abandonment semantics.

Phase 8 (mutation testing) follows: run `cargo mutants -p omega-core
--timeout 60` and `cargo mutants -p omega-agent --timeout 600`; triage
survivors; record in `rust/SCHEMA-8-MUTANTS.md`.

### Carry-overs to Phase 7 (none)

Nothing from Phase 6 carries over.  All five defensive tests landed and
run on every gate.

--- end Phase-6 summary ---

## CURRENT STATE (Phase 6.5 DONE — next: Phase 6 T1–T5 defensive tests) [historical — superseded by Phase 6 above]

**Phase 6.5 complete.** Two commits on `develop` (all Phase 6.5 items
merged into one because they all touch the same files), all gates green
(`just rust-gate` + `cargo test -p omega-e2e --tests -- --ignored
--test-threads=1` + `cargo test -p omega-agent --test goldens`):

- `21ce5d8` schema-8(phase-6.5a-d): drop OmegaEvent::Compacted,
  ::LlmResponse, LlmResponseEvent, CompactedEvent + band-aid.
  All four sub-items landed together because they touch the same files.
  - **6.5a** – OmegaEvent::Compacted + CompactedEvent removed everywhere
    (events.rs, agent.rs, session_resume.rs, common/mod.rs, internal.rs,
    goldens.rs, protocol.rs, event_view.rs, feed.rs).  The
    compaction-detection golden `script_compaction()` now signals
    compaction via `LlmResponseEndedEvent.usage.iterations`
    (type=="compaction") instead of emitting a Compacted item.
    The internal test `compacted_event_clears_history_and_persists_usage`
    rewritten to use iterations-in-usage for compaction detection.
  - **6.5b** – LlmResponseEvent.{text,thinking,streaming_start} deleted;
    struct was then identical to LlmResponseEndedEvent so it was deleted
    outright.  OmegaEvent::LlmResponse variant deleted.  Agent
    band-aid (lr_text_parts / lr_thinking_parts / bandaid_text /
    bandaid_thinking / legacy LlmResponse emission) removed from both
    send_message and perform_resumption.  Anthropic + Ollama providers
    now emit OmegaEvent::LlmResponseEnded as the terminal event
    (context_hash still filled by the agent after writing the context
    record).  session_resume.rs / project_turn now accumulates TextBlock
    events and flushes on LlmResponseEnded instead of reading
    LlmResponse.text.  store.rs test
    llm_response_event_is_inert_post_phase_4d deleted.
  - **6.5c** – LlmRetryEvent.{text_fragment,thinking_fragment} removed
    from the struct and from retry.rs construction.
  - **6.5d** – CompactedEvent type gone (subsumed by 6.5a).
- `08d9791` schema-8(phase-6.5e): e2e feed test selector fixed —
  data-event-type="llm_response" → "llm_response_ended", kind
  "assistant" → "status" (LlmResponseEnded is EventKind::Status).

Gate counts post-Phase-6.5:

  - goldens: 11 (unchanged — 6.5 is server/agent-side;
    context.jsonl stays byte-equal)
  - wasm tests: 376 (was 378; lost 2: llm_response_event_is_inert test
    + kind_compacted_is_status test)
  - host snapshots: 39 (unchanged)
  - e2e: 51 (unchanged — 06_feed.rs selector updated in 6.5e)
  - `just rust-gate`: green throughout

### What the type surface looks like now after Phase 6.5

- **OmegaEvent variants (26 total):** SessionStarted, ServerStarted,
  ServerStopped, UserMessage, LlmCall, LlmResponseStarted,
  LlmResponseEnded, LlmResponseDiscarded, TextBlock, ThinkingBlock,
  ToolUseBlock, ToolCall, ToolResult, TurnEnd, LlmError, AgentError,
  TurnInterrupted, LlmRetry, ModelChanged, EffortChanged,
  TransportError, ResumingSession, SessionResumed, PauseRequested,
  TurnPaused, TurnContinued.
- **LlmResponseEndedEvent fields:** time, stop_reason,
  cleared_tool_uses, cleared_input_tokens, usage (LlmResponseUsage
  with optional iterations), context_hash, response_summary.
- **LlmRetryEvent fields:** time, attempt, http_status, wait_ms,
  error, retry_at, error_body, reason. (text_fragment +
  thinking_fragment removed.)
- **Agent event loop:** The provider now emits
  OmegaEvent::LlmResponseEnded as the terminal event (with
  context_hash: String::new()); the agent fills in context_hash after
  writing the context record, then persists and emits.  No duplicate
  LlmResponse emission.
- **session_resume.rs extract_resumption_basis:** reads TextBlock
  events and flushes accumulated text on LlmResponseEnded.  Old
  sessions with llm_response entries in events.jsonl are harmlessly
  skipped by the filter_map deserialization in router.rs.

### Carry-overs to Phase 6 (none remaining from 6.5)

Nothing from Phase 6.5 carries over.  The band-aids are gone.

--- end Phase-6.5 summary ---

## CURRENT STATE (Phase 5 DONE) [historical — superseded by 6.5 above]

**Phase 5 complete.** Eight commits on `develop`, all gates green
(`just rust-gate` + `cargo test -p omega-e2e --tests -- --ignored
--test-threads=1` + `cargo test -p omega-agent --test goldens`):

- `7c74d51` schema-8(phase-5a): per-index streaming buffer; drop
  `StreamingTail`. `streaming_text`/`streaming_thinking` become
  `RwSignal<BTreeMap<usize, String>>`. Per-index `<For>` renders one
  `data-testid="leptos-streaming-text"` (or `-thinking`) `<div>` per
  in-flight content-block index. Drain via `pop_first()` on TextBlock /
  ThinkingBlock event (relies on Anthropic's start-order block completion
  guarantee, so no `index` field on event types needed). Server already
  forwarded `index` since Phase 1a — only the leptos `protocol.rs` mirror
  needed adding. 51/51 e2e on first run, including both previously-flaky
  07_scroll tests.
- `4a51636` schema-8(phase-5b): "discarded" styling on partial TextBlock /
  ThinkingBlock / ToolUseBlock. `data-partial="true"` on the outer
  EventBlock wrapper (dashed border), `block-discarded-header` shows
  `"Discarded — N chars text"` / `"Discarded thinking — N chars"` /
  `"Discarded tool_use — name"`, and `block-discarded-body` (opacity
  0.55, line-through) on the inner content.
- `3a7cf01` schema-8(phase-5c): ThinkingBlockBlock gets an `[expand]`
  button (`data-testid="leptos-thinking-block-expand"`) opening TextModal
  with the full thinking text. Layout adds `block-label-row` wrapper.
  Works for both partial and non-partial.
- `d909c4c` schema-8(phase-5d): ToolUseBlockBlock's entire
  `block-label-row` is clickable to open TextModal with the
  pretty-printed full input JSON. Modal title
  `"tool_use payload — {name}"` (or `— (discarded) —` for partial).
- `5170e36` schema-8(phase-5e): slim ToolCallBlock to just `tool_call
  <name>` + `id=<id>` meta (no preview, no on:click — full input now
  lives on the ToolUseBlock sibling). `assign_tool_corr` extended to
  ALSO assign corrs to ToolUseBlock by `id`: a triple (ToolUseBlock,
  ToolCall, ToolResult) sharing an id all show the same corr badge
  when 2+ tool-calls exist in the group. Adapted `06_feed.rs`
  (`multi_tool_turn_renders_every_family`) and `08_modal_esc.rs`
  (`text_modal_esc_closes`) to point at the new selectors.
- `693dc96` schema-8(phase-5f): `[compacted]` badge on
  LlmResponseEndedBlock when `usage.iterations` contains a
  `iteration_type == "compaction"` entry. `data-testid="leptos-compacted-badge"`,
  `block-badge-compacted` CSS (yellow palette).
- `f6b3172` schema-8(phase-5g): `N partial blocks` count on
  LlmResponseDiscardedBlock. Helper `assign_partial_counts(&events) ->
  Vec<Option<usize>>` mirrors `assign_tool_corr` shape; counter resets
  at LlmResponseStarted/Ended/Discarded; only LlmResponseDiscarded
  indices get `Some(N)`. Wired through the `<For>` mapping as a new
  `partial_count` prop on `EventBlock`.
- `6ed6044` schema-8(phase-5h): cleanup pass — drop two stale
  `StreamingTail` doc references in `style.css` + `STYLE-MAPPING.md`.
  No code change. Phase-5-introduced clippy / build are warning-free;
  the two pre-existing warnings (`context_modal.rs:252`,
  `usage_panel.rs:87`) are NOT from SCHEMA-8 and the gate doesn't
  scope clippy to the leptos crate.

Gate counts post-Phase-5 (vs end of Phase 4):

  - host snapshots: 39 (was 29; +10 across 5a–5g; several existing
    snapshots reblessed for structural changes)
  - wasm tests: 378 (was 364; +14 across 5a–5g — mostly
    `assign_tool_corr` extension tests in 5e and `assign_partial_counts`
    tests in 5g)
  - e2e: 51 (unchanged — 5a updated the two 07_scroll "flakes" which
    now pass deterministically; 5e adapted 06_feed +
    08_modal_esc to the new slim-ToolCall + modal-on-ToolUseBlock
    selectors)
  - goldens: 11 (unchanged — Phase 5 was frontend-only;
    `context.jsonl` untouched)
  - `just rust-gate`: green throughout

### What the frontend looks like now after Phase 5

- **Streaming overlay** is per-content-block-index: one
  `<div data-testid="leptos-streaming-text">` (or `-thinking`) per
  in-flight index. Drains as TextBlock/ThinkingBlock events land in
  the events vec. `StreamingTail` deleted.
- **Partial blocks** (TextBlock/ThinkingBlock/ToolUseBlock with
  `partial: true`) carry `data-partial="true"` on the EventBlock
  wrapper, render a yellow "Discarded — …" header, and grey/strike
  the body content. Operator can tell at a glance which content was
  abandoned mid-stream.
- **ThinkingBlock** has an `[expand]` button that opens TextModal
  with the full thinking text. Works for both partial and non-partial.
- **ToolUseBlock** — the entire label row is clickable to open
  TextModal with the pretty-printed full input JSON.
- **ToolCallBlock** — slim. Just `tool_call <name>` + `id=<id>` meta
  + optional corr-badge. No preview, no modal click. The full input
  is now solely on the ToolUseBlock sibling.
- **Correlation** — ToolUseBlock + ToolCall + ToolResult sharing an
  `id` all show the same corr-badge when 2+ tool-calls exist in the
  LlmCall group. Single-tool groups still suppress the badges.
- **LlmResponseEndedBlock** carries a `[compacted]` badge when the
  response triggered context compaction (yellow, in the label row).
- **LlmResponseDiscardedBlock** shows `N partial blocks` meta. `N=0`
  means a network blip with no streamed content; `N>0` means
  visible struck-through partials above this row.

### What was untouched in Phase 5 (cleaned up in Phase 6.5)

All of the following were removed in Phase 6.5:

- ~~`LlmResponseEvent.{text, thinking, streaming_start}` + the agent band-aid~~ → **DONE Phase 6.5b**
- ~~`OmegaEvent::Compacted` + `CompactedEvent`~~ → **DONE Phase 6.5a**
- ~~`LlmRetryEvent.{text_fragment, thinking_fragment}`~~ → **DONE Phase 6.5c**

--- end Phase-5 summary (historical) ---

## Original Phase 4 commits (kept for trail)

**Phase 4 complete.** Five commits on `develop`, all gates green
(`just rust-gate` + `cargo test -p omega-e2e --tests -- --ignored
--test-threads=1` + `cargo test -p omega-agent --test goldens`):

- `4c657a5` schema-8(phase-4a): store routes LlmResponseStarted →
  clear streaming buffers
- `898ef01` schema-8(phase-4b): TextBlock/ThinkingBlock/ToolUseBlock
  renderers + data-block-id on EventBlock + per-block streaming-
  buffer drain side-effects
- `11b9d4a` schema-8(phase-4c): mute legacy LlmResponseBlock; promote
  LlmResponseEnded to the assistant header block; LlmResponseDiscarded
  marker; 06_feed + 04_composer selectors point at text_block
- `2e6395b` schema-8(phase-4d): drop OmegaEvent::LlmResponse arm from
  store::apply_event_side_effects (legacy event is inert in the store;
  renderer arm returns ().into_any() to keep the empty wrapper)
- `a3484a4` schema-8(phase-4e): T6 browser-refresh replay test
  (`rust/crates/omega-e2e/tests/09_refresh.rs` —
  data_block_ids_stable_across_reload_post_turn_end) + harness gains
  `TestHarness::reload()`

What the frontend looks like now:

- **Store** (`frontends/leptos/src/store.rs`):
  - `LlmResponseStarted` clears streaming_text/streaming_thinking.
  - `TextBlock` clears streaming_text (the persisted block takes
    over from the in-flight buffer); `ThinkingBlock` clears
    streaming_thinking. Both partial and non-partial variants.
  - `LlmResponseEnded` and `LlmResponseDiscarded` both clear the
    streaming buffers.
  - The legacy `OmegaEvent::LlmResponse` is inert — it still arrives
    on the wire (band-aid until Phase 6.5) but no longer triggers
    any side-effect or visible body. The test
    `legacy_llm_response_event_is_inert_in_store` locks this.
  - All new events flow through the catch-all `into_omega_event`
    branch and land in the `events` Vec; their position in that Vec
    is the `data-block-id`.

- **Feed** (`frontends/leptos/src/feed.rs`):
  - `EventBlock` carries `data-block-id` from the `<For>` index
    (prop is `Option<usize>`; harness omits it ⇒ no attribute).
  - `TextBlockBlock` renders `MarkdownBody` inside
    `<div data-testid="leptos-assistant-text">`; `[partial]` indicator
    when the partial flag is set. This is now the sole owner of the
    assistant-text rendering surface.
  - `ThinkingBlockBlock` renders thinking content + signature length;
    `[partial]` marker on partial.
  - `ToolUseBlockBlock` renders tool name + input via
    `tool_call_preview`; `[partial]` marker on partial.
  - `LlmResponseEndedBlock` is the new assistant header — label
    "assistant" + stop_reason + [context]/[payload] buttons +
    `<div data-testid="leptos-assistant-usage">…</div>` usage line.
    NO `[thinking]` button (thinking lives in ThinkingBlock siblings).
    NO body markdown (lives in TextBlock siblings).
  - `LlmResponseDiscardedBlock` is a minimal "[response discarded]"
    marker.
  - Legacy `LlmResponseBlock` component is DELETED. Its match arm in
    `render_event_body` returns `().into_any()` — the EventBlock
    wrapper with `data-event-type="llm_response"` still exists
    (empty body), so any selector relying on that boundary still
    works.

- **kind_for** (`frontends/leptos/src/event_view.rs`):
  - TextBlock/ThinkingBlock/ToolUseBlock → Assistant
  - LlmResponseStarted/Ended/Discarded → Status (touched in 3b only;
    NOT changed in Phase 4 — these blocks are metadata/affordances
    around assistant content, distinct from the assistant body)

- **Snapshot fixtures** (`frontends/leptos/tests/snapshots.rs`):
  - `ev_assistant(text)` now emits a `TextBlock` event (was
    `LlmResponseEvent`). The 7 assistant_* snapshots reblessed to
    show the slimmer TextBlock wrapper.
  - New: `ev_llm_response_ended()` helper + `snap_event_llm_response_ended`
    snapshot to lock the assistant-header rendering.
  - 8 reblessed + 1 new = 29 total host snapshots, all green.

- **E2e selectors**:
  - 06_feed.rs: queries `[data-event-type="text_block"]` for the
    final assistant text (was `[data-event-type="llm_response"]`).
  - 04_composer.rs: same selector update for `composer_send_pong`.
  - 03_markdown.rs: untouched — its `[data-testid="md-body"]` query
    still works since TextBlock owns the markdown surface now.
  - 09_refresh.rs: new (T6).

- **Test harness** (`rust/crates/omega-e2e/src/lib.rs`):
  - `TestHarness::reload()` wraps `page.reload()` and re-waits for
    `<main data-connected="true">`.

What's still on the wire / in the codebase that the new path doesn't
use (slated for Phase 6.5 cleanup):

- Agent's `LlmResponse.text` / `.thinking` band-aid still populates
  the legacy field on every `LlmResponseEnded` (so the legacy event
  still arrives populated; the store just ignores it now).
- `OmegaEvent::Compacted` MockProvider-driven legacy variant still
  handled in the agent. Real providers emit compaction via
  `usage.iterations` (Phase 2+3 work).
- `LlmResponseEvent.text/.thinking/.streaming_start`,
  `LlmRetryEvent.{thinking_fragment, text_fragment}`,
  `CompactedEvent`, `OmegaEvent::Compacted` variant — all kept until
  Phase 6.5.

Gate verification: GREEN on all of
  - `just rust-gate`
  - `cargo test -p omega-agent --test goldens` (11 passed)
  - `cargo test -p omega-e2e --tests -- --ignored --test-threads=1`
    (51 passed including both known 07_scroll flakes)
  - `cargo test --target wasm32-unknown-unknown -p leptos --lib`
    (364 passed — includes 8 new store tests for Phase 4)
  - `cargo test --test snapshots --no-default-features --features
    ssr` (29 host snapshots)

## Phase 5 — Frontend UI blocks (NEXT)

Goal: polish the per-block UI affordances. Phase 4 landed the wiring
(events flow into the store, block events render their content,
header/closer events render their meta affordances). Phase 5 adds the
visual polish + the bits that Phase 4 explicitly deferred:

### Per `backlog/schema-8.md` § Phase 5 (items 39-49)

- **39. Remove `StreamingTail` component.** Today's overlay renders
  the global `streaming_text` / `streaming_thinking` accumulators as
  a single tail at the bottom of the feed. Replace with per-block
  placeholder rendering driven by per-INDEX streaming buffers (the
  wire StreamSignal `Text`/`Thinking` variants carry `index` since
  Phase 1a). This requires:
  - Server WS: forward `index` field on the Text/Thinking signals
    (currently the wire WsMessage::Text/Thinking only has `text`).
  - Store: replace `streaming_text: RwSignal<String>` with
    `streaming_text: RwSignal<HashMap<usize, String>>` (or BTreeMap).
    Append per-index. Drain entry when matching TextBlock arrives.
  - Feed: render placeholders for in-flight indices interleaved with
    persisted block events.
  - This is the harder Phase 5 commit. Touches protocol.rs +
    server-side WsMessage + store + feed.

- **40. `LlmResponseStartedBlock`** — currently a stub returning
  `().into_any()`. Real treatment: discreet header (timestamp or
  small "↳ assistant" pill), resolves to nothing visible once
  `LlmResponseEnded` arrives. Or stays invisible — the spec is
  intentionally loose. Probably leave as `().into_any()` and let the
  TextBlock/ThinkingBlock/ToolUseBlock siblings carry visual identity.

- **41. `TextBlockBlock` (DONE in 4b) — polish**:
  - Already renders MarkdownBody + leptos-assistant-text wrapper.
  - TODO: per-index live-streaming buffer pickup (item 39's
    consequence). When the persisted block hasn't landed yet, render
    from `streaming_text[index]`.
  - TODO: "discarded" styling for partial:true (currently just shows
    "[partial]"). Strike-through / greyed out per the spec.

- **42. `ThinkingBlockBlock` (DONE in 4b) — polish**:
  - Collapsed-by-default accordion. Today it's a plain `<details>`
    with summary "thinking". Need the modal affordance: long thinking
    content opens in `TextModal`.
  - "discarded" indicator on partial (signature: None).

- **43. `ToolUseBlockBlock` (DONE in 4b) — polish**:
  - Today shows name + `tool_call_preview` input. Need: full input via
    `TextModal` on click. "discarded" styling for partial:true.
  - NOTE: this is rendered IN ADDITION to the actual `ToolCall` event
    (which dispatches the tool). Phase 6.5 may consolidate.

- **44. `LlmResponseEndedBlock` (DONE in 4c) — polish**:
  - Has [context] + [payload] buttons + usage line. Drop `[thinking]`
    button — done.
  - TODO: `[compacted]` badge when `usage.iterations` contains a
    compaction entry. Click → iterations breakdown modal.

- **45. `LlmResponseDiscardedBlock` (DONE in 4c) — polish**:
  - Today: minimal `[response discarded]` text marker. Spec wants
    "response discarded — N partial blocks above" — needs to count
    sibling partial blocks (requires lookback in the store, or pass
    sibling-count via render context).

- **46. Update `ToolCallBlock`**: slim it (name + correlated id only).
  Full input is now on `ToolUseBlockBlock`. Click correlates to
  `ToolResultBlock` below by id.

- **47. `ToolResultBlock` — leave as-is unless polish needed**.

- **48. EventBlock dispatcher + kind_for** — Phase 4 already wired
  these. Phase 5 polish: maybe re-classify LlmResponseEnded → Assistant
  if the visual identity warrants (it shows label "assistant"). Decide
  in Phase 5.

- **49. kind_for** — Phase 4 left the new variants as Status
  (Started/Ended/Discarded) and Assistant (TextBlock/ThinkingBlock/
  ToolUseBlock). Drop the legacy `Compacted`/`LlmResponse` mappings
  in Phase 6.5.

### Plausible commit breakdown (sketch)

1. **5a — per-index streaming buffer + StreamingTail removal**: the
   big structural item (39 + 41-live). Touches server WS, protocol,
   store, feed.
2. **5b — partial:true "discarded" styling** across TextBlock/
   ThinkingBlock/ToolUseBlock (41/42/43).
3. **5c — ThinkingBlockBlock TextModal affordance** (42).
4. **5d — ToolUseBlockBlock TextModal affordance** + correlate to
   sibling ToolCall (43 + 46 prep).
5. **5e — Slim ToolCallBlock; correlation to ToolUseBlock above +
   ToolResult below** (46).
6. **5f — [compacted] badge on LlmResponseEndedBlock** (44).
7. **5g — "N partial blocks" count on LlmResponseDiscardedBlock** (45).
8. **5h — final cleanup pass + bless snapshots for any incidental
   changes**.

(Some of 5b/5f/5g may be combinable into single commits.)

### Migration discipline (same as Phases 2-4)

- Phase 0 goldens MUST remain byte-equal (Phase 5 is frontend-only;
  should not touch context.jsonl).
- omega-e2e suite must stay green. Some tests may need updating as
  affordances change (e.g. if `[thinking]` button removal in 4c
  broke something — check `02_picker.rs` and the 06_feed family).
- Snapshot tests will rebless every visual change. Diff each `.snap`
  before committing.
- Push every ≈3 commits as before.

### Notes for resuming after context compaction

Current develop tip after Phase 4: `a3484a4` (this progress-doc
commit follows immediately after).
Gate commands:
  - `just rust-gate`
  - `cargo test -p omega-e2e --tests -- --ignored --test-threads=1`
  - `cargo test -p omega-agent --test goldens`
Full plan: `backlog/schema-8.md` § Phase 5.
Progress: this file.
Known flakes: 07_scroll's `scroll_tailing` +
`tailing_survives_rapid_streaming_after_button_click`. Both pass solo /
on re-run. Not introduced by SCHEMA-8.

## CURRENT STATE (Phase 3 DONE — next: Phase 4) [historical]

**Phase 3 complete.** Five commits on `develop`, all green on
`just rust-gate` + the omega-e2e ignored suite:

- `c10b72c` schema-8(phase-3a): introduce BTreeMap<usize, BlockSlot>
  in parallel
- `dd9a171` schema-8(phase-3b): emit per-block events + opener/closer
  pair (+ leptos WsMessage mirror + frontend-compat band-aid for
  legacy LlmResponse.text)
- `7fd7a39` schema-8(phase-3c): detect compaction via
  usage.iterations
- `cd43fc3` schema-8(phase-3d): emit partial block events +
  LlmResponseDiscarded on mid-stream abandonment
- `6e16ce9` schema-8(phase-3e): drop flat accumulators + lock
  interleaved-thinking golden

What the agent looks like now:

- `assistant_blocks` is built from `BTreeMap<usize, BlockSlot>` in
  key order. The flat `text_buf`/`current_thinking`/
  `completed_thinking_blocks` accumulators are gone from both
  `send_message` and `perform_resumption`.
- StreamSignals are the sole source of truth for text/thinking/
  tool_use blocks coming via the wire. `OmegaEvent::ToolCall`
  (still emitted by MockProvider scripts in goldens.rs and
  internal.rs) continues to feed the legacy `tool_uses: Vec`,
  which is concatenated after the slot extracts for dispatch.
  Both legacy match arms (`OmegaEvent::ToolCall`,
  `OmegaEvent::Compacted`) remain — deleted in Phase 6.5.
- New events emitted: `LlmResponseStarted`, `TextBlock`,
  `ThinkingBlock`, `ToolUseBlock`, `LlmResponseEnded`,
  `LlmResponseDiscarded`. Legacy `LlmResponse` is still emitted
  alongside `LlmResponseEnded` (band-aid until Phase 4 frontend
  cutover; legacy field deleted in Phase 6.5).
- `LlmResponse.text` / `.thinking` are repopulated post-assembly
  from the assembled slot blocks. This is the frontend-compat
  band-aid keeping the 11 03_markdown e2e tests green; it goes
  away when Phase 4 wires the leptos store to TextBlock/
  ThinkingBlock directly.
- Compaction detection: `lr.usage.iterations` is checked for a
  `"compaction"` entry; if present, `history.clear()` +
  `context_hashes.clear()`. (Legacy `OmegaEvent::Compacted` handler
  still in place for MockProvider scripts.)
- Abandonment closers: when `LlmRetry`/`LlmError`/`TurnInterrupted`
  arrives mid-stream, the agent emits `partial: true` block events
  for any unsealed slot, then `LlmResponseDiscarded`, then the
  terminal event. Slots are cleared via `std::mem::take` so the
  next attempt starts fresh.
- Block-event emission carries `data-block-id` material via the
  block-id fields on the new event types (used by Phase 4/5
  frontend and the Phase 0 T6 browser-refresh replay test).

Goldens state: 7 fixtures locked, all replay byte-equal.
  - simple_turn, thinking_blocks, parallel_tool_calls,
    multi_thinking_tools, mid_stream_retry, compaction (Phase 0).
  - interleaved_thinking (NEW, Phase 3e — Phase 0's deferred case).

Script-side tweaks in 3e to give each block a distinct slot index
(mechanical, no semantic change to context.jsonl):
  - `script_thinking_blocks`: thinking@0+complete@0, thinking@1+
    complete@1, text@2.
  - `script_multi_thinking_tools` call1: text@1 (was text@0;
    thinking is @0).
  - All other scripts unchanged (single-block streams or slots
    already cleared between blocks).

Gate verification: `just rust-gate` GREEN. `cargo test -p omega-e2e
--tests -- --ignored --test-threads=1` GREEN end-to-end (after
re-running two `07_scroll` flakes — `scroll_tailing` and
`tailing_survives_rapid_streaming_after_button_click` — both pass
solo and are unrelated to the slot refactor).

## Phase 4 — frontend protocol & store (NEXT)

Goal: cut the leptos frontend over to the new event grammar so the
`LlmResponse.text` / `.thinking` band-aid in the agent can come out
in Phase 6.5.

### Pre-existing scaffolding from Phase 3

- `frontends/leptos/src/protocol.rs` already mirrors the new
  WsMessage variants (`LlmResponseStarted`, `LlmResponseEnded`,
  `LlmResponseDiscarded`, `TextBlock`, `ThinkingBlock`,
  `ToolUseBlock`) — landed in 3b. Deserialization works; the
  store just doesn't route them yet.
- Phase 1b stubs in `feed.rs::render_event_body`,
  `event_view.rs::kind_for`, `event_view.rs::event_type_tag`
  return placeholder shapes for the new variants. Phase 4/5
  replaces those stubs.

### Required changes (per plan)

In `frontends/leptos/src/store.rs`:
  - On `LlmResponseStarted`: open a new response container
    keyed by `response_id` (or whatever stable id the event
    carries; confirm the field name).
  - On `TextBlock` / `ThinkingBlock` / `ToolUseBlock`: append
    a block child to the response container with the
    `data-block-id` from the event. `partial: true` blocks
    render with an in-flight indicator.
  - On `LlmResponseEnded`: close the container; finalize
    usage display.
  - On `LlmResponseDiscarded`: mark the container as discarded;
    keep partial children visible (per the Phase 0 acceptance:
    "5xx mid-stream: llm_response_started, partial blocks*,
    llm_response_discarded, llm_retry, …").
  - Stop reading `LlmResponse.text` / `.thinking`. The legacy
    `LlmResponse` event still arrives (the band-aid emits both);
    the store should ignore it during Phase 4 to prove the new
    path covers all UI.

### Migration discipline

- Phase 0 goldens MUST remain byte-equal (they test
  context.jsonl, not the frontend; should be untouched by
  Phase 4 work, but re-run `cargo test -p omega-agent
  --test goldens` after each commit anyway).
- omega-e2e (especially `03_markdown.rs`, `06_feed.rs`,
  `07_scroll.rs`) must stay green. These currently depend on
  the band-aid `lr.text`/`.thinking`; once the store reads from
  block events, the band-aid is dead code (removed in 6.5).
- The leptos snapshot tests will likely need re-blessing as the
  new variants land in `event_view`. Diff each `.snap` change
  before committing.
- Push every 3 commits as before.

### Plausible commit breakdown (sketch)

1. Store: route `LlmResponseStarted` → open container; stop
   ignoring it. No visible UI change yet (container is empty;
   legacy `LlmResponse` still renders).
2. Store: route `TextBlock` / `ThinkingBlock` / `ToolUseBlock` →
   render inside the container. Snapshot tests rebless. e2e
   should still pass because both paths render the same content.
3. Store: route `LlmResponseEnded` + `LlmResponseDiscarded`.
   Switch the store to PREFER block-event children over
   `lr.text`/`.thinking`; visually identical.
4. Drop the legacy `LlmResponse` consumer in the store (still
   on the wire — band-aid stays until 6.5).
5. T6 browser-refresh replay test (rust/crates/omega-e2e/tests/
   08_refresh.rs or similar): reload mid-turn + post-TurnEnd,
   assert DOM `data-block-id` equality. (Per the 2025 user
   addition to the acceptance criteria.)

### Notes for resuming after context compaction

Current develop tip after Phase 3: `6e16ce9` (progress-doc commit
follows immediately after).
Gate: `just rust-gate` (rust-only) and `cargo test -p omega-e2e
--tests -- --ignored --test-threads=1` (Playwright e2e).
Full plan: `backlog/schema-8.md` § Phase 4.
Progress: this file.

## CURRENT STATE (Phase 2 DONE — next: Phase 3) [historical]

**Phase 2 complete.** Three commits on `develop`, all green on the
full `just rust-gate`:

- `3b78394` schema-8(phase-2a): anthropic provider → per-block
  completion signals
- `4fb3448` schema-8(phase-2b): ollama provider → per-block completion
  signals
- `7ac743c` schema-8(phase-2c): retry wrapper no longer tracks fragments

Provider-side cutover landed in full:

- Anthropic `content_block_stop` emits the matching
  `StreamSignal::*BlockComplete` carrying the real SSE `index`:
  `TextBlockComplete { index, text }`,
  `ThinkingBlockComplete { index, signature }` (in place since 1a),
  `ToolUseBlockComplete { index, id, name, input }`.
- Anthropic no longer emits `OmegaEvent::ToolCall` mid-stream and no
  longer emits `OmegaEvent::Compacted`. Server-side compaction is
  surfaced via `LlmResponseUsage.iterations` (extracted from the raw
  usage object via the new `extract_iterations` helper).
- Anthropic + Ollama `LlmResponse.text` / `.thinking` /
  `.streaming_start` are always `None` now. The `all_text` /
  `all_thinking` / `streaming_start` accumulators are gone from both
  providers. (The `LlmResponseEvent` type still has the legacy
  fields — deleted in Phase 6.5.)
- Ollama emits `ToolUseBlockComplete` with a synthetic monotonic
  `next_tool_use_index` (Ollama has no SSE block indices).
- `retry.rs::track_fragment` removed; `LoopState.text_fragment` /
  `.thinking_fragment` removed; `build_retry_event` no longer takes
  the fragment params and writes `text_fragment: None` /
  `thinking_fragment: None` on `LlmRetryEvent`.

Agent change is intentionally minimal:
`StreamSignal::ToolUseBlockComplete { id, name, input, .. }` now
pushes `(id, name, input)` into `tool_uses` (replacing the previous
path that captured `OmegaEvent::ToolCall` mid-stream). The flat
accumulators (`text_buf`, `current_thinking`,
`completed_thinking_blocks`) and the existing handlers for
`OmegaEvent::ToolCall` and `OmegaEvent::Compacted` STAY — they're
still used by MockProvider scripts in `goldens.rs` and `internal.rs`,
and keeping them is what kept the goldens byte-equal across the cut.

Gate verification: `just rust-gate` GREEN end-to-end. Phase-0
goldens still byte-equal across all 6 fixtures.

## Phase 2 strategy decision (recorded for future reference)

The Phase 2 prompt offered two options:
  (A) Providers emit BOTH legacy and new shapes; agent flips in Phase 3.
  (B) Providers emit ONLY the new shapes; agent path rewrites alongside.

**Decision: Option B-lite.** Real providers cut over fully (per the
plan's literal text — "stop emitting", "strip"). Agent gets one
additive change to consume `ToolUseBlockComplete`. Existing
`OmegaEvent::ToolCall` and `OmegaEvent::Compacted` handlers in the
agent stay in place — the MockProvider scripts in goldens.rs and
internal.rs still emit those legacy variants directly, and the agent
must handle them until Phase 3 reworks the accumulators.

Why not pure-additive (option A): the plan literally requires removals
in Phase 2. Doing them in providers now (without restructuring the
agent) makes the Phase 2 → Phase 3 boundary a clean swap rather than a
coexistence cleanup, and produces the smallest intermediate diff.

## Phase 3 — agent (NEXT, the big one)

Goal: restructure agent-side assistant content reconstruction to
preserve API content-block index order, and own all the abandonment /
closing logic that Phase 2 stripped from the providers.

### Required changes (per plan)

In `rust/crates/omega-agent/src/agent.rs`:

- Replace flat accumulators (`text_buf`, `current_thinking`,
  `completed_thinking_blocks`, `tool_uses`) with a
  `BTreeMap<usize, BlockSlot>` keyed by API
  `content_block_start.index` (carried on every `StreamSignal`).
  Each slot is one of `Text | Thinking | ToolUse`, accumulating its
  own deltas.
- Emit `OmegaEvent::LlmResponseStarted` on the first signal of a
  response stream (replacing the implicit `streaming_start` field).
- On each `*BlockComplete` signal: emit
  `OmegaEvent::TextBlock` / `ThinkingBlock` / `ToolUseBlock` for
  the corresponding slot, then mark it sealed.
- After `OmegaEvent::LlmResponseEnded` arrives (note: providers still
  emit legacy `OmegaEvent::LlmResponse` — in this phase the agent
  swaps to *also* emitting `LlmResponseEnded`, or pivots its
  consumption; decide at the start of Phase 3 like we did for Phase 2):
    * For each non-partial `ToolUseBlock`, emit `OmegaEvent::ToolCall`
      with the proper `context_hash`, then dispatch.
    * Build the assistant message from blocks in **index order**.
- Compaction detection: check `lr.usage.iterations` for an entry of
  type `"compaction"`; if present do `history.clear()` /
  `context_hashes.clear()`. (The agent's existing
  `OmegaEvent::Compacted` handler may stay until 6.5 to keep MockProvider
  scripts working — same pattern we used for `ToolCall`.)
- **Abandonment closers (replaces `track_fragment`):** when a
  `LlmRetry`/`LlmError`/`TurnInterrupted` arrives mid-stream,
  emit `partial: true` block events for any unsealed slot, then
  emit `OmegaEvent::LlmResponseDiscarded` before the
  `LlmRetry` / etc.
- End of Phase 3: lock the interleaved-thinking golden (which Phase 0
  deferred because today's flat accumulators reorder).

### Migration discipline (same as Phase 2)

- `OmegaEvent::LlmResponse` (legacy) keeps coexisting with
  `OmegaEvent::LlmResponseEnded` until Phase 6.5. Frontend hasn't
  migrated yet (Phase 4 does that).
- Phase 0 goldens MUST remain byte-equal on the 6 non-interleaved
  fixtures. Run `cd rust && cargo test -p omega-agent --test goldens`
  after every commit.
- Phase 3 *will* change the shape of `events.jsonl` because the agent
  starts emitting the new block events. That's expected. Goldens only
  cover `context.jsonl`; the latter is built from assistant content,
  which should remain identical for non-interleaved fixtures (same
  text / thinking / tool_use blocks, just collected by index).

### Plausible commit breakdown (sketch)

1. Introduce `BTreeMap<usize, BlockSlot>` accumulators behind a
   feature gate / parallel path; old path still wins. Verify
   goldens byte-equal.
2. Switch agent to consume `*BlockComplete` signals and emit the
   new block events (still also emitting legacy `LlmResponse`).
3. Compaction detection via `usage.iterations`; emit synthetic
   `Compacted` for backward compat.
4. Abandonment closers: on retry/error mid-stream, emit partial
   block events + `LlmResponseDiscarded`.
5. Drop the flat-accumulator path; lock interleaved-thinking golden.

(Each step ends green. Push every ≈3 commits.)

### Notes for resuming after context compaction

Current develop tip after Phase 2 push: `7ac743c`.
Gate command: `just rust-gate`.
Goldens replay: `cd rust && cargo test -p omega-agent --test goldens`.
Full plan: `backlog/schema-8.md`.

## CURRENT STATE (Phase 1b DONE — next: Phase 2) [historical]

**Phase 1b complete.** All new SCHEMA-8 event-side types added to
`rust/crates/omega-types/src/events.rs` purely additively:

- New structs: `LlmResponseStartedEvent`, `LlmResponseEndedEvent`,
  `LlmResponseDiscardedEvent`, `TextBlockEvent`, `ThinkingBlockEvent`,
  `ToolUseBlockEvent`, `UsageIteration`.
- New `OmegaEvent` variants: `LlmResponseStarted`, `LlmResponseEnded`,
  `LlmResponseDiscarded`, `TextBlock`, `ThinkingBlock`, `ToolUseBlock`.
- `LlmResponseUsage` extended with `iterations: Option<Vec<UsageIteration>>`
  (skip_serializing_if=Option::is_none, default — backward compatible).
- 11 new round-trip tests cover the new shapes + the
  legacy-without-iterations deserialise path.
- `events_reference.rs` golden snapshot extended from 22 to 28 variants
  (renamed test, regenerated `.snap`, deleted stale `_22_` snapshot).
- Frontend exhaustive-match sites patched with minimal stubs:
  `feed.rs::render_event_body`, `event_view.rs::kind_for`,
  `event_view.rs::event_type_tag`. Phase 4/5 will replace the stubs
  with real rendering.
- Every `LlmResponseUsage { ... }` literal updated workspace-wide to
  add `iterations: None,` (12 call sites).

Legacy `LlmResponseEvent`, `CompactedEvent`, and the
`text_fragment`/`thinking_fragment` fields on `LlmRetryEvent` are KEPT.
They get removed in a final Phase 6.5 cleanup commit once every
producer/consumer has migrated to the new grammar.

Gate: `just rust-gate` GREEN end-to-end (rust workspace build, leptos
wasm build, leptos lib tests, leptos snapshot tests, fmt, clippy -D
warnings, cargo test, cargo machete). Phase-0 context.jsonl goldens
still byte-equal (no semantic change to context construction yet).

**Phase 1 is now complete (1a + 1b shipped).** Next session resumes at
Phase 2 — provider migration in `omega-core/src/anthropic.rs` and
`omega-core/src/ollama.rs`.

## Phase 2 — providers (NEXT)

Goal: providers stop emitting mid-stream `OmegaEvent::ToolCall`; instead
they emit per-block completion `StreamSignal`s
(`TextBlockComplete`/`ThinkingBlockComplete`/`ToolUseBlockComplete`)
with the real Anthropic SSE `content_block_start.index`. Drop
`streaming_start` synthesis. Pull Anthropic's usage `iterations` array
into `LlmResponseUsage.iterations`. Strip `all_text`/`all_thinking`
accumulators (moved to the agent in Phase 3).

Detailed strategy still open — must keep gate green: producers can
emit BOTH legacy and new shapes during the migration, OR the agent can
be taught to consume both. Decide at the start of Phase 2.

File sketch:
- `anthropic.rs::content_block_stop` for `tool_use`: emit
  `StreamSignal::ToolUseBlockComplete { index, id, name, input }`
  instead of `OmegaEvent::ToolCall(...)`.
- `anthropic.rs::content_block_stop` for `text`: emit
  `StreamSignal::TextBlockComplete { index, text }`.
- `anthropic.rs::content_block_stop` for `thinking`: keep emitting
  `ThinkingBlockComplete{index, signature}` (already in place from 1a).
- `anthropic.rs::message_stop`: stop emitting `LlmResponse.text`,
  `.thinking`, `.streaming_start`. Pull `iterations` from raw usage if
  present.
- `ollama.rs`: same shape; `index = 0` everywhere (no parallel blocks).
- `retry.rs::track_fragment`: removed; agent owns abandonment in Phase 3.

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

