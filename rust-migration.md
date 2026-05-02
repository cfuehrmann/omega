# Omega — Rust Migration

*Living document. Completed phases are summarised briefly; upcoming phases have full detail.*

---

## Status

| Phase | Status | Deliverable |
|---|---|---|
| 0 — Planning | ✅ Done | This document + architectural decisions |
| 1a — `omega-protocol` | ✅ Done | All 22 `OmegaEvent` variants, `StreamSignal`, serde round-trips; workspace tooling (edition 2024, clippy::pedantic, machete, mutants); honest types |
| 1b — `omega-core` (LLM loop) | ✅ Done | Anthropic + Ollama providers, retry loop, streaming, insta snapshots; 0 surviving mutants |
| 1c — `omega-store` (Persistence) | ✅ Done | `ContextHash`, `SessionPaths`, `EventStore`, `ContextStore`; JSONC stripping; `spawn_blocking` append; 0 surviving mutants |
| 1d.0a — `omega-agent` core + scaffolds | ✅ Done | Agent loop, system prompt, error classifier, MockProvider + 6 integration tests, `omega-tools` stubs + dispatch, `omega-cli --help` |
| 1d.0b — tool body ports + CLI wiring | ✅ Done | 12 real tool implementations + 35 integration tests; `omega-cli run` end-to-end; `OmegaRustAgent` Harbor adapter |
| 1d.0c — mutant killing (`omega-tools`) | ✅ Done | 66 → 16 missed mutants; 2 production bugs found and fixed; surviving mutants fully classified |
| 1d.0d — eliminate external binary deps | ⬜ Next | Replace `rg`/`fd` subprocesses with `ignore`+`globset`+`regex`; kill Group A mutants; document accepted remainder |
| 1d.1 — `omega-agent` advanced | ⬜ Upcoming | Pause/continue/abort, session resumption, compaction, model/effort switching |
| 1e — `omega-server` (WebSocket) | ⬜ Upcoming | tokio/axum server, session mgmt, WS fan-out, HTTP static serving |
| 1f — Bridge (`ts-rs`) | ⬜ Upcoming | Generate `.d.ts` from Rust types, TS UI stays type-checked |
| 2 — Rust as primary driver | ⬜ Future | TS UI talks to Rust backend; TS CLI retired |
| 3 — Leptos UI rewrite | ⬜ Future | SolidJS → Leptos; TS deleted |
| 4 — `chromiumoxide` + LLM oracle | ⬜ Future | Playwright retired; pure-Rust browser tests |

---

## Why Rust (brief)

- **No escape hatches** — no `as any`, `// @ts-ignore`. The compiler refuses structurally.
- **Multi-provider** — once the target is Anthropic + Ollama + others, wire-format code is unavoidable regardless of language. Rust structs + serde + reqwest + SSE are cleaner than juggling multiple TS SDKs.
- **`insta`** — best snapshot-testing DX in any ecosystem (`cargo insta review` TUI, inline diffs, CI integration).
- **`cargo mutants`** — mutation testing that finds weak tests and dead code. Stryker for TS is significantly weaker.
- **Gate speed** — Playwright dominates gate time; `cargo test` is not the bottleneck.

---

## Repo layout

```
dev/
├── rust/                       ← Cargo workspace (all new Rust code)
│   ├── Cargo.toml
│   └── crates/
│       ├── omega-protocol/     ✅ done
│       ├── omega-core/         ✅ done
│       ├── omega-store/        ✅ done
│       ├── omega-tools/        ✅ done (bodies in 1d.0b, mutants in 1d.0c, refactor in 1d.0d)
│       ├── omega-agent/        ✅ core done (1d.0a); advanced in 1d.1
│       └── omega-cli/          ✅ done (wired in 1d.0b)
├── src/                        ← TypeScript (frozen; no new features)
├── Justfile
└── package.json
```

---

## Architectural decisions (settled — do not re-litigate)

**All-in Rust including Leptos web client.** Cross-language type friction at the WebSocket boundary is worse than either pure choice.

**Leptos over Dioxus/Yew/Sycamore.** Fine-grained reactivity identical to SolidJS.

**`omega-protocol` as keystone.** Shared crate with `#[derive(Serialize, Deserialize)]` enforces contract discipline.

**Two providers from day one.** Forces a real provider abstraction.

**`ts-rs` bridge during Phase 1.** Generates `.d.ts`; deleted when UI migrates to Leptos.

**Don't redesign during port.** Success criterion is parity. All ideas go in a deferred file.

**Separate sessions for snapshot review.** Within-session blind prompts are insufficient; separate session breaks priming.

---

## Completed phases — concise record

### Phase 1a — `omega-protocol` ✅

All 22 `OmegaEvent` variants with honest types. Workspace tooling: edition 2024,
`clippy::pedantic -D warnings`, `cargo-machete`, `cargo mutants`. 0 surviving mutants.

### Phase 1b — `omega-core` (LLM loop) ✅

`Provider` trait, `AnthropicProvider` (SSE), `OllamaProvider` (NDJSON),
`RetryingProvider<P>`. All wiremock-fronted; no live API calls. Sub-phases 1b.0 →
1b.7. Final: 0 survived, 2 timeouts (infinite-retry mutations — expected).

Key notes:
- `AgentItem::Event` boxes `OmegaEvent` (large_enum_variant).
- `LlmError::Transport` is reachable: reproduced via in-process flaky-listener.
- Sequential wiremock: mount multiple `Mock`s with `.up_to_n_times(N)`.

### Phase 1c — `omega-store` (Persistence) ✅

Four modules. Key: `spawn_blocking` for file I/O (Tokio `pwrite` ignores
`O_APPEND`); manual JSONC scanner; `serde(alias)` for legacy field names.
0 survived, 4 timeouts.

### Phase 1d.0a — `omega-agent` core + scaffolds ✅

`Agent` struct + `send_message` async-stream generator. All 12 tool stubs.
`omega-cli --help`. 6 integration tests with `MockProvider` + real `omega_store`.
3 missed mutants (all in low-value helpers: `now_iso()` ×2, `read_system_prompt_append`
`NotFound` fallback). Acceptable.

### Phase 1d.0b — tool body ports + CLI wiring ✅

12 tools fully implemented; 35 integration tests; `omega-cli run` end-to-end.
`OmegaRustAgent` Harbor adapter added. `just rust-gate` passes.

`cargo mutants -p omega-tools`: 172 mutants — 87 caught, **66 missed**, 18 unviable,
1 timeout. Missed mutants recorded as a baseline for Phase 1d.0c.

Notable implementation decisions:
- `list_files`: `spawn_blocking` + manual recursive `std::fs`; dirs-first sorted.
- `run_command`: `process_group(0)` + timeout + `kill_group` on timeout for orphan cleanup.
- `grep_files` / `find_files`: `rg`/`fd` subprocess with `grep`/`find` fallback.
- `wait_for_output`: 200 ms poll; `regex` pattern; `try_wait` for exit detection.
- `fetch_url`: SHA-256 URL cache; `html_to_text` (regex strip); postprocess subprocess.

### Phase 1d.0c — mutant killing (`omega-tools`) ✅

Starting from the 66-missed baseline, this phase added ~50 targeted integration
tests (plus inline unit tests in `state.rs` and `read_file.rs`) and fixed two
real bugs. Final: **16 missed**, 136 caught, 18 unviable, 2 timeouts.

#### Bugs found and fixed

**BUG 1 — `kill_group` silently fails (production, fixed in commit `914f6f3`):**
`kill_group` called `/usr/bin/kill -KILL -PGID`. The util-linux `kill` binary
(v2.42 on this system) interprets a leading-hyphen numeric argument as a
*process-name search* rather than a process-group signal, silently discarding
the `ESRCH` error. Background processes spawned by timed-out bash commands were
**never killed** — a silent resource leak. Fixed by using
`sh -c "kill -9 -PGID"`, which uses the POSIX shell builtin and calls
`kill(-pgid, SIGKILL)` correctly.

**BUG 2 — `node_modules` recursion guard is dead code (documented, not fixed):**
`list_files.rs` has `if name_str == "node_modules" { continue; }` early in
the `for entry in entries` loop, which skips the entry before it can reach
the `if recursive && … && name_str != "node_modules"` guard. The `name_str !=
"node_modules"` condition in that recursive guard is therefore unreachable.
The `.git` guard in the same expression *is* live (entries named `.git` are
not skipped by the earlier `continue`, so they do reach the recursive guard).

#### Surviving mutants — full classification

After Phase 1d.0c, 16 mutants remain. They fall into four groups:

---

**Group A — Dead code: grep/find fallback paths (5 mutants)**

```
grep_files.rs:54:12   delete ! in execute             (grep fallback: !case_sensitive)
grep_files.rs:60:26   replace > with ==  in execute   (grep fallback: context_lines > 0)
grep_files.rs:60:26   replace > with <   in execute
grep_files.rs:60:26   replace > with >=  in execute
find_files.rs:47:12   delete ! in execute             (find fallback: if !hidden)
```

`rg` and `fd` are installed on this machine, so `has_command("rg")` /
`has_command("fd")` always return true and the `else` branches (grep/find)
are **never executed**. These mutations are unreachable by any test short
of physically removing the binaries.

Root cause: the external-binary + fallback design creates an untestable code
path by construction. **Resolution in Phase 1d.0d**: replace both
tools with pure-Rust implementations (`ignore` + `globset` for `find_files`;
`ignore` + `regex` for `grep_files`), deleting the fallback branches entirely.

---

**Group B — Truly equivalent mutations (4 mutants) — accepted**

```
grep_files.rs:44:26   replace > with >= in execute
grep_files.rs:126:5   replace has_command -> bool with true
find_files.rs:55:34   replace != with == in execute
list_files.rs:96:51   replace + with * in walk_sync
```

- **`context_lines >= 0`** (`> with >=`): u64 is always ≥ 0, so `--context 0`
  would always be added. But `rg --context 0` is a no-op — identical to no flag.
- **`has_command → true`**: `rg` is installed; the function already returns true.
  Replacing the body with `true` is behaviourally identical.
- **`fd exit-code 1`** (`!= 1 → == 1`): `fd` exits 0 (not 1) for no-match results.
  The `out.code != 1` guard is only meaningful for grep (which exits 1 for
  no-match); it's dead for the fd path. Mutation is behaviourally equivalent.
- **`depth + 1 → depth * 1`**: `depth` is only used in `depth == 0 && !recursive`.
  When `recursive = true` (the only time `walk_sync` recurses), `!recursive = false`
  makes the condition false regardless of `depth`'s value. Truly equivalent.
  The `depth` parameter exists for a future use case that hasn't materialised;
  consider removing it in a future cleanup pass.

---

**Group C — Hard to test without specific infrastructure (3 mutants) — accepted**

```
grep_files.rs:121:46  delete - in run_subprocess      (unwrap_or(-1) → unwrap_or(1))
grep_files.rs:126:5   replace has_command -> bool with false
wait_for_output.rs:76:75  replace >= with < in execute
```

- **`unwrap_or(1)` vs `(-1)`** (signal exit): When `rg`/`grep`/`fd` is killed by
  a signal, `exit_status.code()` returns `None` and the fallback fires. With `1`
  instead of `-1`, a signal-killed subprocess is treated as "no matches found"
  (exit 1 = not-an-error) instead of a real error. Triggering this reliably
  requires engineering a mid-run signal kill with precise timing — hard without
  a specialised test harness. This will become a non-issue after the Phase 1d.0d
  rewrite (no subprocess to kill).
- **`has_command → false`** (forces fallback): With `has_command` always returning
  false, `grep_files` uses grep and `find_files` uses find. For every test pattern
  we use, grep/find produce output close enough to rg/fd that `contains()`
  assertions pass either way. Killing this would require format-specific
  assertions (e.g. checking for `--no-heading` in output) that couple tests to
  implementation details. Also becomes a non-issue after Phase 1d.0d.
- **`wait_for_output` exit-branch `>= → <`** (line 76): The exit-branch
  `minBytesReached` computation is only reached when the process exits AND the
  main-loop `>=` check hasn't fired yet (content was below the threshold at
  the previous poll). The race window between "content < min" at poll time and
  "process exits + final content >= min" at exit detection is so narrow that
  testing it deterministically would require sleep injection or a fake clock —
  disproportionate effort for a one-line edge case.

---

**Group D — Require a live Brave Search API key (4 mutants) — accepted**

```
web_search.rs:45:8   delete ! in execute              (HTTP error check inverted)
web_search.rs:80:31  replace > with == in execute     (truncation guard)
web_search.rs:80:31  replace > with <  in execute
web_search.rs:80:31  replace > with >= in execute
```

All four are inside the HTTP-response handling path, gated behind a real
Brave Search API call. The existing `web_search_live_returns_results` test
skips without `BRAVE_SEARCH_API_KEY`. Killing these would require either a
live key in CI or a reqwest mock — neither is worth doing for four mutants.

---

## Phase 1d.0d — Eliminate external binary dependencies (`omega-tools`) ⬜ Next

This phase resolves **Group A** by removing the `rg`/`fd` subprocess approach
entirely, replacing both tools with pure-Rust implementations built on the
BurntSushi crate family that powers ripgrep. Groups B–D are accepted as-is
(documented above).

### Why this matters beyond mutant count

- **Portability**: works in any environment — minimal containers, NixOS with
  a locked PATH, future Windows support — with zero external tooling.
- **No subprocess overhead**: no fork/exec for every grep or find call.
- **No fallback paths**: the entire `has_command` / fallback branch structure
  disappears, taking all 5 Group A mutants and 3 Group C mutants with it.
- **Simpler error model**: no subprocess exit-code parsing; errors are typed
  Rust values.

### Crates to add

```toml
# rust/crates/omega-tools/Cargo.toml
ignore  = "0.4"    # BurntSushi's directory-walking engine (the core of ripgrep)
globset = "0.4"    # BurntSushi's compiled glob matching (already used by ignore)
```

`regex` is already a dependency. No other new crates needed.

The `ignore` crate is literally the directory-traversal half of ripgrep — it
handles hidden-file filtering, `.gitignore`/`.ignore` rules, and symlink
policy. `globset` handles the `*.rs`-style pattern matching. Together they
give us fd's semantics without the binary.

### Step 1 — Rewrite `find_files.rs`

Replace the current `fd`/`find` subprocess with a `spawn_blocking` closure
using `ignore::WalkBuilder` + `globset::Glob`.

The `WalkBuilder` API maps directly onto the tool's parameters:

| Tool parameter | WalkBuilder method |
|---|---|
| `hidden = false` (default) | `.hidden(true)` — skip dotfiles |
| `hidden = true` | `.hidden(false).git_ignore(false).ignore(false)` |
| `type = "f"` | filter `entry.file_type().is_file()` |
| `type = "d"` | filter `entry.file_type().is_dir()` |

The glob pattern is compiled with `Glob::new(pattern)?.compile_matcher()`
and tested against `entry.file_name()` (basename only, matching fd semantics).

Output format is unchanged: one path per line, truncation notice if
`lines.len() > max_results`. The `run_subprocess`, `has_command`, and the
entire `else` block disappear.

Expected: ≈ 80 lines of implementation; all existing `find_files_*` tests
continue to pass; the 5 mutants from the find fallback disappear; `find_files.rs:55:34`
(exit-code 1, Group B) also disappears since there's no subprocess exit code.

### Step 2 — Rewrite `grep_files.rs`

Replace the `rg`/`grep` subprocess with a `spawn_blocking` closure using
`ignore::WalkBuilder` (for traversal + glob filtering) and `regex::RegexBuilder`
(for case-sensitivity). Implement context-window logic manually.

The output format must match rg's `--no-heading --with-filename --line-number`
style exactly, since the LLM prompt references it:

```
# match line
path/to/file.rs:42:    let x = foo;

# context lines use '-' instead of ':'
path/to/file.rs:41-    // context before
path/to/file.rs:43-    // context after

# separator between non-adjacent match groups
--
```

Implementation sketch:

```rust
fn search_file(
    path: &Path,
    re: &Regex,
    context: usize,
    results: &mut Vec<String>,
    max: usize,
) -> bool /* hit limit */ {
    let Ok(text) = std::fs::read_to_string(path) else { return false };
    let lines: Vec<&str> = text.lines().collect();
    let mut prev_end: Option<usize> = None;

    for (i, line) in lines.iter().enumerate() {
        if !re.is_match(line) { continue; }

        let start = i.saturating_sub(context);
        let end   = (i + context + 1).min(lines.len());

        if prev_end.map_or(false, |e| start > e) {
            results.push("--".into());
        }
        for (j, &l) in lines[start..end].iter().enumerate() {
            let lnum = start + j + 1;
            let sep  = if start + j == i { ':' } else { '-' };
            results.push(format!("{}:{lnum}{sep}{l}", path.display()));
            if results.len() >= max { return true; }
        }
        prev_end = Some(end);
    }
    false
}
```

For glob filtering, pass the glob to `WalkBuilder` via
`WalkBuilder::new(path).add_custom_ignore_filename(…)` — or simpler, build
a `GlobMatcher` from the `file_glob` parameter and filter entries manually
after walking.

Binary files: `std::fs::read_to_string` returns an error for files with
invalid UTF-8; silently skip those entries (matches rg's default behaviour).

The `run_subprocess`, `has_command`, and the entire `else` / fallback block
disappear. `SubprocOutput` stays in `fetch_url.rs` where it is still used for
the bash postprocess call.

Expected: ≈ 160 lines; all existing `grep_files_*` tests pass; Group A
mutants (all 5), Group C `has_command`/`run_subprocess` mutants (2 of 3) and
the `grep_files.rs:44:26` Group B mutant all disappear.

### Post-rewrite mutant projection

After Phase 1d.0d the expected surviving count is **≤ 7**:

| Mutant | Group | Status after 1d.0d |
|---|---|---|
| `grep_files` fallback ×4 | A | ✅ Gone (code deleted) |
| `find_files` fallback ×1 | A | ✅ Gone (code deleted) |
| `depth + 1 → *` | B | Remains (equivalent) |
| `context_lines > 0 → >=` | B | ✅ Gone (rg flag gone) |
| `has_command → true` | B | ✅ Gone (function gone) |
| `fd exit-code 1` | B | ✅ Gone (no subprocess) |
| `has_command → false` | C | ✅ Gone (function gone) |
| `run_subprocess delete -` | C | ✅ Gone (mostly; fetch_url still uses it) |
| `wait_for_output >= → <` | C | Remains (timing) |
| `web_search` ×4 | D | Remains (API key) |

That leaves `depth + 1 → *`, `wait_for_output`, and the 4 web_search mutants
— all documented and accepted.

### Done when

- `cargo mutants -p omega-tools` shows ≤ 7 missed (down from 16), with the
  delta explained entirely by deleted code.
- All existing integration tests (`cargo test -p omega-tools`) pass unchanged.
- `grep_files` output format is validated by running the existing
  `grep_files_context_lines_show_surrounding` and `grep_files_glob_filter`
  tests — no new tests should be necessary.
- `has_command` and the `run_subprocess` helper are removed from
  `grep_files.rs` / `find_files.rs`. `run_subprocess` and `SubprocOutput`
  remain in `fetch_url.rs` only.
- `just rust-gate` passes.

### Session setup — 1d.0d

**Model:** `claude-sonnet-4-6` — **Effort:** Medium

The plan is fully specified above. The main risk is getting the grep output
format exactly right — but the existing tests (`grep_files_context_lines_show_surrounding`,
`grep_files_finds_match`, `grep_files_glob_filter`, `grep_files_max_results_truncation`)
lock it down completely. Sonnet with medium effort is sufficient; escalate to
`claude-opus-4-7` only if the context-lines implementation proves subtler than
expected.

**Prompt:**

> Read `/home/carsten/omega/dev/rust-migration.md` — specifically the
> "Phase 1d.0d — Eliminate external binary dependencies" section — and
> execute it. Rewrite `find_files.rs` first (simpler), then `grep_files.rs`,
> keeping `just rust-gate` green throughout. After both rewrites, run
> `cargo mutants -p omega-tools` and confirm the Group A mutants are gone.

---

## Phase 1d.1 — `omega-agent` advanced features ⬜ Upcoming

Add to the `omega-agent` crate built in Phase 1d.0:

- **`setModel()` / `setEffort()`** — emit + persist `model_changed` / `effort_changed`.
- **Pause/continue/abort** — `requestPause()`, `requestContinue()`, `abort()`,
  the seam logic, `turn_paused` / `turn_continued` events.
- **Session resumption** — `performResumption()`, `seedWithResumptionSummary()`,
  `extractResumptionBasis()` (port `src/session-resume.ts`).
- **Server-side compaction** — handle `Compacted` stop reason; emit `compacted`
  event; clear/reset history.

Session prompt will be written after Phase 1d.0d is complete.

---

## Phase 1e — `omega-server` (WebSocket + HTTP) ⬜ Upcoming

Ports `src/web/server.ts` to a Rust binary crate (`axum`). Session creation,
listing, resumption; WebSocket fan-out; history replay on reconnect; static
file serving (TS bundle during Phase 1–2; Leptos WASM in Phase 3).

---

## Phase 1f — Bridge (`ts-rs`) ⬜ Upcoming

`#[derive(ts_rs::TS)]` on all `omega-protocol` types. Committed `.d.ts`
bindings so the TS web client stays type-checked against the Rust protocol.
Deleted entirely in Phase 3.

---

## Phase 2 — Rust as primary driver ⬜ Future

Rust `omega-server` binary replaces `src/cli.ts` + `src/web/server.ts`.
TS web client still served; all new features in Rust.

---

## Phase 3 — Leptos UI rewrite ⬜ Future

`omega-web` crate. Port `src/web/client/` component by component. Imports
types from `omega-protocol` directly. Once complete: delete `src/`, `ts-rs`
derives, `node_modules`.

---

## Phase 4 — `chromiumoxide` + LLM oracle ⬜ Future

Replace Playwright with `chromiumoxide`. LLM-as-oracle for snapshot review.
Delete `package.json`, `node_modules`, Playwright config.

---

## Settled decisions — format and compatibility

**No backward compatibility with old `events.jsonl` files.** Honest types;
no `#[serde(default)]` shims; no legacy field remapping. Old logs are not
supported by the Rust reader.

**No defaults baked into data shapes.** The `cargo mutants` finding on
`default_effort()` is the canonical example — a serde default is untestable
by design.

---

## What is intentionally deferred

All of the following are post-parity improvements. Do not implement during port:

- Redesigned session resumption UX
- Streaming context compaction (server-side)
- OpenAI provider
- `cargo mutants` integration into CI
- `insta` snapshot tests for rendered Leptos components
- Rate-limit backpressure to UI
