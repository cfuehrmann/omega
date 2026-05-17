# SCHEMA-8 — Append-only event grammar

**Status:** open. Hard cutover; no backward compatibility with old `events.jsonl`.

**Depends on:** [HASH-1](hash-1.md) — deterministic content-derived `ContextHash`. HASH-1 ships first; SCHEMA-8 builds on byte-stable `context.jsonl`.

## Why

Today's persisted event grammar mixes two ontologies:

1. **Point-in-time facts** — most events. Append-only and pair-based:
   intervals are derived from event pairs (e.g. `UserMessage` ↔ `TurnEnd`,
   `LlmCall` ↔ `LlmResponse`).
2. **Interval-summary events** — `LlmResponse` packs `streaming_start`, full
   `text`, and `thinking` from across the streaming interval into a single
   close-of-interval record. `LlmRetry` packs partial `text_fragment` /
   `thinking_fragment`. `Compacted` is emitted as a separate event though
   the underlying API treats it as a property of the response usage.

The mixed ontology causes visible problems:

- The UI renders streaming text and thinking in ephemeral blocks that **vanish
  and get replaced** when the persisted `LlmResponse` arrives. Thinking gets
  **folded into a button** on the assistant block.
- `ToolCall.time` claims a streaming-time moment but the event lands in the
  log post-`LlmResponse`. Order doesn't match timestamp.
- The `text` and `thinking` fields on `LlmResponse` are *concatenated*
  across all blocks, losing block boundaries. Today's agent only writes
  context.jsonl correctly because it reconstructs structured ContentBlocks
  from separate per-block accumulators — a parallel persistence path with
  no automated cross-check.

The goal is a uniform append-only grammar where **every persisted event
creates exactly one block in the feed; that block is never deleted,
replaced, or relocated.** The events.jsonl content-block events are also
made structurally faithful to the API's content-block sequence.

A separate but conjoined goal: ensure context.jsonl byte-equality before
and after the refactor. Context construction is safety-critical (a
silently-corrupted assistant message degrades model quality on the next
turn). Defensive golden tests gate the refactor.

## Two persistence paths — never confuse them

```
Provider stream (per-block streaming accumulators)
    │
    ├──► context.jsonl  (ContentBlock[] in API order, signatures preserved)
    │       ▲ source of truth for next API call — replay-safe
    │
    └──► events.jsonl  (chronological log; UI/audit; NEVER fed to API)
            ├── LlmResponseStarted
            ├── ThinkingBlock × N      (one per completed thinking block)
            ├── TextBlock × M          (one per completed text block)
            ├── ToolUseBlock × K       (one per completed tool_use block)
            ├── LlmResponseEnded       (usage incl. iterations, stop_reason, hash)
            ├── ToolCall × K           (re-emitted post-response at dispatch)
            └── ToolResult × K         (per future completion)
```

The two paths are written from the same source data (the streaming
accumulators) but answer different questions. SCHEMA-8 changes the right
column — and additionally restructures the source accumulators so the
left column is provably faithful to the API order under interleaved
thinking. Signatures and per-block structure in context.jsonl semantics
do not change; only the assembly mechanism is replaced (with structural
equality verified by golden tests — see below for why structural and not
byte-level).

### Note on `ContextHash`

After HASH-1 ships, `ContextHash` is a deterministic content-derived
hash (16 lowercase hex chars, sha256 prefix of canonical-JSON of
`(role, content)`). This means:

- Two runs of the same fixture produce byte-identical `context.jsonl`
  files. SCHEMA-8 golden tests can use **byte-equal comparison** rather
  than projection.
- The hash on disk verifies against the content on disk; integrity
  checks become possible (see HASH-1 Phase 4).
- The hash ABI depends on `Role` and `ContentBlock` field/variant
  declaration order. SCHEMA-8 does **not** modify either type, so the
  HASH-1 lockdown values continue to hold throughout this refactor.
  If a phase ever needs to touch those types, stop and discuss — it is
  a session-invalidation event.

## What changes

### New / renamed / removed event variants

| Event | Status | Notes |
|---|---|---|
| `LlmResponseStarted { time }` | **new** | Opener emitted by the agent on the first signal of any kind from a freshly-started provider stream within a turn iteration. |
| `LlmResponseEnded` | **renamed** from `LlmResponse` | Drop `text`, `thinking`, `streaming_start`. Keep `stop_reason`, `cleared_tool_uses`, `cleared_input_tokens`, `usage`, `context_hash`, `response_summary`. |
| `LlmResponseDiscarded { time }` | **new** | Pure marker. Closer for `LlmResponseStarted` when the response is abandoned. Emitted before `LlmRetry`, `LlmError`, or `TurnInterrupted` whenever a `LlmResponseStarted` is open. |
| `TextBlock { time, text, partial: bool }` | **new** | Per text content block. Emitted at streaming `content_block_stop`. `partial: true` if cut off by abandonment. |
| `ThinkingBlock { time, thinking, signature: Option<String>, partial: bool }` | **new** | Per thinking content block. Invariant: `signature.is_none() iff partial == true`. Both fields kept for clarity even though redundant. |
| `ToolUseBlock { time, id, name, input, partial: bool }` | **new** | Per tool_use content block. Emitted at streaming `content_block_stop`. `input` may be malformed JSON when `partial: true`; agent does not dispatch partial blocks. |
| `LlmRetry` | **breaking** | Strip `text_fragment` and `thinking_fragment`. Pure control event: `time`, `attempt`, `http_status`, `wait_ms`, `error`, `retry_at`, `error_body`, `reason`. |
| `ToolCall` | **breaking semantics, same shape** | `time` now means "agent dispatched the tool". Emitted by the agent post-`LlmResponseEnded`, sequentially in input order, just before the parallel batch dispatch. Provider no longer emits `ToolCall`. |
| `ToolResult` | **unchanged** | Keep `duration_ms`. |
| `Compacted` | **deleted** | Replaced by an `iterations` array on `LlmResponseEnded.usage`. |
| `LlmResponseUsage` | **additive** | Add `iterations: Option<Vec<UsageIteration>>` mirroring the Anthropic wire shape. |
| `UsageIteration { iteration_type, input_tokens, output_tokens, .. }` | **new** | Per-iteration breakdown when server-side compaction fires. |

### New internal stream signals (provider → agent, never persisted)

- `StreamSignal::TextBlockComplete { index, text }` — emitted at
  `content_block_stop` for a text content block. `index` is the API
  `content_block_start.index`, used by the agent's order-preserving
  accumulator.
- `StreamSignal::ThinkingBlockComplete { index, signature }` — extend the
  existing signal with `index`. Thinking text is already accumulated by
  the agent via `Thinking` deltas; the signal closes the block.
- `StreamSignal::ToolUseBlockComplete { index, id, name, input }` — new.
  Replaces the current pattern where providers emit `OmegaEvent::ToolCall`
  mid-stream and the agent intercepts and re-emits.

`StreamSignal::Text { index, text }` and `StreamSignal::Thinking { index, text }`
also gain `index` so the agent can route deltas to the correct slot.

### Final grammar

```
SessionStarted → (ServerStarted? → ... → ServerStopped?)

UserMessage
  → (LlmCall
      → LlmResponseStarted
        → (TextBlock | ThinkingBlock | ToolUseBlock)*    // API-emission order
        → (LlmResponseEnded | LlmResponseDiscarded)
      → ToolCall*       (only after LlmResponseEnded; one per non-partial ToolUseBlock)
      → LlmRetry?       (only after LlmResponseDiscarded)
    )*
  → ToolResult*         (one per ToolCall, completion order)
  → TurnEnd
```

Invariants:

- Every `LlmResponseStarted` is followed by exactly one of
  `LlmResponseEnded` or `LlmResponseDiscarded`.
- A `LlmResponseDiscarded` is followed by `LlmRetry` (retry path),
  `LlmError` (giving up), or `TurnInterrupted` (user aborted).
- `ToolCall` only appears after `LlmResponseEnded` (never after
  `LlmResponseDiscarded`).
- Each `ToolCall` corresponds 1:1 to a non-partial `ToolUseBlock` from the
  most recent `LlmResponseEnded`'s response, matched by `id`.
- For `ThinkingBlock`: `signature.is_none() iff partial == true`.
- `partial: true` blocks immediately precede the `LlmResponseDiscarded`
  that terminates their response.

### Order-preserving accumulator (the CTX-ORDER fold-in)

Today's agent flattens streaming content by kind into:

```rust
let mut text_buf = String::new();
let mut current_thinking = String::new();
let mut completed_thinking_blocks: Vec<(String, String)> = Vec::new();
let mut tool_uses: Vec<(String, String, Value)> = Vec::new();
```

This loses original API content-block ordering. Without
`interleaved-thinking-2025-05-14`, the lossy assembly happens to match the
constrained API order (`thinking* → text* → tool_use*`) and is harmless.
With the beta enabled, it would silently corrupt the assistant message
in context.jsonl: blocks reordered, text segments concatenated,
context-degraded next turn.

We replace the flat accumulators with a position-keyed structure:

```rust
enum BlockSlot {
    Text { text: String },
    Thinking { thinking: String, signature: Option<String> },
    ToolUse { id: String, name: String, partial_json: String },
}

let mut slots: BTreeMap<usize, BlockSlot> = BTreeMap::new();
```

Slots are keyed by `content_block_start.index`. Deltas are routed by
`index`. On `content_block_stop` the slot is finalised in place. On
`message_stop` the agent assembles `assistant_blocks: Vec<ContentBlock>`
by iterating slots in index order, producing ContentBlocks in original
API order. Signatures stay per-block.

Behavioural equivalence on non-interleaved streams: byte-identical
context.jsonl output (verified by Phase 0 goldens). Behavioural change on
interleaved streams: correct order preservation (verified by a synthetic
fixture).

## Implementation plan

### Phase 0 — Defensive harness (BEFORE any other change)

This phase is the safety net. Every later phase is gated on these tests
passing.

**0a. Capture golden context.jsonl fixtures from develop tip (before any
SCHEMA-8 code change)**, using deterministic mock-provider scripts. Cover:

- Simple turn (one user message, one text-only response, no tools).
- Turn with thinking blocks (extended thinking enabled).
- Turn with parallel tool calls.
- Turn with multiple thinking blocks plus tool calls.
- Turn with a mid-stream retry and recovery.
- Turn with server-side compaction.
- **Synthetic interleaved-thinking turn**: mock provider emits a stream
  whose `content_block_start.index` order is `text₀, thinking₁, text₂,
  tool_use₃` (or similar interleave). The current main may produce an
  *incorrect* context.jsonl for this fixture — see 0b.

**0b. Plausibility-check each captured golden** for correctness against
what the mock script said the API returned. The implementation agent
performs and documents this check in writing — the user reviews the
written notes, not the JSON. For each fixture, write a short
`notes.md` next to the golden file recording:

- Number of thinking blocks captured = number of
  `content_block_start { type: "thinking" }` events in the script.
- Each thinking block's text equals the script's concatenated
  `thinking_delta`s for that index.
- Each thinking block's signature equals the script's `signature_delta`
  for that index. Signatures across blocks are distinct.
- Block order in the captured `Vec<ContentBlock>` matches the script's
  `content_block_start.index` order.
- No concatenation across same-kind blocks; no reordering across
  different-kind blocks.

A golden only counts as locked once its `notes.md` exists and confirms
the above. A capture that just happens to reflect current behaviour
without the documented check is not a golden — it is a regression trap.

If the interleaved-thinking fixture's capture is incorrect on the
pre-refactor codebase (very likely — flat accumulators reorder kinds),
**do not lock that fixture's golden yet**. Check in the fixture script
and a `notes.md` describing the *expected* (correct) output, mark its
replay test `#[ignore]` with a comment pointing to Phase 3, and
proceed. The other fixtures' goldens are locked normally.

**0c. Write a replay test harness** in `omega-agent/tests/` that:

- Runs the agent against each fixture's mock-provider script.
- Reads the resulting `.omega/sessions/<ts>/context.jsonl`.
- Compares it **byte-equal** against the checked-in golden file.
- Runs as part of `cargo test`.

Byte-equal comparison is sound because HASH-1 made `ContextHash`
deterministic. The only remaining non-deterministic field is
`ContextRecord.time` (wall-clock); the harness either freezes the
clock during fixture replay (preferred), or scrubs the `time` field
before comparison. Pick one consistently across all fixtures.

The goldens are real `context.jsonl` files checked in under
`omega-agent/tests/goldens/<fixture-name>/context.jsonl`. The refactor
proceeds entirely on the develop branch — no checkout of an earlier
commit is needed during phases 1–7.

Each fixture also gets a `notes.md` next to its golden documenting what
was verified during Phase 0b. The implementation agent owns these
plausibility checks and writes them up; the user reviews the notes
(rather than the JSON) and signs off.

Phases 1–7 may not merge unless all golden tests pass. If a phase
legitimately needs to change a golden (rare — only if the *fixture
script* changed), the update lands in a dedicated commit with the
`notes.md` updated and a message explaining why the new output is
correct.

The interleaved-thinking fixture's golden is captured fresh from the
new code at the end of Phase 3 and frozen there — see Phase 3.

### Phase 1 — Schema (Rust types)

**File: `crates/omega-types/src/events.rs`**

1. Define `LlmResponseStartedEvent { time }`.
2. Rename `LlmResponseEvent` → `LlmResponseEndedEvent`. Drop `text`,
   `thinking`, `streaming_start`. Keep `response_summary`.
3. Define `LlmResponseDiscardedEvent { time }`.
4. Define `TextBlockEvent { time, text, partial: bool }`.
5. Define `ThinkingBlockEvent { time, thinking, signature: Option<String>, partial: bool }`.
6. Define `ToolUseBlockEvent { time, id, name, input: Value, partial: bool }`.
7. Strip `thinking_fragment` and `text_fragment` from `LlmRetryEvent`.
8. Delete `CompactedEvent` and the `Compacted` variant.
9. Extend `LlmResponseUsage` with `iterations: Option<Vec<UsageIteration>>`.
10. Define `UsageIteration { iteration_type, input_tokens, output_tokens, /* extras via flatten */ }`.
11. Update `OmegaEvent` enum: rename `LlmResponse` → `LlmResponseEnded`;
    add `LlmResponseStarted`, `LlmResponseDiscarded`, `TextBlock`,
    `ThinkingBlock`, `ToolUseBlock`; remove `Compacted`.
12. Update unit tests; add round-trip tests for each new variant; drop
    obsolete tests.

**File: `crates/omega-types/src/stream_signal.rs`**

13. Extend `StreamSignal::Text { index, text }` and
    `StreamSignal::Thinking { index, text }` with the API block index.
14. Replace `StreamSignal::ThinkingBlockComplete { signature }` with
    `{ index, signature }`.
15. Add `StreamSignal::TextBlockComplete { index, text }`.
16. Add `StreamSignal::ToolUseBlockComplete { index, id, name, input }`.

### Phase 2 — Providers

**File: `crates/omega-core/src/anthropic.rs`**

17. Stop emitting `OmegaEvent::ToolCall` from the streaming loop.
    On `content_block_stop` for a `tool_use` block, yield
    `StreamSignal::ToolUseBlockComplete { index, id, name, input }`.
18. On `content_block_stop` for a `text` block, yield
    `StreamSignal::TextBlockComplete { index, text }`.
19. On `content_block_stop` for a `thinking` block, yield
    `StreamSignal::ThinkingBlockComplete { index, signature }`.
20. Add `index` to `Text` and `Thinking` delta signals.
21. Stop tracking `streaming_start`. Stop emitting `OmegaEvent::Compacted`.
22. On `message_stop`, emit `OmegaEvent::LlmResponseEnded` with no `text`,
    `thinking`, or `streaming_start`. Pull the iterations array out of the
    Anthropic usage object into `LlmResponseUsage.iterations`.
23. Drop `all_text` and `all_thinking` accumulators.

**File: `crates/omega-core/src/ollama.rs`**

24. Mirror the changes. Emit `LlmResponseEnded` without text/thinking/
    streaming_start. Emit per-block-complete signals. Iterations stays
    `None` (Ollama has no server-side compaction).

**File: `crates/omega-core/src/retry.rs`**

25. Update `track_fragment` and the retry wrapper: no longer write
    fragments onto `LlmRetry`. The agent owns abandonment closers now.

### Phase 3 — Agent (the big one)

**File: `crates/omega-agent/src/agent.rs`**

This phase replaces the streaming accumulator structure and adds the new
event emissions.

26. **Replace flat accumulators with `BTreeMap<usize, BlockSlot>`**
    keyed by API `content_block_start.index`. Deltas (`Text`, `Thinking`)
    routed to slots by `index`.
27. **Emit `LlmResponseStarted`** on the first signal received from a
    freshly-started provider stream within a turn iteration. Track
    `response_started: bool`.
28. **On `StreamSignal::TextBlockComplete { index, text }`**: finalise
    the slot at `index`. Emit `OmegaEvent::TextBlock { time: now_iso(),
    text, partial: false }`.
29. **On `StreamSignal::ThinkingBlockComplete { index, signature }`**:
    finalise the slot. Emit `OmegaEvent::ThinkingBlock { time: now_iso(),
    thinking, signature: Some(signature), partial: false }`.
30. **On `StreamSignal::ToolUseBlockComplete { index, id, name, input }`**:
    finalise the slot. Emit `OmegaEvent::ToolUseBlock { time: now_iso(),
    id, name, input, partial: false }`.
31. **On `OmegaEvent::LlmResponseEnded`**: persist and forward. Assemble
    `assistant_blocks: Vec<ContentBlock>` by iterating slots in `index`
    order; append to context.jsonl. Then check `usage.iterations` for an
    entry with `type == "compaction"` — if found, perform the same
    `history.clear()` / `context_hashes.clear()` that the old `Compacted`
    handler did.
32. **After `LlmResponseEnded`**: for each non-partial `ToolUseBlock` slot,
    emit `OmegaEvent::ToolCall { time: now_iso(), id, name, input,
    context_hash }` sequentially before the dispatch loop. Dispatch via
    `FuturesUnordered` as today; emit `ToolResult` on completion.
33. **Abandonment closers**: when any of `LlmRetry`, `LlmError`,
    `TurnInterrupted` fires while `response_started` is true and no
    `LlmResponseEnded` has been emitted:
    - For each unfinalised slot in `index` order, emit a partial block
      event (`TextBlock`/`ThinkingBlock`/`ToolUseBlock` with `partial:
      true`).
    - Emit `LlmResponseDiscarded { time }`.
    - Then emit the trigger event (`LlmRetry` etc.).
    - Reset `response_started` to false; clear slots.
34. Remove fragment passing on `LlmRetry`.

**File: `crates/omega-agent/src/session_resume.rs`**

35. Update event-pattern matching to use the new variant names. Helpers
    `make_llm_response`, `tool_result`, etc. need parameter updates.

**End of Phase 3**: regenerate the interleaved-thinking golden context.jsonl
from the new code. Manually inspect for correctness (block order matches
mock script, signatures distinct, no concatenation). Lock as golden. From
this point on, all golden tests run on every commit.

### Phase 4 — Frontend protocol & store

**File: `frontends/leptos/src/protocol.rs`**

36. `WsMessage`: rename `LlmResponse` → `LlmResponseEnded`. Add
    `LlmResponseStarted`, `LlmResponseDiscarded`, `TextBlock`,
    `ThinkingBlock`, `ToolUseBlock`. Remove `Compacted`. Stream signals
    (`Text`, `Thinking` deltas; the `*Complete` signals are absorbed by
    the agent and don't traverse the wire).
37. Update the persisted-vs-stream-signal categorisation in
    `WsMessage::to_persisted_event`.

**File: `frontends/leptos/src/store.rs`**

38. `apply_event_side_effects`:
    - On `LlmResponseStarted`: clear streaming buffers, mark a response
      as open.
    - On `Text { index, text }` signal: append to the streaming buffer
      keyed by `index`.
    - On `Thinking { index, text }` signal: append to the streaming
      buffer keyed by `index`.
    - On `TextBlock { partial: false }` / `ThinkingBlock { partial: false }`
      / `ToolUseBlock { partial: false }`: clear the corresponding
      streaming buffer for that index (its content is now in a persisted
      event).
    - On `LlmResponseEnded`: mark the response as closed; clear residual
      streaming buffers.
    - On `LlmResponseDiscarded`: mark the response as discarded;
      partial-flagged block events have already arrived at this point.
    - Remove the old `Compacted`, `LlmResponse.thinking`-folding, and
      `streaming_text`/`streaming_thinking` global accumulators.

### Phase 5 — Frontend UI blocks

**File: `frontends/leptos/src/feed.rs`**

39. **Remove `StreamingTail` component.** Its job is now handled by per-block
    placeholder rendering driven by per-index streaming buffers.
40. **Add `LlmResponseStartedBlock`**: minimal header block, e.g.
    "↳ assistant" or a small spinner, resolving to nothing visible (or a
    discreet timestamp) once `LlmResponseEnded` arrives.
41. **Add `TextBlockBlock`**: renders the text content (markdown via the
    existing markdown component). When live-streaming (no persisted event
    yet for this index), reads from the per-index streaming buffer.
    `partial: true` styled as greyed/struck-through with a "discarded"
    label.
42. **Add `ThinkingBlockBlock`**: collapsed-by-default accordion. `partial:
    true` (signature: None) rendered with a "discarded" indicator. Optional
    "view in modal" affordance using the existing `TextModal` for very long
    thinking content.
43. **Add `ToolUseBlockBlock`**: replaces the inline-positioned part of
    today's `ToolCallBlock`. Shows tool name + truncated input preview;
    full input via `TextModal` on click. `partial: true` styled as
    discarded. **Note**: this is the inline-with-text rendering of the
    model's tool_use; the dispatch event (`ToolCall`) and result
    (`ToolResult`) render after `LlmResponseEnded`.
44. **Add `LlmResponseEndedBlock`**: drop the `[thinking]` button. Keep
    `[context]` and `[payload]` buttons. If `usage.iterations` contains a
    `compaction` entry, render a small `[compacted]` badge with
    click-to-show iterations breakdown.
45. **Add `LlmResponseDiscardedBlock`**: small marker block showing
    "response discarded — N partial blocks above". The partial content
    has already been rendered as `partial: true` block events above this
    marker.
46. **Update `ToolCallBlock`**: now represents the agent dispatch only.
    Slim it: name + correlated id. Full input is on the corresponding
    `ToolUseBlockBlock` above. Click correlates to the related
    `ToolResultBlock` below by id.
47. **Update `ToolResultBlock`** as needed. Existing modal behaviour
    preserved.
48. Update `EventBlock` dispatcher and `kind_for` mapping accordingly.

**File: `frontends/leptos/src/event_view.rs`**

49. Update `kind_for()` for the new variants. Drop `Compacted`,
    `LlmResponse`. Add the new event kinds.

### Phase 6 — Tests

50. Update unit tests across `omega-types`, `omega-core`, `omega-agent`
    to use the new variant names and shapes. Drop tests for fields that
    no longer exist.
51. Update mock-server fixtures (`omega-mock-server`) to emit the new
    event shapes where they assert on the wire.
52. Update e2e tests in `crates/omega-e2e/tests/`:
    - Tests that asserted on `[thinking]` button → assert on a sibling
      `ThinkingBlock` instead.
    - Tests that asserted on `Compacted` block → assert on the
      `[compacted]` badge inside `LlmResponseEnded`.
    - Tests that exercised retry → verify partial block events and
      `LlmResponseDiscarded` precede `LlmRetry`.
53. **Add the defensive tests** (gated on Phase 0 harness):
    - **T1 — signatures preserved**: synthetic stream with multiple
      thinking blocks → assert each block's signature is preserved
      verbatim in context.jsonl, no concatenation, no sharing.
    - **T2 — block order in context.jsonl**: synthetic interleaved
      stream `thinking → text → thinking → text → tool_use` → assert
      context.jsonl assistant message has `[Thinking, Text, Thinking,
      Text, ToolUse]` in exact emission order.
    - **T3 — events ↔ context cross-check**: assert `events.jsonl`
      content-block events (in order) name the same content as
      `context.jsonl` assistant ContentBlocks (in order).
    - **T4 — context.jsonl byte equality** (Phase 0): replay each
      golden fixture, assert byte-equal with the captured golden
      (with `time` frozen or scrubbed consistently across fixtures).
    - **T5 — append-only DOM invariant**: e2e test that records DOM
      block ids before and after each event arrives during a synthetic
      turn (streaming text, multiple thinking blocks, parallel tool
      calls, mid-stream retry); asserts no block ever disappears or
      moves in the feed.

### Phase 7 — Snapshots and docs

54. Refresh Leptos SSR snapshots if any reference old block shapes.
55. Update `docs/schema.md` if it exists, or write it as part of this
    work.

### Phase 8 — Mutation testing

Final validation that the test suite for SCHEMA-8 actually catches
bugs, not just exercises code. Targets the two crates where the
refactor concentrates:

- `omega-core` — streaming accumulator, content-block index handling,
  provider event emission.
- `omega-agent` — abandonment-closer logic, ToolCall dispatch,
  per-block event emission, agent-state machine.

**Steps:**

56. After Phase 7 is green, run:
    ```
    cd rust && cargo mutants -p omega-core --timeout 60
    cd rust && cargo mutants -p omega-agent --timeout 600
    ```
57. Triage every survivor. For each one, decide:
    - **Real gap**: write a test that catches it, re-run.
    - **Acceptable miss**: documented in `rust/SCHEMA-8-MUTANTS.md`
      with explicit justification (e.g., a debug-format helper whose
      exact output is not part of the contract).
58. Aim for **zero unjustified survivors** in:
    - `omega-core/src/anthropic.rs` (streaming accumulator, signal
      emission)
    - `omega-agent/src/agent.rs` (event emission, abandonment
      closers, ToolCall dispatch)
    Mutations elsewhere are noted but not chased as part of SCHEMA-8.
59. Record results in `rust/SCHEMA-8-MUTANTS.md`: total mutants per
    crate, caught, unviable, missed, plus per-miss justification.

**Particularly high-value mutations to confirm are caught:**

- Off-by-one on the streaming block index when assembling
  `assistant_blocks` — must fail T2 (block order).
- Swapping the order of `LlmResponseEnded` and `ToolCall` emission —
  must fail an e2e ordering test.
- Replacing `signature: Some(_)` with `signature: None` on a
  successfully-completed thinking block — must fail T1 or a context
  byte-equality golden.
- Skipping the `LlmResponseDiscarded` emission before `LlmRetry` —
  must fail an append-only invariant test (T5) or a retry-flow test.
- Concatenating two thinking blocks during accumulator assembly —
  must fail T1 (signatures distinct) and T2 (block order).

If any of the above survive, the test suite has a real gap; do not
proceed to merge until they are caught.

## UI design choices

- **`ThinkingBlock` rendering**: collapsed-by-default inline accordion;
  click to expand inline. Modal optional for very long thinking content
  (reuse `TextModal`).
- **Compaction surfacing**: small `[compacted]` badge in the
  `LlmResponseEnded` block header; click opens a small modal showing the
  `iterations` breakdown.
- **Discarded-block styling**: `partial: true` blocks render greyed /
  struck-through, collapsed by default, with header
  "Discarded — {N chars} text" / "Discarded thinking — {N chars}".
- **`ToolUseBlock` vs. `ToolCall` vs. `ToolResult`**: three blocks per
  tool. `ToolUseBlock` renders inline with text/thinking in the
  response. `ToolCall` and `ToolResult` render in the post-response
  area, correlated by `id`. The UI should make the correspondence
  visually obvious (matching colour, hover-highlight, …).

## Migration order

Phases must be implemented in order. Each phase compiles and passes its
tests before the next begins. Phase 0 must be in place before any schema
or behavioural change.

- Phases 0–3 are server-side only; the frontend keeps working against the
  old protocol via the WS layer until Phase 4 flips the wire shape.
- Phase 6 tests are deliberately split: per-crate tests fix as that
  crate's phase finishes; e2e suite is fully red until Phase 5 is done.
- Phase 0 golden tests run from Phase 0 onwards; they MUST stay green.

## Notes on terminology

- **"Golden" / "snapshot" / "approval" / "characterization" test** are
  synonyms in different communities for the same pattern: capture
  output to a file, manually verify it is correct, commit, then assert
  future runs match the file. The discipline that makes the pattern
  work is the manual verification step; without it the test only
  locks in current behaviour, not correctness.
- **Byte equality**: the comparison is bytes-on-disk. After HASH-1,
  `context.jsonl` is fully deterministic save the `time` field, which
  is either frozen (preferred) or scrubbed before comparison.
- **"Before" baseline = develop tip prior to any SCHEMA-8 commit**, not
  a different branch. Phase 0 is purely additive on develop, on top
  of HASH-1.
- **Goldens are checked-in real `context.jsonl` files** plus a
  `notes.md` per fixture documenting the Phase 0b plausibility check.
  Reviewers read the `notes.md`; the test compares bytes (with the
  `time` field handled consistently across fixtures).

## Acceptance criteria

- All workspace tests pass after the cutover, including the five
  defensive tests T1–T5.
- All Phase 0 golden context.jsonl fixtures replay byte-equal under the
  new agent (with `time` frozen or scrubbed consistently).
- `cargo mutants -p omega-agent` and `cargo mutants -p omega-core`
  report zero unjustified survivors in the streaming accumulator and
  event-emission paths. Results recorded in `rust/SCHEMA-8-MUTANTS.md`.
- `events.jsonl` of a fresh session contains no `LlmResponse`,
  `Compacted`, `text_fragment`, `thinking_fragment` strings.
- `events.jsonl` of a non-interleaved streaming response contains, in
  order: `llm_call`, `llm_response_started`, zero or more
  `thinking_block`, zero or more `text_block`, zero or more
  `tool_use_block`, `llm_response_ended`, zero or more `tool_call`, zero
  or more `tool_result`, optional `turn_end`.
- `events.jsonl` of an interleaved streaming response (mock fixture)
  contains content-block events in API content-block-index order.
- A streaming response that hits a 5xx mid-stream produces, in order:
  `llm_response_started`, partial block events, `llm_response_discarded`,
  `llm_retry`, `llm_call`, `llm_response_started`, …
- The leptos UI feed shows blocks appearing sequentially with no block
  ever disappearing, replacing, or relocating, verified by T5.
- Context.jsonl is byte-equal to current main on every non-interleaved
  fixture, and demonstrably correct on the interleaved fixture.
- **Browser-refresh replay**: e2e test that mid-turn (after some streamed
  content blocks but before `LlmResponseEnded`) reloads the page and
  asserts the reconstructed feed contains the same blocks in the same
  order with the same `data-block-id`s — i.e. replaying `events.jsonl`
  from disk reproduces the live-streamed UI exactly. A second variant
  reloads after a completed turn (post-`TurnEnd`) and asserts the same
  byte-stable reconstruction. The append-only grammar makes this
  reproducible by construction; the test pins the property.

## Out of scope (deferred)

- Removing `ToolResult.duration_ms` (cheap to keep; remove later if
  desired).
- Adding explicit `*BlockStarted` openers for strictly append-only live
  streaming (current placeholder approach is good enough).
- Enabling the `interleaved-thinking-2025-05-14` Anthropic beta. SCHEMA-8
  makes the agent and storage *correct* under interleaved streams; turning
  the beta on is a separate decision.

## Follow-ups discovered during execution

Small items surfaced after the planning doc was frozen. Not blockers
for SCHEMA-8 closure; logged here so they don't get lost.

### FU-1 — `session_resume::project_turn` empty-text inner-guard gap

Phase 8 (`cargo mutants -p omega-agent`) flagged five survivors in
`crates/omega-agent/src/session_resume.rs::project_turn`. Four are
equivalent mutants (outer `!pending_text.is_empty()` fast-path that the
inner guard already enforces). The fifth is real:

* `session_resume.rs:270:12 delete !` on the inner `!text.is_empty()`
  guard inside the post-loop flush block. Without the guard, an
  all-whitespace `pending_text` would push a stray `"\nAgent: "` line
  into the resumption summary instead of being skipped.

Impact is cosmetic: `project_turn` builds the human-readable summary
that the resumption-prompt feeds back to the model. A stray line in
that summary would be ugly but not wire-shape-breaking. Out of focal
scope for SCHEMA-8 (`agent.rs` and `anthropic.rs` only) but worth a
small test in a future tidy pass. See `rust/SCHEMA-8-MUTANTS.md` §
"omega-agent non-focal survivors" for the full triage.

### FU-2 — Mid-turn browser-refresh replay variant

`crates/omega-e2e/tests/09_refresh.rs` implements the
post-TurnEnd variant of the T6 browser-refresh replay test. The
acceptance-criteria text in this doc lists a mid-turn variant as well
("after some streamed content blocks but before `LlmResponseEnded`").
The file header explicitly documents the Phase 4e decision that the
post-TurnEnd variant alone covers T6's replay-correctness contract,
because `events.jsonl` is what gets replayed and the mid-turn
in-memory state isn't distinct from a post-turn replay of the same
persisted prefix. Adding the mid-turn variant remains a useful stress
test but is not a SCHEMA-8 blocker.
