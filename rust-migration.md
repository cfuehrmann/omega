# Omega — Rust Migration

*Living document. Completed phases are summarised briefly; upcoming phases have full detail.*

---

## Status

| Phase | Status | Deliverable |
|---|---|---|
| 0 — Planning | ✅ Done | This document + architectural decisions |
| 1a — `omega-protocol` | ✅ Done | `rust/crates/omega-protocol`: all 22 `OmegaEvent` variants, `StreamSignal`, serde round-trips; workspace tooling: edition 2024, `clippy::pedantic -D warnings`, `cargo-machete`, `cargo mutants`; honest types (no `#[serde(default)]` shims) |
| 1b — `omega-core` (LLM loop) | ✅ Done | Anthropic + Ollama providers, retry loop, streaming; 0 surviving mutants |
| 1b.7 — Insta snapshot coverage | ✅ Done | `id_redactor` helper (omega-protocol + omega-core), all-22-variants reference snapshot, Anthropic + Ollama kitchen-sink wire-body snapshots; 0 survived mutants, 2 expected timeouts in retry loop |
| 1c — `omega-store` (Persistence) | ⬜ Next | Session dir, EventStore, ContextStore, ContextHash |
| 1d — `omega-agent` + CLI binary | ⬜ Upcoming | Multi-turn loop, tool execution, compaction, context hashing; **first Harbor-testable binary** |
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

## Phase 1b.7 — Insta snapshot coverage ✅ Done

`id_redactor` helper added to both `omega-protocol/tests/common/mod.rs`
(new) and `omega-core/tests/common/mod.rs`. Uses `Arc<Mutex<HashMap>>`
so multiple `r.redaction()` calls share numbering — same id value gets
`[id_1]` whether it appears at `.id` or `.tool_use_id`.

All-22-variants reference snapshot in `omega-protocol/tests/events_reference.rs`
shows every `OmegaEvent` shape; correlated ToolCall + ToolResult triple
proves id propagation is visible. Per-provider kitchen-sink snapshots in
`omega-core/tests/anthropic.rs` and `omega-core/tests/ollama.rs` show
`LlmRequest → wire-body` transformation with correlated `[id_1]`. Ollama
snapshot clearly shows that `flatten_message` strips tool ids.

`cargo mutants`: 56 caught, 19 unviable, 2 timeouts (retry-termination
mutants cause infinite loops — expected), **0 survived**.

---

## Phase 1c — `omega-store` (Persistence) 🔜 Next

### What this phase builds

A new `rust/crates/omega-store` crate that owns all filesystem persistence:
session directories, the event log (`events.jsonl`), and the context log
(`context.jsonl`). It mirrors `src/session-dir.ts`, `src/event-store.ts`,
`src/context-store.ts`, and `src/context-hash.ts`.

This is the right next step because:
- It's a clean dependency leaf: `omega-store` → `omega-core` → `omega-protocol`
- The agent loop (Phase 1d) needs it to persist events and context records
- The TS source is small (~370 lines total) and well-understood
- Real file-I/O tests keep it honest without mock complexity

### Source reference

Read these TS files before implementing:
- `src/context-hash.ts` — hash type + `randomHash()`
- `src/session-dir.ts` — session folder layout, naming, metadata read/write
- `src/event-store.ts` — `appendEvent`
- `src/context-store.ts` — `ContextRecord`, `appendContextMessage`

### Crate structure

```
rust/crates/omega-store/
├── Cargo.toml
└── src/
    ├── lib.rs           (pub-use all public items)
    ├── context_hash.rs  (ContextHash type, random_hash())
    ├── session_dir.rs   (SessionPaths, make_session_dir, metadata I/O)
    ├── event_store.rs   (EventStore: append OmegaEvent → events.jsonl)
    └── context_store.rs (ContextRecord, append → context.jsonl → returns hash)
```

### Module contracts

#### `context_hash`

```rust
/// 12 lowercase hex characters (6 random bytes). PK of context.jsonl records;
/// FK in events.jsonl (LlmCallEvent.context_hashes, ToolCallEvent.context_hash, etc.)
pub struct ContextHash(String);   // newtype; derives Serialize/Deserialize transparently

pub fn random_hash() -> ContextHash;   // 6 bytes from rand or getrandom, hex-encoded
pub fn hash_from_str(s: &str) -> Result<ContextHash>; // validates 12 hex chars
```

`ContextHash` must satisfy `[0-9a-f]{12}`. `hash_from_str` returns an error
on invalid input (validated by unit test). Implement `Display`, `AsRef<str>`,
`From<ContextHash> for String`.

#### `session_dir`

Session folders live under a configurable root (default `.omega/sessions`).
Folder name format: `YYYY-MM-DDTHH-MM-SS-mmm-<hex8>` (millisecond precision
+ 4 random bytes as 8 hex chars). `ls`-by-name order = chronological order.

```rust
pub const SESSIONS_ROOT: &str = ".omega/sessions";

/// Regex matching all three historical name formats (see TS source):
/// - YYYY-MM-DDTHH-MM-SS                  (legacy, second precision)
/// - YYYY-MM-DDTHH-MM-SS-<hex8>           (v2, second + suffix)
/// - YYYY-MM-DDTHH-MM-SS-mmm-<hex8>       (current, millisecond + suffix)
pub fn session_dir_re() -> &'static Regex;

pub fn make_session_dir_name(now: DateTime<Utc>) -> String;

pub struct SessionPaths {
    pub dir:          PathBuf,
    pub context_file: PathBuf,
    pub events_file:  PathBuf,
}

/// Creates <root>/<name>/ plus empty context.jsonl, events.jsonl,
/// and session.jsonc (containing `{}`). Returns SessionPaths.
pub async fn make_session_dir(root: &Path) -> Result<SessionPaths>;

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name:         Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description:  Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resumed_from: Option<String>,
}

/// Reads session.jsonc; returns SessionMetadata::default() if absent or
/// unparseable. Strip `// …` and `/* … */` comments before parsing
/// (same logic as the TS implementation — a single regex pass suffices).
pub async fn read_session_metadata(dir: &Path) -> SessionMetadata;

pub async fn write_session_metadata(dir: &Path, meta: &SessionMetadata) -> Result<()>;

/// Merges patch into existing metadata (None patch fields leave the
/// existing value unchanged).
pub async fn update_session_metadata(dir: &Path, patch: SessionMetadata) -> Result<()>;
```

JSONC comment stripping: the TS does it with two regex replacements.
In Rust, pull in no extra crate — strip `// … \n` and `/* … */` with
`regex` (already in the workspace) or simple `str` manipulation, then
pass to `serde_json::from_str`. Keep this a private helper.

#### `event_store`

```rust
/// Wraps the events.jsonl path; created once per session.
pub struct EventStore {
    path: PathBuf,
}

impl EventStore {
    pub fn new(path: PathBuf) -> Self;

    /// Serialise `event` with serde_json and append as a single line to
    /// events.jsonl. Creates parent dirs if needed (defensive; the
    /// session dir creator already does this).
    pub async fn append(&self, event: &OmegaEvent) -> Result<()>;
}
```

No UI-only field stripping needed — the Rust `OmegaEvent` type has no
UI-only fields (that concern was TS-specific; Rust derives are honest).

#### `context_store`

```rust
/// The on-disk shape of a context.jsonl record.
#[derive(Debug, Serialize, Deserialize)]
pub struct ContextRecord {
    pub hash:    ContextHash,
    pub time:    String,          // ISO 8601 UTC, same format as ISOTimestamp in events
    pub role:    Role,            // reuse omega_core::types::Role
    pub content: Vec<ContentBlock>, // reuse omega_core::types::ContentBlock
}

/// Wraps the context.jsonl path; created once per session.
pub struct ContextStore {
    path: PathBuf,
}

impl ContextStore {
    pub fn new(path: PathBuf) -> Self;

    /// Build a ContextRecord (random hash, current UTC time), append to
    /// context.jsonl, return the hash. The caller uses it as the FK in
    /// LlmCallEvent.context_hashes etc.
    pub async fn append(&self, role: Role, content: Vec<ContentBlock>) -> Result<ContextHash>;

    /// Build a record without writing — for testing the shape without I/O.
    pub fn build_record(role: Role, content: Vec<ContentBlock>) -> ContextRecord;
}
```

`ContextStore` depends on `omega-core` (for `Role` and `ContentBlock`).  
Add `omega-core = { path = "../omega-core" }` to `omega-store/Cargo.toml`.

### Testing strategy

- Use **real file I/O** with a temp dir per test (use `tempfile` crate or
  just `std::env::temp_dir()` with a unique suffix).
- Do not mock the filesystem.
- Integration tests in `tests/` (one file per module: `tests/session_dir.rs`,
  `tests/event_store.rs`, `tests/context_store.rs`).
- Aim for the same coverage discipline as `omega-core`: run `cargo mutants`
  after the tests land and triage any survivors.

Key test cases:
- `make_session_dir` creates the three expected files; name matches the regex.
- `read_session_metadata` returns default on missing file; parses present
  fields; strips `//` and `/* */` comments.
- `write_session_metadata` / `update_session_metadata` round-trips.
- `EventStore::append` writes valid JSONL; each line round-trips through
  `serde_json::from_str::<OmegaEvent>`.
- `ContextStore::append` returns a 12-hex hash; written record round-trips;
  two calls return different hashes.
- `hash_from_str` rejects strings that aren't exactly 12 lowercase hex chars.

### Done when

- `rust/crates/omega-store` builds cleanly (`just rust-gate`).
- All integration tests pass with real file I/O.
- `cargo mutants -p omega-store` reports 0 survived.
- This section updated to a ✅ done record.

### Session setup

**Model:** `claude-sonnet-4-6` — **Effort:** Medium

(Straightforward port of well-understood TS code. The design is spelled
out above. Sonnet handles async Rust file I/O and serde without needing
Opus. Main risk is getting the JSONC comment stripping right — but the
TS logic is trivial to translate.)

**Prompt:**

> Continuing the Rust migration of Omega. Read
> `/home/carsten/omega/dev/rust-migration.md`, find the Phase 1c session
> prompt, and execute it.

---

## Phase 1d — `omega-agent` + CLI binary

Ports `src/agent.ts` — the multi-turn conversation driver — to a new
`rust/crates/omega-agent` crate. This is the most complex phase:
multi-turn message management, tool execution, context hashing (using
`omega-store`), compaction, session resumption, pause/continue logic.

**Phase 1d is the first Harbor-testable milestone.** In addition to the
library crate, this phase ships a minimal `omega-cli` binary:

```
omega-cli run \
  --instruction "Fix the failing tests" \
  --model claude-sonnet-4-6 \
  --session-dir /tmp/omega-session \
  [--effort medium] \
  [--max-turns 100]
```

The binary is a thin wrapper (~100 lines): parse args, call
`omega-agent` with an auto-approve tool callback, stream response text
to stdout, structured event lines to stderr (matching `cli.ts` format),
exit 0 on `turn_end` / 1 on interruption. No web server, no WebSocket
— Harbor points at this binary directly.

### Harbor adapter update (done at end of Phase 1d)

`bench/omega_agent.py` needs two lines changed:

```python
# install(): replace bun install with cargo build
"git clone ... && cd omega && cargo build --release --bin omega-cli"

# run(): replace the bun invocation
f"{OMEGA_INSTALL_DIR}/target/release/omega-cli run "
f"--instruction {shlex.quote(instruction)} "
f"--model {shlex.quote(self._parsed_model_name)} "
f"--session-dir {OMEGA_SESSION_DIR} {flags}"
```

Everything else in the adapter is unchanged: `populate_context_post_run`
reads `turn_end` from `events.jsonl` — same field names, same format,
because `omega-protocol` is shared. The oracle checks the container
filesystem, not the agent implementation, so it works without any
modification.

**Parity criterion:** run the same oracle task batch that was used to
validate TS Omega. If Harbor scores are statistically indistinguishable,
Rust Omega has reached functional parity with the TS implementation —
a stronger signal than any unit test.

*Build time note:* `cargo build --release` adds ~2–5 min per task
during initial validation (acceptable). For production benchmarking,
publish a pre-built static binary to a GitHub release and have
`install()` `curl` it instead of compiling.

Scope and session prompt will be written once Phase 1c is done and the
persistence API is stable.

---

## Phase 1e — `omega-server` (WebSocket + HTTP)

Ports `src/web/server.ts` to a Rust binary crate:
- `axum` (HTTP + WebSocket) or `tokio-tungstenite` + `hyper`
- Session creation, listing, resumption via HTTP endpoints
- WebSocket fan-out: all connected clients receive each `OmegaEvent`
- History replay on reconnect (reads `events.jsonl`)
- Static file serving (serves TS web UI bundle during Phase 1–2; Leptos
  WASM in Phase 3)

Session prompt will be written once Phase 1d is done.

---

## Phase 1f — Bridge (`ts-rs`)

During the headless-Rust + TS-UI bridge period:

- Add `#[derive(ts_rs::TS)]` to all `omega-protocol` types
- `cargo test` generates `bindings/OmegaEvent.d.ts` etc.
- TS web client imports from `bindings/` instead of `src/events.ts`
- The generated `.d.ts` are committed so the UI is always type-checked against the Rust source
- Deleted entirely in Phase 3 when Leptos replaces the TS client

*Can be executed any time after omega-protocol is stable — i.e. now. But
until the Rust server binary actually runs, the bridge adds friction for
no functional gain. Defer until the server is ready.*

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
