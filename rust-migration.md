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
| 1e — `omega-server` (WebSocket) | ✅ Done | tokio/axum server, session mgmt, WS streaming, HTTP static serving |
| **BUG-A** — adaptive thinking + effort | 🔴 **Top priority** | Wire `thinking: adaptive` + `output_config.effort` into every Anthropic call |
| **BUG-B** — system prompt missing LLM Provider section | 🔴 **Top priority** | Add `platform.claude.com/llms.txt` guidance to `system_prompt.rs` |
| 1f — Bridge (`ts-rs`) | ✅ Done | 35 `.d.ts` files generated from Rust types; TS web client type-checked against them |
| 2 — Rust as primary driver | ✅ Done | TS UI talks to Rust backend; TS CLI retired |
| 2d — `session_renamed` envelope | ⬜ Next | Fix: rename UI shows the saved name |
| 3 — Leptos UI rewrite | ⬜ After 2d | SolidJS → Leptos; TS deleted |
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
| 1e.3 | ✅ Done | History replay on reconnect (filtered `events.jsonl` push before `Ready`) |
| 1e.4 | ✅ Done | `resume_session`; `rename_session`; `GET /api/context`; `GET /api/files`; graceful shutdown |

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

**Carry-forward into 1e.3:** resolved — see 1e.3 record below.

### Phase 1e.3 — done (concise record)

**Scope:** History replay on WebSocket reconnect. On every WS upgrade
(before `Ready`), the server streams persisted events from `events.jsonl`
through the new socket, filtered by `REPLAY_EXCLUDE`.

**`EventStore::read_all()`** added to `omega-store`: reads all parseable
JSON objects line-by-line; skips blank or malformed lines (mirrors TS
`loadReplayEvents`); returns `Ok(vec![])` when the file is absent;
propagates non-NotFound I/O errors as `Err`.

**New symbols in `router.rs`:**
- `REPLAY_EXCLUDE: &[&str]` — `["ready", "text"]`; doc comment cites
  `src/web/server.ts` line by name.
- `pub fn should_replay(event_type: &str) -> bool` — pure helper;
  `!REPLAY_EXCLUDE.contains(&event_type)`; unit-testable without WS.
- `async fn replay_history(state, tx)` — holds the session lock only to
  clone `events_file`; does all file I/O without the lock; deserialises
  each surviving `Value` as `OmegaEvent` and sends `WsMessage::Item`.

**Sequencing in `handle_socket`:** `install_ws_tx` (installs sender into
slot) → `replay_history` (file read, no lock held) → `WsMessage::Ready`.
`ws_tx` is installed *before* replay so any concurrent turn's live events
reach the new socket interleaved after the replay batch — no race.

**Updated existing test:** `reconnect_new_ws_receives_ready` now uses
`recv_until_type("ready")` instead of asserting the first frame is `ready`,
because init events (server_started + session_started) precede `ready` on
reconnect.

**Tests:** 19 new tests total across three files:
- `omega-store/tests/event_store.rs` — 8 tests: missing file, empty file,
  malformed-line skip, order preservation, round-trip, non-NotFound I/O
  error propagation (dir-as-file catches the `NotFound`-guard mutant).
- `omega-server/src/router.rs` (unit) — 8 `should_replay` tests: both
  excluded types (`ready`, `text`) + representative included types
  (server_started, session_started, user_message, turn_end, llm_response,
  tool_call, empty string).
- `omega-server/tests/ws.rs` — 3 WS integration tests: full-turn replay
  with synthetic `{"type":"text"}` injected into `events.jsonl` (verifies
  filter live), empty-events-file → just Ready, init-only → server_started
  + session_started + Ready.

**`cargo mutants -f`:**
- `omega-store/src/event_store.rs`: 7/7 caught, **0 missed**.
- `omega-server/src/router.rs`: 23 caught, 9 unviable, **1 equivalent**:
  same `Message::Close` arm as 1e.2 — deletion leaves identical behaviour
  via the `while let None` exit path; documented in source comment.

**Carry-forward into 1e.4:** resolved — see 1e.4 record below.

### Phase 1e.4 — done (concise record)

**Scope:** Remaining WS frames (`resume_session`, `rename_session`),
HTTP routes (`GET /api/context`, `GET /api/files`), and graceful
shutdown on SIGINT/SIGTERM.

**`ContextStore::read_all()`** added to `omega-store`: reads
`context.jsonl` line-by-line, skips blanks/malformed, returns
`Ok(vec![])` when file absent, propagates non-NotFound I/O errors.
Mirrors `EventStore::read_all` semantics.

**New `ClientFrame` variants in `router.rs`:**
- `ResumeSession { session_dir }` (serde camelCase: `sessionDir`).
  Aborts the active turn, loads the target session's `events.jsonl`,
  derives `basis` via `omega_agent::extract_resumption_basis`, reads
  prior `name`, creates a fresh active session, drives
  `agent.perform_resumption(basis, session_dir, name, cancel)` to
  completion, writes `resumed_from` into the new session's metadata,
  replays history, then `Ready`.
- `RenameSession { name }`. Calls `omega_store::update_session_metadata`
  on the active session's `paths.dir` — no agent interaction.

**New HTTP routes:**
- `GET /api/context?hashes=h1,h2` — `ContextStore::read_all` filtered by
  the requested hash set, **preserving request order**, dropping misses.
- `GET /api/files?prefix=p` — path completions matching `prefix` against
  cwd (or absolute root for `/`-prefixed inputs). Sorted directories-first
  then alphabetically, capped at `MAX_FILE_COMPLETIONS = 50`. The
  comparator `dir_first_then_alpha` is extracted as a free function so
  every match arm is mutation-tested directly — embedded `sort_by`
  closures don't reliably exercise all branches.

**Graceful shutdown in `lib.rs`:** `serve` now wraps `axum::serve` with
`with_graceful_shutdown(shutdown_signal(state))`. `wait_for_signal`
selects on SIGINT and SIGTERM (Unix) or `ctrl_c()` (other). On signal,
`perform_shutdown` snapshots `(controls, events_file, current_turn)`
from the active session under one lock, calls `controls.request_abort()`,
awaits the turn handle with `TURN_DRAIN_DEADLINE = 2s`, then appends a
`server_stopped` event (`outcome: Clean`, `reason: None`) before
`axum::serve` returns. New `current_turn: Option<JoinHandle<()>>` field
on `ActiveSession` is populated by `handle_user_message` after spawning.

**Tests added:**
- `omega-store/tests/context_store.rs` — 5 `read_all` tests (empty,
  multi-record round-trip, malformed-line skip, missing-file → empty,
  non-NotFound I/O error propagation).
- `omega-server/src/router.rs` (unit) — `ClientFrame` parse tests for
  both new variants; `dir_first_then_alpha` exercises all three arms
  directly; six `list_files_for_completion` tests covering filter,
  trailing-slash on dirs, absolute prefix, dir-prefix, max-cap,
  unreadable-dir.
- `omega-server/tests/http.rs` — `/api/context` returns records in
  request order; `/api/files` returns absolute-prefix completions;
  graceful-shutdown spawns the release binary, sends SIGTERM via
  `nix::sys::signal::kill`, asserts exit-0 and `server_stopped` in
  `events.jsonl`.
- `omega-server/tests/ws.rs` — `rename_session` updates metadata;
  `resume_session` emits `resuming_session` referencing the source dir
  and writes `resumed_from` into the new session's metadata.

**Workspace lint constraint:** `unsafe_code = forbid` is non-overridable,
so the SIGTERM test uses `nix` (safe wrappers) rather than raw `libc`.

**`cargo mutants -f`:**
- `omega-store/src/context_store.rs`: 5 caught, 3 unviable, **0 missed**.
- `omega-server/src/session.rs`: 0 mutants (struct field only).
- `omega-server/src/router.rs`: 42 caught, 23 unviable, **1 equivalent**:
  same `Message::Close` arm carried forward from 1e.2/1e.3.
- `omega-server/src/lib.rs`: 6 caught, **0 missed**. `now_iso` is pinned
  by an explicit format-shape assertion (length 24, ISO-8601 separators,
  trailing `Z`).

---

## Phase 1f — Bridge (`ts-rs`) ✅ Done

### Concise record

**ts-rs 12** added as an optional dep behind the `ts-bindings` feature flag in
`omega-protocol`, `omega-core`, and `omega-store`. **35 `.d.ts` files** generated to
`rust/bindings/` and committed.

**Types exported:**
- `omega-protocol` (30): `OmegaEvent`, all 22 variant structs, `StreamSignal`,
  `TurnMetrics`, `LlmResponseUsage`, `ServerStopOutcome`, `InterruptReason`,
  `ContinueMode`, `LlmRetryReason`.
- `omega-core` (2): `Role`, `ContentBlock`.
- `omega-store` (3): `ContextRecord`, `SessionMetadata`, `ContextHash`.

**Key decisions:**
- `serde-compat` (on by default in ts-rs 12) reads existing `rename_all` /
  `skip_serializing_if` / `tag` serde attributes — no annotation duplication.
- `#[ts(optional)]` added *explicitly* to every `Option<T>` field that has
  `skip_serializing_if = "Option::is_none"` because ts-rs only auto-optionalises
  when `#[serde(default)]` is also on the field — and no defaults are permitted
  (see settled decisions).
- `#[ts(type = "unknown")]` on every `serde_json::Value` field — avoids pulling
  in the `serde-json-impl` feature.
- `TS_RS_LARGE_INT = "number"` in `rust/.cargo/config.toml` → all `i64` fields
  become `number` in TypeScript (token counts, byte counts, etc.).
- `TS_RS_EXPORT_DIR = "../bindings"` (relative to `rust/.cargo/`) funnels all
  three crates' output into the single `rust/bindings/` directory.

**`just rust-bindings` recipe:** runs
`cargo test -p omega-{protocol,core,store} --features ts-bindings -- export_bindings`
sequentially; the ts-rs `#[ts(export)]` derive macro emits one `export_bindings_*`
test per type that writes the `.ts` file.

**Drift guard in `just rust-gate`:**
```
just rust-bindings
git diff --exit-code rust/bindings/
```
Stale bindings (Rust type changed without regenerating) fail the pre-commit gate.

**TypeScript changes:**
- `src/events.ts` rewritten as thin re-exports from `rust/bindings/`. Each event
  struct type is intersected with `{ type: "X" }` to restore the discriminator
  field for backward compatibility (generated struct types lack it — the tag lives
  on the Rust enum). `StreamSignal` is defined locally as `TextSignal | ThinkingSignal`
  (same shape as generated; keeps the named aliases referenced).
- `events.schema.ts`: removed `.nullable()` from the three optional
  `LlmResponseUsage` fields — Rust serialises `None` as *absent*, never `null`;
  added missing `reason` field to `LlmRetrySchema`.
- `context-hash.test.ts`: widened `Set<ContextHash>` to `Set<string>` because
  the generated `LlmCallEvent.contextHashes` is `Array<string>` (the
  omega-protocol `ContextHash` is a type alias, not a newtype).
- `knip.json`: `src/events.ts` added as an entry point (all exports are public
  events API).
- `src/rust-bindings-roundtrip.test.ts`: 5 tests verifying that Rust-serialised
  JSON validates against the TypeScript zod schemas and that the generated types
  type-check correctly (`session_started`, `turn_end` + metrics, `llm_response` +
  usage, `StreamSignal` shape, `llm_retry` with `reason`).

**Bar:** `cargo build -p omega-server --release` ✅ · `just rust-gate` ✅ ·
`bun test` 559+5 ✅ · `just test-browser` 109/109 ✅.

---

## Phase 2 — Rust as primary driver ✅ Done

Rust `omega-server` binary replaces `src/cli.ts` + `src/web/server.ts`.
TS web client still served by the Rust binary; all new features in Rust.

### Decomposition

| Sub-phase | Status | Deliverable |
|---|---|---|
| 2a | ✅ Done | Wire `model`/`effort` from `reset` + `POST /api/sessions` through `AgentConfig`; emit `session_info` WS message |
| 2b | ✅ Done | Align URL paths (`/api/*` vs `/`) or update web-client fetch calls; switch replay to `history` frame |
| 2c | ✅ Done | Cut over: Playwright + Justfile use `omega-server`; `src/cli.ts` + `src/web/server.ts` deleted |
| 2d | ⬜ Next | Broadcast `session_renamed` envelope from `handle_rename_session` so the rename UI updates |

### Phase 2a + 2b — done (concise record)

Delivered together in one commit (`214817b`).

**Three new `WsMessage` variants** (`omega-server/src/ws_message.rs`):
- `SessionInfo { dir, model, effort, cwd, name: Option<String> }` — `name`
  field omitted from JSON when `None`. Wire shape matches the TS server's
  `buildSessionInfo()`.
- `History { events: Vec<OmegaEvent>, streaming: bool }` — `streaming` field
  omitted when `false` (matches TS `...(isStreaming ? { streaming: true } : {})`).
- `ResetDone` — `{"type":"reset_done"}`.
Each variant has dedicated unit tests.

**Replaced per-event replay with a single `History` frame.** Emit sequences in
`router.rs`:
- WS connect (active session): `SessionInfo → History(events, streaming=isTurnStreaming) → Ready`
- WS connect (no session): `Ready` only
- `reset`: `SessionInfo → History([]) → ResetDone → Ready`
- `resume_session`: `SessionInfo → History(init events) → [live resumption Items] → Ready`

`is_streaming` computed via `JoinHandle::is_finished()` on the optional
`current_turn`. The `REPLAY_EXCLUDE` filter (`ready`, `text`) is applied when
building `History.events` exactly as it was for per-event replay.

**Model / effort wiring.** `AgentConfig` gained `effort: Option<String>`;
`Agent::new` falls back to `DEFAULT_EFFORT` when `None`. `POST /api/sessions`
accepts an optional JSON body `{ model?, effort? }`. `ClientFrame::Reset` gained
optional `model` / `effort` fields, plumbed into `create_active_session`.

**URL alignment.** `App.tsx` switched its three `fetch()` calls (`/sessions`,
`/context`, `/files`) to the `/api/` prefix. To keep Playwright (still on the
TS server until Phase 2c) green, both `src/web/server.ts` and
`e2e/fixtures/test-server.ts` now accept the legacy *and* `/api/`-prefixed
paths. The aliases retire with the TS server in 2c.

**Test updates.** `tests/ws.rs` replay assertions rewritten to expect the outer
sequence `[session_info, history, ready]` and to inspect `history.events`
instead of individually replayed `Item` frames. Three router unit tests added
for `Reset { model, effort }` parsing.

**Bar:** `just rust-gate` ✅ · `just test` (559 bun + 109 Playwright) ✅.

### Phase 2c — done (concise record)

**Scope:** Playwright real-server project, Justfile, and production `just server`
recipe all cut over to the Rust `omega-server` binary. `src/cli.ts` and
`src/web/server.ts` deleted; the TS web client (SolidJS) is now served by the
Rust binary in production. Net diff: **+1022 / −2710 lines**, 29 files.

**New crate `omega-mock-server`** at `rust/crates/omega-mock-server/`. Wires
`omega_server::serve` through a deterministic `MockProvider` that mirrors the
(now-deleted) `e2e/fixtures/real-server.ts` routing — `MULTI_TOOL_TEST`,
`CONCURRENT_TOOLS_TEST`, `LONG_STREAM_TEST`, `TWO_PAUSES_TEST`,
`abort_sleep_test`, `RESUME_BASIS_TEST`, default `pong`, plus the resumption
summary/description path. Per-turn call index tracking gives the multi-tool
tests one tool per LLM round. Includes a control HTTP API
(`/control/llm-calls`, `/control/reset-calls`) on port 3004 for replay specs.

**Justfile.** New `rust-build-server` and `rust-build-mock-server` recipes;
`test`, `test-browser`, `test-browser-log`, and `e2e` depend on them. `just
server` runs the release `omega-server` binary directly.

**`playwright.config.ts`.** `real-server` `webServer` command builds and runs
`mock-omega-server`. Both projects set
`gracefulShutdown = { signal: "SIGTERM", timeout: 5000 }` — every CI run
exercises the 1e.4 SIGTERM path.

**Rust router additions for parity with the deleted TS server:**
- `ClientFrame::UserMessage` accepts both `"user_message"` and `"message"`
  via `#[serde(alias)]` (the SolidJS client sends the latter).
- New variants + handlers: `SetModel`, `SetEffort`, `DeleteSession`. Each
  refreshes `ActiveSession::info_cache` so subsequent `SessionInfo` broadcasts
  reflect the change.
- `WsMessage::SessionInfo` gained `turnState` (idle / running /
  pause_requested / paused). `WsMessage::SessionDeleted` added.
- `ActiveSession` gained `turn_state: Arc<Mutex<String>>` and
  `info_cache: Arc<Mutex<SessionInfoCache>>` so any handler can broadcast a
  fresh `SessionInfo` *without* locking the agent (which is held by the
  streaming task for the whole turn — naive `build_session_info` deadlocked).
- A pure helper `next_turn_state_for(&OmegaEvent)` lets the streaming loop
  derive transitions from the events it already forwards (`UserMessage` →
  running, `TurnPaused` → paused, `TurnContinued` → running, `TurnEnd` /
  `TurnInterrupted` → idle).
- `handle_user_message` and `handle_resume_session` route streamed events
  through `send_to_active(&state.active_session, msg)` — looking up the
  *current* `ws_tx` per send rather than capturing a clone — so events emitted
  after a browser reload reach the new connection. Fixes the
  pause-during-turn → reload → continue path.
- `handle_pause` broadcasts the `pause_requested` event itself (the agent's
  `request_pause` persists but does *not* yield through the stream) plus the
  resulting `pause_requested` `turnState` transition.

**TS deletes:**
- `src/cli.ts`, `src/cli.test.ts`
- `src/web/server.ts` — helpers (`closeOpenTurn`, `shouldLogEvent`,
  `listFilesForCompletion`) extracted to `src/web/server-helpers.ts`,
  imported by the surviving `e2e/fixtures/test-server.ts` mock and two
  related unit tests
- `e2e/fixtures/real-server.ts` — replaced by `mock-omega-server`
- Obsolete TS server tests covered by Rust integration tests:
  `src/entry.test.ts`, `src/web/context-lookup.test.ts`,
  `src/web/reset-init-events.test.ts`, `src/web/pause-ws.test.ts`
- Dead exports after the deletes: `makeDefaultCreateMessageStream`
  (src/agent.ts), `readEnvPort` (src/env.ts)
- `/api/` aliases added in 2b to `src/web/server.ts` and
  `e2e/fixtures/test-server.ts` (the `test-server.ts` aliases were the
  only remaining ones; `test-server.ts` itself stays as the chromium
  events-only mock fixture)

**`knip.json`.** Scope extended to `e2e/` (`"e2e/**/*.spec.ts"` and
`"e2e/fixtures/test-server.ts"` as entries; `"e2e/**/*.ts"` in `project`)
so `test-server.ts` and the spec files count as consumers of the surviving
web / session helpers.

**Bar:** `just rust-gate` ✅ · 109/109 browser tests ✅ ·
533/533 TS unit tests ✅ · pre-commit gate exit-0.

**Followup deferred (not blocking):** `bench/omega_agent.py` still references
the deleted `src/cli.ts`. Should be retargeted at `rust/target/release/omega`
(the omega-cli binary). Bench is not on the test path.

### Phase 2d — `session_renamed` envelope on rename (next)

**Bug.** Renaming a session in the UI accepts the input and saves on disk,
but the name field empties out again afterwards. Reproduces against the
Rust backend; the user reports the same UX existed against the deleted TS
server too — long-standing, surfaced by the cut-over.

**Diagnosis.** `handle_rename_session` (`rust/crates/omega-server/src/router.rs`)
updates `session.jsonc` on disk and refreshes `ActiveSession::info_cache`,
but sends *nothing* on the WebSocket. The SolidJS client expects a
`{type: "session_renamed", sessionDir, name}` envelope — already declared
in `src/web/protocol.ts:128`, handled in `src/web/client/state.ts:829`
(updates `sessionName` if the dir matches) and `src/web/client/App.tsx:265`
(updates the picker's renamed-sessions map). Without that frame the
client's `sessionName` keeps its previous value (typically `""` for a
fresh session).

The Rust `WsMessage` enum has *no* `SessionRenamed` variant — grep returns
zero matches across `rust/crates/`. The existing test
`rename_session_updates_metadata_for_active_session`
(`rust/crates/omega-server/tests/ws.rs:754`) enshrines the bug with a
comment reading *"No frame is emitted on success"* — it asserts only the
on-disk metadata.

**Fix (server-side only):**

1. Add `SessionRenamed { session_dir: String, name: String }` to
   `WsMessage` in `omega-protocol/src/ws_message.rs` (mirror the existing
   `SessionDeleted` variant: same `ts-rs` derives, same `#[serde(rename_all
   = "camelCase")]` so the wire field is `sessionDir`).
2. In `handle_rename_session`, after the disk write + cache refresh, send
   `WsMessage::SessionRenamed { session_dir: <basename of paths.dir>, name }`
   on `tx`. Use the *relative* dir name to match the client's matching logic
   (`state.sessionDir.endsWith("/" + event.sessionDir)`).
3. Update the existing test to assert the new envelope is received in
   addition to the on-disk write — drop the misleading comment.
4. Add a Playwright spec (e.g. `e2e/web-ui-rename-session.spec.ts`) that
   drives the rename UI end-to-end and asserts the displayed name persists
   after save. This is the real-world failure mode and it's not currently
   covered — hence the bug surviving Phase 2c's gate.

No client changes needed (the handler already exists). No protocol
renegotiation — client and server protocol files have been out of sync
since before the cut-over. Estimated diff: ~30 LOC across
`ws_message.rs`, `router.rs`, the unit test, plus the new Playwright spec.

### Running the UI in real life

With Phase 2 complete, the Rust binary is the production server:

```
export ANTHROPIC_API_KEY=...
just web-build              # bundles the SolidJS client into src/web/public/
just rust-build-server      # builds rust/target/release/omega-server
just server                 # runs it on port 3000
```

`just server` accepts pass-through args (`just server --port 4000
--sessions-root /tmp/omega-sessions`). The binary uses `AnthropicProvider` —
production LLM, real cost. Sessions persist to `.omega/sessions/` by default.

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

---

## BUG-A — Adaptive thinking + effort not sent to Anthropic 🔴 Top priority

**Observed:** Session `2026-05-02T22-49-42-372-4d68835d` — every `llm_response` has
`thinking: False`. The agent produced zero thinking blocks across 50+ API calls.

**Root cause:** Two gaps, both deferred in Phase 1d.1:

1. `ThinkingConfig` in `omega-core/src/anthropic.rs` only has `Enabled { budget_tokens }`
   (the old explicit-budget form). There is no `Adaptive { display: String }` variant.
   `ModelConfig.thinking_budget` is always `None` in `agent.rs`, so no `thinking` field
   is ever included in the Anthropic request body.

2. `ModelConfig` has no `effort` field; `output_config: { effort }` is therefore never
   serialised. The TS agent sends it on every turn via
   `output_config: { effort: capEffortForModel(this.activeEffort, activeModel) }`.

**Fix — three files:**

*`omega-core/src/types.rs`*
- Add `pub adaptive_thinking: bool` to `ModelConfig`. Default `false` (keeps existing
  tests passing with zero code changes). Ignore on non-Anthropic providers.
- Add `pub effort: Option<String>` to `ModelConfig`.

*`omega-core/src/anthropic.rs`*
- Add `Adaptive { display: String }` to `ThinkingConfig`.
- Map `config.adaptive_thinking == true` → `ThinkingConfig::Adaptive { display: "summarized" }`.
- Add `output_config` struct (serialises as `{ "effort": "..." }`, skipped when effort is
  `None`) to `AnthropicRequestBody`.
- Regenerate/update the `anthropic__request_body_kitchen_sink` snapshot.

*`omega-agent/src/agent.rs`*
- In both the main-turn `LlmRequest` builder and the resumption-summary builder, set
  `config.adaptive_thinking = true` and `config.effort = Some(capEffortForModel(active_effort, model))`.
- `capEffortForModel` logic: `xhigh` only for `claude-opus-4-7`; `max` only for
  `claude-opus-4-6`/`claude-opus-4-7`; otherwise cap at `high`. Mirror `src/agent.ts`.

**Tests:** Add an omega-agent integration test that constructs a `MockProvider` response
and asserts the `LlmRequest` received by the provider contains
`thinking_type == Adaptive` and a non-None `effort`. Add/update `omega-core` unit tests
for the new serialisation shapes.

**No protocol or persistence changes needed** — thinking/effort are request-only fields;
the response side (`LlmResponseEvent`, context storage) is unaffected.

---

## BUG-B — Rust system prompt missing `## LLM Provider` section 🔴 Top priority

**Observed:** Session `2026-05-02T22-49-42-372-4d68835d`:
- `web_search` → `BRAVE_SEARCH_API_KEY is not set` (no Brave key in env).
- `fetch_url` to `https://docs.anthropic.com/...` → `request failed` (JS-rendered,
  Cloudflare-blocked; plain HTTP fails).
- `fetch_url` to `https://raw.githubusercontent.com/...` → succeeded (plain HTTP works).

**Root cause:** `rust/crates/omega-agent/src/system_prompt.rs` (`core_prompt()`) is missing
the `## LLM Provider` section that lives in `AGENT.md`. That section tells the agent:
> *To look up Anthropic/Claude API documentation: fetch `https://platform.claude.com/llms.txt`
> to get an indexed list of all docs pages (each entry links to a `.md` URL), find the
> relevant page, then fetch that specific `.md` URL with `fetch_url`.  Individual pages fit
> comfortably within the 20 000-char `fetch_url` limit.*

Without this guidance the agent guesses `docs.anthropic.com` URLs which are JS-rendered and
fail. `platform.claude.com/*.md` URLs return static Markdown and are reliably fetchable.
The Brave-key gap is a separate ops issue (set `BRAVE_SEARCH_API_KEY` in the server
environment), but the model can work around it entirely via `fetch_url` +
`platform.claude.com` if the system prompt points it there.

**Fix — one file:**

*`omega-core/src/system_prompt.rs`* (`core_prompt()` function)
- Add the `## LLM Provider` section verbatim from `AGENT.md`, placed between `## Design
  discipline` and `## Bug fixes` (matching the order in `AGENT.md`). Section text:

```
### LLM Provider

Omega is Anthropic-only. The supported models are:

- `claude-sonnet-4-6` — default, fast
- `claude-opus-4-6` — slower, more capable
- `claude-opus-4-7` — most capable; step-change improvement in agentic coding over 4.6

To look up Anthropic/Claude API documentation: fetch `https://platform.claude.com/llms.txt`
to get an indexed list of all docs pages (each entry links to a `.md` URL), find the
relevant page, then fetch that specific `.md` URL with `fetch_url`. Individual pages fit
comfortably within the 20 000-char `fetch_url` limit.
```

**Tests:** Update the existing `core_prompt_substitutes_cwd_and_tokens` test (or add a
sibling) to assert that `"platform.claude.com/llms.txt"` appears in the prompt.

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
- ~~`capEffortForModel` and effort threading onto `LlmRequest`~~ → **see BUG-A above**
- `context_management` request shape (auto-compaction trigger)
- `max_tokens` thinking-budget recovery (`maxTokensRecoveries`)
