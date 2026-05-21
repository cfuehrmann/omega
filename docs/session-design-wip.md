# Session Concept — Work-In-Progress Design Note

**Status:** Interim. Captured mid-discussion as a paranoia-save before
finalising. A clean, final design note will replace this one.

## Why we're rethinking "session"

Today, "session" in Omega is implicitly tied to process startup. We have
soft-resume (which creates a *new* session seeded from a summary) and
fresh-empty-session, but no strict resume. The notion is operational
rather than principled, and it doesn't have a clear story for:

- Subagents
- LLM context compaction (server-side or client-side)
- Headless benchmark runs (Harbor) that span what would otherwise be
  multiple "sessions"
- Forensics / post-mortem of past runs

The goal of this discussion was to find a **single, simple, universal**
definition of "session" that holds up across all of these.

## Where we landed (most recent agreement)

> **A session is the persisted state of one Omega instance. It is resumable
> at "awaiting user" boundaries, against the same Omega version that
> created it. Subagents are child sessions; everything else is a derived
> view.**

Key consequences:

- **Process incarnation is irrelevant.** A session survives app restarts;
  it ends when the agent's logical life ends, not when its process does.
- **The persisted event stream is canonical.** State at any moment is
  `fold(events)`. The LLM context, the UI feed, and any context-hash tree
  are all *derived projections* of the session, not the session itself.
- **Subagent = child Omega instance = child session.** Structural, not
  stipulated. Child sessions live in a subfolder of the parent's session
  folder; the parent references the child by session ID in its events.
- **Compaction (server-side or agent-side) is a within-session event.**
  Neither kind ends a session. Both are recorded as ordinary events; the
  derived LLM-context projection honours them.
- **Atomic unit is a turn**, from one user message to the next "awaiting
  user" state. Turns either complete or are discarded on resume. Nothing
  intra-turn (streaming buffers, retries, in-flight tool calls, partial
  responses) needs to round-trip across a restart — it's ephemeral and
  matters only for forensics.
- **Cross-Omega-version replay is explicitly not supported.** Sessions
  record the Omega version at creation; resume requires exact match and
  fails loud on mismatch. Migration would dominate development cost at
  Omega's current pace.
- **No mid-flight resumption.** If Omega dies mid-turn, on resume the
  partial turn is abandoned; state is restored to "just after the last
  user message."

### Testable criterion

The definition is operationalised as a property:

> Stop Omega. Start a fresh process from the persisted events. The
> resulting state (modulo declared-ephemeral pieces) must match what the
> state would have been right after the last user message of the dead
> process.

This is a gate-able test, not a philosophical claim.

## Approaches considered and dismissed

Each was held seriously for at least one round of discussion. Recording
*why* they were rejected so we don't relitigate.

### A. Session = lifetime of the running Omega process

**Status quo.** Dismissed because process death is not a meaningful
event from the user's or the agent's perspective. It conflates
"incarnation" with "identity."

### B. Session = lifetime of the LLM-visible context

The LLM's effective context is ground truth; a session is a maximal run
of calls in which each call's prompt is a strict append-extension of the
previous one *as the model actually saw it*.

Attractive because it names something the substrate would recognise, and
exposes provider-side compaction honestly. Dismissed because:

- Session boundaries leave our control (provider decides when to
  compact).
- Observability is partial and provider-specific.
- Sessions become retrospective (you can only know a boundary happened
  after the call returns).
- It captures only LLM-related events; non-LLM events (connection
  failures, model changes, local tool calls) have no home.
- One persisted log can map to N "real" sessions depending on runtime
  conditions, so session count isn't even well-defined.

### C. Session = lifetime of the persisted append-only log

Almost right, but anchored on the storage artifact rather than on
*why* the storage artifact defines identity. Subsumed by the
process-state framing (D), which makes the same prediction in most
cases for an articulable reason and adds a testable criterion.

### D. Session = git-style tree of content-addressed contexts

Tempting because Omega already hashes contexts. Maps cleanly: commit =
context, branch = conversation tip, checkout = strict resume, squash =
compaction. Subagents become side branches. Crucially the resulting
model is a *tree*, not a DAG (no true LLM-context merges exist), so
it's strictly simpler than git.

Dismissed as the *primary* framing because it mistakes a useful
substructure for the identity concept. The tree of contexts is a real
and useful internal index that can sit *inside* a session, but it
doesn't carry the non-LLM events the user cares about (failures, model
changes, tool calls) and doesn't give a recovery criterion for the
agent process. Forking (the operation the git model made attractive)
remains available as a deliberate later feature: "create a new session
seeded with the first N events of session X." Deferred without
commitment cost.

### E. Session = persisted state of one Omega instance — **adopted**

See "Where we landed" above.

## Subagent design choices that keep forking deferrable

We will build subagents next; forking is deferred. To avoid painting
into a corner:

1. Session creation takes a **seed**: a list of events plus metadata.
   For subagents the seed happens to be
   `[system_prompt_event, initial_user_message_event]`; a future fork
   would pass a longer seed. The API doesn't bake in the short case.
2. Session metadata records `parent_session_id` and an `origin` enum
   (initially `Root`, `SubagentOf`; later extendable to `ForkOf` with an
   index).
3. On-disk layout: child sessions in a subfolder of the parent. Forks,
   if added, would live at top level — the parent relationship lives in
   metadata, not in filesystem layout.
4. Events that reference a child session carry the child's session ID;
   the same event shape would work for fork references.
5. Session IDs are flat and opaque. Relationships live in metadata, never
   encoded into IDs.

## Long autonomous runs — known limitation accepted

In autonomous modes (Harbor benchmarks, future autonomous agents) there
may be long gaps between user messages. A crash three hours in loses
three hours of resumable progress (forensic events remain on disk).

This is accepted for now. If it becomes a primary mode later, the
extension is **tool-completion checkpoints** — resume at the last
completed tool call within a turn. This is a strict extension of the
current rule, not a redesign, so deferring it costs nothing.

## Work implied by the adopted framing

1. **State audit, scoped to "what must survive an awaiting-user
   boundary."** Enumerate long-lived state and classify each piece as:
   event-sourced / ephemeral-and-OK-to-lose / gap-to-close.
2. **Round-trip test on the gate.** Stop, restart from events, diff
   non-ephemeral state.
3. **Session version field**, recorded at creation, checked exactly at
   resume. Add now, before sessions-without-the-field accumulate.
4. **Subagent implementation** following the five design choices above.

Deferred without commitment: forking, snapshots, tool-completion
checkpoints, multi-version migration.

## Tomorrow

Finalise the discussion and replace this note with a clean design note.
Open questions to revisit:

- Exact shape of the "ephemeral pieces" allowlist used by the round-trip
  test.
- Whether the session version field is a git SHA, a build hash, or a
  manually bumped schema version. (Leaning: exact build identifier; no
  semver implied.)
- Resume UX for in-flight calls at the moment of crash: silent abandon
  vs. "we were waiting for a reply, retry?" prompt.
- Confirmation that `LlmResponseEnded` (or equivalent) already carries
  enough to reconstruct the parent-context relationships we'd want for
  the internal context-tree projection.
