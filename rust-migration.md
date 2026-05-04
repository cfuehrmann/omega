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
| **BUG-A** — adaptive thinking + effort | ✅ Done | Wire `thinking: adaptive` + `output_config.effort` into every Anthropic call |
| **BUG-B** — system prompt missing LLM Provider section | ✅ Done | Add `platform.claude.com/llms.txt` guidance to `system_prompt.rs` |
| 1f — Bridge (`ts-rs`) | ✅ Done | 35 `.d.ts` files generated from Rust types; TS web client type-checked against them |
| 2 — Rust as primary driver | ✅ Done | TS UI talks to Rust backend; TS CLI retired |
| 2d — `session_renamed` envelope | ✅ Done | Server emits `session_renamed` after rename; rename UI updates without reload |
| 3.0 — Leptos scaffold | ✅ Done | `frontends/leptos/` crate; `/leptos/` mount on `omega-server`; smoke spec green |
| 3.1 — Protocol + reactive store | ✅ Done | Typed `WsMessage` parsing; `SessionStore` reducer; `/leptos/` debug dump |
| 3.2 — Leptos session picker | ✅ Done | `SessionListStore` + picker UI; `/leptos/` lists/creates/renames/deletes; debug dump moved to collapsible panel |
| 3.3 — Leptos conversation feed | ✅ Done | `event_view.rs` pure projection; `feed.rs` component; `/leptos/` renders every `OmegaEvent` variant + live streaming text + auto-scroll seam |
| 3 — Leptos UI rewrite | 🟡 In progress | SolidJS → Leptos; TS deleted (3.0–3.1 done; 3.2–3.7 ahead) |
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
├── rust/                       ← Cargo workspace (backend; native targets)
│   ├── Cargo.toml
│   └── crates/
│       ├── omega-protocol/     ✅ done  (shared types — also consumed by frontends/leptos/)
│       ├── omega-core/         ✅ done
│       ├── omega-store/        ✅ done
│       ├── omega-tools/        ✅ done
│       ├── omega-agent/        ✅ done
│       ├── omega-cli/          ✅ done
│       ├── omega-server/       ✅ done  (HTTP + WS; serves both bundles)
│       └── omega-mock-server/  ✅ done  (Playwright fixture binary)
├── frontends/                  ← alternative web frontends (Phase 3.0+)
│   └── leptos/                 🟡 in progress  (standalone wasm32 Cargo workspace)
├── src/                        ← TypeScript SolidJS frontend (frozen; deleted at 3.7)
├── e2e/                        ← Playwright (retires in Phase 4)
├── Justfile
└── package.json
```

**Two-bundle co-existence (3.0–3.6):** `omega-server` mounts both frontends.
`/` → SolidJS (`src/web/public/`); `/leptos/` → Leptos (`frontends/leptos/dist/`).
`/ws`, `/api/*`, `/health` are shared. Cutover at 3.7 swaps the fallback `ServeDir`.

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
| 2d | ✅ Done | Broadcast `session_renamed` envelope from `handle_rename_session` so the rename UI updates |

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

### Phase 2d — done (concise record)

**Scope (option c — server-side only, Playwright spec skipped).** The
Leptos client (Phase 3) will inherit a correct protocol from day one;
writing a Playwright spec for code about to be deleted was rejected as
throwaway coverage.

**Implementation:** `WsMessage::SessionRenamed { session_dir, name }`
added to `omega-server/src/ws_message.rs` (lives in the server crate, not
`omega-protocol` — server-only wire shape, same as `SessionDeleted`). Wire
projection: `{"type":"session_renamed","sessionDir":...,"name":...}`.
`handle_rename_session` in `router.rs` now sends the envelope on `tx`
after the disk write + `info_cache` refresh, using the client-supplied
`session_dir` (basename) so the SolidJS client's
`state.sessionDir.endsWith("/" + event.sessionDir)` match works for both
active and inactive targets.

**Tests in `tests/ws.rs`:** `rename_session_updates_metadata_for_active_session`
asserts both the on-disk metadata and the broadcast envelope
(`type/sessionDir/name`). A second test covers renaming a non-active
session from the picker. Three serialisation unit tests in
`ws_message.rs` lock down the JSON shape and round-trip.

**Bar:** `cargo test -p omega-server` 59+15+14+16 ✅.

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

## Phase 3 — Leptos UI rewrite 🟡 In progress

**Crate `omega-web`** at `frontends/leptos/` (standalone Cargo workspace,
wasm32-only). Ports `src/web/client/` component by component. Imports types
from `omega-protocol` directly (no more ts-rs `.d.ts` round-trip). Once complete:
delete `src/`, the ts-rs derives, and `node_modules`.

The directory sits under `frontends/` rather than inside `rust/crates/` to
model "alternative frontends" as siblings: today there's one wasm frontend;
tomorrow there could be others. The backend crates are exclusively under
`rust/crates/`. Type sharing flows through a `path = "../../rust/crates/omega-protocol"`
dep — the only thing the wasm and native sides have in common.

### Co-existence strategy — "don't brick Omega before cutover"

The SolidJS UI stays the production frontend until Leptos reaches parity.
Both bundles ship in the same Rust binary; the URL decides which one runs.

```
localhost:3000/         → SolidJS (src/web/public/, Vite-built)
localhost:3000/leptos/  → Leptos  (frontends/leptos/dist/, trunk-built)
        │
        └── both connect to the same /ws and /api/* on omega-server
```

Cutover at the end of Phase 3 is a one-line change: swap which bundle the
`/` static-route fallback serves. Rollback is the inverse one-liner.

### Decomposition

| Sub-phase | Status | Deliverable |
|---|---|---|
| 3.0 | ✅ Done | `frontends/leptos/` crate scaffold; `/leptos/` mount on `omega-server`; hello-world page that renders `Ready` from a real `/ws` connection |
| 3.1 | ✅ Done | Protocol types + WS client: deserialise every `WsMessage` variant via `omega-protocol`; central reactive store for session state |
| 3.2 | ✅ Done | Session picker (list, create, rename, delete) — first feature surface with full read+write WS traffic |
| 3.3 | ✅ Done | Conversation feed: render every `OmegaEvent` variant + streaming `text`/`thinking` signals; auto-scroll seam |
| 3.4 | ⬜ Next | Composer: user-message send, pause / continue / abort, model + effort switchers; file-picker autocomplete via `/api/files` |
| 3.5 | ⬜ | Context inspector (`/api/context`); resume-session flow; LLM-call detail expander |
| 3.6 | ⬜ | Visual parity pass; `leptos::ssr::render_to_string` + `insta` snapshot tests per component (TEST-ARCH-5 lands here) |
| 3.7 | ⬜ | Cutover: route `/` to Leptos; delete `src/`, ts-rs derives, `package.json`, `node_modules`; retire SolidJS Playwright specs whose surface is covered by snapshot tests |

### Phase 3.0 — done (concise record)

**Scope:** establish the Rust→wasm→browser toolchain end-to-end with zero
risk to the SolidJS UI.

**New crate `frontends/leptos/`** (`omega-web`). Standalone Cargo workspace
(empty `[workspace]` table) so `cargo test --workspace` from `rust/` doesn't
try to build wasm-only deps for the host target. Path-deps to
`../../rust/crates/omega-protocol` preserve type sharing across the workspace
boundary.

**Pinned versions** (verified to match leptos 0.8.19's transitive resolution
so no duplicate-version builds):
```toml
leptos                   = { version = "=0.8.19", features = ["csr"] }
wasm-bindgen             = "=0.2.120"
web-sys                  = { version = "=0.3.97", features = [
    "WebSocket", "MessageEvent", "Window", "Location",
    "BinaryType", "Document",
] }
console_error_panic_hook = "=0.1.7"
```
Toolchain: `trunk 0.21.1`, `wasm32-unknown-unknown` rustup target.

**Hello-world page** (`src/main.rs`, ~80 LOC): `console_error_panic_hook` for
browser-visible panics; `mount_to_body(App)`; an `App` component holding a
single `RwSignal<Vec<String>>`; an `Effect` that opens a `web_sys::WebSocket`
to `format!("{proto}://{host}/ws")` derived from `window.location`; an
`onmessage` closure that parses each frame as `serde_json::Value`, reads
`value["type"]`, pushes it into the signal; a `<ul>` view rendering one `<li>`
per frame. The closure is `forget()`-leaked since the page is throwaway.

**Trunk config:** `public_url = "/leptos/"` so generated `<script>` and
`<link>` URLs anchor under the mount point.

**`omega-server` additive route mount:**
- `AppState` gained a `leptos_dir: PathBuf` field defaulted to
  `frontends/leptos/dist`. Existing `AppState::new(provider, sessions_root,
  public_dir)` signature kept unchanged — a builder method
  `.with_leptos_dir(dir)` overrides the default. Result: zero churn at the
  18 existing test call sites.
- `build_router` adds two routes *before* the fallback `ServeDir`:
  `.route("/leptos", get(…Redirect::permanent("/leptos/")))` — 308, modern
  method-preserving — and `.nest_service("/leptos/", ServeDir::new(…))`.
  The original fallback `ServeDir` for `/` is untouched.
- New CLI flag `--leptos-dir <PATH>` on both `omega-server` and
  `mock-omega-server`. Default value is `frontends/leptos/dist`; if the
  directory doesn't exist at runtime the route 404s (non-fatal).

**Justfile.** New recipe `web-leptos-build`: idempotent `rustup target add
wasm32-unknown-unknown` then `cd frontends/leptos && trunk build --release`.
Wired as a recipe-level dep of `server`, `test-browser*`, and `rust-gate`
so both bundles always ship together. **Divergence from plan text:** placed
as a sibling dep, not folded into `rust-build-server` itself — the latter
stays a pure `cargo build -p omega-server`. The "binary always ships both
bundles" goal is preserved at the recipe-graph level.

**Tests:**
- `omega-server/tests/http.rs` — 4 new integration tests:
  `/leptos/index.html` returns 200 from `leptos_dir`; `/leptos/` (trailing
  slash) serves `index.html` via `ServeDir`'s directory-index behaviour;
  bare `/leptos` returns `308` with `location: /leptos/`; the leptos route
  wins over a decoy `public/leptos/index.html` in the fallback `ServeDir`.
- `e2e/leptos-smoke.spec.ts` — 2 specs against `mock-omega-server`
  (real-server project, port 3003): one waits for `<li>ready</li>` inside
  `[data-testid="leptos-frames"]` and asserts the running counter
  increments; one verifies the bare-prefix 308 + Location header.

**Bar:** `just rust-gate` ✅ (incl. `web-leptos-build`) · `just test-browser`
118/118 (109 existing chromium + 7 real-server + 2 new Leptos smoke) ·
manual `curl` confirms `/`, `/leptos/`, `/leptos`, `/api/sessions`, `/health`
all behave as specified.

**Drive-by:** committed a regenerated `rust/bindings/SessionStartedEvent.ts`
that had drifted from a doc-comment-only change in commit `72a14ee`. Pure
doc-string update in a generated file; required to make the bindings-drift
guard in `rust-gate` exit clean.

**Carry-forward into 3.1:**
- The 3.0 page parses frames as `serde_json::Value`. 3.1 replaces this with
  full `WsMessage` deserialisation through `omega-protocol`, which means
  the wire shape on the server side needs review: `WsMessage::Item`
  currently uses untagged `AgentItem` serialisation. Decide whether
  `omega-protocol` should expose `WsMessage` directly (currently lives in
  `omega-server` only — server-only types like `SessionDeleted` /
  `SessionRenamed` argue against the move) or whether the Leptos client
  defines a parallel client-side `WsMessage` enum that re-uses
  `omega-protocol::OmegaEvent` for the inner payloads.
- The closure-leak in `open_ws` is fine for 3.0 but should become a proper
  `StoredValue<Closure>` once the page has a real lifecycle.
- `WebSocket::send` (write path) is unimplemented; 3.2 needs it.
- `frontends/leptos/Cargo.lock` is committed (matches `rust/Cargo.lock`
  policy for binaries). Compile time on a cold cache is ~40s; subsequent
  builds are sub-second incremental.

### Phase 3.1 — done (concise record)

**Scope:** Replace the 3.0 `serde_json::Value` frame parser with strongly-typed
`WsMessage` deserialisation, stand up a single reactive `SessionStore`, render a
live JSON debug dump at `/leptos/`. No visible UI controls; this is the
protocol smoke surface.

**Decision — protocol shape (Option B “parallel client-side enums”).** Lifting
`omega-server::WsMessage` into `omega-protocol` was rejected because
`WsMessage::Item(Box<AgentItem>)` carries a `#[serde(untagged)]`,
`Serialize`-only payload by design — making it `Deserialize`-able would force a
redesign of `AgentItem` and pollute the protocol crate with a transport-level
concern. Instead, `frontends/leptos/src/protocol.rs` declares a single flat
tagged enum that re-uses every typed event/signal struct from
`omega_protocol`. Duplication is purely at the variant-listing layer (~30
idents); field types remain the single source of truth. Same approach for the
write-path `ClientFrame` (no callers in 3.1; locked-in for 3.2's composer).

**Wire-shape collision noted and resolved client-side.** Server emits both
envelope `WsMessage::AgentError("msg")` and forwarded
`OmegaEvent::AgentError(AgentErrorEvent)` under `type: "agent_error"`.
Disambiguated client-side via an `#[serde(untagged)]` `AgentErrorPayload` that
matches by structure (`{message}` vs `{time, error}` are disjoint). **No
omega-server changes required.**

**`SessionStore` (`src/store.rs`).** RwSignal-per-field struct — canonical
Leptos shape, fine-grained reactivity, signals are slotmap handles so the
store is `Copy`. Eight fields: `connected`, `session_info`, `turn_state`,
`streaming`, `events`, `streaming_text`, `streaming_thinking`,
`transport_errors`. `apply(WsMessage)` is the reducer; `snapshot()` returns a
POD `SessionState` for the JSON dump and as the assertion target in tests.
`reactive_stores::Store` was rejected as overkill for eight flat fields; one
big `RwSignal<State>` was rejected because every field touch would re-run all
subscribers.

**`WsClient` (`src/ws.rs`).** `StoredValue<WsState, LocalStorage>` owns the
four JS-bridged closures (`onopen`, `onmessage`, `onclose`, `onerror`) plus
the socket and reconnect bookkeeping. `LocalStorage` because `WebSocket` and
`Closure` are `!Send + !Sync`; CSR is single-threaded so this is correct.
Reconnect on `onclose` with exponential back-off (`0.5 s × 2^(attempt-1)`,
capped at 30 s, ±20 % multiplicative jitter); counter resets on `onopen`.
The `send(&ClientFrame)` write path is wired but unused in 3.1 (3.2 hooks up
the composer).

**Debug view (`src/main.rs::DebugView`).** Reads every signal in one move
closure (so leptos's reactive graph subscribes us to all of them in one shot)
and pretty-prints `store.snapshot()` into
`<pre data-testid="leptos-debug-store">`. Replaces the 3.0
`<ul data-testid="leptos-frames">` hello-world.

**Test runner.** `wasm-bindgen-test 0.3.70` (exact-pinned to the existing
`wasm-bindgen 0.2.120`) running under `wasm-bindgen-test-runner` with the
default Node backend. Lighter than chromedriver/wasm-pack; sufficient because
`SessionStore::apply` is pure Rust + serde — no DOM, no WS in the test path.
New `frontends/leptos/.cargo/config.toml` declares the runner; new just
recipe `web-leptos-test` does `rustup target add wasm32-unknown-unknown`,
`cargo install --locked --version =0.2.120 wasm-bindgen-cli` (idempotent),
then `cargo test --target wasm32-unknown-unknown`. Wired into `rust-gate`
as a sibling step (after `web-leptos-build`, before the `cargo` block).

**47 wasm-bindgen-tests** across the three new modules:
- `protocol.rs`: 21 tests — every envelope variant (Ready, ResetDone,
  SessionDeleted, SessionRenamed, SessionInfo with/without `name`, History
  with/without `streaming`), the `agent_error` envelope/event
  disambiguation, all three stream-signal tags, two representative event
  variants, `into_omega_event` mapping correctness, and four
  `ClientFrame` serialisation shapes.
- `store.rs`: 20 tests — each reducer rule, every `apply_event_side_effects`
  match arm (`UserMessage`, `TurnEnd`, `TurnInterrupted`, `PauseRequested`,
  `TurnPaused`, `TurnContinued`, `LlmResponse`), and a fixture-driven
  end-to-end replay of a realistic frame sequence (`ready` → `session_info`
  → `history` → `user_message` → `text`×2 → `turn_end`).
- `ws.rs`: 6 tests — pure back-off math via injected `Jitter` trait
  (deterministic `FixedJitter` + sequence-driven `SeqJitter`). Validates
  base delay, doubling, exponent cap, 30 s ceiling, jitter bounds, and
  one-sample-per-attempt invariant.

**Mutation testing baseline** (`cargo mutants -- --target wasm32-unknown-unknown`,
run from `frontends/leptos/`):
- `protocol.rs`: 2 mutants — 1 caught, 1 unviable, **0 missed**.
- `store.rs`: 9 mutants — 9 caught, **0 missed**. The four gaps from the
  initial run (`PauseRequested`/`TurnPaused`/`TurnContinued`/`LlmResponse`
  match arms in `apply_event_side_effects`) were real and were closed in
  the same commit.
- `ws.rs`: 29 mutants — 9 caught, **20 missed**. Every miss is in
  JS-interop code (`WebSocket::new`, `set_timeout`, `clear_timeout`,
  `RandomJitter::factor`, `ws_url_from_window`, `WsClient::send`/`connect`/
  `schedule_reconnect`). Catching them requires a headless-browser harness
  (`wasm_bindgen_test_configure!(run_in_browser)` plus chromedriver) and
  was deferred — the missing coverage is at the DOM/WS edge, not in
  pure logic. The 9-caught half is exactly the pure `backoff_delay_ms`
  function and its `Jitter` trait, which were extracted specifically to
  be unit-testable without DOM mocks.

Not wired into `just rust-gate` — a wasm32 mutants run takes ~3 min/file.
Re-runnable manually: `cd frontends/leptos && cargo mutants --file src/<f>.rs -- --target wasm32-unknown-unknown`.

**Pinned versions** (verified against crates.io, no resolution conflicts):
```toml
wasm-bindgen-test = "=0.3.70"   # hard-pins wasm-bindgen = "=0.2.120"
wasm-bindgen-cli  = "=0.2.120"  # installed by `just web-leptos-test`
```
No other version changes; `serde` was promoted from a transitive dep to an
explicit `"1" + features = ["derive"]` direct dep on the leptos side.

**Smoke spec retargeted.** `e2e/leptos-smoke.spec.ts` now asserts
`[data-testid="leptos-debug-store"]` contains `"connected": true` and
`"transportErrors": []`, replacing the 3.0 `<li>ready</li>` assertion. The
bare-redirect spec is unchanged.

**Bar:** `just rust-gate` ✅ (incl. `web-leptos-build` and `web-leptos-test`,
47/47 wasm tests) · `just test-browser` 118/118 (109 chromium + 7 real-server
+ 2 leptos-smoke) · `cargo clippy --target wasm32-unknown-unknown -- -D warnings`
clean on the leptos crate.

**Carry-forward into 3.2:**
- `WsClient::send(&ClientFrame)` exists but has no callers; 3.2 wires it to a
  session-picker UI.
- `provide_context::<SessionStore>` is set up at the App root so 3.2
  components can `use_context::<SessionStore>()` without restructuring.
- `transport_errors` accumulates envelope `agent_error` messages forever; if
  3.2 surfaces transient connection errors prominently, consider a TTL or
  user-dismissable model.
- The trunk asset bundle is now noticeably larger than 3.0's (the wasm has
  grown from hello-world to leptos + serde-driven `WsMessage` parsing). If
  3.2's bundle ergonomics matter, consider `wasm-opt` via `[profile.release]`
  flags or a separate `web-leptos-build-debug` recipe for dev iterations.
- `wasm-bindgen-test-runner` requires `node` on PATH (no Bun-as-node shim
  worked: bun lacks `document` + Node-specific globals the wasm-bindgen
  shim relies on). Node is now a build-time dep alongside
  `wasm-bindgen-cli`. If 3.4+ wants to also exercise the JS-interop
  surface in `ws.rs` (the 20 missed mutants), upgrade to
  `wasm_bindgen_test_configure!(run_in_browser)` and add chromedriver to
  the gate.

### Phase 3.2 — done (concise record)

**Scope:** First user-facing feature surface in the Leptos UI. The 3.1
debug-only JSON dump moves into a `<details>` panel; the primary surface
is a working session picker that lists, creates, renames, and deletes
sessions. The WS write path (`WsClient::send`) gains its first three
callers (`Reset`, `RenameSession`, `DeleteSession`).

**Decision — Reset-vs-POST for "new session" (diverge from TS UI).**
The SolidJS picker uses `POST /api/sessions` for new-from-picker. The
Leptos picker uses `ClientFrame::Reset { None, None }` over WS instead.
Reason: Reset keeps the open socket attached to the new session
immediately and emits a clean `session_info → history → reset_done`
triple that flows through the existing `SessionStore` reducer. POST
creates the session but doesn't notify the open WS, leaving the client
stale until reconnect. The picker has no model/effort UI yet (3.4
territory), so `Reset { None, None }` is exactly equivalent to a default
POST body — less plumbing, fewer race windows. One reactive trigger
(`Effect` watching `session_info.dir`) covers the whole flow: initial
fetch on mount, refetch when Reset replaces the active session.

**Decision — separate `SessionListStore` (not folded into `SessionStore`).**
Different lifecycles: the conversation store resets on every
`ResetDone`; the picker list survives across resets and is only mutated
by `SessionRenamed` / `SessionDeleted` envelopes. Folding would force
either reducer to ignore most of its own input. Per task spec,
`SessionStore` stays unchanged — the new store lives in
`frontends/leptos/src/sessions.rs`.

**Decision — `gloo-net` 0.6.0 over hand-rolled `web_sys::Request`.**
Measured the bundle delta with three controlled `trunk build --release`
runs:

| Variant | wasm size | delta vs stub |
|---|---|---|
| 3.1 baseline | 355,136 B | — |
| 3.2 stub (no HTTP) | 444,818 B | 0 |
| 3.2 + `web_sys::Request` | 457,108 B | +12.3 KB |
| 3.2 + `gloo-net` | 461,758 B | +16.9 KB |

gloo-net costs ~4.5 KB more than the hand-rolled `web_sys::Request`
alternative — ~1 % of the bundle. Not material; gloo-net wins on
ergonomics (`Request::get(...).send().await?.json().await?` vs ~25 lines
of `RequestInit` / `JsFuture` / `dyn_into` / promise-await ceremony).
Version-pinned to `=0.6.0` to match the existing transitive resolution
through `leptos`'s `server_fn` (no duplicate-version build).

**Decision — server-confirmed updates (not optimistic).** Rename and
delete wait for the server's `SessionRenamed` / `SessionDeleted`
broadcasts before mutating the local list. Localhost round-trip is
single-digit milliseconds — below human-noticeable. Honest types: the
UI shows what the server confirms, no rollback path needed. Documented
gap: on a slow link the rename/delete will appear delayed; acceptable
for 3.2 scope. The `SessionListStore::apply` reducer is the only place
the local list mutates in response to writes.

**No server-side changes required.** Every `ClientFrame` variant 3.2
needs (`Reset`, `RenameSession`, `DeleteSession`) and every `WsMessage`
the picker reads (`SessionInfo`, `SessionRenamed`, `SessionDeleted`,
`ResetDone`) was already typed in `frontends/leptos/src/protocol.rs` at
3.1. Confirmed by grep before writing any code.

**New files:**
- `frontends/leptos/src/sessions.rs` (~420 lines) — `SessionListStore`
  + pure reducers `apply_renamed`, `apply_deleted`, `is_active`. Wire
  shape `SessionListItem` mirrors `omega-server::router::SessionListItem`.
- `frontends/leptos/src/http.rs` (~50 lines) — `gloo-net` wrapper for
  `GET /api/sessions`. Single function `get_sessions() -> Result<Vec<...>, String>`.
- `frontends/leptos/src/picker.rs` (~290 lines) — `SessionPicker`
  component + `SessionRow` child. Inline rename, confirm-on-delete,
  active-row marker driven by a `Memo` over
  `conversation_store.session_info.dir`. Per-row `dir` stored in a
  `StoredValue<String, LocalStorage>` so all event-handler closures
  capture only `Copy` values (no `.clone()` ceremony inside `<Show>` /
  `<For>`).
- `e2e/leptos-session-picker.spec.ts` (4 specs) — create / rename /
  delete / multi-session active-distinction. Uses
  `data-active="true"|"false"` and `data-session-dir="<dir>"`
  attributes as stable selectors so the spec is hermetic against
  pre-existing `.omega/test-sessions/` state.

**Modified files:**
- `frontends/leptos/Cargo.toml` — added `gloo-net = "=0.6.0"`
  (`default-features = false`, `["http", "json"]`),
  `wasm-bindgen-futures = "=0.4.70"`, and `"HtmlInputElement"` to the
  `web-sys` features list.
- `frontends/leptos/src/main.rs` — swapped `DebugView` as the primary
  surface for `<SessionPicker />` + `<details data-testid="leptos-debug-panel">{ DebugView }</details>`.
  Constructs the `WsClient` once at the App root and `provide_context`s
  it alongside both stores.
- `frontends/leptos/src/ws.rs` — `WsClient::new` signature gained a
  third arg `list_store: SessionListStore`; `on_message` now dispatches
  each parsed `WsMessage` to the picker store *before* the conversation
  store (`list_store.apply(&msg); store.apply(msg);`). `WsClient::send`
  loses its `#[allow(dead_code)]`.
- `playwright.config.ts` — `leptos-session-picker.spec.ts` added to
  the real-server project's `testMatch` list (and to chromium's
  `testIgnore`).

**Tests — wasm-bindgen-test (`just web-leptos-test`):** 73 passing
(3.1 had 47; 26 new in `sessions.rs`). New coverage:
- 4 pure-reducer tests on `apply_renamed` (match, overwrite, no-match
  returns false, first-match-only on duplicate dirs).
- 3 pure-reducer tests on `apply_deleted` (match, no-match returns
  false, removes-every-match on duplicate dirs).
- 3 pure-helper tests on `is_active` (match / no-match / current=None).
- 4 reactive `SessionListStore::apply` tests — each match arm
  (`SessionRenamed`, `SessionDeleted`, catch-all no-op covering `Ready`
  / `ResetDone` / `Text`).
- 4 setter tests (`set_sessions` clears prior error;
  `set_error` clears loading; `begin_loading` toggles + clears prior
  error; full begin/finish lifecycle).
- 1 wire-shape test confirming `SessionListItem` round-trips the
  server's `GET /api/sessions` JSON output.
- 7 fetch-generation tests covering the
  `finish_loading_if_current` / `fail_loading_if_current` /
  `bump_generation` race-fix machinery (see "Test-side flake" below).

**Test-side flake — caught and fixed in the same commit.** Initial spec
used `[data-active="true"]` to read the just-created session's dir
immediately after clicking `+ new session`. The `data-active` attribute
briefly points at the *previous* active row between the click and the
server's `session_info(new)` arrival, so the spec sometimes deleted /
renamed the wrong row and the assertion failed (~30 % flake rate on a
clean run). Fixed by reading `session_info.dir` from the debug-snapshot
JSON (ground truth) and waiting for that dir to appear in the list
before proceeding. **Defensive production fix landed alongside it:**
`SessionListStore` gained a `fetch_generation` counter (bumped by every
list mutation) and `finish_loading_if_current` / `fail_loading_if_current`
wrappers that drop stale fetch results when a `SessionRenamed` /
`SessionDeleted` broadcast lands while a `GET /api/sessions` is in flight.
The race is real (a stale fetch *could* clobber a server-confirmed
mutation), it just wasn't what was making the spec flake.

**Tests — Playwright (real-server project, port 3003):** 4 new specs
(`e2e/leptos-session-picker.spec.ts`):
1. Create — click `+ new session`, assert list count grows and exactly
   one row is `data-active="true"`.
2. Rename — inline rename submits, label updates after `session_renamed`.
3. Delete — `window.confirm` auto-accepted, row vanishes after
   `session_deleted`.
4. Multi-session active distinction — two consecutive `+ new session`
   clicks, exactly one active row, the previous session is `data-active="false"`.

**Mutation testing** (`cargo mutants -- --target wasm32-unknown-unknown`,
run from `frontends/leptos/`):
- `sessions.rs` (new pure-logic file): 24 mutants — 24 caught,
  **0 missed**. Acceptance criterion met.
- `http.rs` (new JS-interop edge): 3 mutants — 3 missed. All in the
  network-fetch surface (`get_sessions` body). Same documented gap as
  `ws.rs` from 3.1; the network/DOM mutants require a headless browser
  harness to catch.
- `picker.rs` (new component): 9 mutants — 9 missed. All in component
  glue (`Effect` closure, event handlers, `event_target_value` DOM
  helper). Covered functionally by the Playwright spec, not by
  wasm-bindgen-test. Documented as a gap.

**Bundle-size impact.** 355,136 B (3.1) → 461,758 B (3.2),
+106,622 B (+30 %). Decomposition (controlled measurements):
- +89 KB — picker UI + async runtime (`wasm-bindgen-futures`,
  `For`/`Show` machinery, `spawn_local`).
- +17 KB — `gloo-net` HTTP client. (web_sys alternative would have
  saved ~4.5 KB; rejected as immaterial.)
The bulk of the growth is the async runtime + reactive components,
not the HTTP client choice.

**`just rust-gate`** ✅ (incl. `web-leptos-build` 461 KB wasm and
`web-leptos-test` 65/65). **`just test-browser`** ✅ 122/122 (118 from
3.1 + 4 new picker specs).

**Carry-forward into 3.3:**
- `SessionListStore::sessions` is unbounded; with thousands of sessions
  the picker would render slowly. Virtualisation deferred to 3.6 polish.
- The picker has no search/filter input; the SolidJS picker has one and
  3.6 should bring parity.
- Picker `Effect`s use `Effect::new` with a return-prev-value pattern
  rather than `Effect::watch`; works but verbose. Revisit if 3.3+
  patterns warrant a helper.
- The bundle is now 461 KB (115 KB gzipped). 3.3's conversation feed
  will add markdown rendering + likely syntax highlighting; budget for
  another ~150 KB. Consider `wasm-opt -Oz` and a `code-splitting` story
  before 3.7 cutover if the total approaches 1 MB.
- `event_target_value` is hand-rolled; leptos 0.8 ships
  `leptos::ev::event_target_value` — swap when 3.3 needs more form
  inputs.
- The rename input has no Enter-to-submit / Esc-to-cancel keyboard
  handling. Same parity gap as the search-filter; 3.6.
- Picker doesn't emit a frame on session-row click yet (no "resume
  this session" flow). 3.5 lands `ClientFrame::ResumeSession` from the
  picker as part of the resume-session UX.

### Phase 3.3 — done (concise record)

**Scope.** Conversation feed becomes the primary visible surface at
`/leptos/`, sitting between the 3.2 picker and the (new) collapsed
debug panel. Every `OmegaEvent` variant gets a typed view; streaming
`text` / `thinking` signals append into a live overlay; auto-scroll
follows new content unless the user has scrolled up.

**Decision — event-router shape (pure projection function).**
`kind_for(&OmegaEvent) -> EventKind` in `event_view.rs` projects each
variant to one of six visual families: `User`, `Assistant`, `ToolCall`,
`ToolResult`, `Status`, `Error`. The `<EventBlock/>` component still
does the big match for typed field access (each variant carries its
own field shape — unavoidable), but the *family-class decision* lives
in the pure helper. Mutation-tested. Same role `is_active` /
`apply_renamed` played in 3.2. One-component-per-family was rejected
for adding wrappers without behavioural gain; in-component-match-only
was rejected because each arm is glued to JSX with no testable seam.
A `ToolResult` event with `is_error: true` resolves to `Error`, not
`ToolResult` — the visual family follows the outcome.

**Decision — streaming-text rendering (direct append).**
`SessionStore::streaming_text` (an `RwSignal<String>`) is appended to
per `Text` frame by the existing reducer (`store.rs::apply` calls
`update(|s| s.push_str(...))`). The `StreamingTail` component is a
`<Show>` over `streaming_text.with(String::is_empty)` containing a
`<pre>{move || streaming_text.get()}</pre>`. Per-keystroke reactivity
— leptos's strength — matches SolidJS's direct-append pattern. No
rAF buffer; verified with `SCRIPTS.longStream()` (8 chunks × 100 ms)
in the new Playwright spec, which observes the overlay growing live
and collapsing into the persisted `llm_response` block on `turn_end`.
If 3.6's markdown rendering makes per-frame work expensive, *that's*
the point at which a buffer earns its keep.

**Decision — auto-scroll seam (pure predicate + JS-interop edge).**
`should_autoscroll(scroll_top, client_height, scroll_height,
threshold) -> bool` in `event_view.rs` is the testable carve-out
(threshold = 40 px). The reactive `Effect` subscribes to
`events.with(Vec::len)`, `streaming_text.with(String::len)`, and
`streaming_thinking.with(String::len)`, then calls
`sentinel_ref.scroll_into_view()` iff the lockout signal is open. An
`on:scroll` handler reads `scrollTop` / `clientHeight` / `scrollHeight`
from a `NodeRef<html::Section>` and feeds the pure predicate to update
the gate. The DOM-reading half is a JS-interop edge — same
mutation-gap pattern as 3.1's `ws.rs::WsClient::send` and 3.2's
`picker.rs` event handlers.

**Decision — tool_result truncation (match SolidJS at 3000 chars +
inline expand).** SolidJS's `truncate(s, maxChars=3000)` (App.tsx:305)
is what the inline preview actually renders today; the 100 KB modal
path is a 3.5 concern. `truncate_for_preview(s, max_chars) ->
Option<String>` returns `Some(<truncated_with_marker>)` only when the
input exceeds `max_chars`, so callers tell truncated from full output
at the type level. Per-row expansion is held in a `RwSignal<bool>`
inside `<ToolResultBlock/>`; the toggle button only mounts when the
truncate returned `Some`. Mutation-tested. The marker line `\n…
[{total} chars total — showing first {max_chars}]` mirrors the
SolidJS UI byte-for-byte so visual parity holds across the 3.0–3.6
co-existence window. Diverging to 10 KB was rejected — the SolidJS
UI doesn't actually do that.

**Decision — markdown / KaTeX / Mermaid (deferred to 3.6).** Locked
in. 3.3 emits raw text in `<pre class="block-body">` for every
rendering case. Zero new deps.

**No server-side changes.** Confirmed by grep that all 22
`OmegaEvent` variants are typed in `frontends/leptos/src/protocol.rs`
at 3.1; the new `event_type_tag` helper in `event_view.rs` enumerates
all 22 explicitly so a future `omega-protocol` addition either
compiles into a real `data-event-type` or breaks the wasm build.

**One concession to test coverage — `<StubComposer/>` (3.3-temp).**
3.3 needs to drive a multi-tool turn but the Leptos UI has no
composer until 3.4. A 30-line `<StubComposer/>` (`<textarea>` + send
button calling `WsClient::send(ClientFrame::UserMessage)`) lives in
`feed.rs`, marked with `data-testid="leptos-stub-composer-*"` so
3.4's replacement can grep-and-delete it. Alternatives rejected: a
JS-side raw `WebSocket` would conflict with the page's WS
(single-active-WS server) and is racy; exposing the `WsClient`
handle on `window` is uglier than the stub.

**New files:**
- `frontends/leptos/src/event_view.rs` (~430 lines) — `EventKind`
  enum + 6-way `kind_for` projection covering all 22 `OmegaEvent`
  variants; `css_class_for`, `kind_tag`, `event_type_tag` (one stable
  attribute string per variant for Playwright); `should_autoscroll`
  pure predicate; `truncate_for_preview` pure helper. 43 wasm tests.
- `frontends/leptos/src/feed.rs` (~520 lines) — `<ConversationFeed/>`
  with the auto-scroll Effect, `<EventBlock/>` with the per-variant
  body match, `<ToolResultBlock/>` with show-more state,
  `<StreamingTail/>` for live append, `<StubComposer/>` (3.3-temp).
- `e2e/leptos-conversation-feed.spec.ts` — 4 specs: multi-tool turn
  asserts every visible event family renders with both
  `data-event-kind` and `data-event-type`; long-stream verifies the
  streaming overlay appears live and collapses into `llm_response`;
  long `read_file` exercises the truncation toggle; `httpError(400)`
  surfaces the Error family.

**Modified files:**
- `frontends/leptos/src/main.rs` — mounts `<ConversationFeed/>` and
  `<StubComposer/>` between the picker and the (now-collapsed) debug
  panel. Heading bumped to "Phase 3.3".
- `frontends/leptos/Cargo.toml` — added `Element`, `HtmlElement`,
  `HtmlDivElement`, `HtmlTextAreaElement` to the `web-sys` features
  list (transitively pulled by `HtmlInputElement` already; explicit
  for next-reader clarity). **Zero new external deps.**
- `playwright.config.ts` — wired the new spec into the real-server
  project's `testMatch` and the chromium project's `testIgnore`.

**Tests — wasm-bindgen-test (`just web-leptos-test`):** 116 passing
(73 from 3.2 + 43 new in `event_view.rs`):
- 23 tests on `kind_for` — one per `OmegaEvent` variant + the
  `ToolResult` is_error split. Each catches the deletion mutation of
  the variant's match arm.
- 2 tests on `css_class_for` — per-kind values + pairwise uniqueness
  (catches "every arm returns the same string" mutations).
- 2 tests on `kind_tag` — same pattern.
- 1 test on `event_type_tag` — cross-checks against the serde
  discriminator strings; future field-name drift breaks the test
  rather than silently breaking Playwright selectors.
- 8 tests on `should_autoscroll` — boundary cases, exact-equality,
  one-pixel-past-threshold, threshold-lifts-borderline, and
  contribution tests for each summed operand. Catches every
  comparison-operator mutation cargo-mutants emits.
- 7 tests on `truncate_for_preview` — below/equal/above limit,
  exact prefix preservation, marker content, multibyte safety,
  zero-max edge case.

**Tests — Playwright (real-server project, port 3003):** 4 new specs
in `e2e/leptos-conversation-feed.spec.ts`:
1. **Multi-tool turn** — drives `SCRIPTS.multiTool()` (3 tool turns +
   final text). Asserts: 1 `user_message` block with `data-event-kind
   ="user"`; 3 `tool_call` blocks with the right tool name + JSON
   input rendered; 3 `tool_result` blocks with `data-event-kind
   ="tool_result"`; final `llm_response` containing "done multi";
   every block has both `data-event-kind` and `data-event-type`
   attributes set; at least one `kind="status"` block exists.
2. **Streaming overlay** — drives `SCRIPTS.longStream()` (8 chunks ×
   100 ms). Asserts the overlay (`leptos-streaming-text`) becomes
   visible mid-turn and contains the streamed text; clears on
   `turn_end`; final `llm_response` carries the full text "done stream".
3. **Tool-result truncation** — drives `read_file rust-migration.md`
   (≈ 50 KB after the read_file MAX_BYTES cap). Asserts the rendered
   body contains the truncation marker, total visible text length is
   bounded under 3500, the `show more` button reveals strictly more
   content, and toggling back hides it again.
4. **Error family** — drives `httpError(400)`. Asserts at least one
   block with `data-event-kind="error"` becomes visible; the block's
   `data-event-type` is one of `llm_error` / `turn_interrupted`.

**Mutation testing** (`cargo mutants -- --target
wasm32-unknown-unknown`, run from `frontends/leptos/`):
- `event_view.rs` (new pure-logic file): 18 mutants — 17 caught,
  1 unviable, **0 missed**. Acceptance criterion met.
- `feed.rs` (new component): 5 mutants — 4 missed, 1 unviable. All
  4 misses are JS-interop edges: the `if !auto_scroll.get_untracked()`
  guard inside the `scrollIntoView` Effect, the `auto_scroll != next`
  check inside `on_scroll`, and `textarea_value`'s `dyn_into` glue.
  Documented as gaps, same pattern as 3.1's `ws.rs` (20 missed) and
  3.2's `picker.rs` (9 missed) / `http.rs` (3 missed) — catching
  these requires a headless-browser harness with real DOM events.
  Functionally covered by the Playwright specs.

**Bundle-size impact.** 461,758 B (3.2) → 531,821 B (3.3),
+70,063 B (+15 %). Decomposition: the `<For>` keyed-list machinery
now has a real consumer (events) and the per-variant `view!` arms in
`render_event_body` expand into 22 distinct `IntoView` types that
`into_any()` boxes. No new external crates. With markdown
(comrak/pulldown-cmark) + KaTeX + Mermaid all aimed at 3.6, the
remaining bundle budget before the 1 MB target is comfortable.

**`just rust-gate`** ✅ (incl. 116 wasm-bindgen tests, all unit
suites, ts-rs bindings drift). **`just test-browser`** ✅ 127/127
(122 from 3.2 + 4 new feed specs + 1 picker re-tally).

**Carry-forward into 3.4:**
- `<StubComposer/>` is 3.3-temp and must be deleted by 3.4. Search
  for `data-testid="leptos-stub-composer"` to find its three sites
  (component, e2e helper, send-button). 3.4's real composer adds:
  pause / continue / abort buttons (need `ClientFrame::Pause`,
  `Continue`, `Abort` — all already typed in `protocol.rs`); model
  and effort switchers (`SetModel`, `SetEffort`); file-completion
  autocomplete via `GET /api/files?prefix=...` (the `http.rs` HTTP
  layer needs a second `get_files` function alongside
  `get_sessions`).
- The conversation feed has no "jump to bottom" button when the user
  scrolls up. SolidJS shows an inline `↓` button bottom-right; 3.6
  parity pass.
- `<EventBlock/>` clones each `OmegaEvent` once per render of the
  enclosing `<For>`. For long conversations this is O(n) per turn.
  Acceptable today; revisit in 3.6 if perf tooling shows it bites.
  An `Arc<OmegaEvent>` indirection in `SessionStore::events` would
  be the obvious fix — SessionStore stays untouched here per task
  spec.
- The 3000-char preview cap is hard-coded in `feed.rs`; 3.6 may want
  it user-configurable. No protocol/server change needed.
- `event_type_tag` enumerates all 22 OmegaEvent variants explicitly.
  When omega-protocol adds variant #23, the wasm build breaks (no
  default arm), forcing a rendered-tag decision rather than silently
  rendering nothing. Intentional.

### Phase 3.4 — composer (next)

**Goal.** Replace the 3.3 `<StubComposer/>` with a parity composer
that owns user-message send, in-flight pause/continue/abort, model +
effort switchers, and file-completion autocomplete — every
operator-side surface the SolidJS UI has today.

**Server-side surface (already in place — do NOT change):**
- `ClientFrame::UserMessage`, `Pause`, `Continue { content }`,
  `Abort`, `SetModel { model }`, `SetEffort { effort }` are all
  typed in `frontends/leptos/src/protocol.rs` at 3.1.
- `GET /api/files?prefix=...` is implemented in
  `omega-server::router::list_files_for_completion` (Phase 1e.4) with
  the directory-first-then-alphabetical sort and a 50-completion
  cap; tested at the Rust level. Wire shape:
  `[{"path":"src/","isDir":true}, ...]`.
- `OmegaEvent::ModelChanged` / `EffortChanged` / `PauseRequested` /
  `TurnPaused` / `TurnContinued` / `TurnInterrupted` already drive
  `SessionStore::turn_state`; the composer reads turn_state to
  enable/disable buttons.

**Deliverables:**

1. **`composer.rs`** — `<Composer/>` component reading `turn_state`
   from context, rendering: a textarea (multi-line, autosizing); a
   primary action button that flips between `Send` (turn_state=idle),
   `Pause` (turn_state=running), `Abort` (turn_state=pause_requested),
   `Continue` (turn_state=paused); a model picker dropdown; an
   effort picker dropdown.
2. **`http.rs`** — add `get_files(prefix: &str) -> Result<Vec<FileCompletion>, String>`
   alongside the existing `get_sessions`. Same `gloo-net` glue.
3. **`completion.rs`** — pure file-completion state machine: given a
   composer's text + caret position, derive the completion query;
   given completions + selection index, derive the inserted text on
   accept. Mutation-tested.
4. Delete `<StubComposer/>` (and its three call sites) once the real
   composer's e2e spec covers the `user_message` send path.

**Out of scope:** context-modal / resume-session UX (3.5), markdown
/ KaTeX / Mermaid rendering (3.6), visual-parity polish (3.6),
chromiumoxide cutover (Phase 4).

**Acceptance:**
- `localhost:3000/leptos/` lets the operator type a message, send
  it, pause / continue / abort an in-flight turn, switch model and
  effort, and accept file-completion suggestions.
- A new Playwright spec
  (`e2e/leptos-composer.spec.ts`) drives every flow against
  `mock-omega-server`. Existing leptos specs still pass.
- `cargo mutants -- --target wasm32-unknown-unknown` on every new
  pure-logic file: **0 missed**. JS-interop edges (textarea events,
  dropdown reactivity, completion popup positioning) acknowledged as
  gaps, same pattern as 3.1–3.3.
- `just rust-gate` ✅ · `just test-browser` ✅.

**Open questions to resolve in 3.4:**
- Composer keyboard handling: `Enter` to send vs.
  `Cmd/Ctrl-Enter` to send. SolidJS uses `Enter` (Shift-Enter for
  newline). Mirror or diverge?
- File-completion popup placement: floating below the caret
  (SolidJS) or fixed at the bottom of the composer? The former is a
  bigger DOM dance; the latter is simpler but less SolidJS-parity.
- Pause/continue interjection text: SolidJS lets the user type a
  mid-turn message in the composer, and the `Continue` button sends
  it as `ClientFrame::Continue { content }`. Confirm the wire shape
  matches; consider whether an explicit "pause-and-add-context" UI
  state is worth modelling.
- Model + effort dropdowns: hard-code the four supported
  (sonnet-4-6, opus-4-6, opus-4-7) + four effort levels (low,
  medium, high, max) on the client, or fetch a discovery endpoint
  from the server? SolidJS hard-codes; we likely should too —
  changes there are rare and require a UI bump anyway.
- File-completion bundle cost: `gloo-net` is already paid for from
  3.2. Sorted-with-grouping logic should land in a pure helper for
  mutation testing. No new crate expected.

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
