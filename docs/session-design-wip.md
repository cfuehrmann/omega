# Session Concept — Design & Plan

**Status:** Working document. Captures the adopted framing, the
approaches we rejected, and the phased implementation plan. Replaces
the earlier paranoia-save WIP note. Will be promoted to a clean final
design note once the plan is well underway.

## Adopted framing

> **A session is the persisted state of one Omega instance. It is
> resumable at "awaiting user" boundaries, against the same Omega
> version that created it. Subagents are child sessions; everything else
> is a derived view.**

### Consequences

- **Process incarnation is irrelevant.** A session survives app
  restarts; it ends when the agent's logical life ends, not when its
  process does.
- **The persisted event stream is canonical.** State at any moment is
  `fold(events)`. The LLM context, the UI feed, and any context-hash
  tree are *derived projections*, not the session itself.
- **Subagent = child Omega instance = child session.** Structural, not
  stipulated. Child sessions live in a subfolder of the parent's
  session folder; the parent references the child by ID.
- **Compaction (server-side or agent-side) is a within-session event.**
  Neither kind ends a session. The derived LLM-context projection
  honours them.
- **Atomic unit is a turn**, from one user message to the next
  "awaiting user" state. Turns either complete or are discarded on
  resume. Intra-turn state (streaming buffers, retries, in-flight tool
  calls, partial responses) is ephemeral and matters only for
  forensics.
- **No cross-Omega-version replay.** Sessions record the build SHA at
  creation. The version field is not gated at resume; the Rust type
  system + fold invariants reject incompatible events structurally.
  The field is recorded for forensics and as a future hook.
- **No mid-flight resumption.** Crash mid-turn → partial turn is
  abandoned on resume; state restored to "just after last user
  message."

### Testable criterion

> Stop Omega. Start a fresh process from the persisted events. The
> resulting state (modulo declared-ephemeral pieces) must match what
> the state would have been right after the last user message of the
> dead process.

A gate-able test, not a philosophical claim.

## Rejected approaches (one line each)

- **Session = process lifetime** — conflates incarnation with identity.
- **Session = LLM-visible context** — boundaries leave our control;
  retrospective; ignores non-LLM events; provider-dependent.
- **Session = persisted-log lifetime** — right answer for wrong reason;
  subsumed by the process-state framing with a testable criterion.
- **Session = git-style tree of contexts** — useful internal index, not
  identity; doesn't carry non-LLM events. Forking becomes a deliberate
  later feature, not a primitive.

(Full dismissal rationales recorded in git history at `9d311ee`.)

## Subagent-vs-fork design choices (forking deferred indefinitely)

1. Session creation takes a **seed** (events + metadata). Subagent
   seeds happen to be short (`[system_prompt, initial_user_message]`);
   the API doesn't bake in the short case.
2. Session metadata records `parent_session_id` and `origin` enum
   (`Root`, `SubagentOf{...}`; extendable to `ForkOf{...}`).
3. On-disk layout: subagent sessions in subfolders of the parent;
   forks would be top-level. Relationship lives in metadata, not in
   filesystem layout.
4. Pointing events carry typed relationship metadata (the `origin`
   variant), not raw IDs interpreted contextually.
5. Session IDs are flat and opaque (UUID v7). Never encode parent
   relationships into IDs.

## Session and event references

- Both sessions and events have **globally unique UUID v7 IDs**. Not
  content-derived, not position-derived.
- `SessionRef { session_id, event_id: Option<EventId> }`. `None` means
  "the session as a whole"; `Some` means a specific moment.
- References are by ID. Folder layout is a *hint* for resolution.
  Moving the whole `.omega/sessions` tree in unison doesn't break
  references; moving folders relative to each other may.
- Every event that references another session or event must, from day
  one: render visibly in the feed; show the literal IDs; have a "more"
  button revealing full JSON; ideally be clickable to navigate.
  Transparency floor, not polish.

### Concrete event shapes (post-SessionRef)

- `SubagentSpawned { child: SessionRef { session_id, event_id: None } }`
  in the parent.
- Child's session metadata: `origin: SubagentOf { parent: SessionRef {
  session_id, event_id: Some(spawn_event_in_parent) } }`.
- `SubagentReturned { child: SessionRef { session_id, event_id:
  Some(terminal_in_child) }, summary: ... }` in the parent.

## Long autonomous runs — retracted as a concern

Every realistic failure during a long run is either:

- Infrastructure flake → Omega backs off, doesn't crash.
- Broken tool / timeout → surfaces to the agent loop, doesn't crash.
- Omega bug (OOM, panic) → benchmark *should* fail; Omega gets fixed.
- Host-level event → rare; restart acceptable.

Mid-turn resume would mask exactly the cases we want to see fail
loudly. Tool-completion checkpoints are not deferred features; their
absence is correct.

---

# Implementation plan

## Phase 0 — Audits and no-brainer prep ✅ DONE

Full findings in [`docs/session-state-audit.md`](./session-state-audit.md).
Headlines:

- **0.1 / 0.7 State audit (incl. Agent internals)** — every cross-turn
  field in `Agent` is already event-sourced or safely ephemeral; the
  scaffolding for reconstruction (`extract_last_model_and_effort`,
  `seed_history`, `ContextStore::read_all`) exists. **One gap surfaced:**
  server-side compaction silently clears in-memory history with no event
  recording it (F11). Listed as Phase 2.0 below.
- **0.2 "Session" usage scan** — eight distinct meanings catalogued.
- **0.3 Parent-context-hash** — already in `LlmResponseEnded.context_hash`;
  full chain reconstructible from `LlmCallEvent.context_hashes`.
- **0.4 Defensive serde** — catalogued; kept for now, flagged for a
  deliberate later sweep.
- **0.5 Session version field** — already done as
  `SessionStartedEvent.omega_commit`.
- **0.6 UUID v7 dependency** — only Phase 0 item still outstanding;
  small follow-up to fold into the start of Phase 1.
- **F13:** today's `SessionStartedEvent.session_id` equals the folder
  name; Phase 1 replaces this with a UUID.
- **F14:** "awaiting user" = `TurnEnd` or `TurnInterrupted`. Already
  unambiguous in the event stream.
- **F15:** `system_blocks` may differ on resume if `AGENTS.md` changes
  between runs; accepted per the no-cross-version-replay stance.

## Phase 1 — SessionRef & friends (design-first; careful)

These types have outsized blast radius — every cross-session event will
use them forever. Design carefully before any code lands.

- **1.1 Define `EventId` and `SessionId` newtypes** (UUID v7 wrappers).
  Serde shape, Display, parse, equality. Contract-authority territory.
- **1.2 Define `SessionRef { session_id, event_id: Option<EventId> }`.**
  Serde shape, Display, parse.
- **1.3 Define `Origin` enum** (`Root`, `SubagentOf { parent:
  SessionRef }`). Future-extend with `ForkOf` planned but not added.
- **1.4 Wire `EventId` into every existing event** at write time. Read
  path: tolerate absent IDs in old logs only if no cross-version replay
  has been promised — which we haven't. Default: every event in a
  current-version session has an ID.
- **1.5 UI surfacing scaffold.** Render `SessionRef`-bearing fields in
  the feed with: literal IDs, "more" button for full JSON, link to
  referenced item (placeholder where target doesn't yet exist).

## Phase 2 — Strict resume + gate test (closes the framing's promise)

- **2.0 Close F11 compaction gap.** Add an event (e.g.
  `ContextCompacted`) — or annotate `LlmResponseEnded` — that records
  when a server-side `compact_20260112` edit fired and the
  pre-compaction `context.jsonl` records are now stale. Without this,
  the resume path would naïvely replay the full pre-compaction context.
  **Prerequisite for 2.2.**
- **2.1 Pin down "awaiting user" boundary in code.** Per F14 this is
  `TurnEnd` or `TurnInterrupted`. Confirm and document the exact
  read-side check.
- **2.2 Resume entry point.** Given a session folder, fold events into
  a fresh Omega instance state. Discard events after the last
  "awaiting user" mark; honour compaction events from 2.0 by starting
  history fresh at that point. Must handle the
  `TurnInterrupted{Aborted}` case (dangling `ToolUse` blocks — existing
  `send_message` Step 1 repair logic must fire or history must be
  trimmed; see F14 note).
- **2.3 Round-trip gate test.** Two-process test: run an Omega up to
  some point, kill it, restart from the events, assert state
  equivalence on non-ephemeral pieces. Add to the gate.
- **2.4 Close any remaining gaps from the audit.** F11 is the known
  one; flag and close anything else surfaced by writing 2.3.

## Phase 3 — Subagents (the actual reason for all this)

### 3.0 Subagent protocol — design discussion (do this first)

The protocol questions below must be settled before code. Most have a
clear default; flagged here so we make the decisions deliberately rather
than by accident of implementation order.

- **How are subagents *called*?** Tool-style invocation from the parent's
  LLM (i.e. the parent emits a `tool_use` for a `spawn_subagent` tool),
  or a dedicated event/control-plane mechanism? Tool-style is the
  obvious default — it reuses existing machinery and matches how every
  other agent we've inspected does it.
- **How do they *return*?** A summary string back to the parent as a
  tool result, plus a `SessionRef` to the child's terminal event for
  navigation. The summary is what re-enters the parent's LLM context;
  the SessionRef is for observability.
- **Interaction model — continuous vs call-return.** Reference points
  from prior survey: opencode = continuous (parent can interact with a
  live subagent); forgecode and pi = call-return (parent waits, child
  runs to completion); Claude Code = not yet explored. **Default: start
  with call-return.** It's strictly simpler, matches the tool-call
  return shape, and doesn't preclude adding continuous later as a
  separate spawn variant.
- **What triggers a handoff?** Manual to start — the parent's LLM
  decides via the spawn tool. Automatic handoff (e.g. context-budget
  triggered, or a planner deciding to delegate a subtask) is
  follow-up work; design must not preclude it.
- **Observability pointer.** Every subagent spawn surfaces a clickable
  `SessionRef` to the child session at spawn time (not only at return).
  The user can open the child's feed while it's still running.
- **How does the *session model* need to change?** Today's
  `AppState.active_session: Arc<Mutex<Option<ActiveSession>>>` hosts at
  most one live agent (F8). Subagents force a decision:
  - **(a) In-process, multi-session server.** Replace `Option<ActiveSession>`
    with `HashMap<SessionId, ActiveSession>`. One process, many live
    agents, one is "focused" for UI input. Simplest operationally.
  - **(b) Separate Omega process per subagent.** Matches the "subagent =
    child Omega instance" framing most literally; gives crash isolation;
    requires IPC for events to bubble to the parent's UI.
  - **Default: (a)**, with the `SessionRef`-based design ensuring (b) is
    not foreclosed if we want crash isolation later.
- **UI: session modal / picker.** Today there's a single-session UI;
  with subagents the user needs to navigate between sessions. The
  modal/picker needs (i) a tree view of related sessions (parent →
  subagents → grandsubagents), (ii) an indicator of which is currently
  focused, (iii) per-session live status (running / awaiting user /
  finished).

### 3.1+ Implementation steps (after 3.0 is settled)

- **3.1 Spawn API.** Takes a seed (`Vec<OmegaEvent>` + session
  metadata including `Origin`). The fact that subagent seeds are short
  is a property of the caller, not the API.
- **3.2 On-disk layout.** Child session folder under
  `<parent>/subagents/<child_id>/`.
- **3.3 Resolution.** Resolve a `SessionRef` to a session by ID,
  scanning known roots. Future-proof for a registry.
- **3.4 `spawn_subagent` tool** that emits a `SubagentSpawned` event in
  the parent stream when invoked.
- **3.5 `SubagentReturned` event** in parent stream, with the child's
  summary and a `SessionRef` to the child's terminal event. Returned
  to the LLM as the tool result.
- **3.6 UI: render subagent invocation** in the parent's feed —
  expandable block with `SessionRef` link; clicking navigates into the
  child session. Modal/picker shows the tree of related sessions.

## Phase 4 — Deferred (may never happen)

- Forks (independent agents seeded with a prefix of another session).
- Snapshots (folded-state checkpoints to skip early replay).
- Tool-completion checkpoints (within-turn resume granularity).
- Multi-version migration.

The Phase 1 design choices keep all of these reachable without
structural change.

---

## Open questions to resolve before each phase

### Before Phase 1 — SessionRef design (next concrete decision point)

These decisions are persisted in events forever once made. They deserve
explicit discussion before any code lands. **Slated for a dedicated
fresh sub-session**; deliverable is a single self-contained HTML file
at `docs/sessionref-design-proposal.html` (experiment in richer doc
affordances — callouts, sticky TOC, syntax-highlighted code, collapsible
rationale blocks). Main session reviews, accepts/refines, *then*
implementation lands as a separate step.

1. **`SessionId` and `EventId` as newtypes, not aliases.** Confirmation
   only; the blast-radius argument is decisive. `pub struct SessionId(Uuid)`
   with `#[serde(transparent)]` is the likely shape.
2. **One `SessionRef` type with `event_id: Option<EventId>`, vs two
   types (`SessionRef` + `EventRef`).** Single type is simpler; two
   types are more explicit at call sites. Decide and justify.
3. **Serde shape:** struct `{"sessionId": "...", "eventId": "..."}` vs
   string-encoded `"<sid>:<eid>"` / `"<sid>"`. Tradeoff: struct is more
   inspectable in `events.jsonl`; string is more compact and embeddable.
   Lean struct.
4. **`Display` / `FromStr` format** — what does
   `SessionRef::to_string()` produce? Used in logs, error messages, UI
   chips. Must round-trip with `FromStr`.
5. **`Hash` / `Eq` / `Ord` derives** — yes (Hash + Eq needed for the
   in-process `HashMap<SessionId, ActiveSession>` in Phase 3.0). Ord:
   only if we have a use; UUID v7 ordering is time-based which may be
   misleading. Probably leave Ord off.
6. **Crate home** — `omega-types`, alongside `OmegaEvent`. Confirm.
7. **`EventId` placement on events** — on the outer envelope (a common
   header) or on each variant struct? The audit's F3 leans envelope.
   Decide and document.
8. **Bare `SessionId` vs `SessionRef`** — when does a call site take
   the bare ID vs the full ref? E.g. folder names take bare `SessionId`;
   event payloads referencing another session take `SessionRef`.
   Establish the rule.
9. **`Origin` enum exact shape** — `Root` and `SubagentOf { parent:
   SessionRef }`. Confirm that the parent ref carries the *spawn event*
   in the parent (`event_id: Some(_)`), not just the parent session
   (`event_id: None`). The transparency requirement argues for `Some`.

### Before Phase 2

- "Ephemeral pieces" allowlist for the round-trip test. Per the audit,
  most of `ActiveSession` is ephemeral-OK; the explicit list lives in
  the test.
- Crash-recovery UX policy — silent abandon vs. "we were waiting for a
  reply; retry?" prompt on resume.

### Before Phase 3

- Registry vs. scan for `SessionRef` resolution. Scan is fine for now
  if performance allows.
- In-process multi-session server vs. one Omega process per subagent
  (covered in Phase 3.0; default in-process).
