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

## Auxiliary-session prompts

When you propose handing work off to another session (sub-session,
fresh session, parallel run), present the prompt as a **fenced code
block** so the UI surfaces a copy button. Use four-backtick fences if
the prompt itself contains triple-backtick code blocks. Put any
surrounding meta (recommended model, effort, rationale) outside the
fence, in prose — only the prompt text belongs inside.

## Cargo dependencies

When adding or updating a dependency, declare only the features the crate's own source directly uses — never `features = ["full"]` in production or dev dependencies. Use `default-features = false` wherever a crate's default features pull in transitive deps that aren't needed (alternative TLS backends, executors, etc.).

## Testing

**Make tests as end-to-end as possible.** Test through real public APIs rather
than reaching into internals:

- In `omega-tools`: test via `execute_tool`, not by calling tool modules
  directly.
- In `omega-agent`: test via `Agent::send_message` + `MockProvider`, not by
  calling internal helpers.
- When a unit test is genuinely the right fit (e.g. a pure function where
  the agent-level setup would be disproportionate), treat it as a carve-out:
  add a comment explaining why (see existing examples in
  `omega-agent/src/system_prompt.rs` and `omega-tools/src/lib.rs`).

**Verify exhaustive coverage with targeted mutation testing.** After writing
tests for any non-trivial logic, run:

```
cargo mutants -p <crate> --cap-lints=true --file "<path/to/changed/file.rs>"
```

`--cap-lints=true` is required because the workspace sets `-D warnings`;
without it, body-replacement mutations produce unused-variable errors and
appear as *unviable* rather than *caught* or *missed*.

All mutations must end up **caught** or **unviable**. A **survivor**
(mutation compiles and all tests pass) means a test gap — close it with a
new test, or mark with `#[mutants::skip]` and a comment explaining why the
mutation cannot be meaningfully tested.

Add a named Justfile recipe for each targeted run so it can be repeated
easily. See `mutants-system-prompt-guard` as a template.

## Contract Authority — the Rust types define the schema

The **Rust event types** (`OmegaEvent` in `crates/omega-types/src/events.rs`)
are the canonical schema definition. All other representations are derived
from them and must conform:

1. **In-memory event type** (`OmegaEvent`) — the authoritative source.
   Changes here are the change; everything else follows.
2. **Persistence** (`events.jsonl`, `context.jsonl`) — the serde projection
   of `OmegaEvent`. Backward compatibility with old log files is **not**
   required; agility in evolving the schema matters more. Best-effort
   loading of old sessions is the goal: the Rust type system and fold
   invariants reject incompatible events structurally, which is the
   intended guard. Never use `#[serde(rename)]` / `#[serde(default)]`
   **defensively** to suppress a deserialization error — that silently
   masks exactly the structural drift we want to see loudly. Only add
   such attributes **deliberately**, when a field is genuinely optional
   in the domain (e.g. a field that didn't exist in an earlier version
   and whose absence has a well-defined meaning). When in doubt, let it
   fail.

   **Loud schema change rule:** when the *meaning* of a schema element
   changes, the *syntax* must change with it — rename the field, add a
   new variant, or introduce a new type. Do not absorb a semantic change
   silently behind an alias or by reusing an existing name/variant for a
   different purpose. Old readers must fail loudly on the new syntax, not
   silently misinterpret it. A deserialization error on an old log is
   diagnostic signal; a silent wrong interpretation is a latent bug.
3. **WebSocket protocol** (`WsMessage` in
   `crates/omega-server/src/ws_message.rs`) — transport projection of
   `OmegaEvent`; may carry extra ephemeral fields.
4. **Rendered UI** — least stable; can change freely.

Rule: update the UI to match the types — never change the types (and thus
the on-disk format) just to match the UI.

## Tricky bugs — ask for session logs

If you have tried two or more approaches on the same bug and are still stuck,
or if you suspect you may be going in circles, **stop and ask the user
whether a prior session log is relevant** before trying another approach.
Only the user can make the connection between a symptom and the right log.

## LLM Provider

Omega is Anthropic-only. The supported models are:

- `claude-sonnet-4-6` — default, fast
- `claude-opus-4-7` — most capable; use for hard problems

To look up Anthropic/Claude API documentation: fetch `https://platform.claude.com/llms.txt`
to get an indexed list of all docs pages (each entry links to a `.md` URL), find the
relevant page, then fetch that specific `.md` URL with `fetch_url`. Individual pages fit
comfortably within a single `fetch_url` call.
