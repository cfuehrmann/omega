# Seam-typestate spike — working notes

Goal: enumerate the states `drive_turn` actually moves through, to confirm the
proposed conversation **typestate** maps cleanly before implementing. The
typestate must be a **register/EFSM**: the `AwaitingToolResults` state carries
pending tool-call ids as an unbounded `Set<ToolUseId>` *data* field (never
flattened into finite state labels). Decision recorded in
`docs/monitors-design.html` §3 to-do 1.

## Key constraints (model-facing protocol correctness)
- Three `role:user` sources: tool-results, human `UserMessage`, monitor `MonitorDelivery`.
- Anthropic rules: roles alternate (consecutive user records merged by
  `project_messages()`); a `tool_use` is answered by its `tool_result`s in the
  **very next** message, all ids, nothing between.
- Single-writer = structural (loop owns `context_store`; monitors/humans only
  enqueue). Seam-gated drain + atomic tool-result batch = emergent/reactive —
  the part to lift into types.

## Two legal injection seams
- Seam A: after `end_turn` (idle / Gather park).
- Seam B: after the `tool_result` blocks answering a `tool_use`.
- Queue drained ONLY at these two points.

## `run()` loop structure (run_loop.rs ~100-150)
1. Gather: `tokio::select!` on cancel vs `inbox.pop()` (park, zero CPU); then
   `items = [first] + inbox.drain_pending()` (batch → merged user message).
2. `drive_turn(items, inbox.clone(), cancel.clone())` — streams events; does
   NOT park.
3. Park-vs-terminate: run-level cancel → return; headless terminates iff inbox
   empty AND `monitors.live_count()==0`; interactive loops back to park.

## `drive_turn()` structure (run_loop.rs ~163 onward)
- Step 0 (~178-200): `reset_for_turn()` → fresh turn-scoped cancel token;
  spawn cancel forwarder; `TurnGuard` (clears state on drop); snapshot
  `turn_model`/`turn_effort` (mid-turn model change applies NEXT turn);
  clone `monitors`.
- Step 1 (~205-258): dangling tool_use repair. If `history.last()` is Assistant
  with tool_use blocks lacking results → `inject_dangling_tool_results(dangling)`
  → yields ToolResult events. (Reactive recovery — the bug class the typestate
  should prevent at source.)
- Step 2 (~269-282): inject every gathered inbox item via `inject_input_item`
  (human OR monitor) → each one role:user record + one event. (This is the
  Seam-A batch that `run()`'s Gather already pulled; consumed here.)
- Step 3 (~286 onward): outer agentic loop. Per iteration:
  - cancel check → `TurnInterrupted{Aborted}`, return.
  - build `LlmRequest` (`project_messages(&self.history)`), emit `LlmCall`,
    stream provider → fill `slots`.
  - assemble `assistant_blocks` + `combined_tool_uses` (~710-760) in slot order;
    each entry = `(tool_call_id, tool_use_id, name, input)`.
  - empty-response guard (~767+): if no blocks, inject continuation, do NOT
    persist empty assistant turn; loop.
  - append assistant record (~861-884): `context_store.append(Assistant, blocks)`
    + `history.push` + `context_hashes.push`; `assistant_hash` minted here.
  - emit `LlmResponseEnded`.
  - if `stop_reason=="tool_use" && !combined_tool_uses.is_empty()` (~908):
    - emit one `ToolCall` event per use (carries `assistant_hash`).
    - dispatch via `FuturesUnordered` (~946); collect into `by_call_id` map.
    - emit one `ToolResult` event per completed future (~996-1006).
    - assemble `result_blocks` (~1009) by iterating `combined_tool_uses` and
      pairing each `tool_use_id` — missing future → synthetic error result.
    - `inject_tool_results_batch(result_blocks)` (~1033) = ONE role:user record.
    - Halt seam (~1055): optional park (resume / steer / abort).
    - Seam B drain (~1140): `inbox.drain_pending()` → `inject_input_item` each.
    - `continue` (next LLM call).
  - else (no tool calls): emit `TurnEnd` (~1183), Seam A, `return`.

## KEY FINDING — where the gap actually is
Within a single `drive_turn`, the atomic tool-result batch is ALREADY
structural: `result_blocks` is built by iterating `combined_tool_uses`, so the
exact-bijection (one result per emitted id) holds by construction at the
`inject_tool_results_batch` call. The queue drains (Seam A in `run()` after the
turn returns; Seam B after the batch is appended) are likewise already
control-flow-gated to pending-empty points.
The RESIDUAL gap the typestate closes:
  1. The INTERRUPT boundary — a cancel between the assistant-record append
     (~877) and `inject_tool_results_batch` (~1033) leaves a dangling tool_use,
     repaired REACTIVELY next turn by Step 1. The typestate makes "cannot start
     a new turn / drain queue while `pending` non-empty" a compile-time fact.
  2. The discipline is spread across MANY append sites (below); the typestate
     centralises it.

## THE CHOKEPOINT — one primitive to gate
Every context mutation is the SAME triple, repeated in each inject helper:
```
let hash = self.context_store.append(role, blocks).await?;
self.history.push(Message { role, content: blocks });
self.context_hashes.push(hash);
```
Sites: inject.rs:88/129/174/206 (user/monitor/system variants), inject.rs:267
(dangling), inject.rs:293 (tool_results_batch); run_loop.rs:877 (assistant);
lifecycle.rs:399/413; resume.rs:433+.
Design implication: introduce ONE private `append_record(role, blocks)` (or a
typed move on the typestate) that all helpers funnel through, and gate IT on
`ConvState`. That collapses the scattered discipline into a single typed seam.

## Known append sites (single writer = the loop)
- ToolResult event: run_loop.rs ~996-1006 (`event_store.append(&tr)` + `yield`).
- result_blocks assembled ~1009+ then appended to context_store as one user record.
- Dangling repair appends in inject.rs ~250-258.

## Borrow note
`drive_turn(&'a mut self, ...)` borrows `&mut self` for the whole turn; `run`
borrows `&mut self` for the whole session. So the typestate is most likely a
**field-enum tag on the agent** (e.g. `self.conv_state: ConvState`) mutated in
place, NOT a consumed-`self` builder. Confirm the `stream!` macro + `&mut self`
reborrow interplay allows threading a typestate handle.

## Candidate typestate (provider-ABSTRACT; keep wire facts in projection)
```
enum ConvState {
  Idle,                                  // Seam A: may drain queue (user/monitor input)
  AwaitingAssistant,                     // after user input appended; expect assistant
  AwaitingToolResults { pending: HashSet<ToolUseId> },  // register; Seam B when emptied
}
```
- drain_queue available ONLY in Idle (and post-emptied Seam B).
- `add_tool_result(id)` removes from `pending`; cannot leave until empty.
- Abstract events: tool_calls_emitted / tool_result_provided / user_input.
- Provider-specific encoding (role:user merge, etc.) stays in `project_messages`.

## STRICTNESS REQUIREMENT (non-CFG / data-language) — MUST HOLD
The implementation must enforce **id-correctness**, not merely count-matching.
The invariant is an **exact bijection** between the tool_use ids the assistant
emitted and the tool_result ids provided — no missing, no extra, no duplicates.
Concretely on `AwaitingToolResults { pending: HashSet<ToolUseId> }`:
- `pending` is seeded with the EXACT id set from the assistant's tool_use batch.
- `add_tool_result(id)`:
  - MUST reject an `id` not in `pending` (foreign/unknown id) — hard error.
  - removes `id` from `pending`; a duplicate result for an already-answered id
    is therefore also rejected (it is no longer in `pending`).
- The state CANNOT transition out of `AwaitingToolResults` until `pending` is
  empty (every emitted id answered exactly once).
This is precisely the register/data-automaton power: count-only matching
(`aⁿbⁿ`, CFG) is NOT sufficient — ids must correspond. A `HashSet` register
holding the actual ids is the minimal sufficient mechanism; do not weaken it to
a counter or to finite state labels.

## Spike conclusion / recommended design
1. States/transitions: enumerated above — maps cleanly onto
   `Idle → AwaitingAssistant → (AwaitingToolResults{pending} → AwaitingAssistant)* → Idle`.
2. Inbox drains: confirmed two — Gather/Seam A in `run()` (after turn returns)
   and Seam B in `drive_turn` after `inject_tool_results_batch`. Both already
   land only at pending-empty points (to be made compile-time).
3. Append sites: NOT a single point today — the same triple is duplicated across
   inject.rs / run_loop.rs / lifecycle.rs / resume.rs. Step 1 of the work is to
   funnel them through one `append_record(role, blocks)` chokepoint.
4. Shape: **field-enum tag on the agent** (`self.conv_state: ConvState`),
   mutated in place — NOT a consumed-`self` builder. Forced by `run()`/`drive_turn`
   holding `&mut self` for the whole session/turn, and by `stream!` reborrows.
   The typestate is enforced by the gated `append_record`, not by Rust move
   semantics on `self`.

Recommended `ConvState` (provider-ABSTRACT; wire facts stay in `project_messages`):
```
enum ConvState {
    Idle,                                              // Seam A: drain queue legal
    AwaitingAssistant,                                 // expect assistant record next
    AwaitingToolResults { pending: HashSet<ToolUseId> },// register; no drain, no new turn
}
```
Transitions (checked inside `append_record` / dedicated typed moves):
- Idle + user/monitor record           → AwaitingAssistant
- AwaitingAssistant + assistant(no tools) → Idle (emit TurnEnd)
- AwaitingAssistant + assistant(tool_use batch B) → AwaitingToolResults{pending=ids(B)}
- AwaitingToolResults + tool_result batch R → require ids(R) == pending exactly
  (the non-CFG bijection: no missing/extra/dup), then → AwaitingAssistant.
  (Current code builds R atomically, so a one-shot set-equality check matches it;
  a per-id `add_tool_result` with a final empty-check is the stricter, more
  granular alternative if results ever stream in.)
Illegal moves (must be unrepresentable / hard error): appending an assistant
record or draining the queue while `pending` non-empty; a tool_result whose id
is not in `pending`.

## REFINED DESIGN (most elegant) — state as a VIEW of history, not a stored field
`ConvState` is a PURE FUNCTION of the history tail (total mapping):
- empty OR last = assistant WITHOUT tool_use      -> Idle
- last = any role:user record                     -> AwaitingAssistant
- last = assistant WITH tool_use blocks            -> AwaitingToolResults{ pending = those ids }
So DERIVE it (`fn conv_state(&[Message]) -> ConvState`); do NOT store a field.
Benefits: no desync; resume falls out free (`conv_state(loaded_history)`);
dangling-repair becomes COMPELLED by the guard (user_input illegal from
AwaitingToolResults) rather than a proactive Step 1.

Transition function δ (the WHOLE invariant in one place):
```
δ(Idle,                user_input)        -> AwaitingAssistant
δ(AwaitingAssistant,   user_input)        -> AwaitingAssistant   // Seam-B / batch merge
δ(AwaitingToolResults, user_input)        -> ERROR               // no injection mid-pair
δ(AwaitingAssistant,   assistant_plain)   -> Idle                // TurnEnd
δ(AwaitingAssistant,   assistant_tooluse) -> AwaitingToolResults{ids}
δ(AwaitingToolResults, tool_results{R})   -> require R == pending EXACTLY -> AwaitingAssistant
(else)                                    -> ERROR
```
The move is itself derivable from (role, blocks): role==User & all blocks
ToolResult -> tool_results{ids}; role==User else -> user_input; role==Assistant
& has ToolUse -> assistant_tooluse{ids}; role==Assistant else -> assistant_plain.
So the chokepoint signature is just `append_record(role, blocks) -> Result<ContextHash>`.

## CONFIRMED: the live append-triple (identical across inject.rs)
Every inject.rs helper ends with the SAME triple (only role + the pre-emitted
event differ):
```
let hash = self.context_store.append(Role::User, blocks.clone()).await?;
self.history.push(Message { role: Role::User, content: blocks });
self.context_hashes.push(hash);
```
Verified sites (inject.rs): inject_monitor_delivery (~84-94), inject_harness_recovery
(~124-133), inject_monitor_stopped (~169-178), inject_user_message (~201-210),
inject_dangling_tool_results (~265-272, role:User batch), inject_tool_results_batch
(~289-296). run_loop.rs:877 = assistant variant (Role::Assistant), returns
assistant_hash used for lr.context_hash + ToolCall events -> so append_record
MUST RETURN the hash.
STEP 1 = extract `async fn append_record(&mut self, role, blocks) -> Result<ContextHash>`
doing exactly the triple; replace all the above call sites with it. Pure
refactor, no behaviour change. Each helper still emits its OmegaEvent FIRST
(A1 invariant) and only the triple moves into append_record.
lifecycle.rs:399/413 = `seed_*` (compaction seed): synthetic User preamble +
Assistant ack, each does the SAME triple (append User blocks, then Assistant
blocks). resume.rs:433 = replay of a persisted assistant record (append
Assistant blocks, no history.push/context_hashes here actually — it pushes hash
via assistant_hash path; note resume builds blocks from slots). These are
SPECIAL: they write a fixed valid User->Assistant pair (lifecycle) or replay a
trusted record (resume). DECISION: they MAY route through `append_record` too
(the moves are legal: Idle->user_input->AwaitingAssistant->assistant_plain->Idle
for the seed pair), which is even cleaner — but verify the seed pair's role
sequence is legal under δ before forcing it through. Safest for STEP 1: route
ONLY the inject.rs helpers + run_loop.rs:877 through append_record (pure
refactor); leave lifecycle/resume as-is for now and revisit when the guard lands.

## STATUS / RESUME-HERE (as of commit after 92f85fb)
- Spike COMPLETE and committed (92f85fb). This file is the durable record.
- Refined design agreed in discussion: state-as-view-of-history + single δ
  inside a guarded `append_record(role, blocks) -> Result<ContextHash>`.
- User endorsed runtime-checked field/view (compile-time impossible for the id
  bijection — needs dependent types) and asked for the most elegant overall
  solution; the view-of-history design above is that.
- NEXT ACTION = implement STEP 1: extract `append_record` chokepoint, funnel
  the inject.rs helpers + run_loop.rs:877 through it. Pure refactor, gate green,
  commit. THEN STEP 2: add `conv_state` + δ + bijection check + pure-fn tests +
  `cargo mutants -p omega-agent --cap-lints=true --file <changed>` + Justfile recipe.
- Test approach (AGENTS.md): agent-level via Agent::send_message + MockProvider
  for legal sequences; pure-fn unit tests for `conv_state` + δ legality are a
  justified carve-out (agent-level setup for every illegal transition is
  disproportionate — add a comment saying so).

## Open questions for implementation
- Resume path (resume.rs:433) and lifecycle.rs rebuild history from disk — the
  typestate must be RECONSTRUCTED from the loaded tail (last record kind), not
  assumed Idle. Dangling tool_use on load → start in AwaitingToolResults.
- Where does `ConvState` live so both `run` and `drive_turn` see it through
  `&mut self` without fighting the `stream!` borrow? (likely a plain field.)
- Should the empty-response continuation count as staying in AwaitingAssistant?
  (Yes — no record persisted, so no transition.)
