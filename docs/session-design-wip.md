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

## Refinements (second round)

### Long autonomous runs — retracted as a concern

The earlier worry that long Harbor-style runs could lose hours of work
on a crash doesn't hold up. Every realistic failure during a long run is
either:

- An infrastructure flake (network, rate limits) — Omega should back off,
  not crash.
- A broken tool or tool timeout — Omega should surface to the agent loop,
  not crash.
- An Omega bug (OOM, panic) — the benchmark *should* fail and Omega
  should be fixed.
- A host-level event (VM eviction, power loss) — rare, restart acceptable.

Mid-turn resume would mask exactly the cases we want to see fail loudly.
Tool-completion checkpoints are therefore not a deferred feature; their
absence is correct. The "awaiting user" boundary stays the sole resume
point.

Separately: "Omega is making no forward progress" (retry loops, wedged
tools) is a different failure mode — the process is alive, so resume
isn't the answer. Observability / watchdog territory, not session design.

### Version field — hygiene, not gate

Rust's strong type system plus the fold's invariants already give us
two lines of natural defense against loading incompatible events:

1. **Deserialization** rejects malformed events.
2. **Folding** rejects events that produce an invalid state.

Either wall failing is equivalent to "these events aren't valid for this
Omega" — the property we want, achieved structurally rather than by
contract. The session version field is therefore:

- **Recorded** at session creation (for forensics and future use).
- **Not checked** at resume.
- **Format:** exact build identifier (git SHA), not semver — no contract
  implied.

The one habit that protects this defense: **don't add `#[serde(rename)]`
or `#[serde(default)]` attributes defensively** to suppress mismatch
errors. Use them only to honor a real, deliberate compatibility
contract. Defensive serde attributes silently mask exactly the bugs we
most want to see. (Captured in `AGENTS.md` under Contract Authority.)

### Session references and event IDs

**Sessions and events both get globally unique IDs (UUID v7).** Not
content-derived; identity is independent of content. v7's embedded
timestamp gives natural sort order as a bonus, without making the ID
load-bearing for ordering.

**A session reference is a pair:**

```
SessionRef {
    session_id: SessionId,
    event_id:   Option<EventId>,  // None = the session as a whole
}
```

Why globally unique IDs and not line positions / sequence numbers:

- Robust to any future log rewrite, export, snapshot, or replay.
- Decouples identity from physical layout (same principle as not using
  folder paths as references).
- Uniform shape with `session_id`.
- Trivial cost (16 bytes).

A per-session sequence number can still exist alongside, for ordering
and display — but it's separate from identity.

### Folder layout vs. references

- Subagent sessions live in subfolders of the parent session folder.
- Forks (if ever built) live as new top-level folders.
- Folder layout reflects **ownership** (subagent owned by parent; fork
  is independent).
- References are always by ID, never by path.
- Resolution scans known roots (or a future registry).
- Moving the whole `.omega/sessions` tree in unison: doesn't break
  references (one config value updates).
- Moving folders *relative to each other*: may break resolution. That's
  an explicit reorganization, not an incidental one. Acceptable.

### Surfacing session/event references in the UI — from day one

Not a polish item; a transparency floor. Every event that references
another session or event must:

- Render visibly in the feed (not buried in a tooltip).
- Show the referenced `(session_id, event_id)` literally.
- Have a "more" button revealing the full event JSON.
- Ideally: be clickable to navigate to the referenced session/event.

Reviewing what the agent did across session boundaries (especially
subagent boundaries) is impossible without this.

### Concrete event shapes for subagents

- **Parent's spawn event:**
  `SubagentSpawned { child: SessionRef { session_id, event_id: None } }`
- **Child's session metadata** (not an event in the child stream):
  `origin: SubagentOf { parent: SessionRef { session_id, event_id: Some(spawn_event_in_parent) } }`
- **Parent's return event:**
  `SubagentReturned { child: SessionRef { session_id, event_id: Some(terminal_in_child) }, summary: ... }`

The relationship semantics live in the typed event/metadata shape
(`origin` enum variant), not in IDs and not in folder paths. A future
`ForkOf` variant of `origin` reuses the same `SessionRef` machinery
with no other structural changes.

## Tomorrow

Finalise the discussion and replace this note with a clean design note.
Open questions to revisit:

- Exact shape of the "ephemeral pieces" allowlist used by the round-trip
  test.
- Resume UX for in-flight calls at the moment of crash: silent abandon
  vs. "we were waiting for a reply, retry?" prompt.
- Confirmation that `LlmResponseEnded` (or equivalent) already carries
  enough to reconstruct the parent-context relationships we'd want for
  the internal context-tree projection.
- Whether per-session sequence numbers are needed alongside event IDs,
  or whether v7 timestamps suffice for all ordering needs.
- The `SessionRef` and `origin` types should be designed carefully
  before any subagent code lands — they have outsized blast radius.
