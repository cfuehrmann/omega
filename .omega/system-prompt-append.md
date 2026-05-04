## Commit rule

**Always finish with `git add -A && git commit -m "..."` before reporting back.**
The pre-commit hook runs the gate; exit code 0 is the proof of correctness.
Reporting back without committing defeats the gate entirely. Never use `--no-verify`.

## Stack

- Agent core (`src/agent.ts`) must have no UI imports — UI and agent stay cleanly separated.
- `CreateMessageStream` is the type for LLM streaming calls; tests inject a mock — the real API is never called in tests. **If `CreateMessageStream` is renamed, update this file too.**

## Workflow tools

- Use `gh` (not raw `curl`) for GitHub operations — it's authenticated as `cfuehrmann` with `repo` scope. `gh pr create`, `gh issue list`, `gh release create`, `gh auth status`, etc. Still use `git` for push/pull.
- All development work goes on `develop`. Merge to `main` when stable.

## Gate & testing

The pre-commit hook has three paths:

1. **Docs-only** (all staged files are `*.md`, `docs/`, or `backlog/`) → gate skipped.
2. **Rust-only** (all non-doc staged files are under `rust/`) → runs `just rust-gate` (cargo fmt check + clippy + cargo test + cargo machete + bindings drift check). Fast; no TS or Playwright involved.
3. **Everything else** → runs `just gate` (typecheck + full test suite + knip + session-pollution check).

Always commit with `git add -A && git commit -m "..."` (not `git commit -a`, which misses untracked files). **Exit code 0 = committed** — the hook suppresses gate stdout so the tool result stays small; no log reading needed. On failure the hook prints the last 60 lines of `test-output/gate-latest.log`; open the full log only if that isn't enough.

**Never bypass the gate** — no `--no-verify`, no rationalizing failures as "flaky". A test that passes alone but fails in the gate is a real bug. Fix it.

- `just gate` — full TS gate: typecheck + `just test` + knip + session-pollution check. Do not run separately; always commit instead.
- `just rust-gate` — Rust gate: `cargo fmt --check && cargo clippy -- -D warnings && cargo test && cargo machete`, then bindings drift check. Run manually when iterating on Rust before committing.
- `just rust-bindings` — regenerate TypeScript bindings from Rust types. Re-run whenever Rust types change; commit the updated files under `rust/bindings/`.
- `just test` — builds web client + Rust mock server, then runs bun tests and Playwright in parallel (outputs printed sequentially).
- `just test-fast` — `bun test --bail`, fast feedback during iteration.
- `bun test src/foo.test.ts` — single file, preferred while iterating.
- `just test-browser` — full Playwright suite; builds web client + Rust mock server first.
- `just e2e [args]` — **targeted Playwright run, no rebuild.** Use when iterating on specific UI behaviour and the build is already current. Accepts any Playwright CLI args — file paths, `--grep` patterns, etc. Examples: `just e2e e2e/web-ui-mermaid.spec.ts`, `just e2e --grep "reconnect"`. Run `just web-build` (and `just rust-build-mock-server` if Rust changed) first if those are stale.
- `just test-browser-log` — same as `just test-browser` but saves full output to `test-output/playwright-<timestamp>.log` and prints the path. Inspect with `read_file` / `grep_files`; never re-run just to see more output.

`just web-build` bundles the Vite/SolidJS web client into `src/web/public/`. Backend/agent TypeScript is run directly by Bun; this is not a general project build.

Prefer tests that exercise the full stack with real file I/O rather than mocking away storage. Use a unique output path (e.g. timestamp + random suffix) per test run so tests can run in parallel without conflicts. Let test artifacts accumulate — they become inspectable evidence. Mock external services (LLMs, third-party APIs) but always use real I/O with the dedicated test output path.

## Contract Authority — the most public contract wins

When multiple representations of the same information exist, the most public one is authoritative and all others conform to it. For Omega:

1. **Persistence** (`events.jsonl`, `context.jsonl`) — most public. Breaking changes require explicit migration.
2. **In-memory event type** (`OmegaEvent` in `src/events.ts`) — must match persistence.
3. **WebSocket protocol** (`WsEvent`) — transport projection of `OmegaEvent`; may carry extra ephemeral fields.
4. **Rendered UI** — least public; can change freely.

Rule: update the UI to match the log — never the log to match the UI.

## Tricky bugs — ask for session logs

If you have tried two or more approaches on the same bug and are still stuck, or if you suspect you may be going in circles, **stop and ask the user whether a prior session log is relevant** before trying another approach. Only the user can make the connection between a symptom and the right log.
