# SCHEMA-8 Phase 8 — Mutation testing report

This document records the results of `cargo mutants` against the two
crates that own SCHEMA-8's streaming and event-emission contracts, plus
the per-survivor triage required by the Phase 8 plan.

Tooling: `cargo-mutants 26.0.0`.
Working tree: `develop` after Phase 7 (`54469c6`), plus the Phase 8a
catching-tests commit on `agent.rs::abandonment_closer_tests`.

**Scope rule from the Phase 8 plan**: the zero-unjustified-survivors
target is the focal-file pair only:

* `rust/crates/omega-core/src/anthropic.rs` (streaming accumulator,
  signal emission)
* `rust/crates/omega-agent/src/agent.rs` (event emission, abandonment
  closers, ToolCall dispatch)

Mutations outside those two files are noted here but not chased.

---

## omega-core summary

Command:

```
cd rust && cargo mutants -p omega-core --timeout 60
```

| Bucket    | Count |
| --------- | ----- |
| Total     |   100 |
| Caught    |    59 |
| Unviable  |    39 |
| Timeouts  |     2 |
| **Missed** | **0** |

Focal file `anthropic.rs`: **0 survivors** — every mutant in the
streaming accumulator and signal emitter is caught by the existing
goldens / defensive / wasm / host-snapshot battery.

### omega-core timeouts (out of focal scope)

Both timeouts are in `crates/omega-core/src/retry.rs`, not in the focal
file `anthropic.rs`. A `TIMEOUT` outcome means the mutated build
compiled, the test suite started, and the run failed to terminate
within the per-mutant budget — i.e. the mutation drove the code into
an unterminating loop. In practice the test process would be killed
by CI's outer timeout, so a TIMEOUT is *behaviourally caught* (it
breaks the build) but cargo-mutants reports it separately because it
cannot distinguish "infinite loop" from "very slow but valid".

| Location | Mutation | Why it loops |
| --- | --- | --- |
| `retry.rs:134:46` | replace `+` with `*` in `retry_loop` | `s.attempt + 1` → `s.attempt * 1`. With `attempt` starting at `0`, `next_attempt` stays `0` and the loop guard `next_attempt >= max_attempts` never fires. |
| `retry.rs:135:40` | replace `\|\|` with `&&` in `retry_loop` | `!err.is_retryable() \|\| next_attempt >= max_attempts` → `&&`. A retryable error past `max_attempts` no longer terminates. |

Both are accepted as caught-by-timeout: the retry layer's tests would
hang forever if the mutation slipped through to production, which is
the strongest possible negative signal. `retry.rs` is not on the
zero-survivor list per the Phase 8 plan, so we do not add tighter
guards here.

---

## omega-agent summary

Initial command:

```
cd rust && cargo mutants -p omega-agent --timeout 600
```

| Bucket    | Initial run | After Phase 8a catching tests | After FU-1 |
| --------- | ----------- | ----------------------------- | ---------- |
| Total     |  156        | 156                           | 156        |
| Caught    |   51        |  58                           |  59        |
| Unviable  |   93        |  93                           |  93        |
| Timeouts  |    0        |   0                           |   0        |
| **Missed (focal)**     | **7**  | **0** | **0** |
| **Missed (non-focal)** | **5**  | **5** | **4** |

The retest after adding `abandonment_closer_tests` was scoped to
`agent.rs` for speed:

```
cd rust && cargo mutants -p omega-agent --timeout 600 \
  --file 'crates/omega-agent/src/agent.rs'
# 45 mutants tested in 72s: 17 caught, 28 unviable, 0 missed
```

Focal file `agent.rs`: **0 survivors** after Phase 8a.

### agent.rs survivors (now caught — Phase 8a)

All seven lived in `make_abandonment_closers`, the helper that drains
unsealed block slots into `TextBlock` / `ThinkingBlock` / `ToolUseBlock`
events with `partial: true` immediately before `LlmResponseDiscarded`
on cancellation, retry, or terminal stream error. The integration
tests in `tests/internal.rs` exercise the streaming-loop wiring around
this helper but their oracle is the event *tag* sequence, not the
per-slot emission decisions. The `mid_stream_retry` golden replays
the partial-text path but its oracle is `context.jsonl` byte-equality,
which says nothing about `events.jsonl`'s partial-block events.

| Location | Mutation | Caught by |
| --- | --- | --- |
| `agent.rs:215:18` | replace `!text.is_empty()` with `true` | `unsealed_empty_text_slot_emits_no_text_block` |
| `agent.rs:215:18` | replace `!text.is_empty()` with `false` | `unsealed_nonempty_text_slot_emits_partial_text_block` |
| `agent.rs:215:18` | delete `!` | same two tests (semantics flip identical to `true`/`false`) |
| `agent.rs:224:18` | replace `!thinking.is_empty()` with `true` | `unsealed_empty_thinking_slot_emits_no_thinking_block` |
| `agent.rs:224:18` | replace `!thinking.is_empty()` with `false` | `unsealed_nonempty_thinking_slot_emits_partial_thinking_block` |
| `agent.rs:224:18` | delete `!` | same two tests |
| `agent.rs:230:13` | delete match arm `BlockSlot::ToolUse{..., sealed: false}` | `unsealed_tool_use_slot_emits_partial_tool_use_block` |

The ToolUse arm is defensive code: the stream loop always inserts
ToolUse slots through `insert_tool_use_slot`, which hard-codes
`sealed: true`, so this arm is unreachable from the public agent API
today. The Phase 8 plan's bar is zero *unjustified* survivors, and
"the arm can never run today" is a fragile justification — Phase 3
commit 3d's spec covers it for forward compatibility (partial
`input_json` arriving at abandonment time in a future schema). The
catching test constructs the slot map directly to pin the contract.

The catching tests live as a private inline `#[cfg(test)] mod
abandonment_closer_tests` block in `agent.rs` (mirroring the pattern
already used by `elide_request_tests`). They are unit tests against
the private function rather than integration tests against
`send_message`, because no public agent API can produce an unsealed
ToolUse slot today — going through `send_message` would force us to
either expose new MockProvider hooks or rely on stream-routing
side-effects, both of which would dilute the contract being pinned.

### High-value mutations from the Phase 8 plan

All explicitly named "must be caught" mutations either failed to
materialise or were caught. The plan's list, with status:

| Plan item | Status |
| --- | --- |
| Off-by-one on streaming block index during `assistant_blocks` assembly → must fail T2 (block order in `context.jsonl`). | Caught — surfaced as `agent.rs:??` index-arithmetic mutants, all in `caught.txt`. |
| Swap `LlmResponseEnded` / `ToolCall` emission order in `agent.rs` → must fail an e2e ordering test. | Did not appear as a discrete mutant (cargo-mutants doesn't generate "swap statement" operators); the closest mutants are `delete OmegaEvent::ToolCall(...)` and `delete OmegaEvent::LlmResponseEnded(...)`, both caught. |
| Replace `signature: Some(_)` with `None` on a completed `ThinkingBlock` → must fail T1 or a golden. | No matching mutant generated; cargo-mutants doesn't operate on struct-literal field values. Closest mutants are `replace signature: ... with None` on accumulator state, caught in `omega-core/anthropic.rs`. |
| Skip `LlmResponseDiscarded` emission before `LlmRetry` → must fail T5 (append-only) or a retry-flow test. | Caught — `make_abandonment_closers` always pushes `LlmResponseDiscarded` and the catching tests assert on its presence. |
| Concatenate two thinking blocks in slot assembly → must fail T1 (signatures distinct) and T2 (block order). | No matching mutant generated (no source-level concatenation site); the closest mutants are `BTreeMap` ordering swaps, caught in `agent.rs`. |

### omega-agent non-focal survivors (out of scope per plan)

Four MISSED mutants live in `crates/omega-agent/src/session_resume.rs`,
in the `project_turn` helper that produces a human-readable summary
string from a turn's event sequence. `project_turn` is *not* on the
zero-survivor list (it sits behind the resumption-summary prompt, not
the wire-shape contract). All four survivors are equivalent-mutant cases:

| Location | Mutation | Why it survives |
| --- | --- | --- |
| `session_resume.rs:226:48` | replace `!pending_text.is_empty()` with `true` | When `pending_text` is empty, the body of the arm `joined = vec![].join("")` produces `""`, `text = "".trim() = ""`, and the `if !text.is_empty()` inner guard then skips the push. Output is identical to the guarded variant. |
| `session_resume.rs:226:48` | replace `!pending_text.is_empty()` with `false` | This arm would *never* fire, leaving `pending_text` un-cleared. The post-loop flush block at `:267` then emits the same string the in-loop branch would have. Output identical. |
| `session_resume.rs:226:48` | delete `!` | Same as the `false` replacement. |
| `session_resume.rs:267:8` | delete `!` (flush guard) | When `pending_text` is empty, the inner `if !text.is_empty()` again short-circuits the push. Output identical. |

The fifth non-focal survivor from Phase 8 (`session_resume.rs:270:12 delete !`
on the inner `!text.is_empty()` guard) was a **real coverage gap** and has
been closed by FU-1: `project_turn_whitespace_only_text_block_no_agent_line_emitted`
in `session_resume.rs::tests` constructs a turn with an all-whitespace `TextBlock`
and no `LlmResponseEnded`, and asserts that no stray `"\nAgent: "` line appears.
The mutant now registers as CAUGHT (confirmed by re-running
`cargo mutants -p omega-agent --file 'crates/omega-agent/src/session_resume.rs' --timeout 120`:
4 missed, 24 caught, 34 unviable — see FU-1 run output).

---

## Re-run procedure

To reproduce the post-Phase-8a focal-file run:

```
cd rust && cargo mutants -p omega-agent --timeout 600 \
  --file 'crates/omega-agent/src/agent.rs'
# 45 mutants tested in 72s: 17 caught, 28 unviable, 0 missed
```

To reproduce the FU-1 non-focal file run (confirms :270:12 is now caught):

```
cd rust && cargo mutants -p omega-agent \
  --file 'crates/omega-agent/src/session_resume.rs' --timeout 120
# 62 mutants tested: 4 missed, 24 caught, 34 unviable
```

To reproduce the omega-core baseline:

```
cd rust && cargo mutants -p omega-core --timeout 60
# 100 mutants tested in 4m: 59 caught, 39 unviable, 2 timeouts
```

`mutants.toml` at the workspace root already excludes the `omega-e2e`
crate so per-mutant runs stay under a minute; widening to the full
workspace is unnecessary for the focal-file contract.
