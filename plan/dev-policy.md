# Development Phase Policies

Policies in this file are **temporary** — they apply during an active
refactoring phase and may be promoted to `manifest.md` or retired once the
relevant work stabilises.

---

## Event naming — persisted names are authoritative

`events.jsonl` names win. In-memory type, WebSocket message, and UI block names
all conform to the persisted name — never the reverse. See `manifest.md` §
"Contract authority — the most public contract wins" for the full rationale.

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

Enforcement: exhaustive switch statements (TypeScript `never` check) are in
place in `src/terminal/app.ts` (switch on `OmegaEvent | StreamSignal`) and
`src/web/client/App.tsx` (switch on `WsEvent`). The `exhaustiveCheck(x: never)`
helper is exported from `src/events.ts` for server-side code and defined
locally in `App.tsx` for client-side code. A new `OmegaEvent` variant without
a render case is a compile-time error, not a silent omission.

Status: **ENFORCED** as of EU-4. Verified: `bun test` (471 pass), `vite build`
(no errors).

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
