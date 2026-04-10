## Project conventions and state

### Purpose

Omega is a general-purpose coding agent. It can be pointed at any project
directory and will orient itself by reading available documentation and project
files. The user interacts via terminal or web UI.

### Stack

- Agent core (`src/agent.ts`) must have no UI imports — UI and agent stay cleanly separated.
- `StreamProvider` is the interface for LLM provider calls; tests inject a mock — the real API is never called in tests. **If `StreamProvider` is renamed, update this file too.**

### LLM Provider

Omega is Anthropic-only. The two supported models are:

- `claude-sonnet-4-6` — default, fast
- `claude-opus-4-6` — slower, more capable

To look up Anthropic/Claude API documentation: fetch `https://platform.claude.com/llms.txt`
to get an indexed list of all docs pages (each entry links to a `.md` URL), find the
relevant page, then fetch that specific `.md` URL with `fetch_url`. Individual pages fit
comfortably within the 20 000-char `fetch_url` limit.

### Workflow tools

- Use `gh` (not raw `curl`) for GitHub operations — it's authenticated as `cfuehrmann` with `repo` scope. `gh pr create`, `gh issue list`, `gh release create`, `gh auth status`, etc. Still use `git` for push/pull.

### Branch State

All development work goes on `develop`. Merge to `main` when stable.

### Testing

- **Never bypass the gate** — no `--no-verify`, no rationalizing failures as
  "flaky". A test that passes alone but fails in the gate is a real bug. Fix it.
- `just gate` runs as the **pre-commit hook** — do not run it separately.
  Always commit with `git add -A && git commit -m "..."` (not `git commit -a`,
  which misses untracked files). **Exit code 0 = committed, gate passed** — no
  log reading needed. Non-zero: check the `run_command` output, or for the full
  log: `test-output/gate-latest.log` (sections: `=== typecheck / test / knip ===`).
- `just test` — test-core and test-browser in parallel (outputs printed
  sequentially)
- `just test-fast` — `bun test --bail`, fast feedback during iteration
- `bun test src/foo.test.ts` — single file, preferred while iterating
- `just test-browser` — full Playwright suite (builds web client first, ~30 s)
- `just e2e [args]` — **targeted Playwright run, no rebuild.** Use when
  iterating on specific UI behaviour and the build is already current. Accepts
  any Playwright CLI args — file paths, `--grep` patterns, etc. Examples:
  `just e2e e2e/web-ui-mermaid.spec.ts`, `just e2e --grep "reconnect"`.
  Run `just web-build` first if frontend source has changed since the last build.
- `just test-browser-log` — builds frontend (~30 s), then runs Playwright with
  `--reporter=list`, saving full output to `test-output/playwright-<timestamp>.log`
  and printing the path. Use `run_command("just test-browser-log", { timeout: 120 })`.
  The stdout shows the playwright log path — inspect that log with `read_file` /
  `grep_files` (use `offset`/`limit` to paginate). The playwright log persists in
  `test-output/` — never re-run just to see more output.

`just web-build` bundles the Vite/SolidJS web client into `src/web/public/`. It
is not a general project build — backend/agent TypeScript is run directly by
Bun.

Prefer tests that exercise the full stack with real file I/O rather than mocking
away storage. Use a unique output path (e.g. timestamp + random suffix) per test
run so tests can run in parallel without conflicts. Let test artifacts
accumulate — they become inspectable evidence. Mock external services (LLMs,
third-party APIs) but always use real I/O with the dedicated test output path.

### Contract Authority — the most public contract wins

When multiple representations of the same information exist, the most public one
is authoritative and all others conform to it. For Omega:

1. **Persistence** (`events.jsonl`, `context.jsonl`) — most public. Breaking
   changes require explicit migration.
2. **In-memory event type** (`OmegaEvent` in `src/events.ts`) — must match
   persistence.
3. **WebSocket protocol** (`WsEvent`) — transport projection of `OmegaEvent`;
   may carry extra ephemeral fields.
4. **Rendered UI** — least public; can change freely.

Rule: update the UI to match the log — never the log to match the UI.

### Bug fixes — red-green testing

When fixing a bug, write a failing test that reproduces it first (red), then
fix the code so the test passes (green), wherever this is practical. Practical
means: the bug is deterministic, the failure mode is directly observable in a
test, and writing the test doesn't cost more than the fix itself. Skip red-green
when the bug is a one-liner typo or the reproduction requires complex
infrastructure that already exists only in production.

### Tricky bugs — ask for session logs

If you have tried two or more approaches on the same bug and are still stuck,
or if you suspect you may be going in circles, **stop and ask the user whether
a prior session log is relevant** before trying another approach. Only the user
can make the connection between a symptom and the right log.


