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

