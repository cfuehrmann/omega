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
| 1d.0d — eliminate external binary deps | ✅ Done | Replaced `rg`/`fd` subprocesses with `ignore`+`globset`+`regex`; refactored remaining boundaries (sentinels → `Option`, depth → `is_root`, extracted pure helpers); **0 missed** across `omega-tools` |
| 1d.1 — `omega-agent` advanced | 🟡 In progress | Pause/continue/abort, session resumption, compaction, model/effort switching (decomposed 1d.1a–e) |
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
  a specialised test harness. Became a non-issue after the Phase 1d.0d
  rewrite (no subprocess to kill); only the `fetch_url` postprocess call remains.
- **`has_command → false`** (forces fallback): With `has_command` always returning
  false, `grep_files` uses grep and `find_files` uses find. For every test pattern
  we use, grep/find produce output close enough to rg/fd that `contains()`
  assertions pass either way. Killing this would require format-specific
  assertions (e.g. checking for `--no-heading` in output) that couple tests to
  implementation details. Became a non-issue after Phase 1d.0d (functions deleted).
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

> **Forward note.** All 16 surviving mutants from Phase 1d.0c were eliminated
> by the time Phase 1d.0d closed: the 9 reachable-only-via-fallback mutants
> (Groups A + the rg/fd ones in B/C) by the `ignore`/`globset`/`regex` rewrite,
> and the remaining 7 (`fetch_url` sentinel, `list_files` depth, `wait_for_output`
> exit-branch, `web_search` ×4) by the small refactors documented below.

---

## Phase 1d.0d — Eliminate external binary dependencies (`omega-tools`) ✅

Replaced `rg`/`fd` subprocesses in `find_files.rs` and `grep_files.rs` with
pure-Rust implementations using `ignore::WalkBuilder` + `globset::Glob` +
`regex::RegexBuilder`. `has_command`, all fallback branches, and the
`run_subprocess` / `SubprocOutput` helpers were deleted from both tool files.
`SubprocOutput`/`run_subprocess` remain in `fetch_url.rs` for its bash
postprocess call.

Additional tests written to cover type-filter guards (`"f"`, `"d"`, `"l"`),
gap-separator `--` logic, 1-indexed line numbers, and `:`/`-` separator
characters in `grep_files` output.

One equivalent mutation (`!ft.is_file()` in `grep_files`) was isolated into
`not_a_regular_file()` and suppressed with `#[mutants::skip]` + comment.

**Final: 0 missed** across these four files (61 mutants, 59 caught, 2
unviable). The 7 survivors recorded earlier were eliminated by small
refactors that turned each "untestable" boundary into a directly testable
seam:

| Original survivor | File | Refactor |
|---|---|---|
| `delete -` in `run_subprocess` | `fetch_url.rs` | Replaced the `unwrap_or(-1)` sentinel with `code: Option<i32>` (`None` = killed by signal). Signal kill is now reported as `[killed by signal]`; integration test invokes a postprocess that signal-kills its bash shell with `kill -KILL` on its own PID (literal `$`, written as `concat!("kill -KILL $", "$")` in the test source so the dollar pair survives JSON/markdown round-trips). |
| `replace + with *` in `walk_sync` | `list_files.rs` | Replaced the `depth: usize` counter with `is_root: bool`. The dotfile filter still hides `.foo` only at the top level of a non-recursive listing, but the arithmetic mutant has nothing to mutate. |
| `replace >= with <` in `execute` (post-exit `min_bytes_reached`) | `wait_for_output.rs` | Hoisted the in-loop and post-exit predicates into a single `evaluate(content, pattern, min_bytes) -> (bool, bool)` helper, with direct unit tests pinning the `len() >= min` boundary at exact equality and one byte below. |
| `delete !` + 3 truncation mutants in `execute` | `web_search.rs` | Extracted `check_status(StatusCode) -> Result<(), String>` and `render_results(&Value) -> String`. Pure unit tests on synthetic JSON pin both the 2xx/non-2xx branch and the `> MAX_OUTPUT_CHARS` truncation boundary at exact equality and one above. No live API key, no mock HTTP server. |

---

## Phase 1d.1 — `omega-agent` advanced features ⬜ In progress

Add to the `omega-agent` crate built in Phase 1d.0. Decomposed into five
sub-phases, ordered smallest-surface → highest-concurrency. Each sub-phase
closes with `cargo mutants -f <touched files>` at **0 missed**, holding the
bar set by 1d.0d.

| Sub-phase | Deliverable | Status |
|---|---|---|
| 1d.1a | `set_model` / `set_effort` + `active_model` / `active_effort` state + `extract_last_model_and_effort` pure helper | ✅ Done |
| 1d.1b | Session-resumption **pure** helpers: `extract_resumption_basis`, `extract_summary_from_response`, `extract_description_from_response` | ✅ Done |
| 1d.1c | `perform_resumption` + `seed_with_resumption_summary` on `Agent` (one-shot LLM call + history seeding) | ✅ Done |
| 1d.1d | Server-side compaction — `omega-core` provider detects compaction content-block; agent clears `history` + `context_hashes` on `Compacted` event | ✅ Done |
| 1d.1e | Pause / continue / abort + the seam — `request_pause` / `request_continue` / `request_abort`; seam fires only after a tool batch's `tool_results` are appended; emits `pause_requested` / `turn_paused` / `turn_continued{mode}` | ⬜ |

### Order rationale

- **a → b** are pure helpers, easy to mutation-test exhaustively, land fast.
- **c** depends on (a) — resumption needs the active-model/effort fields.
- **d** is independent of (a)–(c) but is the only cross-crate sub-phase
  (touches `omega-core`); doing it before pause keeps the provider/agent
  contract honest before pause control layers on top.
- **e** lands last — the seam is the riskiest mutation-testing target;
  doing it after the rest of the loop is settled means nothing is moving
  under it.

### Test seam strategy

- Pure helpers get inline `#[cfg(test)]` unit tests pinning each branch.
- Agent-method behaviour uses the existing `MockProvider` scaffolding,
  extended where needed (e.g. a `BlockingProvider` for the pause seam test
  that holds the LLM stream open on a `tokio::sync::Notify` until the test
  releases it). No real time-based synchronisation in tests.

### Progress notes

- **1d.1a** (commit `2d5db0c`) — added `Agent::set_model` / `set_effort` /
  `active_model()` / `active_effort()`, plus `pub const DEFAULT_EFFORT =
  "medium"`. `send_message` reads `active_model` (was `config.model`) so
  switches take effect from the next turn.  Effort is stored but **not yet
  threaded onto `LlmRequest`** — that is provider-shape work owned by
  `omega-core` and remains deferred. New `omega_agent::session_resume`
  module hosts `extract_last_model_and_effort` (left-to-right scan,
  latest-wins) with seven inline unit tests. Nine integration tests pin
  field mutation, persistence, defaults, key independence, and that the
  next `send_message` sends the new model on the wire (captured via
  `MockProvider::take_requests`). Persistence tests now assert the
  RFC3339-with-`Z` shape on `time`, killing pre-existing `now_iso`
  mutants. `cargo mutants -f` on `agent.rs` and `session_resume.rs`:
  **26 mutants, 0 missed** (7 unviable).

- **1d.1b** (commit `ba9396d`) — added three public pure helpers to
  `omega_agent::session_resume`: `extract_resumption_basis` (groups events
  into turns, pairs tool calls with results by ID, renders carry-forward
  context from the last `session_resumed` event), `extract_summary_from_response`
  (parses `<summary>…</summary>`, falls back to trimmed full text), and
  `extract_description_from_response` (parses `<description>…</description>`,
  hard-capped at 120 chars, `None` when absent). Supporting private helpers:
  `first_meaningful_line`, `primary_tool_arg` (port of `primaryToolArg` from
  `tools.schema.ts`), `group_into_turns`, `project_turn`, `extract_block`,
  and `slice_start_after`. One equivalent mutation (`i + 1 → i * 1` in the
  post-`session_resumed` slice-start calculation) suppressed with
  `#[mutants::skip]` — `session_resumed` events are transparent to
  `group_into_turns` so including vs. excluding the event from the slice
  produces identical output. `cargo mutants -f session_resume.rs`:
  **57 mutants, 55 caught, 2 unviable, 0 missed**.

- **1d.1c** (commit `0e2493d`) — added two methods on `Agent` that close
  the resumption loop. `seed_with_resumption_summary(summary, resumed_from)`
  emits a `SessionResumed` event then injects a synthetic user (canned
  preamble + summary) and assistant (canned ack) message pair into both
  in-memory `history` and the persistent context store, preserving the
  user/assistant alternation Anthropic expects. `perform_resumption(basis,
  resumed_from, name, cancel)` makes a one-shot LLM call (hard-coded
  `RESUMPTION_MODEL = "claude-sonnet-4-6"`, `system =
  RESUMPTION_SUMMARY_INSTRUCTIONS`, `messages = [{user, basis}]`, no
  tools, `cache_breakpoint_index = null`) and streams `ResumingSession
  → LlmCall → signals → LlmResponse → SessionResumed`. The basis
  user-record is appended to `context.jsonl` but **not** to in-memory
  `history` (matches TS `performResumption`). On terminal `LlmError` the
  stream yields `LlmError` and stops; on cancellation it stops cleanly
  without `TurnInterrupted` (resumption is not a user turn). The pure
  helpers ported in 1d.1b (`extract_summary_from_response`,
  `extract_description_from_response`, `extract_resumption_basis`)
  compose with the new methods. **Deferred:** `capEffortForModel` —
  effort isn't on `LlmRequest` yet (consistent with the 1d.1a deferral
  note); parity will be restored when `omega-core` wires effort. Tests:
  9 + 25, covering full event-order pinning, basis-only on the wire,
  `LlmCall` shape (Anthropic URL, resumption model, single context_hash,
  null cache breakpoint), `LlmResponse.context_hash` matching the on-disk
  assistant record, both `<summary>` and fallback-trim summary paths,
  thinking-block persistence, `LlmRetry` partial-buffer clearing,
  pre-cancellation leaving history untouched, and a subsequent
  `send_message` consuming the seeded pair. `cargo mutants -f`:
  `session_resume.rs` **57 mutants, 55 caught, 2 unviable, 0 missed**;
  `agent.rs` **20 mutants, 8 caught, 12 unviable, 0 missed** — the
  `Pin<Box<dyn Stream>>` return type makes most function-body mutants
  compile-fail (same coverage limitation as `send_message` from prior
  phases).

- **1d.1d** — server-side compaction wired end-to-end across
  `omega-core` and `omega-agent`. **omega-core** changes: `LlmRequest`
  gains optional `context_management: Option<serde_json::Value>` (opaque
  pass-through — the Anthropic edits-array shape evolves often, and
  this isn't persisted, so a typed struct would be premature
  ossification); `AnthropicRequestBody` plumbs the field with
  `skip_serializing_if = Option::is_none`; the SSE parser gains a
  `ContentBlockStart::Compaction` variant (silently consumed — no
  `BlockAccum` sink, the matching `compaction_delta` falls through the
  existing match-no-accum path), a `compaction_seen: bool` flag, a
  `usage_value: serde_json::Map` captured via a second JSON parse on
  `message_start` and merged on `message_delta` (preserves nested
  `iterations[]` arrays verbatim), plus `MessageDeltaContextMgmt`
  carrying `applied_edits[]` with an `AppliedEdit::ClearToolUses`
  variant that populates `cleared_tool_uses` /
  `cleared_input_tokens` on the resulting `LlmResponse`. On
  `message_stop`, if `compaction_seen` the parser yields
  `OmegaEvent::Compacted { time, usage }` strictly **before**
  `OmegaEvent::LlmResponse`. The agent does **not** yet set
  `context_management` on outgoing requests — that is owned by a
  later phase per the deferrals list. **omega-agent** changes: a
  new `OmegaEvent::Compacted` arm in the `send_message` drain loop
  clears both `history` and `context_hashes` (mirrors
  `src/agent.ts:1432–1453`), persists the event, and forwards it.
  The clear runs before the same turn's `LlmResponse` is processed,
  so the post-compaction history holds only the new assistant
  summary. Tests: 8 new omega-core integration tests in
  `tests/anthropic.rs` (request-shape emit/omit, ordered
  Compacted-then-LlmResponse, `iterations[]` round-trip, applied-edits
  match vs. ignore, no-Compacted on plain turns, RFC3339 time check),
  plus 5 new omega-agent tests in `tests/compaction.rs` (history+hash
  clearing with a follow-up turn that pins the cleared
  `context_hashes` via `LlmCall.contextHashes` length, `events.jsonl`
  usage round-trip, post-compaction wire payload via
  `MockProvider::take_requests`, stream order at the agent layer,
  control test for non-compacting turns). `cargo mutants -f`:
  `types.rs` **18 mutants, 14 caught, 4 unviable, 0 missed**;
  `anthropic.rs` **19 mutants, 12 caught, 7 unviable, 0 missed**;
  `agent.rs` **20 mutants, 8 caught, 12 unviable, 0 missed** — the
  hot-path branches inside `stream_impl` (`async_stream::try_stream!`
  macro) and `send_message` (`Pin<Box<dyn Stream>>` return) are not
  mutated by cargo-mutants and are covered by the integration tests
  instead, same constraint as 1d.1a/c.

### 1d.1d pre-flight notes

1d.1d is the first **cross-crate** sub-phase: it touches `omega-core`
(provider request shape + stream parser) and `omega-agent` (history
clearing on `Compacted` event). The protocol is already prepared —
`OmegaEvent::Compacted` and `LlmResponseEvent.cleared_tool_uses` /
`cleared_input_tokens` exist in `omega-protocol` since 1a.

**TS reference points:**

- `src/agent.ts:1432–1453` — compaction-block detection in `response.content`,
  history+hashes cleared, `Compacted` event emitted, **then** assistant
  response appended (so its hash points to the post-compaction record).
- `src/agent.ts:1455–1469` — `applied_edits` parsing for
  `clear_tool_uses_20250919`; populates `cleared_tool_uses` /
  `cleared_input_tokens` on the resulting `LlmResponse`.
- `src/config.ts` — `autoCompactThreshold = 750_000`,
  `COMPACTION_INSTRUCTIONS` (verbatim Anthropic-cookbook prompt).

**Request shape (omega-core):** add an optional `context_management`
field on `LlmRequest` carrying `edits[]` with the compaction trigger
(`input_tokens` threshold), `instructions` (= `COMPACTION_INSTRUCTIONS`),
and `keep` (`{ type: "tool_uses", value: 6 }`). `build_request_body`
emits it when present.

**Stream parser (omega-core::anthropic):**

- On `content_block_start` with `content_block.type == "compaction"`,
  set a `compaction_seen` flag (don't emit yet — we need ordering).
- On `message_delta`, parse `context_management.applied_edits[]`. The
  `clear_tool_uses_20250919` entry — when present — populates
  `cleared_tool_uses` and `cleared_input_tokens` on the `LlmResponseEvent`
  emitted at `message_stop`.
- On `message_stop`, if `compaction_seen`, yield `OmegaEvent::Compacted`
  **before** `OmegaEvent::LlmResponse`. The agent then handles ordering.

**Agent reaction (omega-agent):** in the `send_message` event-drain loop,
on `OmegaEvent::Compacted`: clear `history` + `context_hashes`, persist
the event, forward it to the caller. Do **not** clear after the
assistant message has been appended for this turn — same ordering as TS
(clear → emit `Compacted` → append assistant → emit `LlmResponse`).

**Test seam strategy:**

- `omega-core::anthropic::tests` — feed a synthetic SSE byte stream
  including a `content_block_start` with `type: compaction` and a
  `message_delta` carrying `applied_edits` with `clear_tool_uses_20250919`.
  Assert the provider yields `Compacted`, then `LlmResponse` with
  `cleared_tool_uses` / `cleared_input_tokens` populated.
- `omega-agent::tests` — `MockProvider` yields `Compacted` then
  `LlmResponse` for a turn. Assert: history cleared and re-populated with
  exactly the new assistant message; `context_hashes.len() == 1` after
  the turn; `Compacted` and `LlmResponse` both persisted to
  `events.jsonl` in that order; the next `send_message` sends only the
  post-compaction messages on the wire.
- Mutation bar: `cargo mutants -f` on every touched file, **0 missed**.

### 1d.1e pre-flight notes

omega-agent-only. Protocol is ready (`PauseRequestedEvent`,
`TurnPausedEvent`, `TurnContinuedEvent`, `ContinueMode::{Manual,Auto}`
shipped in 1a). No cross-crate change.

**TS reference points:**

- `src/agent.ts:505–587` — pause/continue/abort state fields and
  the three control methods.
- `src/agent.ts:1105–1140` — turn-entry pause-state reset and
  `turnAbort` plumbing (external signal forwarded into a fresh
  per-turn AbortController).
- `src/agent.ts:1765–1832` — the seam: fires only after
  `appendToHistory(tool_results)`; emits `turn_paused`, suspends on
  `pausedResolver` if `pendingContinue` is null, post-wake checks
  abort first, optionally appends interjection + emits `user_message`,
  then `turn_continued{mode}`.
- `src/agent.ts:1850–1869` — turn-exit `finally` clears
  pause-control state and fires the resolver if the generator
  unwinds while still suspended.

**Architectural choice — separate `ControlHandle`, not methods on
`Agent`.** TS puts `requestPause` / `requestContinue` / `requestAbort`
on the Agent class. In Rust, `Agent::send_message(&mut self, …)`
exclusively borrows the agent for the lifetime of the returned
`Pin<Box<dyn Stream>>`, so external code (server, tests) cannot call
any other `&self` method on the same `Agent` while a turn runs. The
clean translation is a cloneable `ControlHandle` (`Arc`-backed)
returned by `Agent::controls()` before the turn starts. The handle
carries the three control methods.

**Concurrency primitives:**

- `Arc<std::sync::Mutex<ControlState>>` for `pause_requested`,
  `pending_continue`, `suspended` — brief critical sections, no need
  for an async mutex.
- `Arc<tokio::sync::Notify>` for the seam suspend/wake. Reusable
  across multiple pause cycles in one turn (which TS allows). Lost-
  wakeup is handled by re-checking `pending_continue` under the
  state lock after waking. `oneshot` would force a fresh channel per
  cycle; `watch` is over-spec'd; a manual `Mutex<Option<Waker>>`
  reinvents what `Notify` already provides.
- `Arc<Mutex<CancellationToken>>` for the turn-scoped abort token.
  Replaced with a fresh token at every `send_message` entry;
  `request_abort` locks, clones, calls `cancel()`. The existing
  `cancel: CancellationToken` parameter on `send_message` is kept
  for explicit external cancellation and forwarded into the
  controls' turn-cancel via a `tokio::select!` task aborted by the
  turn-exit guard.

**State-reset strategy:** turn-entry clear at the top of the
`stream!` body, plus a `TurnGuard` RAII struct whose `Drop` impl
re-clears state and calls `notify.notify_one()` if `suspended` is
still true. Drop-guard is the only mechanism — `async_stream::stream!`
offers no `finally`-like hook — and it covers the caller-drops-stream-
mid-suspend path.

**Internal refactor:** `Agent.event_store: EventStore` becomes
`Arc<EventStore>` so `ControlHandle` can clone the Arc and log
`PauseRequested` directly. omega-agent-internal change only; public
constructor signature unchanged.

**Event-ordering invariants** (pinned by tests):

```
… ToolResult …
TurnPaused                          ← only after tool_results appended
UserMessage{content}                ← only if request_continue had content
TurnContinued{mode}                 ← mode=Manual if seam was suspended,
                                       Auto if request_continue beat the seam
LlmCall                             ← next call sees the interjection
```

`PauseRequested` is logged by `request_pause` itself and lands in
`events.jsonl` even when no turn is running. Abort during pause
yields `TurnPaused → TurnInterrupted{aborted}` and stops.

**Test seam:** integration tests use the existing `MockProvider`
plus a new `BlockingProvider` (in `tests/common/mod.rs`) that holds
the stream parked on a `Notify` between transcripts — needed for
multi-cycle-in-one-turn and drop-guard tests. The basic pause/
continue/abort tests just call `request_pause` before polling the
stream (deterministic since async streams advance only under poll).

**Branches `cargo mutants -f` cannot reach** (same `Pin<Box<dyn Stream>>`
limitation as 1d.1a/c/d — covered by integration tests):

- `pause_requested` reset at seam entry
- `TurnPaused` emission
- `pending_continue.is_none()` suspend-or-skip
- post-wake `cancel.is_cancelled()` abort branch
- interjection `is_some()` → `UserMessage` emission
- `mode` selection (Manual when suspended, Auto when not)
- `TurnContinued` emission
- turn-entry state-reset and `TurnGuard::drop` cleanup

Mutants in `controls.rs` (the three methods, the handle, `TurnGuard`)
*are* visited by cargo-mutants and covered directly.

**Test plan:** 14 integration tests covering manual/auto continue
with and without interjection (×4), abort-during-pause, idempotent
`request_pause` (already-pending and while-suspended), no-turn-
running logging, state-reset between turns, drop-guard cleanup,
full events.jsonl ordering, multiple pause cycles in one turn,
abort without prior pause, and `request_continue` without prior
pause as inert.

**Mutation bar:** `cargo mutants -f` on `controls.rs` and `agent.rs`,
**0 missed** (modulo the unviable stream-body branches above).

### Explicit deferrals (not part of 1d.1)

The TS agent has three further features that are intentionally **out of
scope** for this phase. Reopen if any turn out to be wrong calls:

- `max_tokens` thinking-budget no-output recovery and `max_tokens`
  mid-tool-call recovery (the `maxTokensRecoveries` counter).
- The `activeGeneration` superseded-generator guard — irrelevant until a
  multi-WS server (1e) holds the agent.
- Anthropic prompt-cache breakpoints / `context_management` request shape —
  those are LLM-request-shape concerns owned by `omega-core`'s
  `AnthropicProvider`, not this phase.

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
