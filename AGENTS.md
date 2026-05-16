## Commit rule

**Always finish with `git add -A && git commit -m "..."` before reporting back.**
The pre-commit hook runs the gate; exit code 0 is the proof of correctness.
Reporting back without committing defeats the gate entirely. Never use
`--no-verify`. On failure the hook prints the last 60 lines of
`test-output/gate-latest.log`; open the full log only if that isn't enough.

`git commit -a` is wrong — it skips new untracked files. Always `git add -A`.

## Workflow

- Use `gh` (not raw `curl`) for GitHub operations; it's authenticated as
  `cfuehrmann`.
- Active branch: `develop`. Merge to `main` when stable.

## Contract Authority — the most public contract wins

When multiple representations of the same information exist, the most public
one is authoritative and all others conform to it:

1. **Persistence** (`events.jsonl`, `context.jsonl`) — most public. The
   on-disk format is the serde projection of `OmegaEvent` (see 2).
   Breaking changes require explicit migration.
2. **In-memory event type** (`OmegaEvent` in
   `rust/crates/omega-types/src/events.rs`) — must match persistence.
   Use `#[serde(rename)]` / `#[serde(default)]` to evolve the type without
   breaking the file format.
3. **WebSocket protocol** (`WsMessage` in
   `rust/crates/omega-server/src/ws_message.rs`) — transport projection of
   `OmegaEvent`; may carry extra ephemeral fields.
4. **Rendered UI** — least public; can change freely.

Rule: update the UI to match the log — never the log to match the UI.

## Tricky bugs — ask for session logs

If you have tried two or more approaches on the same bug and are still stuck,
or if you suspect you may be going in circles, **stop and ask the user
whether a prior session log is relevant** before trying another approach.
Only the user can make the connection between a symptom and the right log.
