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

## Phase 0 — Audits and no-brainer prep (no SessionRef dependency)

Safe to run in any order. Output is mostly written; small code changes
where noted. Goal: have all the information we need before designing
`SessionRef`, and clear the trivial code work.

- **0.1 State audit.** Enumerate Omega's long-lived in-process state.
  Classify each as: event-sourced / ephemeral-and-OK-to-lose /
  gap-to-close. Output: `docs/session-state-audit.md`.
- **0.2 "Session" usage scan.** Every place in the codebase that uses
  "session" and what it means there. Feed into a "current
  vocabulary" section of the audit. Same doc.
- **0.3 Parent-context-hash check.** Does `LlmResponseEnded` (or
  equivalent) already carry the predecessor context hash? Code
  investigation; result in audit.
- **0.4 Defensive-serde scan.** List existing `#[serde(default)]` and
  `#[serde(rename)]` attributes on event types. Classify each as
  deliberate (honours a real contract) or defensive (suppresses a
  mismatch). Result in audit. No removals yet — that's a follow-up.
- **0.5 Session version field.** Record the build's git SHA in session
  metadata at session creation. Pure additive; no resume-time check.
- **0.6 UUID v7 dependency.** Enable the `v7` feature on the `uuid`
  crate (or add the crate). Trivial; needed by everything in Phase 1.

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

- **2.1 Pin down "awaiting user" boundary in code.** Identify the
  precise event(s) and runtime point that marks it.
- **2.2 Resume entry point.** Given a session folder, fold events into
  a fresh Omega instance state. No mid-flight recovery: discard any
  events after the last "awaiting user" mark on the way in (or refuse
  to load if the trailing events suggest a non-clean shutdown — pick
  in Phase 2 design).
- **2.3 Round-trip gate test.** Two-process test: run an Omega up to
  some point, kill it, restart from the events, assert state
  equivalence on non-ephemeral pieces. Add to the gate.
- **2.4 Close gaps from 0.1.** Anything in the gap-to-close column gets
  event-sourced.

## Phase 3 — Subagents (the actual reason for all this)

- **3.1 Spawn API.** Takes a seed (`Vec<OmegaEvent>` + session
  metadata including `Origin`). The fact that subagent seeds are short
  is a property of the caller, not the API.
- **3.2 On-disk layout.** Child session folder under
  `<parent>/subagents/<child_id>/`.
- **3.3 Resolution.** Resolve a `SessionRef` to a session by ID,
  scanning known roots. Future-proof for a registry.
- **3.4 `SubagentSpawned` event** in parent stream.
- **3.5 `SubagentReturned` event** in parent stream, with the child's
  summary and a `SessionRef` to the child's terminal event.
- **3.6 UI: render subagent invocation** in the parent's feed —
  expandable block; clicking navigates into the child session.

## Phase 4 — Deferred (may never happen)

- Forks (independent agents seeded with a prefix of another session).
- Snapshots (folded-state checkpoints to skip early replay).
- Tool-completion checkpoints (within-turn resume granularity).
- Multi-version migration.

The Phase 1 design choices keep all of these reachable without
structural change.

---

## Open questions to resolve before each phase

- **Before Phase 1:** exact serde shape for `SessionRef` (struct vs.
  string-encoded `<session_id>:<event_id>`?). Probably struct, but
  worth a moment.
- **Before Phase 2:** "ephemeral pieces" allowlist for the round-trip
  test. Feeds in from 0.1.
- **Before Phase 2:** crash-recovery UX policy — silent abandon vs.
  "we were waiting for a reply; retry?" prompt on resume.
- **Before Phase 3:** registry vs. scan for `SessionRef` resolution.
  Scan is fine for now if performance allows.
