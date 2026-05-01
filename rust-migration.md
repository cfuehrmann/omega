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
| 1d.0 — `omega-tools` + `omega-agent` core + `omega-cli` | ⬜ Next | All 13 tools ported; core multi-turn loop; context hashing; **first Harbor-testable binary** |
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
│       ├── omega-tools/        ⬜ next (Phase 1d.0)
│       ├── omega-agent/        ⬜ next (Phase 1d.0 core + 1d.1 advanced)
│       └── omega-cli/          ⬜ next (Phase 1d.0 binary)
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

## Completed phases — concise record

### Phase 1a — `omega-protocol` ✅

`rust/crates/omega-protocol`: all 22 `OmegaEvent` variants serialised/deserialised
with honest types (no `#[serde(default)]` shims). `StreamSignal` type. Workspace
tooling: edition 2024, `clippy::pedantic -D warnings`, `cargo-machete`, `cargo mutants`.
Insta snapshot (`events_reference.rs`) covers all 22 variants with `id_redactor` helper.
0 surviving mutants.

### Phase 1b — `omega-core` (LLM loop) ✅

`rust/crates/omega-core`: `Provider` trait, `AnthropicProvider` (SSE),
`OllamaProvider` (NDJSON), `RetryingProvider<P>` (honours `Retry-After`,
emits `LlmRetry` with text/thinking fragments). Both providers built
simultaneously to force a real abstraction. All tests wiremock-fronted; no live
API calls. Sub-phases:

- **1b.0**: initial implementation (17 omega-core + 17 omega-protocol tests).
- **1b.5**: mutation tested; killed 30 newly-discovered mutants. One documented
  skip in `compute_backoff` (`replace * with /` — equivalent under RNG).
- **1b.6**: replaced internal `ScriptedProvider` with e2e tests through real
  providers + wiremock + flaky-TCP listener. Deleted ~450 lines of test
  infrastructure. 2 expected timeout mutants (infinite-retry mutations).
- **1b.7**: `id_redactor` helper; all-22-variants reference snapshot;
  per-provider kitchen-sink wire-body snapshots. 0 survived, 2 timeouts.

**Implementation notes carried forward:**
- `AgentItem::Event` boxes `OmegaEvent` (large_enum_variant). Construct
  with `AgentItem::event(ev)` or `.into()`.
- `Provider::stream` → `BoxStream<'static, Result<AgentItem, LlmError>>`.
- `LlmError::Transport` is reachable: reproduced via in-process flaky-listener.
- Sequential wiremock responses: mount multiple `Mock`s with `.up_to_n_times(N)`.

### Phase 1c — `omega-store` (Persistence) ✅

`rust/crates/omega-store`: four modules porting `src/context-hash.ts`,
`src/session-dir.ts`, `src/event-store.ts`, `src/context-store.ts`.

Key decisions:
- **`spawn_blocking` for file I/O** — Tokio's `File` uses positioned writes
  (`pwrite`) that ignore `O_APPEND`; using `std::fs::OpenOptions` on a
  blocking thread gives correct append semantics.
- **`strip_jsonc_comments` as a manual byte-scanner** — avoids an extra crate
  dependency; handles `// …` and `/* … */` comments.
- **`debug_assert!(i < len)`** inside the outer while loop makes the
  `< → <=` equivalent mutation observable in debug builds.
- **`serde(alias = "continuationOf")`** on `resumed_from` handles legacy
  session metadata.
- **Backward compat** for `session.jsonc` only; `events.jsonl` / `context.jsonl`
  have no serde defaults (policy unchanged).

Mutation testing: 76 mutants — 66 caught, 6 unviable, 4 timeouts, **0 missed**.
Deployed 8 boundary-condition tests targeting specific mutations in
`strip_jsonc_comments`, including a `debug_assert!` and carefully chosen inputs
that force each mutation's bounds check to access OOB memory.

---

## Phase 1d.0 — `omega-tools` + `omega-agent` core + `omega-cli` ⬜ Next

This is the biggest phase. The TypeScript source spans ~3000 lines across
`src/agent.ts` (1866 lines) and `src/tools.ts` (1102 lines). The session
produces three new crates and the first Harbor-testable binary.

**Scope for this session (1d.0):**
- `omega-tools` crate — all 13 tool implementations
- `omega-agent` crate — core agent loop (multi-turn, tool dispatch, context
  hashing, event emission); no pause/resumption/compaction yet
- `omega-cli` binary crate — thin wrapper around `omega-agent`

### Source reference — read these before implementing

```
src/agent.ts        – Agent struct, sendMessage, streamLlmCall, processStreamEvents,
                      isRetryable, isContextTooLong, elide helpers, capEffortForModel
src/tools.ts        – all 13 executeTool implementations + toolDefinitions
src/config.ts       – COMPACTION_INSTRUCTIONS, maxOutputTokensForModel, DEFAULT_MODEL
src/system-prompt/  – buildSystemPrompt, readSystemPromptAppend, corePrompt
src/session-resume.ts – only needed in Phase 1d.1; skip for now
```

### Crate dependency graph

```
omega-protocol
     ↑
omega-core        (Provider trait, RetryingProvider, AgentItem)
     ↑
omega-store       (EventStore, ContextStore, SessionPaths)
     ↑
omega-tools       (executeTool — NEW, Phase 1d.0)
     ↑
omega-agent       (Agent struct — NEW, Phase 1d.0 core)
     ↑
omega-cli         (main() binary — NEW, Phase 1d.0)
```

### `omega-tools` crate

All 13 tools from `src/tools.ts`. This is a pure-Rust port of Bun/Node
filesystem and subprocess APIs.

**Cargo.toml dependencies:**
```toml
tokio     = { version = "1", features = ["full"] }
serde     = { version = "1", features = ["derive"] }
serde_json = "1"
reqwest   = { version = "0.12", features = ["json", "stream"] }
omega-protocol = { path = "../omega-protocol" }  # for tool schema types
thiserror = "2"
```

**Tool implementations (map TS → Rust):**

| TS function | Rust equivalent |
|---|---|
| `executeReadFile` | `tokio::fs::read_to_string` + line slicing |
| `executeWriteFile` | `tokio::fs::create_dir_all` + `tokio::fs::write` |
| `executeEditFile` | read → replace-once (multiple replacements array) → write |
| `executeRunCommand` (sync in TS) | `tokio::process::Command`, timeout via `tokio::time::timeout` |
| `executeListFiles` | `tokio::fs::read_dir`, recursive via `walkdir` or manual recursion |
| `executeWebSearch` | `reqwest` GET to DuckDuckGo or Brave, parse JSON/HTML |
| `executeFetchUrl` | `reqwest` GET + `html2text` or similar → cache file + postprocess |
| `executeGrepFiles` | `ripgrep` subprocess (`rg`) or `grep` fallback |
| `executeFindFiles` | `fd` subprocess or `find` fallback |
| `executeRunBackground` | `tokio::process::Command::spawn`, write stdout/stderr to log file |
| `executeWaitForOutput` | poll log file for pattern/minBytes, honour timeout |
| `executeWriteStdin` | write to stdin of tracked background process |
| `executeWebSearch` | Brave Search API key from env; DuckDuckGo fallback |

**Public API:**

```rust
pub struct ToolResult {
    pub content: String,
    pub is_error: bool,
}

/// Dispatch to the correct tool implementation.
/// Returns ToolResult (never Err — errors are returned as is_error: true).
pub async fn execute_tool(name: &str, input: serde_json::Value) -> ToolResult;

/// The tool schema array sent to the LLM. Build from the TS toolDefinitions.
pub fn tool_definitions() -> Vec<serde_json::Value>;

/// A human-readable summary of a tool call for logging.
pub fn format_tool_call(name: &str, input: &serde_json::Value) -> String;
```

`execute_tool` never returns `Err` — errors become `ToolResult { is_error: true }`.
Background process state (pid → log file path, pid → stdin) lives in a
`tokio::sync::Mutex<HashMap<u32, BackgroundProcess>>` inside the crate.

**Testing strategy for omega-tools:**
- Integration tests with real file I/O in a temp dir (same discipline as omega-store).
- No mocking of filesystem or subprocess.
- `executeRunCommand` timeout test: spawn `sleep 60`, set timeout 100 ms.
- `executeWebSearch`: if `BRAVE_SEARCH_API_KEY` env var absent, skip or use
  DuckDuckGo; the test should not fail in CI without the key.
- Background process tests: start `sleep 2`, wait for output, kill.

### `omega-agent` crate (core — Phase 1d.0)

Port `Agent` struct and `sendMessage` from `src/agent.ts`, omitting
pause/continue/abort, session resumption, and compaction (those are Phase 1d.1).

**Cargo.toml dependencies:**
```toml
omega-protocol = { path = "../omega-protocol" }
omega-core     = { path = "../omega-core" }
omega-store    = { path = "../omega-store" }
omega-tools    = { path = "../omega-tools" }
tokio          = { version = "1", features = ["full"] }
serde          = { version = "1", features = ["derive"] }
serde_json     = "1"
reqwest        = { version = "0.12" }
chrono         = { version = "0.4", default-features = false, features = ["clock", "serde"] }
thiserror      = "2"
```

**Core struct:**

```rust
pub struct Agent {
    // Provided at construction
    provider:      Box<dyn Provider + Send + Sync>,
    context_store: ContextStore,
    event_store:   EventStore,
    session_dir:   PathBuf,

    // State
    model:     String,          // current model name
    effort:    String,          // "low" | "medium" | "high"
    session_id: String,         // random UUID or hex
    history:   Vec<Message>,    // compacted context history (Message from omega_core)
    context_hashes: Vec<ContextHash>, // parallel to history

    // Counters for turn isolation (see TS source)
    turn_counter: u64,
}

pub struct AgentConfig {
    pub model:       String,
    pub effort:      String,
    pub session_dir: PathBuf,
    pub provider:    Box<dyn Provider + Send + Sync>,
}

impl Agent {
    pub async fn new(config: AgentConfig) -> Result<Self, AgentError>;

    /// Emit session_started event. Idempotent — only fires once.
    pub async fn init(&mut self) -> Result<(), AgentError>;

    /// Core multi-turn loop. Yields AgentItems (events + stream signals).
    /// Terminates with turn_end on success, or turn_interrupted on abort/error.
    pub fn send_message(
        &mut self,
        content: String,
        signal: Option<tokio_util::sync::CancellationToken>,
    ) -> impl Stream<Item = Result<AgentItem, AgentError>>;
}
```

**`send_message` logic (from TS `sendMessage`):**

1. Emit + persist `user_message` event.
2. Append user message to `history`; store in `ContextStore`; push hash to
   `context_hashes`.
3. Inner loop — `streamLlmCall` equivalent:
   a. Build Anthropic request: `model`, `max_tokens`, `system`, `tools`,
      `messages` (from `history`), `betas: ["interleaved-thinking-2025-05-14"]`.
      Add cache-control to the last human message (same logic as TS
      `addCacheControlToLastMessage`).
   b. Call `provider.stream(request)`.
   c. Process stream: accumulate text, collect tool calls (parallel),
      detect `Compacted` stop reason.
   d. Emit + persist `llm_call` (with `context_hashes` FK), `llm_response`.
   e. Append assistant message to history; store in `ContextStore`; push hash.
   f. If stop reason is `tool_use`: dispatch all tool calls in parallel via
      `omega_tools::execute_tool`; emit + persist `tool_call` + `tool_result`
      per call; append tool-result message to history + context.
   g. If stop reason is `end_turn` or context too long: emit `turn_end` /
      `agent_error`; break.
   h. Loop (go to step 3).
4. Emit + persist `turn_end` with aggregated `TurnMetrics`.

**What to omit from Phase 1d.0:**
- `requestPause()`, `requestContinue()`, `abort()` — Phase 1d.1
- `performResumption()`, `seedWithResumptionSummary()` — Phase 1d.1
- Server-side compaction (`Compacted` stop reason handling) — Phase 1d.1
- `setModel()`, `setEffort()` — Phase 1d.1
- `buildSystemPrompt` with append file — stub as `corePrompt(cwd, max_tokens)`

**System prompt:** Port `src/system-prompt/index.ts` as a function
`build_system_prompt(cwd: &Path, max_output_tokens: u32) -> String`. Read
the `.omega/system-prompt-append` file and append it if present (mirrors
`readSystemPromptAppend`). The core prompt text is a constant string — port
it verbatim from `src/system-prompt/core-prompt.ts`.

**Elision helpers:** Port `elideAnthropicRequest` and `elideAnthropicResponse`
from `src/agent.ts`. These produce the `requestSummary` / `responseSummary`
fields on `llm_call` / `llm_response` events — important for the event log
to be readable without walls of text.

**Invalid tool JSON recovery:** Port the invalid-JSON nudge logic
(see `isInvalidToolJson`, `feedbackOnExhaustion`, the 3-attempt nudge in
`sendMessage`). This is exercised by the TS tests in
`src/agent-invalid-tool-json.test.ts`.

**Dangling tool-use repair:** Before a `user_message`, check if the most recent
assistant message has an unmatched `tool_use` block and inject a synthetic
`tool_result` if so (see the TS guard in `sendMessage`).

**Testing strategy for omega-agent core:**
- Use `omega_core`'s wiremock pattern: inject a scripted `Provider` (or a
  simple mock) rather than hitting the real API.
- Key scenarios: single-turn text response, tool-call loop (one tool, two
  tools in parallel), error on non-retryable status, context-too-long.
- Use real `omega_store` with temp dirs (consistent with the project discipline).
- Do NOT add `#[cfg(test)]` scripted providers inside the crate — put them in
  `tests/common/mod.rs` as a `MockProvider` that returns pre-canned
  `AgentItem` streams.

### `omega-cli` binary crate

```
rust/crates/omega-cli/
├── Cargo.toml
└── src/
    └── main.rs   (~100 lines)
```

```
USAGE: omega-cli run \
  --instruction <text> \
  --model <model-name> \
  [--effort <low|medium|high>] \
  [--session-dir <path>]   (default: .omega/sessions/<timestamp>)
  [--max-turns <n>]        (default: 100)
```

`main.rs` logic:
1. Parse CLI args (use `clap` with derive).
2. Create `SessionPaths` via `omega_store::make_session_dir`.
3. Construct `AnthropicProvider` (or `RetryingProvider<AnthropicProvider>`)
   using `ANTHROPIC_API_KEY` env var.
4. Construct `Agent` with the session paths.
5. Call `agent.init()` → emit `session_started`.
6. Call `agent.send_message(instruction, None)` → collect the stream.
7. For each item: print text chunks to stdout; print structured event lines
   to stderr (`event: <json>`); detect `turn_end` or `turn_interrupted`.
8. Exit 0 on success, 1 on interruption/error.

No web server, no WebSocket. Harbor points at this binary directly.

**Harbor adapter changes** (apply at end of Phase 1d.0):
```python
# bench/omega_agent.py  install():
"git clone ... && cargo build --release -p omega-cli"

# run():
f"target/release/omega-cli run "
f"--instruction {shlex.quote(instruction)} "
f"--model {shlex.quote(self._parsed_model_name)} "
f"--session-dir {OMEGA_SESSION_DIR}"
```
`populate_context_post_run` reads `turn_end` from `events.jsonl` — same field
names, same format. The oracle checks the container filesystem. No other
adapter changes needed.

### Done when

- `just rust-gate` passes (all three new crates, clippy clean, tests pass).
- `omega-cli run --instruction "list the files in the current directory" --model claude-sonnet-4-6`
  completes end-to-end (requires `ANTHROPIC_API_KEY` — run manually, not in CI).
- `cargo mutants -p omega-tools` and `cargo mutants -p omega-agent` report 0 missed.
- This section updated to ✅ Done.

### Session setup

**Model:** `claude-opus-4-7` — **Effort:** High

(Most complex phase: ~3000 lines of TS to port, two new crates plus a binary,
first live API integration. `opus-4-7` is the step-change improvement in
agentic coding. High effort because wrong design decisions here propagate to
1d.1 and 1e. Sonnet at high effort is the fallback if opus-4-7 is unavailable.)

**Prompt:**

> Continuing the Rust migration of Omega. Read
> `/home/carsten/omega/dev/rust-migration.md`, find the Phase 1d.0 session
> prompt, and execute it. Before writing any code, state your intended
> approach — including any adjustments to the plan you think are warranted
> (different crate splits, reordered sub-tasks, scope cuts). The plan in
> the doc is a strong prior but not a constraint; revise it if you see a
> better path.

---

## Phase 1d.1 — `omega-agent` advanced features ⬜ Upcoming

Add to the `omega-agent` crate built in Phase 1d.0:

- **`setModel()` / `setEffort()`** — emit + persist `model_changed` / `effort_changed`.
- **Pause/continue/abort** — `requestPause()`, `requestContinue()`, `abort()`,
  the seam logic, `turn_paused` / `turn_continued` events.
- **Session resumption** — `performResumption()`, `seedWithResumptionSummary()`,
  `extractResumptionBasis()` (port `src/session-resume.ts`).
- **Server-side compaction** — handle `Compacted` stop reason in stream
  processing; emit `compacted` event; clear/reset history.

Session prompt will be written after Phase 1d.0 is complete and the core
agent API is stable.

---

## Phase 1e — `omega-server` (WebSocket + HTTP) ⬜ Upcoming

Ports `src/web/server.ts` to a Rust binary crate:
- `axum` (HTTP + WebSocket) or `tokio-tungstenite` + `hyper`
- Session creation, listing, resumption via HTTP endpoints
- WebSocket fan-out: all connected clients receive each `OmegaEvent`
- History replay on reconnect (reads `events.jsonl`)
- Static file serving (serves TS web UI bundle during Phase 1–2; Leptos
  WASM in Phase 3)

Session prompt will be written once Phase 1d.1 is done.

---

## Phase 1f — Bridge (`ts-rs`) ⬜ Upcoming

During the headless-Rust + TS-UI bridge period:

- Add `#[derive(ts_rs::TS)]` to all `omega-protocol` types.
- `cargo test` generates `bindings/OmegaEvent.d.ts` etc.
- TS web client imports from `bindings/` instead of `src/events.ts`.
- The generated `.d.ts` are committed so the UI is always type-checked
  against the Rust source.
- Deleted entirely in Phase 3 when Leptos replaces the TS client.

*Can be executed any time after omega-protocol is stable — i.e. now. But
until the Rust server binary actually runs, the bridge adds friction for
no functional gain. Defer until the server is ready.*

---

## Phase 2 — Rust as primary driver ⬜ Future

- Rust `omega-server` binary replaces `src/cli.ts` + `src/web/server.ts`.
- TS web client (`src/web/`) still served, now talking to Rust over WebSocket.
- TS codebase read-only; all new features go into Rust.
- Parity criterion: all existing E2E tests pass against the Rust backend.

---

## Phase 3 — Leptos UI rewrite ⬜ Future

- Add `omega-web` crate (`leptos`, `trunk` / `wasm-pack`).
- Port `src/web/client/` component by component.
- `omega-web` imports types from `omega-protocol` directly — no `ts-rs` bridge.
- Once all components ported: delete `src/`, delete `ts-rs` derives,
  delete `node_modules`.

---

## Phase 4 — `chromiumoxide` + LLM oracle ⬜ Future

- Replace Playwright E2E tests with `chromiumoxide` (Chrome DevTools Protocol).
- LLM-as-oracle for snapshot review: a separate agent session compares
  rendered output against expected behaviour descriptions.
- `package.json`, `node_modules`, Playwright config deleted.

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
