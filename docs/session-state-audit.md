# Session State Audit

**Status:** First pass. Phase 0 work toward the session-design plan.
Captures what Omega calls a "session" today, what state lives where, and
what classifies as event-sourced / ephemeral / gap-to-close under the
adopted framing.

## Current vocabulary — where "session" appears

The word "session" today refers to several different things; one job of
the redesign is to untangle these.

| Layer | What "session" means | Notes |
|---|---|---|
| Disk | A folder at `.omega/sessions/<timestamp>-<hex>/` | Contains `context.jsonl`, `events.jsonl`, `session.jsonc` |
| Disk | The `SessionMetadata` struct (`name`, `resumed_from`) | Stored in `session.jsonc` |
| Server | `ActiveSession` struct (one per process, via `Arc<Mutex<Option<ActiveSession>>>` in `AppState`) | Holds in-memory agent + ws + turn state |
| Server | `SessionInfoCache` | Derived projection for WS broadcasts |
| WS protocol | `WsMessage::SessionInfo` | Announces session identity to the client |
| HTTP API | `POST /api/sessions`, `GET /api/sessions` | Create / list session folders |
| Events | `SessionStartedEvent.session_id` | String identifier on every session-started event |
| Events | `ResumingSessionEvent`, `SessionResumedEvent` | Soft-resume lifecycle |
| Metadata | `resumed_from` field (alias `continuationOf`) | Cross-session reference by folder name |

**Observation:** today "session" is implicitly tied to (a) a folder on
disk and (b) a single live `ActiveSession` slot in the server. The
proposed redesign keeps (a) and replaces (b) — multiple sessions can be
relevant (parent + subagents) but only one is "active for input."

## Current session folder layout

```
.omega/sessions/<timestamp>-<hex>/
├── context.jsonl     # content-addressed LLM context records
├── events.jsonl      # OmegaEvent log (canonical)
└── session.jsonc     # SessionMetadata (name, resumed_from)
```

Folder name format: `YYYY-MM-DDTHH-MM-SS-mmm-<hex8>`. Regex tolerates
two legacy formats (without ms / without suffix).

## OmegaEvent variants (current)

Grouped by role.

**Session lifecycle**
- `SessionStarted` — first event in every session. Carries `time`,
  `session_id`, `path`, `model`, `effort`, `system_prompt`,
  `omega_commit`, `agent_time_zone`.
- `ServerStarted`, `ServerStopped` — *process* lifecycle (not session).
  Logged into the active session's events.jsonl.

**User / agent interaction**
- `UserMessage`, `LlmCall`, `ToolCall`, `ToolResult`, `TurnEnd`
- `TextBlock`, `ThinkingBlock`, `ToolUseBlock` (assistant content blocks)
- `LlmResponseStarted`, `LlmResponseEnded`, `LlmResponseDiscarded`

**Failures & retries**
- `LlmError`, `AgentError`, `TurnInterrupted`, `LlmRetry`,
  `TransportError`

**Configuration changes**
- `ModelChanged`, `EffortChanged`

**Resume lifecycle (current soft-resume)**
- `ResumingSession` — has `resumed_from` (folder name), `name`, `basis`
- `SessionResumed` — has `resumed_from` (folder name), `summary`

**Pause / continue**
- `PauseRequested`, `TurnPaused`, `TurnContinued`

## Findings against the design

### F1 — Phase 0.5 (session version field) is essentially DONE

`SessionStartedEvent.omega_commit` already records the build's git SHA
on session creation. We don't need to add it.

**Caveat:** the field has `#[serde(default = "default_omega_commit")]`
which fills in `"unknown"` for old sessions. That's a defensive
attribute (per the new policy). For sessions created *before* this field
landed it's load-bearing; for new sessions it's redundant. Decision:
keep the default for now (matches "no resume-time check" stance — the
field is forensic, not gating).

### F2 — `SessionStartedEvent.session_id` is a string, not yet a UUID

Current ID is the folder name (`String`). Phase 1 wants UUID v7 newtypes
for both session and event IDs. Migration: when `SessionId` lands, this
field becomes typed. Folder name can either become a derived form of the
ID or stay as a separate "directory hint" string.

### F3 — Events do not have stable IDs

No event in the current schema has a UUID. They're identified
implicitly by their position in `events.jsonl`. Phase 1.4 will need to
add an `EventId` to every event at write time. The structural place to
put it is on the wrapper (a common header), not on each variant.

### F4 — `LlmResponseEndedEvent.context_hash` already carries the
parent-context relationship

This addresses Phase 0.3 (open question from yesterday). The
`context_hash` field is "FK into `context.jsonl` for the assistant
record written for this response." Combined with the append-only
`context.jsonl`, the predecessor relationships *between LLM contexts*
are already reconstructible: each context record presumably references
the one before it (need to verify in `context_store.rs`).

**Verdict:** the substrate for the "internal context tree" projection
exists; we don't need new events to support it.

### F5 — Cross-session references today use folder names

`SessionMetadata.resumed_from` and the two resume events all reference
prior sessions by folder name string. Under Phase 1 this becomes a
`SessionRef` (UUID-based, layout-independent).

Migration concern: existing sessions on disk reference predecessors by
folder name. The redesign needs either (a) to mint UUIDs for old
sessions on first access, or (b) tolerate string-folder-name references
alongside ID references. (a) is cleaner; (b) is what cross-version
compatibility hacks look like. Lean (a).

### F6 — Defensive serde attributes in event types (Phase 0.4)

Cataloged in `crates/omega-types/src/events.rs`:

| Field | Attribute | Category | Why |
|---|---|---|---|
| `SessionStartedEvent.omega_commit` | `#[serde(default = "default_omega_commit")]` | Defensive | Old sessions before the field |
| `SessionStartedEvent.agent_time_zone` | `#[serde(default = "default_agent_tz")]` | Defensive | Old sessions before the field |
| `LlmResponseUsage.iterations` | `#[serde(default)]` | Possibly defensive | Absent on non-compaction responses; arguably correct (the field really is sometimes absent on the wire) |
| `SessionMetadata.resumed_from` | `#[serde(alias = "continuationOf")]` | Defensive | Old field name |
| `SessionMetadata` (struct level) | `#[serde(default)]` | Mixed | Treats absent file as empty metadata; arguably defensive but also handles the "no metadata yet" case |

**Recommendation:** flag these in code with a TODO comment referencing
the new policy, but don't remove them yet — most have real value for
not-yet-migrated on-disk data within the current Omega lineage. Removal
is a separate, deliberate sweep after a clear "schema cutoff" decision.

### F7 — `ActiveSession` in-memory state classification

| Field | Type | Classification | Notes |
|---|---|---|---|
| `agent` | `Arc<Mutex<Agent>>` | **needs audit** | Agent state TBD — see open work below |
| `controls` | `ControlHandle` | **ephemeral OK** | Bound to live agent; reconstructed on resume |
| `paths` | `SessionPaths` | **derivable** | Computed from session dir at startup |
| `ws_tx` | `Option<UnboundedSender<WsMessage>>` | **ephemeral OK** | Per-WS connection |
| `current_turn` | `Option<JoinHandle<()>>` | **ephemeral OK** | Task handle for currently-running turn; n/a on resume since we always resume at awaiting-user |
| `turn_state` | `Arc<Mutex<String>>` | **derivable** | Computed from events; should be `Idle` on resume |
| `info_cache` | `Arc<Mutex<SessionInfoCache>>` | **derivable** | Pure projection of session metadata + config events |

**Open:** the internals of `Agent` itself (`omega-agent` crate) —
specifically what state it carries across turns, what's reconstructable
from events, what isn't. This is the most important remaining piece of
the audit; it should be done as Phase 0.7.

### F8 — Server is single-session today (`Option<ActiveSession>`)

`AppState.active_session` is `Arc<Mutex<Option<ActiveSession>>>`. The
server can only host one live session at a time. Subagents will need to
either (a) host child agents in-process (multiple `ActiveSession`s
keyed by `SessionId`) or (b) run subagents in separate processes with
their own server slots.

For Phase 3 design: (a) is simpler operationally; (b) matches the
"subagent = child Omega instance" framing more literally. Both are
compatible with the `SessionRef` design — choice can be deferred.

## Open audit items still to do

- **Phase 0.7 (added):** audit `Agent` internals — what state is carried
  in `omega-agent` across turns; what's already event-sourced; what's a
  gap. This is the most consequential remaining piece.
- Verify `context.jsonl` records carry predecessor pointers (F4's
  caveat).
- Confirm `SessionStartedEvent.session_id` equals the session folder
  name in current code, or differs.
- Catalog where the "awaiting user" boundary is currently signalled in
  events (`TurnEnd`? Some other marker? Possibly implicit in "no
  in-flight turn"?).

## Verdict on Phase 0 progress

| Step | Status |
|---|---|
| 0.1 State audit (in-process) | **In progress** — high-level done; Agent internals remain |
| 0.2 "Session" usage scan | **Done** (see vocabulary table above) |
| 0.3 Parent-context-hash check | **Done** — `LlmResponseEndedEvent.context_hash` already there |
| 0.4 Defensive-serde scan | **Done** — see F6 |
| 0.5 Session version field | **Already in place** — `SessionStartedEvent.omega_commit` |
| 0.6 UUID v7 dependency | **Not yet** — small follow-up |
| 0.7 Agent internals audit (new) | **Not yet** — most important remaining work |
