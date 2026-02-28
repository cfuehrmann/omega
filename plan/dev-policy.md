# Development Phase Policies

Policies in this file are **temporary** — they apply during an active
refactoring phase and may be promoted to `manifest.md` or retired once the
relevant work stabilises.

---

## Event naming — persisted names are authoritative

`events.jsonl` is the single source of truth for event type names. When any
representation (in-memory type, WebSocket message, rendered UI block) conflicts
with the persisted name, **the persisted name wins**. All other consumers are
updated to match — never the other way around.

This is an instance of the **contract authority rule** in `manifest.md` §
"Contract authority — the most public contract wins": persistence is the most
public contract; the stream and the UI are derived projections of it.

Concretely (as of EU-3): the stream-facing names `agent_to_agent_tool_call`,
`agent_to_agent_tool_result`, and `llm_to_agent` were renamed to match the
persisted names `tool_call`, `tool_result`, `llm_response` in the unified
`OmegaEvent`. The `WsEvent` web protocol follows the same names.

---

## UI sync invariant

**Every variant of `OmegaEvent` must be rendered in the terminal UI and the
web UI.**

Minimum rendering: event name + timestamp on one line. Some variants warrant
more detail (tool calls, errors, `turn_end` metrics). No variant is silently
dropped.

Rationale: during the persistence contract refactoring, maximum situational
awareness is required. The event log (`sessions/events.jsonl`) is the complete
representation; the UI is its user-friendly projection. Any event that exists
in the log but is invisible in the UI is a gap in that projection.

Enforcement: exhaustive switch statements (TypeScript `never` check) in
`src/terminal/renderer.ts` and `src/web/client/App.tsx`. A new `OmegaEvent`
variant without a render case must be a compile-time error, not a silent
omission.

Applies from: completion of EU-3 (event system unification).
Review at: completion of Step 3f (session resume) — decide whether to
promote to `manifest.md` or retire.

---

## Schema stability

No breaking changes to `plan/schema.md` without explicit operator agreement.

A **breaking change** is: removing a field, renaming a field, changing a
field's type or semantics, removing an event variant.

An **additive change** is: adding a new optional field to an existing event,
adding a new event variant. These are allowed without a schema revision, per
the forward-compatibility policy in `plan/schema.md` § Forward compatibility.

`plan/schema.md` is the authoritative record of agreed event shapes. Any
proposed breaking change must be discussed and the schema doc updated before
implementation begins.

Applies from: completion of `plan/schema.md` (step 3e-viii).
