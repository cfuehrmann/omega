# Omega — Rust Migration

*Living document. Completed phases are summarised briefly; upcoming phases have full detail.*

---

## Status

| Phase | Status | Deliverable |
|---|---|---|
| 0 — Planning | ✅ Done | This document + architectural decisions |
| 1a — `omega-protocol` | ✅ Done | `rust/crates/omega-protocol`: all 22 `OmegaEvent` variants, `StreamSignal`, serde round-trips; workspace tooling: edition 2024, `clippy::pedantic -D warnings`, `cargo-machete`, `cargo mutants`; honest types (no `#[serde(default)]` shims) |
| 1b — `omega-core` (LLM loop) | ✅ Done | Anthropic + Ollama providers, retry loop, streaming; 0 surviving mutants |
| 1b.7 — Insta snapshot coverage | 🔜 Next | Wire-format reference snapshot, kitchen-sink request-body snapshots, id-redactor utility |
| 1c — `omega-server` (WebSocket) | ⬜ Upcoming | tokio-tungstenite server, session dir, event store |
| 1d — Bridge (`ts-rs`) | ⬜ Upcoming | Generate `.d.ts` from Rust types, TS UI stays type-checked |
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
│       ├── omega-core/         🔜 next
│       └── omega-server/       ⬜ upcoming
├── src/                        ← TypeScript (frozen; no new features)
├── Justfile                    ← just rust-gate for Rust-only commits
└── package.json
```

The `src/` directory is TypeScript only. The `rust/` directory is Rust only. No mixing.

The pre-commit hook routes automatically:
- All staged files under `rust/` → `just rust-gate` (cargo fmt + clippy + test, ~5 s)
- Any non-Rust code staged → full TS gate (typecheck + bun test + playwright + knip)

---

## Architectural decisions (settled — do not re-litigate)

**All-in Rust including Leptos web client.** Cross-language type friction at the WebSocket boundary is worse than either pure choice. Rust agent + TS web client gives the worst of both worlds.

**Leptos over Dioxus/Yew/Sycamore.** Leptos uses fine-grained reactivity identical to SolidJS. Component migration is syntax translation, not paradigm shift.

**`omega-protocol` as keystone.** A shared crate with `#[derive(Serialize, Deserialize)]` types breaks compilation in all consumers when a variant is missing — enforces contract discipline that `events.schema.ts` required manually.

**Two providers from day one.** Building Anthropic + Ollama simultaneously forces a real provider abstraction. Retrofitting on day 90 is much more expensive.

**`ts-rs` bridge during Phase 1.** Generates `.d.ts` from Rust structs so the TS web UI stays type-checked against the Rust protocol. Deleted when UI migrates to Leptos.

**Don't redesign during port.** Success criterion is parity, not improvement. All ideas go in a deferred file. Mixing redesign with migration dilutes the parity test.

**Separate sessions for snapshot review.** Coding session and review session must be independent agents. Within-session "blind" prompts are insufficient — the LLM is anchored on prior history. Separate session breaks priming cleanly.

---

## Phase 1b — `omega-core` ✅ Done

`rust/crates/omega-core` complete: `Provider` trait, `AnthropicProvider`
(SSE), `OllamaProvider` (NDJSON), and a generic `RetryingProvider<P>` retry
wrapper that honours `Retry-After` and emits `OmegaEvent::LlmRetry` with
text/thinking fragments. Both providers built simultaneously to force a real
abstraction. All tests wiremock-fronted; no live API calls. **0 surviving
mutants.**

**Sub-phases:**

- **1b.0** (commit `22a8f17`) — initial implementation. 17 omega-core tests
  + 17 omega-protocol tests passing.
- **1b.5** (commit `96620be`) — mutation tested with `cargo mutants`.
  Killed 30 newly-discovered mutants by adding `LlmError::body()` unit
  tests, retry-policy tests, and Ollama request-body tests. One documented
  skip in `compute_backoff` (`replace * with /`): `x/f ≈ x*(1/f)` for
  `f∈[0.9,1.1]`, so the mutant is statistically indistinguishable from the
  original under RNG. Skip config in `rust/.cargo/mutants.toml`.
- **1b.6** — replaced internal `ScriptedProvider` with e2e tests through
  real providers + wiremock + a flaky-TCP listener. Deleted
  `ScriptedProvider`, `dummy_request`, `http_*` helpers,
  `RetryConfig::for_tests`, and the entire `#[cfg(test)] mod tests` block in
  `src/retry.rs` (~450 lines). New `tests/retry.rs` (14 tests) and
  `tests/common/mod.rs` (helpers). Two cargo-mutants timeouts (both
  infinite-retry mutations the auto-timeout catches) treated as caught.

**Implementation adjustments worth carrying forward:**

- `AgentItem::Event` boxes `OmegaEvent` (large_enum_variant). Construct
  with `AgentItem::event(ev)` or `.into()`.
- `Provider::stream` returns `BoxStream<'static, Result<AgentItem, LlmError>>`
  (alias `AgentItemStream`) — ergonomic with trait-object composition.
- `context_hash` on emitted `LlmResponse`/`ToolCall` events left empty;
  Phase 1c persistence layer fills it in.
- `LlmError::Transport` IS reachable through both real providers when
  `reqwest::Client::send().await` fails (connection-refused, mid-stream
  TCP close). Reproduced in `retries_transport_errors` via an in-process
  flaky-listener Tokio task that drops the first connection and serves a
  hand-rolled HTTP/1.1 SSE response (`Connection: close`) on the second.
- An HTTP-status retry (e.g. 529) cannot stream text before failing — the
  failure happens at response-status time. To exercise `text_fragment`
  population on retry, emit an Anthropic SSE `event: error` with an
  `overloaded_error` payload *after* a couple of text deltas; the
  provider lifts that to `LlmError::Stream { message }` whose message
  contains `"overloaded_error"`, which `is_retryable` recognises.
- Sequential wiremock responses: mount multiple `Mock`s with
  `.up_to_n_times(N)` in registration order. wiremock matches the first
  un-exhausted mock — no stateful response builder needed.

---

## Phase 1b.7 — Insta snapshot coverage for the wire-format contract 🔜 Next

### Motivation

`insta` is currently used in only two places — happy-path streaming
snapshots in `tests/anthropic.rs` and `tests/ollama.rs`. Several
high-value snapshot opportunities are unrealised:

1. **No catalogue of what each `OmegaEvent` variant looks like on disk.**
   Per-variant unit tests in `omega-protocol/src/events.rs` pin serde
   rules (snake_case `type` discriminator, camelCase fields, `None` ↦
   absent), but no single artefact says "this is the wire format of every
   event." When persistence shape changes, no diff surfaces it.
2. **No request-body snapshot for either provider.** Existing
   `request_body_*` tests spot-check individual fields against specific
   mutants. Useful for mutant pinning, but no canonical "this `LlmRequest`
   becomes this wire body" reference exists.
3. **ID propagation across events is not visible in any snapshot.** Tool
   call IDs flow through `tool_call → tool_result → llm_response` events;
   nothing yet asserts they stay correlated when wired through real code.

### Two principles to apply

- **Equivalence via numbered placeholders.** A stateful redactor that
  emits `[id_1]`, `[id_2]`, … makes "the same id" visible in the snapshot
  text itself. If a future bug routes a different id through, the
  placeholder number changes and the diff surfaces it. (For Omega this
  matters most once events flow through the agent loop with random or
  server-generated ids — Phase 1c+. The redactor utility is built now so
  it's ready when needed.)
- **Include input in the snapshot when the snapshot exists to assert a
  *transformation*.** Without input, a snapshot reads as "X came out of
  *something*" — reviewers must flip to the test source to make sense of
  a diff. With input alongside, the snapshot is self-explanatory. *Don't*
  include input when the snapshot is itself the catalogue (e.g. the
  all-variants reference) — that just duplicates the test source.

### Tasks

#### Step 1 — `id_redactor()` helper in `tests/common/`

Small utility that returns a closure suitable for
`insta::dynamic_redaction(|value, path| …)`. Internally maintains a
`HashMap<String, usize>` mapping each unique value to a stable
placeholder `[id_<n>]`. State scoped per call — constructed fresh per
test so two snapshots don't share a numbering space.

Decide where it lives once you see the consumers: probably
`omega-core/tests/common/mod.rs` (which already exists) and a sibling in
`omega-protocol/tests/common/mod.rs` (new). Don't put the helper in
production code — it is test infrastructure only.

#### Step 2 — All-22-variants `OmegaEvent` reference snapshot

In `omega-protocol`, add an integration test (`tests/events_reference.rs`)
that builds a `Vec<OmegaEvent>` containing one example of every variant.
Include a deliberately correlated triple: a `ToolCallEvent` and matching
`ToolResultEvent` sharing one id, plus an `LlmResponseEvent` whose
`cleared_tool_uses` (when populated) references the same id. Apply
`id_redactor` to id-bearing paths. Use `insta::assert_json_snapshot!`.

This becomes the living "wire format reference" for the persistence
contract. Reviewers can see at a glance what every variant looks like;
the correlated triple proves id propagation is visible-by-default in
future snapshots.

The existing per-variant rule tests stay — they pin specific mutants the
catalogue snapshot wouldn't reliably catch.

#### Step 3 — Per-provider request-body kitchen-sink snapshot

In `omega-core/tests/anthropic.rs` and `omega-core/tests/ollama.rs`, add
one `request_body_kitchen_sink` test each:

- Build an `LlmRequest` with: a multi-turn conversation (user text,
  assistant tool_use, user tool_result — id-correlated), a system
  prompt, two tool definitions, a non-default `ModelConfig`.
- Drive through the provider, capture `received[0].body` from wiremock.
- Snapshot a struct that includes both an input projection of the
  `LlmRequest` *and* the output JSON wire body, so the transformation is
  visible in the snapshot text.
- Apply `id_redactor` to tool-id-bearing paths so the
  `tool_use_id ↔ tool_use.id` correlation appears as `[id_1]` in both.

The existing `request_body_*` mutant-pinned tests stay — they target
specific mutants and are clearer as targeted assertions than as
snapshots.

#### Step 4 — *Optional* retrofit: file-pair pattern for streaming snapshots

Lower priority. The two existing `streams_*` snapshots use a Rust DSL
(`sse_body(&[("message_start", json!({...})), …])`) to build the
SSE/NDJSON fixture inline. An alternative: move the fixture to
`tests/fixtures/streams_kitchen_sink.{sse,ndjson}` (real wire bytes the
provider would receive), have the test read the file and feed wiremock,
and have the snapshot's header comment reference the fixture path.

Pros: the snapshot + fixture file pair is self-contained living
documentation of the parser; "what does Anthropic actually send?" is
answerable by reading a single file. Cons: more invasive change; the
current Rust DSL is fine.

Skip if you're short on time or judge the current shape good enough.
Don't agonise — the first three steps are the meat.

#### Step 5 — Run `cargo mutants` and triage

After the new snapshots land, run `cargo mutants -p omega-core` and
`cargo mutants -p omega-protocol`. Expect both to still report 0
surviving mutants. If any new survivors appear, triage with the same
three-option framework as Phase 1b.5 (new test / dead code removal /
documented skip). **Stop and discuss with the user before applying any
skip.**

### Done when

- `id_redactor()` lives in a `common/` test module and is re-used.
- An events-reference snapshot exists in `omega-protocol` showing all 22
  variants with at least one correlated tool-id triple.
- Each provider has a request-body kitchen-sink snapshot showing
  `LlmRequest` → wire-body transformation with correlated ids.
- `cargo mutants -p omega-core` and `cargo mutants -p omega-protocol`
  both report 0 surviving mutants.
- All commits passed `just rust-gate`.
- This section updated to a ✅ done record.

### Session setup

**Model:** `claude-sonnet-4-6` — **Effort:** Medium

(Design is laid out and patterns are established. Novel parts —
`insta::dynamic_redaction` API, fixture-file mechanics — are well-trodden
ground in the Rust community. Sonnet handles this comfortably; Opus is
overkill.)

**Prompt:**

> Continuing the Rust migration of Omega. Read
> `/home/carsten/omega/dev/rust-migration.md`, find the Phase 1b.7 session
> prompt, and execute it.

---

## Phase 1c — `omega-server` (WebSocket)

- `tokio` async runtime, `tokio-tungstenite` for WebSocket
- Session directory creation (mirrors `src/session-dir.ts`)
- Event store: append-only writes to `events.jsonl` (mirrors `src/event-store.ts`)
- Context store: append-only writes to `context.jsonl` (mirrors `src/context-store.ts`)
- WebSocket message handler: receives user messages, drives `omega-core` agent loop, fans out `OmegaEvent`s to all connected clients
- HTTP server for static asset serving (Leptos WASM bundle in Phase 3; TS bundle in Phase 1–2)

---

## Phase 1d — Bridge (`ts-rs`)

During the headless-Rust + TS-UI bridge period:

- Add `#[derive(ts_rs::TS)]` to all `omega-protocol` types
- `cargo test` generates `bindings/OmegaEvent.d.ts` etc.
- TS web client imports from `bindings/` instead of `src/events.ts`
- The generated `.d.ts` are committed so the UI is always type-checked against the Rust source
- Deleted entirely in Phase 3 when Leptos replaces the TS client

---

## Phase 2 — Rust as primary driver

- Rust `omega-server` binary replaces `src/cli.ts` + `src/web/server.ts` as the entry point
- TS web client (`src/web/`) is still served, now talking to Rust over WebSocket
- TS codebase is read-only at this point; all new features go into Rust
- Parity criterion: all existing E2E tests pass against the Rust backend

---

## Phase 3 — Leptos UI rewrite

- Add `omega-web` crate to the workspace (`leptos`, `trunk` / `wasm-pack`)
- Port `src/web/client/` component by component; Leptos fine-grained reactivity maps directly to SolidJS
- `omega-web` imports types from `omega-protocol` directly — no `ts-rs` bridge needed
- Once all components are ported: delete `src/`, delete `ts-rs` derives, delete `node_modules`
- The repo becomes a pure Cargo workspace

---

## Phase 4 — `chromiumoxide` + LLM oracle, retire Playwright

- Replace Playwright E2E tests with `chromiumoxide` (Chrome DevTools Protocol, pure Rust)
- LLM-as-oracle for snapshot review: a separate agent session compares rendered output against expected behaviour descriptions — reduces snapshot review load but is not the primary correctness mechanism; property-based assertions remain authoritative
- `package.json`, `node_modules`, Playwright config deleted

---

## Settled decisions — format and compatibility

**No backward compatibility with old `events.jsonl` files.**
The Rust implementation makes no attempt to parse log files written by the
TypeScript agent. Data shapes are honest — every field that the struct declares
is required in the JSON. There are no `#[serde(default)]` shims, no legacy
field remapping, and no `Option` fields whose sole purpose is to paper over
historical log gaps. Old logs are simply not supported by the Rust reader.

Corollary: do not encode defaults into data shapes. Backward-compat shims
belong at an explicit parsing boundary with their own tests, or not at all.
The `cargo mutants` finding on `default_effort()` in the initial
`omega-protocol` draft is the canonical example of why this matters —
a default baked into a serde attribute is untestable by design.

---

## What is intentionally deferred

All of the following are post-parity improvements. Do not implement during port:

- Redesigned session resumption UX
- Streaming context compaction (server-side)
- OpenAI provider (add after Anthropic + Ollama abstraction is proven)
- `cargo mutants` integration into CI
- `insta` snapshot tests for rendered Leptos components
- Rate-limit backpressure to UI
