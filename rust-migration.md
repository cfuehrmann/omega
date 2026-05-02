# Omega — Rust Migration

*Living document. Completed phases are summarised briefly; upcoming phases have full detail.*

---

## Status

| Phase | Status | Deliverable |
|---|---|---|
| 0 — Planning | ✅ Done | This document + architectural decisions |
| 1a — `omega-protocol` | ✅ Done | All 22 `OmegaEvent` variants, `StreamSignal`, serde round-trips; workspace tooling; honest types |
| 1b — `omega-core` (LLM loop) | ✅ Done | Anthropic + Ollama providers, retry loop, streaming; 0 surviving mutants |
| 1c — `omega-store` (Persistence) | ✅ Done | `ContextHash`, `SessionPaths`, `EventStore`, `ContextStore`; JSONC stripping; `spawn_blocking` append; 0 surviving mutants |
| 1d.0a — `omega-agent` core + scaffolds | ✅ Done | Agent loop, system prompt, error classifier, MockProvider + 6 integration tests, `omega-tools` stubs + dispatch, `omega-cli --help` |
| 1d.0b — tool body ports + CLI wiring | ✅ Done | 12 real tool implementations + 35 integration tests; `omega-cli run` end-to-end; `OmegaRustAgent` Harbor adapter |
| 1d.0c — mutant killing (`omega-tools`) | ✅ Done | 66 → 16 missed mutants; 2 production bugs found and fixed; surviving mutants fully classified |
| 1d.0d — eliminate external binary deps | ✅ Done | Replaced `rg`/`fd` subprocesses with `ignore`+`globset`+`regex`; refactored remaining boundaries; **0 missed** across `omega-tools` |
| 1d.1 — `omega-agent` advanced | ✅ Done | Pause/continue/abort, session resumption, compaction, model/effort switching (decomposed 1d.1a–e) |
| 1e — `omega-server` (WebSocket) | 🟡 In progress | tokio/axum server, session mgmt, WS streaming, HTTP static serving |
| 1f — Bridge (`ts-rs`) | ⬜ Upcoming | Generate `.d.ts` from Rust types, TS UI stays type-checked |
| 2 — Rust as primary driver | ⬜ Future | TS UI talks to Rust backend; TS CLI retired |
| 3 — Leptos UI rewrite | ⬜ Future | SolidJS → Leptos; TS deleted |
| 4 — `chromiumoxide` + LLM oracle | ⬜ Future | Playwright retired; pure-Rust browser tests |

---

## Why Rust (brief)

- **No escape hatches** — no `as any`, `// @ts-ignore`. The compiler refuses structurally.
- **Multi-provider** — Rust structs + serde + reqwest + SSE are cleaner than juggling multiple TS SDKs.
- **`insta`** — best snapshot-testing DX in any ecosystem.
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
│       ├── omega-tools/        ✅ done
│       ├── omega-agent/        ✅ done
│       └── omega-cli/          ✅ done
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

`Provider` trait, `AnthropicProvider` (SSE), `OllamaProvider` (NDJSON), `RetryingProvider<P>`.
All wiremock-fronted; no live API calls. Key notes: `AgentItem::Event` boxes `OmegaEvent`
(large_enum_variant); `LlmError::Transport` reachable via in-process flaky listener;
sequential wiremock via `.up_to_n_times(N)`. 0 survived, 2 timeouts (infinite-retry — expected).

### Phase 1c — `omega-store` (Persistence) ✅

`ContextHash`, `SessionPaths`, `EventStore`, `ContextStore`. Key: `spawn_blocking` for file I/O
(Tokio `pwrite` ignores `O_APPEND`); manual JSONC scanner; `serde(alias)` for legacy field names.
0 survived, 4 timeouts.

### Phase 1d.0a — `omega-agent` core + scaffolds ✅

`Agent` struct + `send_message` async-stream generator. All 12 tool stubs. `omega-cli --help`.
6 integration tests with `MockProvider` + real `omega_store`. 3 missed mutants in low-value
helpers (`now_iso()` ×2, `read_system_prompt_append` NotFound fallback). Acceptable.

### Phase 1d.0b — tool body ports + CLI wiring ✅

12 tools fully implemented; 35 integration tests; `omega-cli run` end-to-end.
`OmegaRustAgent` Harbor adapter added. `just rust-gate` passes.
`cargo mutants -p omega-tools`: 172 mutants — 87 caught, **66 missed**, 18 unviable, 1 timeout.
Missed mutants recorded as a baseline for Phase 1d.0c.

### Phase 1d.0c — mutant killing (`omega-tools`) ✅

Added ~50 targeted tests; fixed 2 production bugs. Final: **16 missed** (all classified;
all later eliminated by 1d.0d), 136 caught, 18 unviable, 2 timeouts.

**BUG 1 — `kill_group` silent failure (fixed):** Used `/usr/bin/kill -KILL -PGID`; util-linux
`kill` interprets a leading-hyphen numeric as a process-name search, silently discarding ESRCH.
Background processes spawned by timed-out bash commands were never killed. Fixed with
`sh -c "kill -9 -PGID"` (POSIX shell builtin calls `kill(-pgid, SIGKILL)` correctly).

**BUG 2 — `node_modules` recursion guard dead code (documented):** Early `continue` skips
the entry before the `!= "node_modules"` guard in the recursive branch can fire. `.git`
guard is live; `node_modules` guard is not. Harmless — exclusion still works via the earlier skip.

16 survivors in 4 groups: (A) grep/find fallback paths unreachable while `rg`/`fd` installed;
(B) 4 truly equivalent mutations; (C) 3 requiring signal-kill or race-window infrastructure;
(D) 4 requiring live Brave Search API key.

### Phase 1d.0d — Eliminate external binary dependencies (`omega-tools`) ✅

Replaced `rg`/`fd` subprocesses in `find_files.rs` and `grep_files.rs` with pure-Rust
`ignore::WalkBuilder` + `globset::Glob` + `regex::RegexBuilder`. Deleted `has_command`,
all fallback branches, and `run_subprocess`/`SubprocOutput` (retained in `fetch_url.rs`).

The 7 surviving non-fallback mutants from 1d.0c eliminated by seam refactors:
`unwrap_or(-1)` → `code: Option<i32>` in `fetch_url.rs`; `depth: usize` → `is_root: bool`
in `list_files.rs`; `evaluate()` pure helper in `wait_for_output.rs`; `check_status()` +
`render_results()` in `web_search.rs` (all pure unit-testable, no mocks needed).

**Final: 0 missed** (61 mutants, 59 caught, 2 unviable) across all four tool files.

---

## Phase 1d.1 — `omega-agent` advanced features ✅ Done

Five sub-phases; all closed with `cargo mutants -f` at **0 missed**.

| Sub-phase | Deliverable |
|---|---|
| 1d.1a | `set_model` / `set_effort` + `active_model` / `active_effort`; `extract_last_model_and_effort` |
| 1d.1b | `extract_resumption_basis`, `extract_summary_from_response`, `extract_description_from_response` |
| 1d.1c | `perform_resumption` + `seed_with_resumption_summary` (one-shot LLM call; history seeding) |
| 1d.1d | Server-side compaction — cross-crate (`omega-core` + `omega-agent`); history + hash clear on `Compacted` |
| 1d.1e | Pause / continue / abort seam — `ControlHandle` (Arc-backed), `ControlState`, `TurnGuard` RAII |

Key notes:
- **1d.1a:** Effort stored but not threaded onto `LlmRequest` (deferred — `omega-core` concern).
- **1d.1c:** `RESUMPTION_MODEL` hard-coded to `claude-sonnet-4-6`; `capEffortForModel` deferred.
- **1d.1d:** `context_management` on `LlmRequest` is opaque `Option<serde_json::Value>`; agent does **not** yet set it on outgoing requests.
- **1d.1e:** Control methods live on `ControlHandle` (not `Agent`) because `send_message(&mut self)` exclusively borrows the agent for the stream lifetime. 47 mutants: 23 caught, 22 unviable, 0 missed, 2 timeouts (genuine detections via test hang in race test).

**v0.1.4 tagged 2026-05-02. Harbor smoke (`prove-plus-comm`, Sonnet 4.6, `OmegaRustAgent`): reward = 1.0, 24 s inference, harness fully wired (rustup → cargo build → binary run → events.jsonl download).**

### Carry-forward deferrals

- `max_tokens` thinking-budget no-output recovery / mid-tool-call recovery (`maxTokensRecoveries`).
- `activeGeneration` superseded-generator guard — irrelevant until multi-WS server (1e).
- Effort threading onto `LlmRequest` + `capEffortForModel`.
- `context_management` request shape (auto-compaction trigger) — `omega-core` concern.

---

## Phase 1e — `omega-server` (WebSocket + HTTP) 🟡 In progress

New binary crate `omega-server`. Ports `src/web/server.ts` (954 lines) to axum/tokio.

### Important: TS server is single-session, single-WS

Read `src/web/server.ts` before writing any code. The key architectural fact: there is
**one persistent agent** and **one active WebSocket at a time**. `broadcast()` in the TS
code simply sends to the currently held `ws` reference — it is not a fan-out channel.
A reconnecting browser gets the same session (history replay) but replaces the WS reference.
Multi-session and multi-client fan-out are post-parity enhancements, not phase 1e scope.

### Deliverables

- `omega-server` binary: HTTP + WebSocket server matching the TS server's public contract.
- Single active session held at server scope: `Arc<Mutex<Option<ActiveSession>>>`.
- WebSocket streaming: all `OmegaEvent`s forwarded to the connected client; `{ type: "ready" }` sent after history replay.
- History replay on reconnect: reads `events.jsonl` via `EventStore`, filters `REPLAY_EXCLUDE` set (`ready`, `text`).
- Client→server messages: `reset`, `resume_session`, `user_message`, `pause`, `continue`, `abort`, `rename_session`.
- HTTP routes: `GET /sessions`, `GET /context?hashes=...`, `GET /files?prefix=...`, static file fallback.
- Static file serving: `tower-http::ServeDir` on `--public-dir` (defaults to `src/web/public/`).
- CLI: `omega-server [--port N] [--sessions-root PATH] [--public-dir PATH]` via `clap`.
- Graceful shutdown on `SIGINT` / `SIGTERM`.

### TS reference points

- `src/web/server.ts` — canonical reference. Read it fully before writing any code.
- `src/web/protocol.ts` — `ClientMessageSchema` (all message types the client sends).
- `src/events.ts` — `WsEvent` variants (any extra transport-only fields beyond `OmegaEvent`).
- `src/session-dir.ts` — `makeSessionDir`, `readSessionMetadata`, `updateSessionMetadata`.

### Key design decisions

**`ActiveSession`** struct (held inside the server-scope `Arc<Mutex<Option<ActiveSession>>>`):
- `agent: Agent` — exclusively owned; `send_message` borrows `&mut self`
- `controls: ControlHandle` — cloneable; WS handler calls pause/continue/abort on it
- `paths: SessionPaths` — for history replay and metadata updates
- `ws_tx: Option<tokio::sync::mpsc::UnboundedSender<WsMessage>>` — current WS sender, replaced on reconnect

**WS handler flow** — on upgrade: (1) lock session, clone `controls` + `paths`, replace `ws_tx`;
(2) unlock; (3) spawn WS write loop draining `ws_rx`; (4) replay history from `EventStore`;
(5) send `{ type: "ready" }`; (6) loop reading client frames → dispatch to session.

**Turn dispatch** — `user_message` acquires `&mut agent` (session lock held for the duration
or — better — agent wrapped in `Arc<tokio::sync::Mutex<Agent>>` so the lock is held only per
`send_message` call). Events streamed from `send_message` are forwarded to `ws_tx`. This is
the main lifetime puzzle; resolve it before writing other handlers.

**`GET /sessions`** — reads `sessions_root` directory, returns metadata list (matches TS `listSessions()`).

**`GET /context?hashes=h1,h2`** — reads `context.jsonl`, returns matching `ContextRecord` entries.

**`GET /files?prefix=p`** — reads working directory, returns completions (for the file-picker UI).

### Suggested workspace additions

```toml
axum            = { version = "0.8", features = ["ws"] }
tower-http      = { version = "0.6", features = ["fs", "cors"] }
tokio-tungstenite = "0.26"    # dev-dependency for test WS client only
```

`tokio` is already in the workspace; ensure `features = ["full"]`.

### Test seam strategy

- `TcpListener::bind("127.0.0.1:0")` for random port — no conflicts in parallel tests.
- `MockProvider` for all agent turns — no live API calls.
- `tokio-tungstenite` test client: connect, send `reset`, send `user_message`, collect WS frames, assert event sequence.
- Session data to a `TempDir` per test (real I/O, unique path, same pattern as omega-agent tests).
- Mutation bar: `cargo mutants -f` on each new source file, **0 missed**.

### Decomposition

| Sub-phase | Status | Deliverable |
|---|---|---|
| 1e.0 | ✅ Done | Crate skeleton; `GET /health`; `ServeDir` static serving; placeholder routes returning 501 |
| 1e.1 | ✅ Done | `ActiveSession`, `AppState`, `serve()`, `POST /api/sessions`, `GET /api/sessions` |
| 1e.2 | ✅ Done | WebSocket upgrade; `user_message` → turn → event stream; `pause`/`continue`/`abort`/`reset` |
| 1e.3 | ⬜ Upcoming | History replay on reconnect; `resume_session` |
| 1e.4 | ⬜ Upcoming | `resume_session`; `rename_session`; `GET /context`; `GET /files`; graceful shutdown |

### Phase 1e.0 — done (concise record)

New binary crate `rust/crates/omega-server/`. Stack: axum 0.8 (`ws` feature
ready for 1e.2), tower-http 0.6 (`fs`), clap derive, tokio `full`.

- `build_router(public_dir: &Path) -> Router`: `GET /health` → 200
  `{"status":"ok"}`; `/api/sessions`, `/ws`, `/context`, `/files` →
  `501` via `any(...)` (all methods, not just GET); `ServeDir` as
  `fallback_service` for static assets.
- `Args` (clap): `--port` (3000), `--sessions-root` (`.omega/sessions`),
  `--public-dir` (`src/web/public/`). Defaults match the TS server and
  `omega_store::SESSIONS_ROOT`.
- `main` is pure glue (`#[mutants::skip]`); all behaviour lives in
  testable helpers (`build_router`, `Args`).
- 8 integration tests in `tests/http.rs`, all binding `127.0.0.1:0`
  (parallel-safe). Live smoke test against the release binary confirmed
  `/health` → 200 and all four placeholders → 501.
- `cargo mutants -p omega-server`: 6 mutants — 2 caught, 4 unviable,
  **0 missed**.

**Carry-forward into 1e.1:** resolved — see 1e.1 record below.

### Phase 1e.1 — done (concise record)

`omega-store` and `omega-agent` added as `omega-server` dependencies.
`DEFAULT_SESSIONS_ROOT` in `cli.rs` is now `omega_store::SESSIONS_ROOT` (alias,
no duplicate literal).

**`omega-agent` changes:** `AgentConfig` gains `session_dir: PathBuf`;
`Agent::init()` writes `server_started` + `session_started` events to
`events.jsonl` (model, effort, system prompt recorded). Direct unit tests
in `omega-agent/tests/init.rs` (2 tests). `omega-cli` updated to thread
`session_dir` through.

**New structs:** `ActiveSession { agent: Arc<Mutex<Agent>>, controls:
ControlHandle, paths: SessionPaths, ws_tx: Option<UnboundedSender<Value>> }`
(placeholder `Value` type; concrete WS message type lands in 1e.2).
`AppState { active_session: Arc<Mutex<Option<ActiveSession>>>, sessions_root,
public_dir, provider: Arc<dyn Provider> }` — threaded via `Router::with_state`.

**`pub async fn serve(listener, state)`** extracted into `lib.rs`; `main` is
still `#[mutants::skip]` pure glue but is now smaller (calls `serve()`).
`MockProvider` lives in `omega-server/tests/` for integration tests.

**`POST /api/sessions`:** `make_session_dir` → `Agent::new` + `init()` →
slot replace → `201 Created` with `{ "dir": "<folder-name>" }` JSON body.

**`GET /api/sessions`:** reads `sessions_root`, filters by
`omega_store::session_dir_re()`, sorts newest-first, attaches
`read_session_metadata` per entry. Returns `[]` if root missing.
`folder_name_to_timestamp` converts `2025-07-11T09-14-22-037-…` →
`2025-07-11T09:14:22.037Z`.

14 integration tests in `tests/http.rs` (5 carried from 1e.0, 9 new):
POST→201 + `events.jsonl` non-empty, GET→`[]` for missing root, GET after
2 POSTs → length 2, newest-first ordering, metadata-after-rename,
`serve()` direct call (catches the `Ok(())` replacement mutant).

`cargo mutants -p omega-server`: 14 mutants — 8 caught, 6 unviable, **0 missed**.
`cargo mutants -p omega-agent --file …/agent.rs`: 22 mutants — 9 caught,
13 unviable, **0 missed**.

**Carry-forward into 1e.2:** `ws_tx` is `Option<UnboundedSender<serde_json::Value>>`
— replace `Value` with a concrete `WsMessage` type when the WebSocket handler
lands. The `POST /api/sessions` handler hard-codes `model: "claude-sonnet-4-6"`
and `cwd: env::current_dir()` — wire through proper config when 1e.2 adds the
full reset/resume flow.

### Phase 1e.2 — done (concise record)

**Scope:** WebSocket upgrade route, `user_message` turn dispatch, `pause` /
`continue` / `abort` / `reset` control frames, `agent_error` envelope on
handler errors. History replay on reconnect deferred to 1e.3.

**New module `src/ws_message.rs`:** `WsMessage` enum (`Ready`,
`AgentError(String)`, `Item(Box<AgentItem>)`) with `to_json()` + `to_text()`.
Untagged `AgentItem` serialisation gives the wire shape — `text`/`thinking`
signals, every `OmegaEvent` variant, all forwarded verbatim. 8 unit tests.

**`ActiveSession::ws_tx`:** type changed from
`Option<UnboundedSender<serde_json::Value>>` to
`Option<UnboundedSender<WsMessage>>`. The slot field is currently *write-only*
— nothing reads it back yet. 1e.3 history replay will read it from inside
the WS handshake to push the persisted event log to the new socket.

**`/ws` route (`router.rs::ws_handler` + `handle_socket`):** axum 0.8
`WebSocketUpgrade`. Per connection:
  1. Build `mpsc::unbounded_channel::<WsMessage>` and spawn a writer task that
     drains the receiver into the WS sink (closes on `send` error).
  2. If the slot already holds a session, install `tx` into its `ws_tx`.
  3. Send `WsMessage::Ready`.
  4. Read loop: `Message::Text` → `dispatch_text_frame`; `Message::Close` →
     break; binary/ping/pong ignored. Handler errors emit
     `WsMessage::AgentError(e)` instead of closing.
  5. Disconnect cleanup: `ws_tx = None` in slot; drop the local sender so the
     writer task exits.

**Frame parsing:** `enum ClientFrame` with `#[serde(tag = "type",
rename_all = "snake_case")]` covers `user_message`, `pause`, `continue`
(optional `content`), `abort`, `reset`. Unknown discriminators are rejected
at parse time.

**`user_message`:** acquires the agent `Arc<Mutex<Agent>>`, calls
`send_message(content, CancellationToken::new())`, drains the resulting
`AgentItemStream` into the WS channel. The whole turn runs in a *spawned*
task so `pause` / `abort` frames can be processed by the read loop
concurrently. The lock is held for the duration of the turn (per task spec
— single-session, single-WS, single concurrent turn).

**`pause` / `continue` / `abort`:** dispatched to
`ControlHandle::request_pause()` / `request_continue(content)` /
`request_abort()`. No-ops when no session is active.

**`reset`:** aborts any in-flight turn (so the orphan agent doesn't keep the
cwd / disk paths busy), runs `create_active_session` (the helper now shared
with `POST /api/sessions`), installs the *existing* WS `tx` into the new
session's slot, emits a fresh `Ready`.

**Tests:** `tests/ws.rs` — 7 integration tests using `tokio-tungstenite 0.26`
as the WS client + a local `MockProvider` (with optional per-item delay):
  1. Happy path: `reset` → `user_message` → `text` + `turn_end` frames.
  2. First reset creates the on-disk session dir.
  3. Pause during a turn → `turn_paused`; `continue` resumes → `turn_end`.
  4. Abort during a turn → `turn_interrupted` (mock pads turn 2 with
     `end_turn` so the test fails if `request_abort` is mutated away —
     without abort the agent runs to natural completion).
  5. Disconnect + reconnect — new WS gets `Ready`.
  6. Garbage payload → `agent_error` frame, socket stays open.
  7. `user_message` without a prior session → `agent_error`.
  Plus 4 direct unit tests in `router::tests` for `install_ws_tx` /
  `clear_ws_tx` (these helpers have no observable contract until 1e.3, so
  they are tested directly to keep mutation testing honest).

**Race-control trick:** the spawned turn task drains the agent stream as
fast as possible, so `pause` over the WS may race the post-tool-results
seam. The `MockProvider` exposes `set_item_delay(Duration)` which wraps the
stream with `.then(|x| async move { sleep(d).await; x })`. A 30 ms delay
is enough headroom for a localhost WS round-trip; the abort test reuses
the same knob.

**Existing test fixups:** `tests/http.rs` dropped `/ws` from the
placeholder-501 lists (it is now a real WS route; non-upgrade GETs return
426/400 from axum, not 501). Two doc comments fixed for the
`clippy::doc_markdown` lint that became hard-error after a clippy bump.

**`cargo mutants -p omega-server -f`** (per file):
  - `ws_message.rs`: 3 mutants — 3 caught, **0 missed**.
  - `session.rs`: 0 mutants (struct field type alias only).
  - `router.rs`: 28 mutants — 18 caught, 9 unviable, **1 equivalent**:
    `delete match arm Message::Close(_) in handle_socket`. Without the
    explicit arm, `Close` falls through to `_ => continue` and the next
    `reader.next().await` returns `None` because the socket is actually
    closed — the loop exits anyway. The arm is a one-poll-faster shortcut,
    not a behavioural guarantee.
  - `lib.rs`: 1 mutant — 1 caught, **0 missed**.

**Carry-forward into 1e.3:** `ActiveSession::ws_tx` is currently *write-only*.
1e.3 history replay must read it inside `handle_socket` (or a dedicated
`replay_history` helper) and push every persisted `OmegaEvent` from
`events.jsonl` before the read loop starts. The current `install_ws_tx`
becomes the natural insertion point. Also: `handle_user_message` captures
`tx` at dispatch time — mid-turn reconnects will not receive the running
turn's events. If multi-WS-per-turn delivery is wanted, switch to reading
`active.ws_tx` from the slot inside the turn task's forwarding loop.

---

## Phase 1f — Bridge (`ts-rs`) ⬜ Upcoming

`#[derive(ts_rs::TS)]` on all `omega-protocol` types. Committed `.d.ts` bindings so the
TS web client stays type-checked against the Rust protocol. Deleted entirely in Phase 3.

---

## Phase 2 — Rust as primary driver ⬜ Future

Rust `omega-server` binary replaces `src/cli.ts` + `src/web/server.ts`.
TS web client still served; all new features in Rust.

---

## Phase 3 — Leptos UI rewrite ⬜ Future

`omega-web` crate. Port `src/web/client/` component by component. Imports types from
`omega-protocol` directly. Once complete: delete `src/`, `ts-rs` derives, `node_modules`.

---

## Phase 4 — `chromiumoxide` + LLM oracle ⬜ Future

Replace Playwright with `chromiumoxide`. LLM-as-oracle for snapshot review.
Delete `package.json`, `node_modules`, Playwright config.

---

## Settled decisions — format and compatibility

**No backward compatibility with old `events.jsonl` files.** Honest types; no `#[serde(default)]`
shims; no legacy field remapping. Old logs are not supported by the Rust reader.

**No defaults baked into data shapes.** The `cargo mutants` finding on `default_effort()` is
the canonical example — a serde default is untestable by design.

---

## What is intentionally deferred

All of the following are post-parity improvements. Do not implement during port:

- Redesigned session resumption UX
- Streaming context compaction (server-side)
- OpenAI provider
- `cargo mutants` integration into CI
- `insta` snapshot tests for rendered Leptos components
- Rate-limit backpressure to UI
- Multi-session server (beyond TS parity)
- `capEffortForModel` and effort threading onto `LlmRequest`
- `context_management` request shape (auto-compaction trigger)
- `max_tokens` thinking-budget recovery (`maxTokensRecoveries`)
