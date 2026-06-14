# Uniform event-emission spike — working notes

Goal: ground §3 to-do 1 of `docs/monitors-design.html` ("Uniform event
emission — one emitter, observers as projections") in the actual code, and
produce a concrete plan for the first two phases. **READ-ONLY spike** — no
production code or tests were changed; this notes file is the only artefact.

The design (target, invariants, the declined one-channel-actor alternative,
tests-first sequencing) is settled in the design doc and is NOT re-litigated
here. This file answers: *where does each event actually leave the agent
today, how do tests observe it, and what is the smallest Phase-1 step.*

---

## 1. EMISSION PATHS — the two ways an `OmegaEvent` reaches disk/WS

There are **two live emission paths** plus a **third init-only append** path.
The contract is documented in `crates/omega-agent/src/event_sink.rs:30-39`
("Each event source uses exactly **one** path, so nothing is emitted twice").

### Path A — the loop: `event_store.append()` + `yield AgentItem::event(...)`
The in-turn path. Every event born *inside* a turn is appended to
`events.jsonl` **and** yielded on the run-stream, as an identical pair:

```rust
let _ = self.event_store.append(&ev).await;   // durable projection
yield AgentItem::event(ev);                    // live run-stream
```

Sites (the repeated pair): `run_loop.rs` — 246/252/277/302/354/396/500/561/
570/577/596/604/630/652/671/688/694/806/810/822/828/853/871/894/898/914/
986/998/1032/1054/1090/1111/1121/1144/1181 (plus a handful of `yield`-only
re-forwards at 238/271/645/846/1101/1138 where the inner block-grammar match
already appended). The resume/replay variant lives in `resume.rs`
(91/113/162/191/274/310/318/326/351/367/377/390/444/465/469/480/486). The
inject helpers append-only (the `yield` of their event happens at the loop
call-site that drives them): `inject.rs` 81/119/140/183/215/244/296.

`store.append` is `EventStore::append` (`crates/omega-store`), per-line
atomic. The yield surfaces on `Agent::run`'s `BoxStream<AgentItem>`.

### Path B — the sink: `EventSink::emit` / `emit_detached`
The out-of-band path (`event_sink.rs`). A single call **appends then
broadcasts** (`emit`, `event_sink.rs:113-117`) or **broadcasts then spawns
the append** (`emit_detached`, `event_sink.rs:125-131`). It does **not**
touch the run-stream — there is no `yield`. Used for events born *outside* a
turn (no generator in scope to yield from):

| Call site | Event | Method |
|---|---|---|
| `agent/mod.rs:241` (`set_model`) | `ModelChanged` | `emit` |
| `agent/mod.rs:256` (`set_effort`) | `EffortChanged` | `emit` |
| `controls.rs:138` (`request_halt`) | `HaltRequested` | `emit` |
| `controls.rs:154` (`unrequest_halt`) | `HaltUnrequested` | `emit` |
| `input_queue.rs:327` (`InboxSink::deliver_stderr`) | `MonitorStderr` | `emit_detached` |

`emit_detached` exists because its caller (`MonitorSink::deliver_stderr`) is
synchronous; it broadcasts in-order and spawns the disk write
(`event_sink.rs:119-124`).

### Path C — init-only append (no yield, no sink)
Three lifecycle events are appended directly before the run-stream exists:
`ServerStarted` (`lifecycle.rs:126`), `SessionStarted` (`lifecycle.rs:200`),
and the resume seed `SessionResumed` (`lifecycle.rs:389`, returned to the
server which broadcasts it by hand). These run during `init`/`seed`, outside
any turn and outside the sink. Minor, but real — a third place an event
reaches `events.jsonl`.

### Per-variant path table
| OmegaEvent variant(s) | Path |
|---|---|
| `UserMessage`, `LlmCall`, `LlmResponseStarted`, `LlmResponseEnded`, `LlmResponseDiscarded`, `LlmError`, `LlmRetry` | A (loop) |
| `TextBlock`, `ThinkingBlock`, `ToolUseBlock` (block-grammar, partial+final) | A (loop) |
| `ToolCall`, `ToolResult` | A (loop) |
| `TurnEnd`, `TurnInterrupted`, `TurnHalted`, `TurnResumed` | A (loop) |
| `ContextCompacted` | A (loop, `run_loop.rs:894/806`) |
| `HarnessRecovery` | A (loop, via inject helper) |
| `MonitorStarted`, `MonitorDelivery`, `MonitorStopped` | A (loop — delivered through the inbox as `InputItem`, then injected) |
| `ConversationInvariantViolated` | A (loop — `append_record` guard, forensic) |
| `ResumingSession`, `SessionResumed` (replay), block events on resume | A (`resume.rs`) |
| **`ModelChanged`, `EffortChanged`** | **B (sink, `emit`)** |
| **`HaltRequested`, `HaltUnrequested`** | **B (sink, `emit`)** |
| **`MonitorStderr`** | **B (sink, `emit_detached`)** |
| `ServerStarted`, `SessionStarted`, `SessionResumed` (seed) | C (init append) |

### "No double-emit" invariant — how it is guaranteed today
**By construction, via disjoint variant ownership** — not by any runtime
check. Each `OmegaEvent` variant is emitted from exactly one path because the
*physical code site* for that variant lives on exactly one path. Verified for
the sink set: `ModelChanged`/`EffortChanged`/`HaltRequested`/`HaltUnrequested`/
`MonitorStderr` appear **only** at the sink call sites above and nowhere in
`run_loop.rs`/`resume.rs`/`inject.rs` (grep-confirmed: `run_loop.rs` contains
`TurnHalted`@1051 and `TurnResumed`@1118 — the *loop's* halt/resume lifecycle
events — but never `HaltRequested`/`ModelChanged`/etc.). So the halt story is
split cleanly: **`HaltRequested` (the click) = sink; `TurnHalted`/`TurnResumed`
(the loop reaching/leaving the seam) = loop.** No variant is on two paths.

This is exactly the fragility the design targets: the invariant is a
*maintained convention* ("don't emit the same variant from two places"),
enforced only by reviewer discipline, not structurally. One emit point makes
it unrepresentable.

---

## 2. RUN-STREAM CONTENTS — `Agent::run` yields `AgentItem`

`AgentItem` (`crates/omega-core/src/types.rs:142-149`) is a 2-variant enum:

```rust
pub enum AgentItem {
    Signal(StreamSignal),     // ephemeral, NEVER persisted
    Event(Box<OmegaEvent>),   // fact, persisted via Path A append
}
```

- **`AgentItem::Event`** — a fact. On Path A it is *always* paired with an
  `event_store.append`, so it is also on disk. The run-stream `Event` and the
  `events.jsonl` line are the *same value*.
- **`AgentItem::Signal`** — a `StreamSignal` (`crates/omega-types/src/
  stream_signal.rs`). Module doc (line 1-4): *"These are never written to
  `events.jsonl`. They are yielded by the agent loop to drive live
  rendering."* Provider-sourced streaming primitives.

### The signals (all of `StreamSignal`, `stream_signal.rs:20-61`)
| Signal | Meaning | Forwarded to UI? |
|---|---|---|
| `Text { index, text }` | text token fragment | yes |
| `Thinking { index, text }` | thinking token fragment | yes |
| `TextBlockComplete { index, text }` | assembled text block | (agent finalises slot) |
| `ThinkingBlockComplete { index, signature }` | thinking signature | **no** (internal) |
| `ToolUseBlockStart { index, tool_use_id, name }` | tool block opened | yes (early label) |
| `ToolInput { index, partial_json }` | partial tool JSON | yes (raw) |
| `ToolUseBlockComplete { index, tool_use_id, name, input }` | parsed tool input | **no** (internal) |

### Where signals are yielded
Signals enter the run-stream by being **forwarded from the provider stream**.
The provider yields `AgentItem::Signal(...)`; the loop matches it
(`run_loop.rs:401` `Ok(AgentItem::Signal(sig))`), updates its slot
accumulator, may emit a derived block *event* (e.g. `TextBlock` — appended +
yielded, Path A), and re-yields the signal iff `forward` is true
(`run_loop.rs:503-505`). The resume path mirrors this (`resume.rs:195/278`).
So signals are **pass-through ephemera**: provider → loop accumulator →
(optional derived event on Path A) → re-yield. They never touch
`event_store`.

Net: the run-stream is an **interleaving** of persisted `Event`s (= Path A
events.jsonl lines, in order) and ephemeral `Signal`s (= live tokens, gone
after the turn). Path B/C events are **not** on the run-stream at all.

---

## 3. SERVER AS CONSUMER — `spawn_run_task` (router.rs ~990-1027)

The server drives the run-stream in a spawned task
(`crates/omega-server/src/router.rs:990`):

```rust
let mut stream = guard.run(input_queue.clone(), run_cancel);
while let Some(item) = stream.next().await {
    let next = match &item {
        AgentItem::Event(ev) => next_turn_state_for(ev),   // router.rs:994-997
        AgentItem::Signal(_) => None,
    };
    let push_roster = is_monitor_event(&item);             // router.rs:999
    let push_queue  = is_inbox_drain_event(&item);         // router.rs:1000
    send_to_active(&slot_arc, WsMessage::Item(Box::new(item))).await;  // 1001
    if push_roster { ...roster_snapshot_msg... }           // 1003-1006
    if push_queue  { ...queue_snapshot_msg(input_queue.snapshot())... } // 1010-1013
    if let Some(target) = next { ...turn-state transition + SessionInfo... } // 1014-1023
}
```

Every run-stream item is forwarded verbatim to the WS as `WsMessage::Item`
(both `Event` and `Signal` — the frontend needs the live signals). On top of
that raw forward, the server **derives four reactions** — these are the
"control/UI reactions as subscribers" surface:

1. **Turn-state transitions** (`next_turn_state_for`, `router.rs:574-584`):
   pure event→state map. `UserMessage`/`TurnResumed`/`MonitorDelivery`/
   `MonitorStopped` → `"running"`; `TurnHalted` → `"halted"`; `TurnEnd`/
   `TurnInterrupted` → `"idle"`; everything else → `None`. On a real change
   it re-broadcasts `SessionInfo` (`cache_into_message`, `router.rs:541`).
2. **Roster pushes** (`is_monitor_event`, `router.rs:83-95`): after any
   `MonitorStarted`/`MonitorDelivery`/`MonitorStderr`/`MonitorStopped`, push a
   fresh `MonitorRoster` snapshot.
3. **Queue snapshots** (`is_inbox_drain_event`, `router.rs:115-128`): after
   `UserMessage`/`MonitorDelivery`/`MonitorStopped` (an inbox item just
   drained), push a fresh `InputQueue` snapshot. (Also pushed on *enqueue* via
   the `InputQueue::set_on_change` callback wired at `router.rs:982-989` —
   that path is independent of the run-stream.)
4. **Info messages** — the `SessionInfo`/`InputQueue`/`MonitorRoster`
   snapshots above. All three are **ephemeral, transport-only** and explicitly
   MUST NOT be persisted (`router.rs:130-131`, `145-146`).

Key point for Phase 2: **all four reactions are pure functions of the
forwarded `OmegaEvent`** (plus side-state read from `monitor_manager` /
`input_queue`). They are already "subscribers that project events to UI
state" — exactly the observer shape the design wants. They depend only on
Path-A events though; Path-B events (`ModelChanged`/`Halt*`/`MonitorStderr`)
reach the WS via the sink's `EventBroadcaster`, **not** this loop, so they
bypass these reactions (e.g. `MonitorStderr` does NOT trigger a roster push
here — it is broadcast straight to the client). Unifying emission would let
these reactions see *every* event uniformly.

---

## 4. TEST OBSERVATION — classifying every agent test

Files: `crates/omega-agent/tests/{internal.rs, goldens.rs, defensive.rs}`
(+ shared `tests/common/mod.rs`). Four observation channels exist:

- **(a) run-stream via `collect_stream`** (`common/mod.rs:286-296`) — drives
  `Agent::run` (through `drive`, `common/mod.rs:262-280`) and collects the
  `Vec<AgentItem>`. Assertions via `tags(&items)` (`common/mod.rs:300-360`,
  projects each item to a string tag, **including `Signal:*` tags**).
- **(b) sink via `RecordingBroadcaster`** (`common/mod.rs:44-66`) — installs
  an `EventBroadcaster` that records every `OmegaEvent` the *sink* broadcasts;
  read with `.snapshot()`.
- **(c) `events.jsonl` read directly** — `defensive.rs:read_jsonl`
  (148-153) + `block_events_from_events_jsonl` (170-180); `internal.rs:
  read_events_jsonl` (253-259). The "more e2e" style the design points at.
- **(d) `take_requests`** (`common/mod.rs:107-113`) — drains the captured
  `LlmRequest`s to assert on the payload the agent *sent* to the provider.

(A fifth, **context.jsonl** read — `defensive.rs:last_assistant_content`
156-167; `goldens.rs` byte-goldens — is a separate durable projection of the
*conversation*, not events. Out of scope for event-assertion migration; stays.)

### `defensive.rs` (3 tests)
All three (`t1_signatures_preserved` 187, `t2_block_order_in_context_jsonl`
247, `t3_events_and_context_carry_same_blocks` 309) use **(c)**: they call
`collect_stream` only to *drive* the turn (`_items` discarded, lines 195/263/
329) then assert against `events.jsonl` + `context.jsonl`. **Already the
target shape.** t3 asserts EVENTS (events.jsonl block events vs context
blocks) → already migrated. t1/t2 assert context.jsonl only → orthogonal.

### `goldens.rs` (11 tests)
Ten are **context.jsonl byte-goldens** (drive a turn, scrub `time`,
byte-compare) — assert on the *conversation* projection, **not events**.
Stay as-is (one reads events.jsonl incidentally). No event-assertion
migration applies.

### `internal.rs` (64 tests) — per-mechanism tally
Counts are *test functions touching* each channel (a test may use several;
"event-assertion" = asserts on `OmegaEvent` content/order):

| Channel | ~Tests | Notes |
|---|---|---|
| (a) `collect_stream` (drive + inspect) | ~30 | of which **10** assert a `tags()` sequence |
| (a) `tags()` strict sequence | ~7 | the event-order assertions (some interleave `Signal`) |
| (b) `RecordingBroadcaster` | **5** | `mid_turn_model_change`, `monitor_stderr_emitted_to_sink_not_projected`, `monitor_stderr_emitted_promptly_while_agent_parked`, `push_human_item_processed_queue_empty_after_drain`, `snapshot_reflects_pending_state` |
| (c) `read_events_jsonl` already | **~12** | e.g. `compacted_event_clears_history…`, `turn_end_metrics_carry_cache_tokens`, `context_compacted_event_emitted_before_response…`, `append_monitor_started_writes_event_to_log`, `context_hashes_accessor…`, `mid_turn_model_change…` |
| (d) `take_requests` | **~30** | assert on `LlmRequest` payload (system prompt, tools, context mgmt, wrapper format) |

### EVENT-assertion vs SIGNAL/REQUEST (the migration axis)
- **Assert on EVENTS, currently via the run-stream (movable to events.jsonl):**
  the `tags()`-sequence tests where the tags are events. Strict-sequence:
  `dangling_tool_use_synthesises_is_error_tool_results` (142),
  `malformed_tool_json_triggers_nudge_and_retry` (602),
  `empty_end_turn_injects_continuation_and_completes` (2815),
  `empty_tool_use_stop_injects_continuation` (2874),
  `non_empty_end_turn_ends_turn_normally` (2911),
  `empty_response_cap_exceeded_surfaces_error` (2963),
  `monitor_stopped_wrapper_format_in_llm_context` (tags incidental).
- **Assert on EVENTS, already via events.jsonl (done):** the ~12 (c) tests +
  defensive t3. No work.
- **Assert on EVENTS, via the sink (movable once sink events join the bus):**
  the 5 (b) `RecordingBroadcaster` tests. Today the sink is the *only* place a
  test can see `ModelChanged`/`MonitorStderr` (they are NOT on the run-stream).
  But they **are** on disk (sink `emit` appends). So these are *already*
  movable to `events.jsonl` — `mid_turn_model_change` already does both
  (broadcaster + `read_events_jsonl`, line 964).
- **Assert on SIGNALS / interleaving (must stay on the run-stream):** the
  tests whose `tags()` vector includes `"Signal:Text"` interleaved with
  events — `dangling…`(150), `malformed…`(614), `empty_end_turn…`(2827),
  `empty_tool_use…`(2887), `non_empty_end_turn…`(2920) = **5 tests**.
  `events.jsonl` cannot show signal↔event interleaving (signals are never
  persisted), so the signal-ordering portion of these is a **hard blocker**.
- **Assert on REQUESTS (must stay):** the ~30 (d) `take_requests` tests.
  `LlmRequest` is not an event; orthogonal to emission unification.
- **Loop/settle-condition `tags(from_ref)`** (`internal.rs:2501, 3222, 3271,
  3701`): scan items as a wake/settle condition, not a sequence assertion.
  Drive-only; unaffected.

---

## 5. PHASE-1 FEASIBILITY — migrate event-assertions to `events.jsonl`

**Verdict: feasible and low-risk for the bulk; a small, bounded blocker set.**

- **Already done:** ~12 `internal.rs` (c) tests + `defensive.rs` t3 already
  assert against `events.jsonl`. The pattern is proven and in-tree.
- **Clean migrations (~2–3 tests):** the strict `tags()`-sequence tests whose
  tags are *all events* (`empty_response_cap_exceeded_surfaces_error`,
  `monitor_stopped_wrapper_format_in_llm_context`) — replace the
  `tags(&items)` vector with a filtered `read_events_jsonl` type-sequence.
- **Mixed (the 5 signal-interleaving tests):** `dangling…`, `malformed…`,
  `empty_end_turn…`, `empty_tool_use…`, `non_empty_end_turn…`. Split each:
  the **event subsequence** moves to an `events.jsonl` type-list assertion;
  the **signal presence/position** stays as a (reduced) run-stream check. The
  interleaving claim itself (`Signal:Text` *between* `LlmResponseStarted` and
  `LlmResponseEnded`) is the part `events.jsonl` structurally cannot express —
  keep it on the stream, but it need no longer carry the event-order load.
- **Sink tests (the 5 (b)):** point at `events.jsonl` instead of the
  `RecordingBroadcaster` snapshot — the sink already appends, so the events
  are on disk. `mid_turn_model_change` already reads both, proving the move.
  (Keep ONE broadcaster test as the dedicated "WS projection still fires"
  guard; the rest assert content via disk.)

**Shared helper they'd need.** `defensive.rs` already has the exact parser —
promote it to `tests/common/mod.rs` so all three files share it:

```rust
// defensive.rs:148-153 — the helper to lift into common/mod.rs
fn read_jsonl(path: &std::path::Path) -> Vec<Value> {
    let raw = std::fs::read_to_string(path).expect("read jsonl file");
    raw.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("parse jsonl line"))
        .collect()
}
```

plus a typed-sequence convenience (mirroring `block_events_from_events_jsonl`,
`defensive.rs:170-180`) e.g. `event_types(events_path) -> Vec<String>` that
maps each line's `"type"` to a string — the `events.jsonl` analogue of
`tags()` minus the signals. `internal.rs:read_events_jsonl` (253-259) is the
same parser already; dedupe the two into the shared one.

**Blockers (explicit).**
1. **Signal↔event interleaving** (the 5 mixed tests) — `events.jsonl` omits
   signals by definition (`stream_signal.rs:1-4`). Cannot be expressed on
   disk; the signal-ordering assertion *must* stay a run-stream check. Not
   fatal — split the test.
2. **Path-B/C events have no run-stream presence** — `ModelChanged`/`Halt*`/
   `MonitorStderr`/`ServerStarted`/`SessionStarted` never appear via
   `collect_stream`, so a stream-only test *cannot* see them at all today.
   Migrating them to `events.jsonl` is strictly an *improvement* (one uniform
   place), not a loss.
3. **Pure stream-mechanics / settle tests** (the `tags(from_ref)` loop
   scanners, drive-only tests) — nothing to migrate; leave.

**Count to migrate:** of the 64 `internal.rs` tests, **~10–12** carry an
event-assertion that *should* move to `events.jsonl` (≈7 `tags`-sequence +
5 sink), of which **5 are partial** (signal portion stays). The other ~52
are requests (~30), context/goldens, accessors, drive-only, or already-on-disk
(~12). So **Phase-1 is on the order of a dozen targeted test edits + one
shared helper promotion**, with the 5 signal-interleaving tests split rather
than moved wholesale.

### Per-file Phase-1 migration steps
- **`tests/common/mod.rs`**: add `read_jsonl` (lift from `defensive.rs`) +
  `event_types(path) -> Vec<String>` (type-sequence). Keep `tags()` for the
  residual signal checks.
- **`defensive.rs`**: already on `events.jsonl`; just switch its local
  `read_jsonl` to the shared one (no behaviour change).
- **`internal.rs`**:
  - dedupe `read_events_jsonl` → shared `read_jsonl`.
  - clean `tags`-sequence tests (`empty_response_cap…`,
    `monitor_stopped_wrapper…`): replace event tags with `event_types`.
  - 5 mixed tests: assert event subsequence via `event_types`; retain a
    reduced `tags` check only for `Signal:*` position.
  - 5 sink tests: assert content via `events.jsonl`; keep one broadcaster
    test as the explicit WS-projection guard.
- **`goldens.rs`**: untouched (context.jsonl byte-goldens; not events).

---

## 6. PHASE-2 SKETCH — the single emitter

**Shape.** Introduce one emit primitive on the agent — the natural home is the
existing `EventSink` (already owns `Arc<EventStore>` + a swappable
broadcaster, `event_sink.rs:54-57`), generalised so **every** event goes
through it. Rename/recast its role from "out-of-band sink" to "the emitter".

```text
                       ┌──────────────► disk observer  (EventStore::append → events.jsonl)   [DURABLE, ordered]
agent emits ev ──► Emitter.emit(ev) ──┼──────────────► WS observer    (EventBroadcaster → WsMessage::Item)    [live, best-effort]
                       └──────────────► test recorder  (RecordingBroadcaster, in tests)
```

- **Observers subscribe**, each a projection: the disk observer is the
  comprehensive ordered one (today's `store.append`); the WS observer is the
  live best-effort one (today's `EventBroadcaster`); tests subscribe a
  recorder. The disk + WS observers already exist *inside* `EventSink::emit`
  (`event_sink.rs:113-117`) — Phase 2 makes the loop route its Path-A events
  through the same call instead of the hand-written `append`+`yield` pair.
- **The run-stream becomes a projection too**, not a separate emission path:
  `AgentItem::Event` items are yielded *by an observer that forwards to the
  stream*, so the server keeps its current consumer (§3) unchanged. The yield
  stops being an independent emit and becomes a subscription.

**What stays OUT of the bus (explicitly):**
- **The loop's drive/effects** — calling the LLM, dispatching tools, deciding
  the next turn. These are *commands*, not facts (invariant (b): "an effect
  must never reach `events.jsonl`"). They remain ordinary sequential code in
  `run_loop.rs`/`drive_turn`. In sequential code an effect is a function call,
  so it physically cannot land in the log — that structural guarantee is the
  whole reason the one-channel-actor alternative was declined.
- **Signals** (`StreamSignal`) — ephemeral, never persisted, stay on the
  run-stream projection only (they are not events).
- **Inputs** (invariant (a): inputs are peers, not projections) — the inbox
  (`InputQueue`: user / monitor) is folded *alongside* events, not derived
  from them. The Gather/Seam drains stay as they are.
- **The server's UI snapshots** (roster/queue/SessionInfo) — already ephemeral
  transport-only derivations; they remain subscribers (§3).

**Hardest part — the `&mut self` / `stream!` tension.** `run`/`drive_turn`
borrow `&mut self` for the whole turn and emit from inside an `async_stream::
stream!` generator (same tension flagged in the seam-typestate spike's "Borrow
note"). The current code yields events *directly out of the generator*; a true
"emit to a bus, observer forwards to the stream" needs the emit call and the
yield to cooperate without a second `&mut self` borrow. Two viable shapes:
  1. **Emitter owns the channel; the stream is an observer.** `emit(ev)` does
     `store.append(&ev)` + `broadcaster.broadcast(&ev)` + push onto an
     in-turn channel that the `stream!` drains and yields. Keeps the loop's
     `&mut self` drive intact; the generator just forwards what the emitter
     produced. Adds one channel hop per event.
  2. **Keep the yield, centralise the append.** A single
     `self.emit(ev)` helper that appends + broadcasts + `yield`s in one place,
     called everywhere instead of the open-coded triple. Lighter, but the
     `yield` means it must be a macro/inline within the generator, not a plain
     async fn — `yield` cannot cross a fn boundary. This is the smaller step
     and likely the right Phase-2a.

**Synchronous/ordered?** **Yes for the durable projection.** `events.jsonl`
is *the* ordered comprehensive record; its append must stay synchronous and
in emission order (today `emit` appends *before* broadcast precisely so a
reconnecting client never sees a wire event not yet on disk —
`event_sink.rs:108-112`). `emit_detached` (spawned append) is the one
exception, justified only for the synchronous `MonitorStderr` reader where
§17 explicitly allows file-order ≠ time-order. The WS observer may be
best-effort/lossy; the disk observer may not. So the emitter must offer an
**ordered, awaited** path for durable correctness, with the detached variant
as a deliberate opt-out.

---

## STATUS / RESUME-HERE

- **Spike COMPLETE (read-only).** This file is the durable record. No
  production code or tests changed.
- **Two emission paths confirmed** (+ a minor third init-append): Path A
  loop `append`+`yield` (in-turn, ~50 sites across `run_loop.rs`/`resume.rs`/
  `inject.rs`); Path B sink `emit`/`emit_detached` (5 out-of-band sites:
  `mod.rs:241/256`, `controls.rs:138/154`, `input_queue.rs:327`); Path C init
  append (`lifecycle.rs:126/200/389`). **No double-emit**, guaranteed only by
  disjoint per-variant code sites (a convention, not a structural check) —
  which is exactly what unification makes structural.
- **`events.jsonl` is already complete & correct** — both A and B append. The
  split bites in the *live* observers and in tests (`collect_stream` sees only
  A; `RecordingBroadcaster` sees only B). `defensive.rs` already shows the
  more-e2e style (assert against `events.jsonl`).
- **Phase-1 verdict: feasible, ~a dozen targeted test edits + 1 shared
  helper.** ~12 `internal.rs` tests + defensive t3 already on disk; ~7
  `tags`-sequence + 5 sink tests to move; **5 signal-interleaving tests are
  partial** (event subsequence moves, signal-ordering stays). ~30
  `take_requests` tests + 10 context goldens are orthogonal and stay.
- **Phase-2 home = generalise `EventSink` into THE emitter**; observers =
  disk (ordered/awaited) + WS (best-effort) + test recorder; run-stream
  becomes a forwarding observer. Drive/effects, signals, and inputs stay OUT.
  Hardest part = `&mut self` + `stream!` `yield` cannot cross a fn boundary
  (same borrow tension as the seam-typestate work); durable projection must
  stay synchronous & ordered.

### Recommended Phase-1 step list (crisp)
1. Promote `read_jsonl` (`defensive.rs:148-153`) into `tests/common/mod.rs`;
   add `event_types(path) -> Vec<String>` (the disk analogue of `tags()`).
   Dedupe `internal.rs:read_events_jsonl` into it.
2. Migrate the all-event `tags`-sequence tests (`empty_response_cap…`,
   `monitor_stopped_wrapper…`) to `event_types` assertions.
3. Split the 5 signal-interleaving tests: event subsequence → `event_types`;
   keep a minimal `tags` check for `Signal:*` position only.
4. Point the 5 `RecordingBroadcaster` tests at `events.jsonl`; retain exactly
   one broadcaster test as the explicit "WS projection still fires" guard.
5. Leave `take_requests` tests, context/goldens, accessors, and drive-only /
   settle-condition tests untouched.
6. Run `cargo test -p omega-agent`; the suite now asserts on the unified
   projection, so Phase-2's emission internals can be unified behind it
   without rewriting the suite (the design's tests-first enabler).
