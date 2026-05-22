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

## Phase 0.7 — Agent internals audit

### F9 — `Agent` struct field classification

The `Agent` struct in `crates/omega-agent/src/agent.rs` carries the
following cross-turn state:

| Field | Type | Classification | Notes |
|---|---|---|---|
| `provider` | `Arc<dyn Provider>` | **ephemeral OK** | Bound to process config; reconstructed from CLI args / env |
| `context_store` | `ContextStore` | **ephemeral OK** | Reconstructed from session dir path |
| `event_store` | `Arc<EventStore>` | **ephemeral OK** | Reconstructed from session dir path |
| `controls` | `ControlHandle` | **ephemeral OK** | All pause/continue/cancel flags are intra-turn; cleared at turn entry by `reset_for_turn` |
| `config` | `AgentConfig` | **ephemeral OK** | Process-startup config (`cwd`, `session_dir`, `headless`, initial model/effort) |
| `active_model` | `String` | **event-sourced** | Mutable via `set_model`; last value reconstructible from `ModelChanged` events using the existing `extract_last_model_and_effort` helper |
| `active_effort` | `String` | **event-sourced** | Mutable via `set_effort`; last value reconstructible from `EffortChanged` events using the same helper |
| `system_blocks` | `Vec<SystemBlock>` | **ephemeral OK** | Rebuilt at `init()` from disk (global and repo `AGENTS.md` files); the final concatenated prompt is archived in `SessionStartedEvent.system_prompt` for forensics, but the live struct is always rebuilt fresh |
| `system_prompt_paths` | `Arc<HashSet<PathBuf>>` | **derivable** | Derived from `system_blocks` immediately after they are assembled; zero I/O to reconstruct |
| `history` | `Vec<Message>` | **event-sourced** | In-memory mirror of `context.jsonl`; fully reconstructible via `ContextStore::read_all()` (exists today) |
| `context_hashes` | `Vec<ContextHash>` | **event-sourced** | Parallel to `history`; `ContextHash` is the `record.hash` field in each `ContextRecord` returned by `read_all()` |

**Summary:** every field that mutates across turns is already event-sourced. The
scaffolding for reconstruction (`extract_last_model_and_effort`,
`seed_history`, `ContextStore::read_all`) exists today. What is **missing**
is a Phase-2.2 entry point that assembles these building blocks into a
single "reconstruct Agent from session dir" path.

### F10 — `ControlHandle` intra-turn state is safely ephemeral

`ControlHandle` (in `controls.rs`) carries three mutable fields in
`ControlState`:

| Field | What it tracks | Cross-turn? |
|---|---|---|
| `pause_requested` | Pending pause click before the turn seam | No — cleared by `reset_for_turn` at turn entry |
| `pending_continue` | Continue click received before or after the seam | No — cleared by `reset_for_turn` |
| `suspended` | Agent parked at the seam waiting for continue | No — cleared by `reset_for_turn` and by `TurnGuard::drop` |

The turn-scoped `CancellationToken` is rotated fresh on every `send_message`
entry (`reset_for_turn` replaces it). Nothing in `ControlHandle` survives
a turn boundary. **No gap here.**

### F11 — Compaction silently resets `history` / `context_hashes` in-memory (gap)

When the Anthropic server fires the `compact_20260112` edit (triggered at
750 000 input tokens), `LlmResponseEnded.usage.iterations` contains an entry
with `iteration_type == "compaction"`. The agent detects this and calls:

```rust
self.history.clear();
self.context_hashes.clear();
```

This is correct for the current turn — the compacted response carries a new
compressed context baseline and nothing further needs to be sent as prior
history. However, **no event is emitted to record that compaction happened**
and that the pre-compaction records in `context.jsonl` are now stale.

Consequence: a naïve Phase-2.2 resume (load all `context.jsonl` records into
history) would send the full pre-compaction context — exactly what compaction
was trying to avoid. The reconstructed `history` would not match what the
original agent had at the "awaiting user" boundary.

**Classification: gap-to-close before Phase 2.2.** The agent should emit a
`ContextCompacted` event (or annotate `LlmResponseEnded` with a flag) so the
resume path knows to start from an empty history at that point. Compaction is
already classified by the design framing as a within-session event that the
derived LLM-context projection must honour.

### F12 — `context.jsonl` records carry no explicit predecessor pointers (F4 caveat resolved)

`ContextRecord` (in `context_store.rs`) has fields: `hash`, `time`, `role`,
`content`. There is **no** `prev_hash` or sequence index. Predecessor ordering
is purely by file-append sequence.

The predecessor relationship is recoverable from `events.jsonl`: each
`LlmCallEvent.context_hashes` field lists the full ordered array of hashes
sent in that call, so the context chain is reconstructible from the event log
without relying on append order in `context.jsonl`. This is sufficient.

**Verdict:** F4's claim holds. The context tree projection can be built from
`LlmCallEvent.context_hashes`; no changes to `context.jsonl`'s schema are
needed.

### F13 — `SessionStartedEvent.session_id` equals the folder name (confirmed)

In `Agent::init()`:

```rust
let session_id = self.config.session_dir.file_name().map_or_else(
    || "unknown".to_owned(),
    |n| n.to_string_lossy().into_owned(),
);
```

`session_id` is the last path component of `session_dir` — i.e. the folder
name (e.g. `2025-06-15T10-23-45-123-abcd1234`). It is **not** an independent
UUID. This confirms F2: Phase 1 must either (a) adopt a UUID-derived folder
name, or (b) treat the folder name as a separate "directory hint" alongside
the new `SessionId` UUID.

### F14 — "Awaiting user" boundary is `TurnEnd` or `TurnInterrupted` (confirmed)

In `router.rs`, the `turn_state` string is updated on every event from the
running turn:

```rust
OmegaEvent::UserMessage(_) | OmegaEvent::TurnContinued(_) => "running",
OmegaEvent::TurnPaused(_)                                  => "paused",
OmegaEvent::TurnEnd(_) | OmegaEvent::TurnInterrupted(_)   => "idle",
```

"Awaiting user" = `turn_state == "idle"`. This state is entered on:

- **`TurnEnd`** — clean turn completion (model replied without tool calls).
  The definitive safe resume point.
- **`TurnInterrupted`** — turn ended via abort (`reason: Aborted`) or error
  (`reason: Error`). Also a safe resume point per the design framing (partial
  turn is discarded on resume).

Both events are distinct, typed, and appear at most once per turn at the
very end. The "awaiting user" boundary is **unambiguous and already in the
event stream**. Phase 2.1 can use exactly these two events as the
resume-point marker.

**Note:** `TurnInterrupted{reason: Aborted}` implies the last assistant
record in `context.jsonl` may contain dangling `ToolUse` blocks without
matching `ToolResult` records. The existing `send_message` Step 1 ("dangling
tool_use repair") handles this at turn start — the Phase-2.2 resume path
must ensure this repair fires, or the loaded history must be trimmed to
exclude the incomplete assistant record before calling `seed_history`.

### F15 — `system_blocks` may differ on resume (known, accepted)

`system_blocks` are rebuilt from disk at `Agent::init()`. If `AGENTS.md`
files change between the original session and a resumed one, the system
prompt will differ. The design framing accepts this ("no cross-version
replay"). The archived `SessionStartedEvent.system_prompt` captures what
the model actually saw for forensic comparison. **No action required.**

### F16 — `active_model` / `active_effort` reconstruction path exists

`extract_last_model_and_effort(events)` is a public, tested helper in
`session_resume.rs`. The Phase-2.2 resume entry point should call it to set
the initial model and effort before constructing `AgentConfig`. Priority
order: last `ModelChanged` / `EffortChanged` event → `SessionStartedEvent`
field → hard-coded default.

## Phase 0 status — DONE

All audit steps complete. Only outstanding item is **0.6 UUID v7
dependency**, folded into the start of Phase 1.

One gap surfaced (**F11 — compaction event**) is now tracked as
**Phase 2.0** in `session-design-wip.md` and must be closed before the
resume entry point can be built.
