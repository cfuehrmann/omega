# TEST-ARCH — Test architecture & web-surface honesty

**Owner:** open
**Status:** 🔴 Top priority
**Pre-Leptos work; some sub-items deferred until after Phase 3.**

This is the umbrella plan for bringing every test surface in Omega onto a
single, honest pattern: **test through the outermost user-visible surface of
each binary; fake only what we can't run for real (Anthropic); let coverage of
internal modules flow down from the e2e tier**.

The current state has three different patterns covering three slices of the
codebase, an unjustified asymmetry between CLI and server, and a known
mutation-coverage gap on `omega-cli`. This document defines the target and the
ordered steps to get there.

---

## Why now (and why not later)

The TS web client is frozen pending the Leptos rewrite (Phase 3 of
`rust-migration.md`). Before Leptos lands we want:

1. The CLI test pattern established and validated. The CLI surface won't
   change with Leptos, so any tests written now survive the rewrite.
2. A server-side Rust-level WS-protocol test layer in place, so post-Leptos
   we already have a fast non-browser path for `omega-server`.
3. The `omega-mock-server` ↔ `wiremock-style HTTP fake` decision made and
   migrated, so post-Leptos we don't carry two LLM-fake patterns forward.

What we explicitly *don't* do before Leptos: invest in tightening Playwright
mutation coverage of `omega-server`. The current TS web UI is going away;
mutation-tightening assertions in tests that disappear is wasted effort. After
Leptos, the bulk of `omega-server` mutation kill rate will come from cheap
Rust HTML-snapshot tests, and the work is justified.

---

## Target architecture

```mermaid
flowchart LR
    subgraph TestsCLI["omega-cli/tests/cli.rs"]
        T1["assert_cmd → real omega binary<br/>snapshot: stdout+stderr+exit<br/>+ written session files"]
    end

    subgraph TestsWS["omega-server/tests/ws.rs (new)"]
        T2["raw WebSocket client<br/>→ real omega-server subprocess<br/>snapshot: WS frames"]
    end

    subgraph TestsUI["e2e/*.spec.ts (trimmed post-Leptos)"]
        T3["Playwright → real browser<br/>→ real omega-server<br/>snapshot: rendered DOM"]
    end

    subgraph TestsLeptos["leptos/tests/snapshots.rs (post-Leptos)"]
        T4["render_to_string<br/>component + scripted state<br/>snapshot: HTML"]
    end

    subgraph Bins["Production binaries"]
        CLI["omega-cli<br/>(stdout/stderr/exit)"]
        SRV["omega-server + web client<br/>(WS frames + Leptos)"]
    end

    AGENT["omega-agent + omega-store<br/>(coverage flows down — orchestration)"]
    CORE["omega-core::AnthropicProvider<br/>(parser — leaf, owns its own tests)"]
    TOOLS["omega-tools<br/>(per-tool I/O — leaf, owns its own tests)"]
    WM["axum SSE fake / wiremock<br/>(scriptable per test,<br/>real HTTP on a random port)"]

    T1 --> CLI
    T2 --> SRV
    T3 --> SRV
    T4 -.->|render only| AGENT
    CLI --> AGENT
    SRV --> AGENT
    AGENT --> CORE
    AGENT --> TOOLS
    CLI -.->|ANTHROPIC_BASE_URL| WM
    SRV -.->|ANTHROPIC_BASE_URL| WM
```

### Principles

1. **Each binary is tested through its outermost user-visible surface.**
   - `omega-cli` → stdout, stderr, exit code, files written under `--session-root`.
   - `omega-server` → WebSocket frames in/out, plus (post-Leptos) rendered HTML.
2. **One LLM mocking boundary, ever.** All fakes plug into
   `omega_core::AnthropicProvider::with_base_url`, either directly or via the
   `ANTHROPIC_BASE_URL` env-var hook in each binary's `main.rs`. No parallel
   `Provider`-trait injection, no `Agent`-level seams, no `RetryingProvider`
   swap-outs. See "LLM mocking: one boundary, two fixture libs" below for
   why two libraries are deliberate.
3. **Coverage of orchestration modules flows down from the e2e tier.** A
   surviving mutant in `omega-agent::send_message` after both e2e suites have
   run is a dead-code signal, not a missing test.
4. **Leaf utilities still own their own unit tests.** Two carve-outs:
   - `omega-core::AnthropicProvider`'s SSE parser — many edge cases (malformed
     deltas, missing fields, truncated streams, retry-after parsing) that you'd
     never reach by scripting an LLM scenario. Deserves dedicated unit tests
     against wiremock or hand-fed byte streams.
   - `omega-tools` per-tool I/O — each tool's input domain (path traversal,
     glob edges, command timeouts, large output truncation) has its own
     surface. Driving every branch via "tell the LLM to call this tool with
     this exact input" is brittle and noisy. Per-tool tests against `tempdir`
     and real subprocesses stay.
5. **Real storage in tests, isolated from production.** Tests use `TempDir` or
   `--session-root` overrides; never write into production `.omega/sessions/`.
   The existing gate session-pollution check enforces this.

---

## LLM mocking: one boundary, two fixture libs

This section records the conclusion of a recurring design discussion so it
doesn't have to happen again.

### Rules

1. **One mocking boundary, ever.** Every fake LLM in this repo plugs into
   `omega_core::AnthropicProvider::with_base_url`, either directly or via
   the `ANTHROPIC_BASE_URL` env-var hook in each binary's `main.rs`. No
   parallel `Provider`-trait injection, no `Agent`-level seams, no
   `RetryingProvider` swap-outs. (TEST-ARCH-3 finished enforcing this.)
2. **Two fixture *implementations* behind that boundary, by deliberate
   choice:**
   - `omega-core/tests/{anthropic,ollama,retry}.rs` use **`wiremock`**.
     Their job is leaf-parser / retry-policy testing: "given this exact
     wire response, does the parser do the right thing?". Declarative
     `(matcher, response)` is the natural shape; `wiremock`'s
     `up_to_n_times` covers the retry-sequencing case cleanly.
   - All binary-level e2e tests (`omega-cli`, `omega-server`,
     `omega-mock-server`) use the **`omega-test-fixtures`** axum SSE
     fake. Their job needs FIFO queue semantics and streaming responses
     with timed deltas (the `LONG_STREAM_TEST` case), which `wiremock`
     cannot do without contortion.
3. **No more than one copy of each fixture.** The axum fake lives in
   `omega-test-fixtures` and is consumed by re-export from each test
   crate's `common/mod.rs`. Forked copies are a regression — fix on sight.

### Why not unify on one library?

Neither library can comfortably take over the other's territory:

- `wiremock` cannot stream timed SSE chunks. Hard requirement for
  `LONG_STREAM_TEST` and any other pause-during-stream scenario.
- The axum fake *can* do everything `wiremock` does, but at ~3× the
  boilerplate per leaf-parser test (~30 tests in `omega-core/tests/` would
  need rewriting). Stylistic uniformity isn't worth the churn.

If this calculus changes (e.g. `wiremock` gains streaming support, or the
leaf-parser tests start needing axum-only features), revisit. Until then:
keep both, document the split, stop discussing it.

---

## Status quo vs. target

| Surface | Status quo | Target |
|---|---|---|
| `omega-protocol` types | unit tests | unchanged |
| `omega-core::AnthropicProvider` parser | unit tests | unchanged (leaf carve-out) |
| `omega-store` I/O | unit tests | unchanged |
| `omega-tools` per-tool | integration tests with `tempdir` | unchanged (leaf carve-out) |
| `omega-agent` Agent loop | dedicated MockProvider tests in `omega-agent/tests/` | retired — coverage flows down from CLI + server e2e |
| `omega-cli` binary | subprocess + HTTP fake via `ANTHROPIC_BASE_URL` | unchanged |
| `omega-server` binary (Rust-side) | subprocess + raw-WS client + same HTTP fake | unchanged |
| `omega-server` binary (browser-side) | Playwright via `omega-mock-server`, hosting an internal SSE fake + real `AnthropicProvider` | trimmed Playwright suite + Leptos HTML snapshots (post-Phase 3) |
| `omega-mock-server` | thin Playwright wrapper: real `omega-server` + internal SSE fake on `127.0.0.1:0` + control HTTP API on `:3004` | unchanged |
| LLM HTTP fake implementation | single `omega-test-fixtures` workspace dev-helper crate | unchanged |

---

## Steps, in order

### TEST-ARCH-1 — `omega-cli` e2e via subprocess + HTTP fake (BUG-C)

**Status:** ✅ **Done.** 17 caught, 0 missed.

`ANTHROPIC_BASE_URL` + `OMEGA_RETRY_INITIAL_MS` env hooks; axum SSE fake on a
random port; six tests in `omega-cli/tests/cli.rs` (`--help`, missing key,
happy turn, tool-use round trip, retry exhaustion, stderr snapshot).
Dev-deps: `assert_cmd`, `insta`, `tempfile`, `axum`.

---

### TEST-ARCH-2 — `omega-server` Rust-level WS-protocol tests

**Status:** ✅ **Done.** 67 caught, 1 missed (documented equivalent:
`Message::Close` arm — deletion falls through to identical behaviour via
the next `reader.next()` returning `None`).

`ANTHROPIC_BASE_URL` hook in `omega-server/src/main.rs`; 16 in-process WS
tests + one subprocess e2e test; insta snapshots for key frames with
`time`/`dir`/`cwd` redacted. Two production bugs fixed: **BUG-S1** (ABBA
deadlock in `send_session_info_and_history`); **BUG-S2**
(`session_info.turnState` stayed `"idle"` during resumption).

---

### TEST-ARCH-3 — Retire / repurpose `omega-mock-server`

**Status:** ✅ **Done.** Outcome: option B (repurpose as a thin Playwright
wrapper around the HTTP fake), plus the workspace-wide axum-fake
de-duplication that was the second half of this task.

What shipped:

- `omega-mock-server` no longer injects a `Provider` trait. It hosts the
  production [`omega_server::serve`] driven by a real
  [`omega_core::AnthropicProvider`] whose base URL points at an internal
  Anthropic-shaped SSE fake on a random `127.0.0.1` port. The full HTTP/SSE
  code path (request serialisation, `reqwest`, SSE parser) now runs under
  every Playwright test.
- The Playwright control surface keeps its 3004 port: `POST /control/script`
  loads a per-test queue of `MockResponse`s, `GET /control/llm-calls`
  returns captured requests, `POST /control/reset-calls` clears the
  history. TS-side helper at `e2e/fixtures/real-server-control.ts`.
- New workspace dev-helper crate **`omega-test-fixtures`** — single
  source of the LLM HTTP fake. The previously forked copies in
  `omega-cli/tests/common/`, `omega-server/tests/common/`, and
  `omega-mock-server/src/fake.rs` (847 lines combined) collapsed into one
  ~530-line crate consumed by all three call-sites.

**Success criterion (met):**
`grep -r "MockProvider" rust/crates/omega-mock-server/` returns no results;
all 116 Playwright browser tests (`just test-browser`) pass; `just rust-gate`
passes.

---

### TEST-ARCH-4 — Retire `omega-agent/tests/` MockProvider suite

**Status:** ⬜ Ready (TEST-ARCH-1 + TEST-ARCH-2 done).

The six existing `omega-agent/tests/*.rs` files date from Phase 1d.0a, when
the agent loop had no downstream e2e coverage. Once TEST-ARCH-1 and
TEST-ARCH-2 are in place, those scenarios are covered transitively by CLI
and/or server e2e tests, and the in-crate suite becomes:

- Double-counted coverage (mutants killed twice).
- A coupling layer between tests and `omega-agent`'s internal types
  (Agent struct, send_message signature, AgentItem).
- A reason mutation runs scoped per-crate look healthier than the system
  actually is.

Retire after verifying the equivalent scenarios are present in TEST-ARCH-1 /
TEST-ARCH-2.

**Success criterion:** `omega-agent/tests/` is empty (or contains only
genuinely agent-internal pure-function tests, e.g. dangling-tool-use repair
that's awkward to provoke through a real LLM script). Mutation run on
`omega-agent` either passes via downstream coverage, or surviving mutants are
explicitly accepted as dead code.

---

### TEST-ARCH-5 — Leptos HTML snapshot tests *(post-Phase 3)*

**Status:** ⬜ blocked on Leptos rewrite landing.

When the Leptos UI ships, add a fast Rust test layer:

- For each component, construct a reactive state via the same event-sequence
  scripts the WS protocol tests use.
- Render via `leptos::ssr::render_to_string` (or component-level testing
  utilities).
- Snapshot the HTML with `insta`.

This is the cheap bulk of post-Leptos UI testing. Expected to replace ~80% of
the current Playwright surface area. Keep Playwright for genuinely
browser-only concerns: keyboard navigation, focus, scroll behaviour,
reconnection UX, mobile layout, hydration mismatches.

---

### TEST-ARCH-6 — Drive `rust-mutants-server` to zero-missed *(post-Phase 3)*

**Status:** ⬜ blocked on TEST-ARCH-5.

With the bulk of UI coverage now in fast Rust tests (TEST-ARCH-5), running
mutation testing on `omega-server` is finally cheap. Drive it to the same
zero-missed bar as `omega-tools` and (per TEST-ARCH-1) `omega-cli`.

---

## Cross-references

- `rust-migration.md` — BUG-C is the same work as TEST-ARCH-1; the Phase-3
  Leptos rewrite gates TEST-ARCH-5 and TEST-ARCH-6.
- `rust/PHASE-1d.0-NOTES.md` — Phase 1d.0a's MockProvider tests are the
  suite slated for retirement in TEST-ARCH-4.
- `nutriterm/tests/cli.rs`, `nutriterm/tests/common.rs` — reference pattern
  for TEST-ARCH-1's `assert_cmd` + `insta` + path-normalisation shape.
