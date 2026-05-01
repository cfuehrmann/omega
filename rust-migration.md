# Omega — Rust Migration

*Living document. Completed phases are summarised briefly; upcoming phases have full detail.*

---

## Status

| Phase | Status | Deliverable |
|---|---|---|
| 0 — Planning | ✅ Done | This document + architectural decisions |
| 1a — `omega-protocol` | ✅ Done | `rust/crates/omega-protocol`: all 22 `OmegaEvent` variants, `StreamSignal`, serde round-trips; workspace tooling: edition 2024, `clippy::pedantic -D warnings`, `cargo-machete`, `cargo mutants`; honest types (no `#[serde(default)]` shims) |
| 1b — `omega-core` (LLM loop) | 🔜 Next | Anthropic + Ollama providers, retry loop, streaming |
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

## Phase 1b — `omega-core` (✅ done)

**Status:** complete and committed (commit `22a8f17`).

**What landed:** `rust/crates/omega-core` with `Provider` trait,
`AnthropicProvider` (SSE), `OllamaProvider` (NDJSON), and a generic
`RetryingProvider<P>` retry wrapper that honours `Retry-After` and
emits `OmegaEvent::LlmRetry` with text/thinking fragments. 17 omega-core
tests (9 retry + 4 anthropic + 4 ollama) plus 17 omega-protocol tests —
all green; no live API calls (wiremock-fronted). Implementation
adjustments worth carrying forward:

- `AgentItem::Event` boxes `OmegaEvent` (large_enum_variant). Construct
  with `AgentItem::event(ev)` or `.into()`.
- `Provider::stream` returns `BoxStream<'static, Result<AgentItem, LlmError>>`
  (alias `AgentItemStream`) rather than `impl Stream` — ergonomic with
  trait-object composition.
- `context_hash` on emitted `LlmResponse`/`ToolCall` events is left empty;
  Phase 1c persistence layer fills it in.
- Pre-existing clippy errors in `omega-protocol`'s test (now exposed by
  `cargo clippy --all-targets`) fixed inline.

### Session setup

**Model:** `claude-opus-4-7` — **Effort:** High

**Prompt:**

> You are continuing the Rust migration of Omega. Context is in `/home/carsten/omega/dev/rust-migration.md`. The current state: Phase 1a (`omega-protocol`) is complete and committed. Phase 1b (`omega-core`) is next.
>
> Build `rust/crates/omega-core` with the following contract:
>
> - A `Provider` trait with a single `stream` method that takes an `LlmRequest` and returns `impl Stream<Item = Result<AgentItem, LlmError>>`
> - `AgentItem` is either a `StreamSignal` (ephemeral text/thinking fragment) or an `OmegaEvent` (persisted event — `LlmResponse`, `ToolCall`, `LlmRetry`, `LlmError`)
> - Two concrete providers built simultaneously: `AnthropicProvider` and `OllamaProvider`. Building both at once forces a real abstraction.
> - A retry loop that wraps any `Provider`, emits `OmegaEvent::LlmRetry` on transient errors (429/529/500/503), respects `Retry-After` headers, uses exponential backoff with jitter
> - No live API calls in tests — mock the HTTP layer. Use `insta` for snapshot assertions on serialized event sequences.
>
> Reference implementations (read before writing any provider code):
> - `/home/carsten/forgecode/crates/forge_repo/src/provider/anthropic.rs` — SSE streaming + beta headers pattern
> - `/home/carsten/forgecode/crates/forge_app/src/dto/anthropic/request.rs` — request struct shape
> - `/home/carsten/forgecode/crates/forge_repo/src/provider/provider_repo.rs` — provider dispatch
>
> Workspace conventions already in place: edition 2024, `clippy::pedantic -D warnings`, `cargo-machete`, `cargo mutants` run manually to check coverage. All decisions in `rust-migration.md` are settled — do not re-litigate them. Run `just rust-gate` to verify before each commit.


### Contract

```rust
// Input
pub struct LlmRequest {
    pub messages:  Vec<Message>,
    pub tools:     Vec<ToolDefinition>,
    pub model:     String,
    pub config:    ModelConfig,    // max_tokens, thinking budget, etc.
}

// Output: an async stream
// Item = Ok(AgentItem) | Err(LlmError)
pub enum AgentItem {
    Signal(StreamSignal),   // ephemeral — text/thinking fragments
    Event(OmegaEvent),      // persisted — llm_response, tool_call, llm_retry, …
}
```

### Provider abstraction

```rust
pub trait Provider: Send + Sync {
    async fn stream(&self, req: LlmRequest)
        -> impl Stream<Item = Result<AgentItem, LlmError>>;
}

pub struct AnthropicProvider { client: reqwest::Client, api_key: String }
pub struct OllamaProvider    { client: reqwest::Client, base_url: String }
```

Both providers are built simultaneously. If only one is built first, the abstraction will be shaped by that provider.

### Implementation plan

1. **Types** — `Message`, `ToolDefinition`, `ModelConfig`, `LlmError` in `omega-core/src/types.rs`
2. **Anthropic provider** — SSE streaming via `reqwest` + `eventsource-stream`; beta headers; request/response structs with serde; maps raw events to `AgentItem`
3. **Ollama provider** — same shape, different wire format (OpenAI-compatible `/api/chat` with `"stream": true`)
4. **Retry loop** — wraps any `Provider`; emits `OmegaEvent::LlmRetry` on transient errors (429/529/500/503); respects `retry-after` header; exponential backoff with jitter
5. **Tests** — mock `reqwest` responses (no live API calls); assert `AgentItem` sequences match expected `OmegaEvent` shapes; use `insta` for snapshot tests of serialized events

### Reference files (forgecode patterns)

- `/home/carsten/forgecode/crates/forge_repo/src/provider/anthropic.rs` — SSE + beta headers
- `/home/carsten/forgecode/crates/forge_app/src/dto/anthropic/request.rs` — request structs
- `/home/carsten/forgecode/crates/forge_repo/src/provider/provider_repo.rs` — provider dispatch

Beta headers currently in use:
```
anthropic-version: 2023-06-01
anthropic-beta: structured-outputs-2025-11-13
anthropic-beta: interleaved-thinking-2025-05-14   (older models)
```

---

## Phase 1b.5 — mutation-test `omega-core` ✅ Done

**Result:** `cargo mutants -p omega-core` reports **0 surviving mutants**
(79 mutants tested: 60 caught, 19 unviable, 1 excluded).

| Outcome | Count | Notes |
|---|---|---|
| Killed by new tests | 30 | See commit `96620be` |
| Equivalent / skipped | 1 | `replace * with / in compute_backoff` — `x/f ≈ x*(1/f)` for `f∈[0.9,1.1]`; ranges overlap; non-deterministic RNG makes them indistinguishable |
| Dead code removed | 0 | — |

**New tests added** (all in `omega-core`):
- `types`: 4 unit tests for `LlmError::body()` all variants
- `retry`: `error_body_populated_from_http_body`, `retry_at_is_not_before_event_time`, `backoff_grows_on_second_attempt`, `jitter_rounds_to_base_ms_not_double`
- `tests/anthropic`: `parse_retry_after_zero_is_some_zero`, `parse_retry_after_negative_is_none`, `parse_retry_after_nonfinite_is_none`, `response_event_time_fields_are_valid_rfc3339`
- `tests/ollama`: `maps_429_to_http_error_with_retry_after`, `parse_retry_after_{zero,negative,nonfinite,subsecond}`, `with_client_custom_header_is_propagated`, `response_event_time_is_valid_rfc3339`, `request_body_{contains_user_text_message,thinking_only_message,tool_use_only_message,tool_result_no_extra_message,contains_tool_definitions}`

**Skip config:** `rust/.cargo/mutants.toml` with `exclude_re` entry.

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
