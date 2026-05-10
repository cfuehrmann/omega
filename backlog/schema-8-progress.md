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

### Phase 0 — Defensive harness — **TODO**
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

## Active session decisions

- Time-freezing strategy: introduce a `now_iso()` injection point or use
  `OMEGA_TEST_FREEZE_TIME` env var, applied uniformly across goldens.
- Replay test harness lives in `rust/crates/omega-agent/tests/goldens.rs`.
- Goldens directory: `rust/crates/omega-agent/tests/goldens/<fixture>/`
  with `context.jsonl` + `notes.md` + `script.json` (the mock script).
