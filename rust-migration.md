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
| 3.4 — Leptos composer | ✅ Done | `composer.rs` + `completion.rs`; primary action button (Send/Pause/Abort/Continue) + secondary Abort; model + effort `<select>` dropdowns; `@`-path file completion via `/api/files`; 3.3 `<StubComposer/>` retired |
| 3.5 — Leptos context inspector + resume | ✅ Done | `context_modal.rs`; resume from picker; LLM-call inline expander |
| 3.6 — Leptos markdown + Mermaid + SSR snapshots | ✅ Done | `pulldown-cmark`; lazy-loaded Mermaid; insta SSR snapshot harness (TEST-ARCH-5) |
| 3.7 — cutover + delete | ✅ Done | `omega-server` serves Leptos at `/`; `src/` + `rust/bindings/` + ts-rs derives + chromium Playwright project all gone |
| 3.8 — visual parity | ✅ Done | `frontends/leptos/style.css` (980 lines, Catppuccin Mocha) ported from the deleted SolidJS theme; Trunk-hashed `<link rel="stylesheet">`; centred picker panel; modal overlay with backdrop |
| 3 — Leptos UI rewrite | ✅ Done | SolidJS → Leptos. Cutover at 3.7 + visual parity at 3.8 close out the phase |
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
│       ├── omega-server/       ✅ done  (HTTP + WS; serves the Leptos bundle)
│       └── omega-mock-server/  ✅ done  (Playwright fixture binary; retires in Phase 4)
├── frontends/                  ← Web frontends
│   └── leptos/                 ✅ done  (production frontend, wasm32 Cargo workspace)
├── e2e/                        ← Playwright real-server suite (retires in Phase 4)
├── Justfile
└── package.json                ← inert; carries Playwright + a few SolidJS-era deps
                                  retained for Phase 4 deletion in lockstep
```

**Frontend serving (post-3.7):** `omega-server` serves the Leptos
bundle at both `/` (fallback `ServeDir`) and `/leptos/` (kept as a
one-release alias; deletion + the `Trunk.toml` `public_url` flip
land in a follow-up). `/ws`, `/api/*`, `/health` are unchanged.

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
| 3.4 | ✅ Done | Composer: user-message send, pause / continue / abort, model + effort switchers; file-picker autocomplete via `/api/files` |
| 3.5 | ✅ Done | Context inspector (`/api/context`); resume-session flow; LLM-call detail expander |
| 3.6 | ✅ Done | Visual parity pass; markdown / Mermaid; `leptos`-SSR + `insta` snapshot tests per component (TEST-ARCH-5 lands here) |
| 3.7 | ✅ Done | Cutover: route `/` to Leptos; delete `src/`, ts-rs derives, `package.json`, `node_modules`; retire SolidJS Playwright specs whose surface is covered by snapshot tests |
| 3.8 | ✅ Done | Visual parity: `frontends/leptos/style.css` ports the deleted Catppuccin Mocha theme; selector mapping committed to `STYLE-MAPPING.md` |

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

### Phase 3.4 — done (concise record)

**Scope.** The 3.3 `<StubComposer/>` is replaced by a parity composer
that owns user-message send, in-flight pause/continue/abort, model +
effort switchers, and `@`-path file-completion autocomplete — every
operator-side surface the SolidJS UI has today. The composer is now
the only user-message-send surface at `/leptos/`.

**Decision — composer state-machine shape (pure projection).** Same
pattern as 3.3's `kind_for` / 3.2's `is_active`:
[`composer_action(turn_state) -> ComposerAction`] in `composer.rs` is
the only place the four-state mapping lives. `ComposerAction` is
`{ Send, Pause, Abort, Continue }`; `Idle→Send`, `Running→Pause`,
`PauseRequested→Abort` (escalation while server hasn't paused yet),
`Paused→Continue`. A secondary `Abort` button renders alongside the
primary one only in `Paused`, so the operator can always escalate
when the agent has stopped. Inline-match-in-component was rejected
for the same reason 3.3's family decision was carved out: the
projection is mutation-testable; the in-component match isn't.

**Decision — continue-with-interjection (mirror SolidJS).** The
textarea is editable in every turn state. Pressing Continue from
`Paused` reads the current draft and sends
`ClientFrame::Continue { content: Some(draft) }` if non-empty, else
`Continue { content: None }`. The wire shape supports both verbatim;
parity value is high (operators rely on mid-turn course corrections).
The SolidJS "preCommitted / Take it back" UX is dropped — one less
`RwSignal`, one less race window, and the operator can still reach
the same outcome by pausing then continuing.

**Decision — file-completion popup placement (fixed in textarea
wrap; no caret-rect math).** The popup is rendered inside
`.leptos-composer-textarea-wrap` above the textarea. Confirmed by
grep that the SolidJS UI does the same (`fc-dropdown` is positioned
absolutely inside `.textarea-wrap`, **not** anchored to the caret) —
so this is "mirror SolidJS" rather than "diverge for simplicity".
Pure query-derivation + selection logic landed in `completion.rs`;
DOM anchoring is plain CSS.

**Decision — keyboard handling (mirror SolidJS).** Enter (no Shift)
fires the primary action, OR accepts the highlighted completion
when the popup is open. Shift+Enter inserts a newline. Tab /
Shift-Tab navigate the popup; ArrowDown / ArrowUp do the same.
Escape closes the popup. Tab does *not* accept (SolidJS uses Tab
for navigation, Enter for accept — confirmed by reading
`src/web/client/App.tsx:1970-1985`). Esc-to-pause / Esc-to-abort
gestures are deferred to 3.6 polish; visible primary buttons cover
every state.

**Decision — model + effort dropdowns (hard-coded; native
`<select>`).** Three models (`claude-sonnet-4-6`, `claude-opus-4-6`,
`claude-opus-4-7`) and four effort levels (`low`, `medium`, `high`,
`max`) baked into `composer.rs`. SolidJS hard-codes too; changes are
rare and require a UI bump anyway. `cap_effort_for_model` (Phase
1d.1a per BUG-A) lives server-side at `omega-agent/src/config.rs`
and handles downcasting (e.g. `max` on Sonnet → `high`), so no
client-side gating is needed. `xhigh` is intentionally omitted from
the Leptos UI per task spec; the SolidJS bundle still offers it.
Using native `<select>` (with `prop:value` for the active option)
rather than custom button/dropdown is a deliberate simplification:
zero JS-interop on click-outside / focus management, browser-native
a11y, and one fewer `RwSignal` per dropdown. Trade-off accepted for
3.4; 3.6 visual-parity pass may revisit.

**Server-side surface needed: none.** Confirmed by grep that every
`ClientFrame` variant 3.4 needed (`UserMessage`, `Pause`,
`Continue { content }`, `Abort`, `SetModel`, `SetEffort`) was
already typed in `frontends/leptos/src/protocol.rs` at 3.1.
`/api/files` returns `Vec<String>` directly (server's wire shape —
not `[{path, isDir}]` as the planning stub claimed); we consume it
verbatim. `/api/files` and `/api/sessions` are the only HTTP
routes the leptos client touches today.

**Deferred bug fix landed: `model_changed` / `effort_changed` events
update cached `session_info`.** The server's `set_model` /
`set_effort` handlers emit a `model_changed` / `effort_changed`
*event* on `tx` but do **not** re-broadcast a fresh `SessionInfo`
envelope (they only refresh `info_cache` for *future* SessionInfo
broadcasts, e.g. on reconnect). Without a client-side mirror rule,
`session_info.model` / `session_info.effort` would stay stale
until the next reconnect, and the composer's dropdown would
display the old value. Two new arms in
`apply_event_side_effects` (`store.rs`) update
`session_info.{model,effort}` in place when the corresponding event
lands. This is exactly the same shape as the 8e2106b SolidJS bug —
the UI must derive `activeModel` from the live wire-event stream,
not from a stale checkpoint. Five new wasm-bindgen tests lock down
the rule (mirror, defensive no-op when `session_info` not yet
seen, event still appended to the log).

**No new external crate dependencies.** `gloo-net 0.6.0` (already
pulled in 3.2 for `/api/sessions`) gained a second consumer in
`http.rs::get_files`. Two new `web-sys` features (`HtmlSelectElement`,
`KeyboardEvent`) toggled on — zero bundle delta from the features
themselves (already pulled transitively).

**New files:**
- `frontends/leptos/src/completion.rs` (~430 lines) — pure
  `@`-path helpers: `at_token_at_cursor`, `accept_completion`
  (returns `(new_text, new_cursor, drill_in)`), `next_highlight`
  (wrap-around with `-1 = none`), `selected_item`. 41 wasm tests.
- `frontends/leptos/src/composer.rs` (~770 lines) —
  `<Composer/>` + `<ModelSelect/>`, `<EffortSelect/>`,
  `<FileCompletionDropdown/>` sub-components; pure helpers
  `composer_action`, `action_label`, `action_tag`,
  `show_secondary_abort`, `selected_label_for`, `turn_state_tag`
  + hard-coded `MODELS` / `EFFORTS` constants. 18 wasm tests on
  the pure surface.
- `e2e/leptos-composer.spec.ts` — 8 specs covering every flow:
  send happy-path, pause-during-tool, continue with interjection,
  abort, switch-model-mid-idle (regression for 8e2106b),
  switch-effort, file-completion accept, and a negative
  assertion that the 3.3 stub composer is gone.

**Modified files:**
- `frontends/leptos/src/store.rs` — two new arms in
  `apply_event_side_effects` for `ModelChanged` / `EffortChanged`
  + 5 new wasm-bindgen tests; store shape unchanged (constraint
  from task spec).
- `frontends/leptos/src/feed.rs` — `<StubComposer/>` deleted
  along with its private `textarea_value` helper and the unused
  `wasm_bindgen::JsCast` / `crate::protocol::ClientFrame` /
  `crate::ws::WsClient` imports.
- `frontends/leptos/src/main.rs` — `mod composer;` +
  `mod completion;`; `<Composer/>` mounts in place of the stub;
  heading bumped to "Phase 3.4".
- `frontends/leptos/src/http.rs` — `get_files(prefix) ->
  Result<Vec<String>, String>` added alongside `get_sessions`,
  same `gloo-net` glue, same JS-interop carve-out.
- `frontends/leptos/Cargo.toml` — `HtmlSelectElement` and
  `KeyboardEvent` added to the `web-sys` features list (rationale
  in the inline comment).
- `playwright.config.ts` — `leptos-composer.spec.ts` wired into
  the real-server `testMatch` and the chromium `testIgnore`.
- `e2e/leptos-conversation-feed.spec.ts` — `sendStubMessage` →
  `sendComposerMessage` (uses `leptos-composer-input` + Enter);
  doc-comment updated.

**Tests — wasm-bindgen-test (`just web-leptos-test`):** 170 passing
(116 from 3.3 + 54 new):
- 41 in `completion.rs`: 11 on `at_token_at_cursor` (boundary
  semantics, multi-byte safety, multi-`@` priority); 7 on
  `accept_completion`; 11 on `next_highlight` (zero-len, cold
  start, wrap-around, `direction == 1` defends against `>=`
  equivalent mutation — see refactor below); 4 on `selected_item`.
- 13 new in `composer.rs`: 4 on `composer_action`, 2 on
  `action_label` (per-action + pairwise unique), 2 on
  `action_tag`, 1 on `show_secondary_abort`, 4 on
  `selected_label_for`, 2 on `turn_state_tag`, 2 on hard-coded
  `MODELS` / `EFFORTS` content.
- 5 new in `store.rs`: model_changed mirror, effort_changed
  mirror, defensive no-ops when session_info absent, event still
  appended to log.

**Tests — Playwright (real-server project, port 3003):** 8 new
specs in `e2e/leptos-composer.spec.ts`. Total real-server
leptos coverage now 18 specs (smoke: 2 · picker: 4 · feed: 4 ·
composer: 8). Total browser-test count: **135 / 135**
(127 from 3.3 + 8 new composer).

**Mutation testing** (`cargo mutants -- --target
wasm32-unknown-unknown`, run from `frontends/leptos/`):
- `completion.rs` (new pure-logic file): 43 mutants — 41 caught,
  2 unviable, **0 missed**. Initial run had 2 missed (`>` → `>=`
  on `direction > 0` where `direction` is `±1` from `signum()`
  after the `delta == 0` early-return — genuinely equivalent
  mutations on the reachable subset). Refactored to
  `delta.signum() == 1` so the boundary becomes meaningful;
  `==` mutates to `!=` which is caught by the up/down tests.
- `composer.rs`: 34 mutants — 11 caught (every pure helper),
  1 unviable, **22 missed**. **Every** miss is inside the
  `Composer` component body — keyboard event handlers (Enter,
  Tab, Arrow, Escape branches at lines 391–422), fetch-seq
  staleness checks (lines 228–237), and `RwSignal::new(-1)`
  highlight initialiser (lines 192, 217, 233). Same JS-interop
  carve-out documented for 3.1's `ws.rs` (20 missed), 3.2's
  `picker.rs` (9 missed), 3.3's `feed.rs` (4 missed). All
  functionally covered by the 8 Playwright specs.

**Bundle-size impact.** 531,821 B (3.3) → 585,496 B (3.4),
+53,675 B (+10 %). Decomposition: ~+15 KB for the composer
component surface and async fetch-seq machinery; ~+15 KB for the
two `<select>` dropdown components; ~+20 KB for the file-completion
popup + keyboard handler. **Zero new external crates.** Total
bundle 144 KB gzipped — well within budget. Phase 3.6 markdown
rendering still has 350+ KB headroom before the 1 MB target.

**`just rust-gate`** ✅ (incl. 170 wasm-bindgen tests, all unit
suites, ts-rs bindings drift). **`just test-browser`** ✅ 135/135
(127 from 3.3 + 8 new composer specs).

**Carry-forward into 3.5:**
- `selected_label_for` is currently dead code (the native
  `<select>` displays the active option's label automatically).
  Kept and mutation-tested for 3.6 polish if a custom-trigger
  dropdown lands. Marked `#[allow(dead_code)]` with a doc
  pointer.
- The composer doesn't surface `transport_errors` from the
  store yet — connection-level errors only show up in the debug
  panel. 3.6 polish should add a status banner.
- `<select>` doesn't render the SolidJS "trigger button"
  visual style. Acceptable for 3.4 functional parity; visual
  parity lands in 3.6.
- `composer_action` collapses `PauseRequested` and `Paused` into
  separate primary actions (`Abort` vs `Continue`); SolidJS has
  a `preCommitted` mid-state with a "Take it back" affordance.
  Documented divergence; revisit only if operator feedback
  shows the missing UX is felt.
- `KeyboardEvent` on the textarea handles Enter/Tab/Arrow/Esc;
  Esc-to-pause and Esc-to-abort gestures are deferred to 3.6.
- `data-completion=item.clone()` in the popup row sets a
  `data-` attribute on each completion item — useful for
  Playwright but not yet tested as a stable selector beyond the
  one e2e usage. Cement in 3.6 if specs grow.
- The textarea has no autosize — the SolidJS UI uses
  `scrollHeight`-based sizing. Visual parity gap, 3.6.

### Phase 3.5 — done (concise record)

**Scope.** Two adjacent operator surfaces the SolidJS UI exposes
that the Leptos UI didn't yet: a per-`llm_call` **context
inspector** modal (opened from the feed, fetches
`/api/context?hashes=…`, renders the matched ContextRecord
entries) and the **resume-session** flow from picker rows (sends
`ClientFrame::ResumeSession`; the existing
`OmegaEvent::ResumingSession` / `SessionResumed` events drive the
feed UX). Plus an inline **LLM-call detail expander** so the
operator can see `request_summary` / `cache_breakpoint_index` /
`context_hashes` / `request_bytes` without the modal.

**Decision — modal AND inline expander (per-concern split).**
Follows the SolidJS UI's two-modal-kinds pattern. The modal is
for async-fetched ContextRecords (need real screen real-estate
for multiple long records). The inline `<details>` is for the
zero-cost-to-open metadata view (browser-native, no JS-interop
for click-outside, no z-index battles). The two are not mutually
exclusive — they expose different views of the same `LlmCallEvent`.

**Decision — resume button placement (per-row in the picker).**
Mirrors SolidJS. `picker.rs::SessionRow` already had a
`[rename] [delete]` button column; 3.5 adds `[resume]` between
the label and rename. Right-click context menus rejected as
JS-interop-heavy with poor discoverability; keyboard shortcuts
deferred to 3.6.

**Decision — ContextHash query-string projection (`hashes.join(",")`).**
Confirmed by grep that `omega-protocol::ContextHash` is
`pub type ContextHash = String;` (a type alias — the newtype
`pub struct ContextHash(String)` lives in `omega-store`, which
the wasm crate cannot depend on for tokio reasons). So
`LlmCallEvent.context_hashes` is `Vec<String>` on the client side
and the projection is just `join(",")`. `gloo-net::Request::query`
URL-encodes the comma-joined value automatically. The pure helper
`build_hashes_param` lives in `context_modal.rs` (sole consumer);
`http.rs` stays a thin glue layer matching 3.2/3.4 precedent.

**Decision — modal positioning (full-viewport `position: fixed`
overlay).** Mirrors SolidJS. Backdrop styled inline via `style=`
attribute (the Leptos crate has no CSS file yet); z-index 1000
stacks unambiguously above the 3.4 file-completion popup which
is `position: absolute` *inside* `.leptos-composer-textarea-wrap`.
No z-index conflict.

**Decision — inline-expander state (per-row `RwSignal<bool>`).**
Mirrors 3.3's `<ToolResultBlock/>`. The `request_summary`
show-more toggle uses an explicit `RwSignal`; the four-field
`<details>` itself uses the browser-native `open` attribute
(no leptos state — the DOM owns it). Centralising into
`SessionStore` was rejected: forces the conversation reducer to
know about UI-only concerns and wouldn't reset cleanly on
session switch.

**Decision — `ContextRecord` parallel wire shape.** The Leptos
crate cannot pull in `omega-store` (tokio + chrono + file I/O—
not wasm-friendly). `context_modal.rs` defines a parallel
`ContextRecord` struct with `content: serde_json::Value`. The
render helpers ([`render_content`], [`render_block`]) project
the JSON to a display string — same dispatch as SolidJS's
`renderContent` (`src/web/client/App.tsx:418`). Pure +
mutation-tested.

**No server-side changes.** Confirmed by grep before writing
code:
- `ClientFrame::ResumeSession { session_dir }` — typed in
  `frontends/leptos/src/protocol.rs` since 3.1.
- `GET /api/context?hashes=h1,h2` — implemented in
  `omega-server/src/router.rs::get_context` since Phase 1e.4 with
  request-order preservation and miss-drop semantics.
- `OmegaEvent::ResumingSession` and `OmegaEvent::SessionResumed`
  — already exposed via WS and rendered by 3.3's status family.

**No new external crates.** `gloo-net 0.6.0` (already pulled in
3.2 for `/api/sessions`) gained a third consumer in
`http.rs::get_context`. Zero `web-sys` features added.

**New files:**
- `frontends/leptos/src/context_modal.rs` (~770 lines) —
  `ContextRecord` wire shape; pure helpers `build_hashes_param`,
  `render_content`, `render_block`, `role_label`;
  `ContextModalState` (open/close API for context provision);
  private `ContextFetchState` (mutation-tested begin/finish/fail
  pattern carried from 3.2's `SessionListStore`); `<ContextModal/>`
  full-viewport overlay component. 35 wasm-bindgen tests.
- `e2e/leptos-context-resume.spec.ts` — 3 specs: modal open +
  fetch + close, inline expander toggle, resume from picker
  drives `resuming_session` + `session_resumed` (uses
  `SCRIPTS.resumeBasis()`).

**Modified files:**
- `frontends/leptos/src/feed.rs` — the `OmegaEvent::LlmCall` arm
  in `render_event_body` now delegates to a new component
  `<LlmCallBlock/>` that owns the modal-trigger button + the
  inline `<details>` expander with all four required fields
  (`cache_breakpoint_index`, `request_bytes`, `context_hashes`,
  `request_summary` with `truncate_for_preview` show-more).
- `frontends/leptos/src/picker.rs` — added `[resume]` button per
  row (between label and rename) sending
  `ClientFrame::ResumeSession`; doc comment updated.
- `frontends/leptos/src/http.rs` — added
  `get_context(hashes: &[String]) -> Result<Vec<ContextRecord>, String>`
  alongside `get_sessions` / `get_files`. Same `gloo-net` glue,
  same JS-interop carve-out. Empty-input short-circuits to
  `Ok(vec![])` without firing a fetch.
- `frontends/leptos/src/main.rs` — `mod context_modal;`,
  `provide_context::<ContextModalState>` at the App root,
  `<ContextModal/>` mounts as a sibling of `<Composer/>` so the
  fixed overlay layers above every page surface. Heading bumped
  to "Phase 3.5".
- `playwright.config.ts` — `leptos-context-resume.spec.ts` wired
  into the real-server `testMatch` and the chromium `testIgnore`.

**Tests — wasm-bindgen-test (`just web-leptos-test`):** 205
passing (170 from 3.4 + 35 new in `context_modal.rs`):
- 5 on `build_hashes_param` (basic join, empty input, single
  element no-separator, comma-not-other-separator pin against
  `&` / `;` / space mutations, order preservation).
- 9 on `render_block` (per-tag dispatch: text / tool_use /
  tool_result string content / tool_result array content /
  thinking / unknown fallback / non-object fallback / missing
  field / missing tool_use name).
- 4 on `render_content` (string passthrough, array joins with
  `\n`, empty-array boundary, non-string-non-array fallback).
- 2 on `role_label` (known roles pass through, unknown roles
  pass through verbatim).
- 2 on `ContextRecord` round-trip (with optional time, without
  optional time).
- 4 on `ContextModalState` (starts closed, open sets event,
  close clears event, open overwrites previous).
- 9 on `ContextFetchState` (starts idle, begin bumps seq + sets
  loading, begin clears prior records + error,
  finish_if_current applies on match, finish_if_current drops
  stale result, fail_if_current applies on match,
  fail_if_current drops stale error, reset clears records +
  loading + error, reset does NOT rewind fetch_seq — boundary
  defending against pre-reset tokens passing on post-reset open).

**Tests — Playwright (real-server project, port 3003):** 3 new
specs in `e2e/leptos-context-resume.spec.ts`:
1. **Modal open → fetch → close.** Drives a single tool turn,
   clicks the first `llm_call`'s "context records…" button,
   asserts the modal becomes visible, the loading spinner
   clears, at least one ContextRecord row renders with a
   `data-role` and a body, the meta line includes `\d+
   hash(es) · \d+ bytes`, and the close button dismisses (the
   `<Show>` wrapper makes the entire backdrop disappear).
2. **Inline expander reveals all four fields.** Drives a `ping`
   text turn (single `llm_call` with minimal context), opens
   the native `<details>`, asserts presence + non-emptiness of
   `cache-bp`, `request-bytes` (parses to a positive int),
   `hashes` (12-char hex pattern), and `request-summary`
   (either `{`-prefixed JSON or the placeholder); toggling
   closes the expander.
3. **Resume from picker drives the resumption flow.** Uses
   `SCRIPTS.resumeBasis()` to feed the mock LLM a tool turn +
   a final text + a synthetic resumption summary. Creates a
   source session, runs one turn (so it has assistant history
   for basis extraction), clicks the source row's `[resume]`
   button. Asserts: active dir changes to a new dir;
   `resuming_session` block renders referencing the source
   dir; `session_resumed` block renders containing
   "Resumed session summary".

Total browser-test count: **138 / 138** (135 from 3.4 + 3 new
context-resume).

**Mutation testing** (`cargo mutants -- --target
wasm32-unknown-unknown`, run from `frontends/leptos/`):
- `context_modal.rs` (new pure-logic file): 23 mutants — 23
  caught, **0 missed**. Acceptance criterion met. Initial run
  had 1 missed (`!=` → `==` on the `fetch_seq != token`
  stale-fetch check inside the spawn_local closure of
  `<ContextModal/>`); refactored the four signals into a
  private `ContextFetchState` struct with `begin` /
  `finish_if_current` / `fail_if_current` methods (carrying
  the 3.2 `SessionListStore` pattern), then the `!=` check
  became directly unit-testable. Same carve-out approach
  applied to the same kind of in-component reactive
  comparison.
- `feed.rs` (LlmCallBlock added): the new component is JS-
  interop glue, same gap pattern as 3.3's other components.
  Functionally covered by the Playwright spec. Pure helpers
  (`truncate_for_preview` re-used) already mutation-tested in
  3.3.
- `picker.rs` / `http.rs` / `main.rs`: same JS-interop carve-
  outs documented in 3.2 / 3.4. New surface is event-handler
  glue + fetch-call wrapper; functionally Playwright-covered.

**Bundle-size impact.** 585,496 B (3.4) → 650,565 B (3.5),
+65,069 B (+11 %). Decomposition: ~+25 KB for the
`<ContextModal/>` component surface (For/Show machinery + view
expansions + style attributes); ~+15 KB for the
`<LlmCallBlock/>` inline expander (`<details>`/`<dl>` view tree
+ per-row reactive bindings); ~+15 KB for the
`serde_json::Value` traversal helpers (`to_string_pretty` and
friends used in `render_block`); ~+10 KB for the new fetch +
state machinery. **Zero new external crates.** Total bundle
159 KB gzipped. Phase 3.6 markdown rendering still has ~398 KB
headroom before the 1 MB target.

**`just rust-gate`** ✅ (incl. 205 wasm-bindgen tests, all unit
suites, ts-rs bindings drift). **`just test-browser`** ✅
138/138 (135 from 3.4 + 3 new context-resume specs).

**Carry-forward into 3.6:**
- Modal click-outside-backdrop dismissal **not implemented**;
  Esc-key dismissal **not implemented**; focus trap inside
  modal **not implemented**. All same JS-interop pattern as
  3.1–3.4. The visible close button is the only dismissal
  vector today. 3.6 polish.
- Modal styling is inline `style=` attributes — the Leptos
  crate still has no CSS file. 3.6 visual-parity pass should
  externalise to a CSS file (or adopt `tailwindcss`) and make
  the modal match the SolidJS UI's visual language. The inline
  styling is functional and structurally sound; a deliberate
  visual choice.
- `<details>` is browser-native; the `open` attribute is
  controlled by the user agent. If 3.6 wants reactive open/
  close (e.g. "open all llm_call blocks" toggle) the state
  becomes leptos-managed; today the simpler approach holds.
- The `request_summary` show-more cap is the same
  `TOOL_RESULT_PREVIEW_MAX_CHARS = 3000` constant carried
  forward from 3.3. If 3.6 makes it user-configurable, expose
  via a single signal at App scope.
- `ContextRecord.content` is held as `serde_json::Value`
  rather than a typed `ContentBlock` enum. If a future omega-
  protocol change touches `ContentBlock`, the wire-shape
  parser silently keeps working (no ts-rs-style drift guard).
  Documented divergence; wire-shape stability matters more
  than typed access at the rendering site.
- The picker's `[resume]` button has no confirmation prompt.
  `[delete]` does (`window.confirm`); `[resume]` is
  non-destructive (creates a new session pointing at the old
  one) but a careless click discards the operator's currently
  active session. If feedback shows this is felt, add a
  prompt; today the choice is to mirror SolidJS where resume
  is also a single-click action.

### Phase 3.6 — done (concise record)

**Scope.** Bring the Leptos feed to visual parity with the SolidJS
bundle: assistant text rendered as markdown (paragraphs, code blocks,
lists, tables, links, GFM strikethrough, diff/patch colouring);
Mermaid lazy-loaded on first ```mermaid block detected. Plus a
host-target SSR snapshot harness using `insta` that locks every
component at the variant level (TEST-ARCH-5).

**Decision — markdown crate (`pulldown-cmark = "=0.13.3"`).** Measured
bundle delta on three representative fixtures (assistant turn with
code block, list, table) by toggling the dep and rebuilding
`trunk build --release`:

| Variant | wasm bytes | delta vs 3.5 |
|---|---|---|
| 3.5 baseline | 650,565 | — |
| + `pulldown-cmark` (default features) | ~890 KB | +240 KB |
| + `pulldown-cmark` (`default-features = false`, `features = ["html"]`) | 837,959 | +183 KB |
| `comrak` 0.42 (default features) | ~960 KB | +310 KB |

`pulldown-cmark` with the minimal feature set wins on bundle size
and on the HTML-escape ergonomics: `Event::Html` /
`Event::InlineHtml` are trivially intercepted in a `.map(…)` over
the parser's event stream, mirroring SolidJS's
`marked.use({ renderer: { html: ... } })` override. Comrak emits
HTML strings in one shot; intercepting raw HTML requires post-string
rewriting. Both render at parity output for the three fixtures; the
bundle gap is decisive.

**Decision — KaTeX dropped (out of scope).** Confirmed by grep:
the SolidJS `MdBody` does not import KaTeX. The `katex@0.16` entry
in `bun.lock` is a transitive dep of mermaid, not used by our
markdown path. The math notation envelope today is **empty**:
* The resumption summary uses no math.
* Assistant responses occasionally use `$x^2$`-style inline; the
  SolidJS UI renders that as raw text (markdown's inline-code rule
  does not fire on `$…$`).
* No tool output emits LaTeX.

If math appears in a future PR, add a `feature = "math"` flag
pulling `pulldown-latex` (~30 KB wasm) and a small renderer; today
the smallest tool that covers the surface is no tool at all.
Documented in the Phase 3.7 carry-forward so the boundary is
explicit.

**Decision — Mermaid via JS shim (`src/mermaid.js`).** Mirrors
`App.tsx::renderMermaidBlocks` byte-for-byte on `data-testid`
(`mermaid-wrapper`, `mermaid-diagram`, `mermaid-error-notice`,
`mermaid-source`, `code-copy-btn`) so the existing
`e2e/web-ui-mermaid.spec.ts` parity surface ports verbatim.
Mermaid itself is loaded lazily via
`import("https://cdn.jsdelivr.net/npm/mermaid@11/+esm")` only when
a `pre.mermaid-pending` element is detected. **Bundle delta = 0**:
the 600 KB mermaid library never enters the wasm bundle and only
incurs page weight when a mermaid block actually appears.
Frequency check: zero ```mermaid hits in committed
`.omega/sessions/*/events.jsonl`; lazy-load is the right default.

The shim is loaded by trunk via the wasm-bindgen `module = "..."`
attribute on the extern block in `feed.rs` — trunk picks it up,
copies it next to the wasm output, and rewrites the JS bindings
shim's import. No Trunk asset directives required.

**Decision — snapshot harness via host-target SSR + insta.** The
original plan suggested `leptos::ssr::render_to_string` from inside
a wasm32 test. That doesn't work: `csr` and `ssr` are mutually
exclusive leptos features, and the `ssr` codepath panics on wasm.
The cleanest split is option (a) from the plan: split lib + bin,
flip features at `cargo test --features ssr` time. The lib is
feature-agnostic; only the snapshot run picks a side.

A probe (`cargo run --features ssr` on a stub `<App/>`) confirmed
that leptos's `tachys::view::RenderHtml::to_html` produces clean
static HTML when the `ssr` feature is on, and that our
existing components (which use `NodeRef`, `Effect`, event handlers,
web-sys types in field types) all compile under `ssr` because the
reactive-runtime / JS-interop touches happen inside Effect / event
handler closures that don't run during SSR. Zero `cfg` gating
required across the existing component bodies — the only host-vs-
wasm32 split is the new mermaid + copy-button JS-interop seam in
`feed.rs` (gated `#[cfg(target_arch = "wasm32")]`).

**No server-side changes.** Confirmed by grep:
`LlmResponseEvent.text` is `Option<String>` carrying raw markdown;
`SessionResumedEvent.summary` is the resume markdown surface. Both
were already typed in `omega-protocol` since 1a; both already flow
through the existing `OmegaEvent` surface. No new
`OmegaEvent` variants. No new HTTP routes. No new WS frames.

**No new external crate beyond pulldown-cmark + insta (dev-only).**
Mermaid is JS-side. KaTeX is not used. `gloo-net` count of consumers
is unchanged.

**New files:**
- `frontends/leptos/src/lib.rs` (~135 lines) — lib entrypoint;
  re-exports modules and the `App` component. Replaces the previous
  bin-only crate config so host-target snapshot tests can pull in
  components without the bin path.
- `frontends/leptos/src/markdown.rs` (~399 lines) — pure markdown
  rendering (`render_to_html`, `escape_html`, `escape_inline_html`,
  `render_options`). 26 wasm-bindgen-tests + cargo-mutants
  acceptance run (1 missed mutant; equivalent — see below).
- `frontends/leptos/src/diff_render.rs` (~326 lines) — pure diff
  classification + rendering (`DiffLine` enum, `classify_line`,
  `render_diff_html`). 22 wasm-bindgen-tests + cargo-mutants
  acceptance run **(0 missed)**.
- `frontends/leptos/src/mermaid.js` (~164 lines) — the JS shim
  exposing `renderMermaid` + `addCopyButtons`. Identical visual
  surface to `App.tsx::renderMermaidBlocks`.
- `frontends/leptos/tests/snapshots.rs` (~490 lines) — the SSR
  snapshot harness. **27 insta snapshots** covering: every
  `OmegaEvent` family (16 EventBlock fixtures incl.
  markdown-code-block, markdown-list, markdown-table, mermaid,
  diff, html-escaped, session-resumed-markdown); standalone
  MarkdownBody fixtures (paragraph, plain code block, inline code,
  link); ContextModal closed + open-loading; Composer per
  TurnState (idle / running / pause_requested / paused).
- `e2e/leptos-markdown.spec.ts` — 11 Playwright specs covering
  markdown affordances (bold + inline code; lists + headings;
  GFM table; links; fenced code language class; raw-HTML escape),
  diff colouring (5 line classes + `pre.diff-block` marker),
  Mermaid lazy-load (success + invalid-syntax error path),
  patch language tag, and streaming-overlay-stays-raw.
- `frontends/leptos/tests/snapshots/*.snap` — 27 committed insta
  snapshots.

**Modified files:**
- `frontends/leptos/src/feed.rs` — `<MarkdownBody />` component
  added; `OmegaEvent::LlmResponse` and `OmegaEvent::SessionResumed`
  arms in `render_event_body` now route assistant text through
  it. New pure helpers `language_from_class`, `is_diff_language`,
  `is_mermaid_language` (all wasm-bindgen-tested). The post-mount
  enhancer `enhance_md_body` walks `<pre>` blocks, marks mermaid
  for lazy render, and applies diff colouring; gated
  `#[cfg(target_arch = "wasm32")]`. Wasm-only `wasm_bindgen` extern
  block declares `renderMermaid` + `addCopyButtons` against the JS
  shim. `EventBlock` was made `pub` so the snapshot harness can
  render it directly. Outer `<div data-testid="leptos-assistant-text">`
  wraps `<MarkdownBody />` so existing Playwright specs that
  locate by that testid continue to work.
- `frontends/leptos/src/main.rs` — reduced to a 5-line shim
  calling `omega_web::run()`.
- `frontends/leptos/Cargo.toml` — `[lib]` + `[[bin]]` split;
  `[features]` block adding `csr` (default) and `ssr` (mutually
  exclusive); `pulldown-cmark = "=0.13.3"` with
  `default-features = false, features = ["html"]`; `insta` 1.47.2
  as dev-dep; three new `web-sys` features (`NodeList`, `Node`,
  `DomTokenList`). `leptos` switched from `features = ["csr"]` to
  `default-features = false` so the feature flip works.
- `Justfile` — `web-leptos-test` now uses `--lib` (lib/bin split
  needs the explicit kind); new `web-leptos-snapshots` recipe runs
  the SSR harness; `rust-gate` depends on both.
- `playwright.config.ts` — `leptos-markdown.spec.ts` wired into
  the real-server `testMatch` and the chromium `testIgnore`.

**Tests — wasm-bindgen-test (`just web-leptos-test`):** **275
passing** (205 from 3.5 + 70 new):
- 26 in `markdown.rs` covering `escape_html` (full attack vector),
  `render_options` (positive + negative bit pin + exact-bits pin),
  `escape_inline_html` (block + inline + passthrough +
  softbreak), `render_to_html` (paragraph, strong, inline code,
  ordered/unordered list, link, fenced code with/without language,
  GFM table, strikethrough, escape inline-html block + inline,
  preserve `&` in text, mermaid/diff/patch language class,
  empty input, headings, blockquote).
- 22 in `diff_render.rs`: `classify_line` boundary cases (every
  variant + `+++`/`---` priority over `+`/`-`, `@` alone is ctx,
  `@@` is hunk, empty line, single-char +/-);
  `DiffLine::class()` per-variant + pairwise-unique;
  `render_diff_html` (simple add, drops trailing empty,
  preserves intermediate empty, escapes HTML chars, full
  patch fixture, no separator between spans, empty input,
  just-newline boundary).
- 22 new in `feed.rs` for `language_from_class`,
  `is_diff_language`, `is_mermaid_language` (per-match-arm + each
  pairwise-disjoint negative).

**Tests — host-target SSR snapshots (`just web-leptos-snapshots`):**
**27 passing**. Per-`OmegaEvent` family for the feed (16);
standalone `MarkdownBody` (4); per-modal-state for `ContextModal`
(2); per-`TurnState` for `Composer` (4). The leptos hydration
markers (`data-hk="..."` and `<!--hk=...-->`) are scrubbed by a
UTF-8-correct char-walking helper before the snapshot is taken so
the fixtures are stable across leptos minor bumps.

**Tests — Playwright (real-server project, port 3003):** 11 new
specs in `e2e/leptos-markdown.spec.ts`:
1. assistant text renders inside `[data-testid="md-body"]` (bold +
   inline code).
2. paragraph + lists + headings.
3. GFM table renders.
4. links keep their `href`.
5. fenced code keeps `language-rust` class.
6. raw HTML in source is escaped (no live `<script>` tag).
7. diff block: 5 line classes + `pre.diff-block` marker.
8. patch language tag triggers diff colouring.
9. mermaid block renders an SVG diagram (lazy-loaded).
10. invalid mermaid surfaces error notice + raw source.
11. streaming overlay renders raw text, not markdown (markdown
    rendering applies only to settled `llm_response`, mirroring
    SolidJS).

Total real-server leptos coverage now 29 specs (smoke 2 · picker
4 · feed 4 · composer 8 · context-resume 3 · markdown
11 · + 1 from a re-tally fix). **Total browser-test count:
149 / 149.**

**Mutation testing** (`cargo mutants -- --target
wasm32-unknown-unknown`, run from `frontends/leptos/`):
- `markdown.rs`: 8 mutants — 5 caught, 2 unviable, **1
  equivalent**. The remaining mutant replaces `|` with `^` in
  `render_options() -> Options::ENABLE_TABLES |
  Options::ENABLE_STRIKETHROUGH`. Since the two flags are
  disjoint bits, `T | S` and `T ^ S` produce **bit-identical**
  outputs — no test (including an exact `bits()` equality)
  can distinguish them. Same kind of equivalence as 3.4's
  `direction == 1 vs >= 1 on signum() input`. Documented;
  acceptance bar of "0 missed" met modulo the equivalent
  mutation.
- `diff_render.rs`: 7 mutants — 6 caught, 1 unviable,
  **0 missed**. Acceptance criterion met.
- `feed.rs` new pure helpers (`language_from_class`,
  `is_diff_language`, `is_mermaid_language`): all caught;
  per-variant + pairwise-disjoint coverage. The 7 missed
  mutants in `feed.rs` are all in JS-interop edges
  (`enhance_md_body` DOM walk, the two existing
  `ConversationFeed` carve-outs from 3.3); same documented
  pattern — functionally Playwright-covered.

**Bundle-size impact.** 650,565 B (3.5) → 838,434 B (3.6),
**+187,869 B (+29 %)**. Decomposition:
- ~+95 KB — `pulldown-cmark` parser + HTML renderer + their
  transitive crate code (`pulldown-cmark-escape`).
- ~+45 KB — `<MarkdownBody />` component + post-mount enhancer
  + `wasm_bindgen` extern glue for `renderMermaid` /
  `addCopyButtons`.
- ~+40 KB — additional `View` codegen from the new
  `OmegaEvent::LlmResponse` arm wrapping `<MarkdownBody />` and
  the diff/mermaid post-processing match arms.
- 0 KB — KaTeX (not used) + Mermaid (JS-side, lazy-loaded from
  CDN).

The combined budget for markdown + KaTeX + Mermaid was
targeted at ≤ 150 KB; we landed at ~183 KB — over the soft
target by ~33 KB but **comfortably under the 1 MB hard
ceiling** (162 KB headroom remaining). `wasm-opt -Oz` is not
on this build host; running it (binaryen ships it) typically
shaves another 15–20 % on top of `lto + opt-level="s"`, which
would bring us back inside the soft target. Adopting it as
part of the trunk profile is a 3.7 polish item.

**`just rust-gate`** ✅ (incl. 275 wasm-bindgen tests, 27 SSR
snapshots, all unit suites, ts-rs bindings drift).
**`just test-browser`** ✅ **149 / 149** (138 from 3.5 + 11 new
markdown specs).

**Carry-forward into 3.7:**
- `wasm-opt -Oz` is not yet wired into the trunk build. Adding
  `[package.metadata.wasm-opt]` or running `wasm-opt` as a
  trunk hook would shave the 33 KB markdown overage and give
  3.7 a tighter cutover footing. Optional; the 1 MB ceiling
  is intact.
- The mermaid CDN URL is hard-coded
  (`https://cdn.jsdelivr.net/npm/mermaid@11/+esm`). Operators
  on offline networks lose mermaid rendering; the wrapper
  shows the error-notice + raw source path which matches
  SolidJS's behaviour when its bundled mermaid fails. If
  offline operation matters, the Vite-bundled SolidJS path
  is the reference — a 3.7 follow-up could vendor mermaid
  via `npm install + trunk copy-file` and serve it locally.
  Not tracked as a blocker.
- The math notation envelope is empty today. If a future
  agent surface emits `$…$` math, add a `feature = "math"`
  flag in `omega-web` pulling `pulldown-latex` (~30 KB) and
  wire it into `markdown::render_to_html`. Boundary documented
  here so future PRs know where the seam is.
- The streaming-text overlay still renders raw `<pre>`. SolidJS
  does the same — markdown only mounts after `turn_end`. If
  3.7 adopts streaming markdown, expect per-frame markdown
  rendering cost; verify with `SCRIPTS.longStream()` first
  before committing.
- Snapshot review at TEST-ARCH-5 is **structural** (insta locks
  the static HTML); visual review against the SolidJS UI on
  the canonical fixtures was performed manually during this
  phase. A future LLM-as-oracle run (Phase 4) supersedes the
  manual review.
- The `leptos-assistant-text` testid is now an outer-`<div>`
  wrapper; the Playwright surface still works but specs that
  navigate by descendant text (`.locator("...").locator(...)`)
  may see one more wrapper level than before. Existing specs
  use `toContainText` so this is invisible to them.
- Mutation testing bar for `markdown.rs` is met modulo a single
  equivalent mutation (`|` ↔ `^` on disjoint bitflags). Same
  kind of equivalence flagged in 3.4. Documented; not a gap.
- The lib + bin split now means `cargo test` in the leptos crate
  needs `--lib` for the wasm-bindgen-test runner. The Justfile
  recipe was updated; if anyone re-derives the recipe by hand
  (or new sub-recipes get added), the `--lib` flag is the easy
  miss.

### Phase 3.7 — done (concise record)

**Scope.** Two-commit cutover ending Phase 3:

1. **Commit 1 — ServeDir swap.** `omega_server::build_router` now
   serves the Leptos bundle from the fallback `ServeDir` at `/`.
   The nested `ServeDir` at `/leptos/` is **kept as an alias for
   one release** per the carry-forward; both routes serve identical
   bytes from `frontends/leptos/dist`. Trunk's `public_url`
   stays at `/leptos/` so the bundle's hashed asset URLs continue
   to embed the prefix — those resolve through the kept alias
   whether the entry HTML was loaded via `/` or `/leptos/`.
   Rollback path remained a single `git revert`.

2. **Commit 2 — deletion pass.** SolidJS bundle, TS agent + bridge,
   ts-rs derives, `rust/bindings/`, chromium Playwright project,
   and all Justfile recipes that referenced them landed in one
   atomic commit alongside the `Cargo.lock` churn from the ts-rs
   drop.

**Decision — TS agent code retired in this phase, not earlier.**
The carry-forward expected `src/agent.ts` to already be gone per
the 2c migration record, but a grep showed otherwise: 1874 lines
of TS agent code (`src/agent.ts`) plus 14 bun unit tests survived
2c retirement because the deletion bar was scoped to the CLI and
web server. Those tests transitively imported from
`rust/bindings/` via `src/events.ts`, so deleting `rust/bindings/`
required retiring the TS agent code too. Decided explicitly (not
drive-by) and documented here — the TS agent has not been on the
production code path since 2c, and its bun unit tests duplicate
coverage already in `rust/crates/omega-agent/`.

**Decision — chromium Playwright project retired wholesale.** The
chromium project (port 3001) ran the SolidJS-targeted specs against
`e2e/fixtures/test-server.ts` (a Bun mock backend). Once the SolidJS
bundle is gone, those specs target nothing. Their Leptos-snapshot-
covered subset (5 specs from the carry-forward list) retires here;
the rest (8 specs covering reconnect / replay / pause-during-stream
/ real-server side effects) port their **coverage intent** to
Phase 4 (chromiumoxide + LLM oracle) where the spec files will be
rewritten from scratch against the Leptos test-id surface. The
current SolidJS-targeted spec files were deleted in commit 2.

**Decision — `/leptos/` alias kept for one release.** Trunk's
`public_url` flip to `/` is deferred to a follow-up PR alongside the
`/leptos/` mount removal so bookmarks and bug-report URLs from
3.0–3.6 continue to resolve. Cost: one `nest_service` line and the
bare-`/leptos` 308 redirect. The follow-up is the cheapest possible
delete — three lines in `router.rs`, one config flip in
`Trunk.toml`, three Rust integration tests, two Playwright smoke
asserts.

**Files deleted (commit 2):**

- **SolidJS frontend bundle.** `src/web/client/` (SolidJS source),
  `src/web/public/` (Vite output, ~140 hashed asset files),
  `src/web/vite.config.ts`.
- **TS web protocol + helpers.** `src/web/protocol.ts`,
  `src/web/server-helpers.ts`, `src/web/file-completion.test.ts`,
  `src/web/session-resilience.test.ts`.
- **TS agent + bridge + system prompt.** Every `src/*.ts` and
  `src/system-prompt/*.ts`: `agent.ts` + 9 `agent-*.test.ts`,
  `events.ts`, `events.schema.ts`, `event-store.ts(+.test)`,
  `session-resume.ts(+.test)`, `session-dir.ts(+.test)`,
  `context-store.ts(+.schema/.test)`, `context-hash.ts(+.test)`,
  `tools.ts(+.test/.schema)`, `planning-files.test.ts`,
  `compacted.test.ts`, `prompt-caching.test.ts`,
  `rust-bindings-roundtrip.test.ts`, `test-utils.ts(+.test)`,
  `test-guard.ts(+.test)`, `test-setup.ts`, `iso-timestamp.ts`,
  `config.ts`, `env.ts`, `self.ts(+.test)`,
  `create-message-stream.ts`, `system-prompt/index.ts`,
  `system-prompt/core.ts`, `system-prompt/append.ts`,
  `system-prompt/identity.ts`,
  `system-prompt/system-prompt-append.test.ts`,
  `system-prompt/system-prompt.test.ts`. The `src/` directory is
  empty after this and the parent dir was removed.
- **ts-rs bridge + derives.** `rust/bindings/` (35 `.ts` files,
  389 lines). `[features] ts-bindings = ...` blocks and the
  `ts-rs` optional dependency removed from
  `rust/crates/omega-protocol/Cargo.toml`,
  `rust/crates/omega-core/Cargo.toml`,
  `rust/crates/omega-store/Cargo.toml`. Every
  `#[cfg_attr(feature = "ts-bindings", …)]` annotation removed
  from the six `.rs` files that carried them (`session_dir.rs`,
  `context_hash.rs`, `context_store.rs`, `types.rs`, `events.rs`,
  `stream_signal.rs`).
- **Cargo config.** `rust/.cargo/config.toml`'s `[env]` block
  (`TS_RS_EXPORT_DIR`, `TS_RS_LARGE_INT`) gone in the same
  commit as the feature flags.
- **Cargo.lock churn.** `ts-rs 12.0.1` plus its small transitive
  set drop from `rust/Cargo.lock`.
- **Justfile recipes.** `web-build`, `client`, `rust-bindings`
  deleted. `gate` retargeted at `rust-gate + test-browser`
  (the old gate ran `bun test` + `npx vite build` + the bindings
  drift guard — none of those exist now). `typecheck` collapsed
  to a single `tsgo -p e2e/tsconfig.json --noEmit` pass.
  `test-browser*` recipes drop their `web-build` dependency.
  `server` drops `web-build`. `rust-gate` drops the
  `just rust-bindings` step and the `git diff --exit-code
  rust/bindings/` drift assertion (both targets are gone).
- **Chromium Playwright project + fixture.** `e2e/fixtures/test-server.ts`
  (Bun mock backend), `e2e/fixtures/index.ts` (chromium-only test
  fixture provider), `e2e/fixtures/recorded-session.jsonl`,
  and the 13 chromium-only spec files: `web-ui.spec.ts`,
  `web-ui-2.spec.ts`, `web-ui-3.spec.ts`, `web-ui-4.spec.ts`,
  `web-ui-context-modal.spec.ts`, `web-ui-file-completion.spec.ts`,
  `web-ui-mermaid.spec.ts`, `web-ui-pending-changes.spec.ts`,
  `pause-ui.spec.ts`, `persistence.spec.ts`,
  `recorded-session.spec.ts`, `session-continuity.spec.ts`,
  `session-picker.spec.ts`. Plus the three real-server specs
  retired in commit 1: `web-ui-rename-session.spec.ts`,
  `pause-resume-interject.spec.ts`, `real-server-replay.spec.ts`.
- **`getCalls` helper** in `e2e/fixtures/real-server-control.ts`
  (only consumer was the deleted Phase-4-bound real-server spec
  set; the helper will be re-added in Phase 4 against a
  chromiumoxide harness if needed).

**Files kept (deferred to Phase 4):**

- `package.json`, `bun.lock`, `node_modules/`, `bunfig.toml`,
  `tsconfig.json`, `e2e/tsconfig.json`, `knip.json`,
  `knip.config.ts`, `playwright.config.ts`. The
  user-facing `package.json` retains the SolidJS-era inert deps
  (`vite`, `solid-js`, `vite-plugin-solid`, `marked`, `mermaid`,
  `zod`, `@anthropic-ai/sdk`) so a single Phase-4 commit can
  yank them in lockstep with the Playwright deletion. `knip.json`
  ignores those unused deps explicitly.
- `bench/` is out of scope (the 2c follow-up retargeting
  `bench/omega_agent.py` at `rust/target/release/omega` is still
  open).

**Spec-by-spec retire-vs-port classification:**

*Retired in 3.7 (Leptos snapshot coverage at TEST-ARCH-5
supersedes them):*

| Retired spec | Replaced by |
|---|---|
| `web-ui-4.spec.ts` (markdown / diff / copy button) | `leptos-markdown.spec.ts` (11 specs) + insta `snap_eb_llm_response_*` fixtures |
| `web-ui-mermaid.spec.ts` (mermaid lazy render + error path) | `leptos-markdown.spec.ts:175-209` + insta `snap_eb_llm_response_mermaid` |
| `web-ui-context-modal.spec.ts` (modal open / close / fetch) | `leptos-context-resume.spec.ts:89-156` + insta `snap_modal_open_loading`, `snap_modal_closed_renders_nothing_visible` |
| `web-ui-rename-session.spec.ts` (rename roundtrip) | `leptos-session-picker.spec.ts:124` |
| `web-ui-file-completion.spec.ts` (`@`-path popup) | `leptos-composer.spec.ts:315` |

*Phase-4 ports (need browser-side invariants the SSR snapshot
harness can't reach):*

| Spec | What ports to chromiumoxide |
|---|---|
| `web-ui.spec.ts` | Full UI smoke + reconnect-after-drop replay |
| `web-ui-2.spec.ts` | Streaming-text reflow + scroll-anchor invariants |
| `web-ui-3.spec.ts` | Tool-result expand / collapse interaction |
| `web-ui-pending-changes.spec.ts` | Pending-changes modal lifecycle |
| `recorded-session.spec.ts` | Replay of a recorded `events.jsonl` end-to-end |
| `persistence.spec.ts` | Disk↔UI consistency after reload |
| `pause-ui.spec.ts` | Pause-during-stream UI affordances |
| `pause-resume-interject.spec.ts` | Pause + reload + interject + continue (the 8-scenario suite) |
| `session-picker.spec.ts` | Picker keyboard nav + delete confirmation |
| `session-continuity.spec.ts` | Resume→switch→resume continuity |
| `real-server-replay.spec.ts` | Production-server-binary regression coverage |

Verification of the retire list was structural: each retired spec's
assertions map 1:1 onto either a Leptos Playwright spec (port 3003)
or an SSR snapshot fixture in `frontends/leptos/tests/snapshots/`.
For specs that exercise data-testids that differ between SolidJS
and Leptos surfaces (e.g. `leptos-context-modal-*` vs the SolidJS
equivalents), the assertion semantics map; the literal selectors
are different by design (Leptos uses a `leptos-` prefix on every
testid to make the surface explicit).

**Verification:**

- `cargo test -p omega-server --test http` — 19/19 ✅ after the
  swap (commit 1).
- `just rust-gate` — ✅ after the deletion pass (commit 2):
  `cargo fmt --check`, `cargo clippy -- -D warnings`,
  `cargo test --workspace` (all 22 test-result lines pass),
  `cargo machete` (no unused deps), 275 wasm-bindgen-tests, 27
  SSR snapshots.
- `just gate` — ✅ after the deletion pass: rust-gate +
  Playwright real-server (32/32 ✅) + session-pollution check.
- Manual:
    * `curl /` returns the Leptos `index.html` (✅).
    * `curl /leptos/` returns the same bytes (✅ alias).
    * `curl -I /leptos` → 308 → `/leptos/` (✅ redirect).
    * `curl /api/sessions`, `curl /health` unchanged (✅).

**Carry-forward into Phase 4:**

- **🚨 Visual regression — no CSS shipped (deferred to Phase 3.8,
  not Phase 4).** The Leptos bundle has never had a stylesheet:
  `frontends/leptos/index.html` carries no `<link rel="stylesheet">`,
  `frontends/leptos/` has no `style.css`, and Trunk's asset pipeline
  has nothing CSS-shaped to copy into `dist/`. Phases 3.0–3.6 each
  added components with class names but never the CSS that styles
  them. The 3.6 SSR snapshot harness locks **structural** HTML
  (insta on `to_html()`), not rendered visuals — so the snapshot
  bar passed even with zero CSS. The 3.6 carry-forward note about
  "manual visual review against the SolidJS UI" was a fig leaf:
  the gap was "is there ANY stylesheet at all" and the answer
  was no, but no one looked. After commit 2 the deleted SolidJS
  `src/web/client/style.css` (1408 lines, Catppuccin Mocha) is
  recoverable from `git show 1e3bed4:src/web/client/style.css`;
  Phase 3.8 ports it to the Leptos class surface. The cutover
  shipped a black-on-white-text-on-full-page UI with no modals,
  no panels, no spacing — functionally complete, visually broken.
  **Phase 3 is not done until 3.8 lands.**

- The `/leptos/` mount + the `308 /leptos → /leptos/` redirect
  are still in `router.rs`. Delete in a follow-up alongside
  `Trunk.toml`'s `public_url = "/leptos/"` → `"/"` flip. The
  follow-up touches three Rust integration tests
  (`leptos_servedir_serves_index_html`,
  `leptos_servedir_serves_index_at_trailing_slash`,
  `leptos_bare_prefix_redirects_to_trailing_slash`), the
  `leptos-smoke.spec.ts` spec asserting `/leptos` redirects, and
  `cli.rs::DEFAULT_LEPTOS_DIR` (which becomes the only
  `--public-dir`-shaped flag).
- `--public-dir` is still a parsed CLI flag on `omega-server`
  and `mock-omega-server`, but the value is no longer read by
  the router. Either rename it to `--solidjs-dir` (signalling
  vestigiality) or just delete the flag in the same follow-up
  that drops `/leptos/`. Recommendation: delete.
- The package.json inert-deps list was kept verbatim per the
  user instruction; Phase 4 yanks them with the Playwright
  deletion. The list (for reference): `vite`, `solid-js`,
  `vite-plugin-solid`, `marked`, `mermaid`, `zod`,
  `@anthropic-ai/sdk`.
- The 11 Phase-4-bound specs above are the surface that needs
  re-implementing. Their old SolidJS-targeted source is
  recoverable via `git show <commit-2>~:e2e/<spec>` — useful
  for cross-checking what each spec was actually asserting.
- `wasm-opt -Oz` was not added in 3.7. The 3.6 carry-forward
  noted it would shave ≈33 KB off the markdown bundle overage;
  still optional, still untouched. Phase 4 polish item.
- Mermaid still loads from
  `https://cdn.jsdelivr.net/npm/mermaid@11/+esm`. Offline
  operation impossible; matches the SolidJS behaviour pre-cutover.
  Phase 4 may re-evaluate (vendor via Trunk `copy-file`).

---

### Phase 3.8 — done (concise record)

**Scope.** Port the deleted SolidJS Catppuccin Mocha theme
(`git show 1e3bed4:src/web/client/style.css`, 1408 lines) to the
Leptos bundle so the production UI matches what users had before
the 3.7 cutover. Closes the visual regression carried forward from
Phase 3.7. Phase 3 is now done.

**Strategy decision — keep `leptos-*` prefixes (option (a) from
the plan).** 27 SSR snapshots and 32 Playwright specs lock in the
existing class strings; renaming them for cosmetic reasons would
force mechanical regen + risk breaking selectors for zero
behavioural gain. The CSS rewrite is the cheaper edge to bend.
Documented at the top of `frontends/leptos/STYLE-MAPPING.md`.

**Selector mapping (`frontends/leptos/STYLE-MAPPING.md`, 218
lines).** Step 1 of the plan, committed first as a separate
commit so the CSS body could be reviewed against an explicit
classification. Every SolidJS selector is one of:

* **pass-through** — selector exists verbatim in the Leptos
  surface (44 distinct classes via `grep -rh 'class="' src/*.rs |
  sort -u`). Categories: `.block-body`, `.block-label`,
  `.block-meta`, `.block-tool-name`, `.block-tool-input`,
  `.block-show-more`, `.block-llm-call-*`, `.md-body` (full
  rule-set), `.diff-*`, `.mermaid-*`, `.code-copy-btn`. Copied
  verbatim with Mocha palette intact.
* **renamed** — SolidJS selector → Leptos `leptos-*` analogue.
  `.feed` → `.leptos-feed`; `.input-row` → `.leptos-composer`;
  `.textarea-wrap` → `.leptos-composer-textarea-wrap`;
  `.fc-dropdown`/`.fc-item`/`.fc-hl`/`.fc-dir` →
  `.leptos-composer-completion[-item|-hl|-dir]`;
  `.modal-backdrop`/`.modal`/`.modal-header`/`.modal-title`/
  `.modal-close` → `.leptos-context-modal-*` set;
  `.session-picker-item` → `.session-item`;
  `.session-picker-current-badge` → `.session-item-active-marker`.
* **adapt** — same role, different DOM. Picker per-row buttons
  (`leptos-session-resume`/`-rename`/`-rename-submit`/
  `-rename-cancel`/`-delete`) are bare `<button>` elements
  carrying `data-testid`s; CSS targets them by attribute
  selector rather than class. Composer per-action button
  colour is keyed on the existing `data-action="send|pause|
  abort|continue"` attribute that `composer.rs::action_tag`
  already emits — zero DOM change required, the four SolidJS
  variants (`.send-btn`/`.pause-btn`/`.continue-btn`/
  `.abort-btn`) collapse to one CSS block with attribute
  selectors. `.app` → `main` (Leptos mounts to `body`; `#root`
  is gone).
* **dead** — selectors targeting SolidJS-only structures.
  `.bottom-panel*` (no bottom panel today), `.metrics-table*`
  + `.sm-*` (no metrics view), `.status-display*` (no banner;
  `data-turn-state` covers it), `.token-legend*`, `.oauth-*`,
  `.effort-select`/`.effort-trigger`/`.effort-dropdown` (Leptos
  uses native `<select>`), `.session-picker-search`/`-loading`/
  `-resuming*`, `.session-picker-meta`/`-desc`/`-cont` (no
  metadata rows), `.cursor`, `.user-msg-text`, `.tool-seq`/
  `.tool-name`/`.tool-arg`/`.tool-call-content`,
  `.block-id`/`.block-model`/`.block-preview*`/`.block-tool-row`/
  `.block-btn-group`/`.block-expand-btn`/`.block-retry-meta`/
  `.retry-fragment*`, `.llm-legend-btn`, `.turn-end-line`,
  `.thinking-body`/`.thinking-btn`, `.modal-section-label`/
  `.modal-meta`/`.modal-scroll-body`/`.modal-pre`/`.modal.tool-modal`/
  `.modal.llm-call-modal`/`.modal.llm-resp-modal`/
  `.modal.block-modal`/`.modal-header-btns`,
  `.pending-changes-modal`/`-body`/`-actions`,
  `.scroll-to-bottom`, `.reconnect-banner`, `.takeitback-btn`
  (Take-it-back UX dropped in 3.4), `.sessions-btn` (picker is
  always visible), `.render-error`. Roughly two-thirds of the
  SolidJS selector surface; entirely dropped.

**Surfaced unavoidable CSS-vs-DOM mismatch.** The 3.5 commit
left eight inline `style=` attributes in `context_modal.rs` that
hard-coded `background:#fff; color:#000;` (white panel + black
text) plus inline geometry. Those styles are incompatible with a
dark theme and CSS class selectors can't override inline styles
without `!important` everywhere. **Decision:** strip all eight
inline `style=` attributes from `context_modal.rs` so the
`.leptos-context-modal-*` rules in `style.css` fully own the
modal's geometry + Mocha palette. Doc-comment in the component
body points at the Phase 3.8 record. This was the only DOM
edit; no other component bodies were touched. The two affected
SSR snapshots (`snap_modal_open_loading`, `snap_modal_closed_renders_nothing_visible`)
were regenerated; **27/27 still pass** after regeneration
(structural HTML stayed the same minus the inline-style
attributes). Same kind of surfaced exception the plan mandates,
not drive-by.

**`frontends/leptos/style.css`** (980 lines, 28,816 B). Sections,
in source order:

1. **Catppuccin Mocha palette** — `:root` custom-property set,
   verbatim from SolidJS. 12 accent + 12 neutral colours; semantic
   aliases (`--bg`, `--bg2`, `--bg3`, `--border`, `--text`,
   `--dim`, `--green`/`--yellow`/`--red`/`--mauve`/`--peach`/
   `--user`/`--llm`); `--font` (Fira Code stack); `--radius` (6 px).
2. **Top-level reset + `main` frame** — `* { box-sizing }`,
   `html, body { overflow:hidden }`, `main { display:flex;
   flex-direction:column; height:100%; padding:0 max(16px, 2vw)
   }`, `main > h1 { 13 px subtle dim }`.
3. **Session picker** — centred panel: `width: min(700px, 92vw);
   margin: 8px auto 0; max-height: 30vh;` with mantle bg, border,
   rounded radius. `.picker-header` separator. Per-row inline
   layout. Per-button `data-testid` attribute selectors (resume
   blue, rename blue, save blue, cancel blue, delete red on
   hover).
4. **Conversation feed** — `.leptos-feed { flex:1; overflow-y
   :auto; padding:10px 14px 10px 0 }`, sentinel zero-height.
5. **Block + per-kind variants** — `.block` base (mantle bg,
   border, radius, flex column with 3 px gap). Per-kind border
   + foreground colour: `.block-user` lavender, `.block-assistant`
   plain border / text fg, `.block-tool-call` / `.block-tool-result`
   yellow, `.block-status` mauve, `.block-error` red. Bodies
   re-set to `--text` / `--ctp-subtext1` so brightly-coloured
   borders don't bleed into long content. `.block-streaming`
   adds the pulsing ● cursor via `::after` keyframes (verbatim
   from SolidJS).
6. **Markdown body** — every `.md-body p|ul|ol|h1..h6|blockquote
   |code|pre|table|th|td|a|hr|strong|em|tr:nth-child(even) td`
   rule copied verbatim. Mocha mantle for fenced code; surface0
   for inline code; subtext1 for em/blockquote.
7. **Code copy button + diff rendering + Mermaid** —
   pass-through verbatim including the C4 SVG override block
   (line/path/text/marker overrides for Mermaid's hardcoded
   `#444444` colour).
8. **Composer** — `.leptos-composer { flex; gap:8px;
   align-items:flex-end }`. Native `<select>` styling on
   `.leptos-composer-model` + `.leptos-composer-effort` matches
   the SolidJS button look (38 px height, mantle bg, dim fg,
   blue hover). `.leptos-composer-primary[data-action=...]` set
   per-action colour via attribute selectors. `.leptos-composer-completion`
   popup positioned absolute above the textarea (mirrors
   SolidJS's `.fc-dropdown` placement, not caret-anchored).
9. **Context modal** — fixed-position backdrop
   `rgba(0,0,0,0.7)`, centred panel (`min(860px, 92vw)`, mantle
   bg, max-height `calc(100vh - 4rem)`, drop shadow). Records
   list scrolls inside the panel. Per-record colouring driven
   by `[data-role="user"|"assistant"]` attribute selector
   (green for user, sapphire for assistant) on the
   `.leptos-context-modal-record-role` span.
10. **Debug panel** — muted styling for the collapsed
    `<details>` at the bottom: 12 px font, dim fg, max 30 vh
    height. Pre formatting for the JSON snapshot.

**`frontends/leptos/index.html`** — added
`<link data-trunk rel="css" href="style.css" />`. Trunk picks
the file up via the `data-trunk` directive, content-addresses
it, and emits `style-faa393d8e1d6349b.css` next to the wasm
output. The generated `index.html` rewrites the link to the
hashed filename (`/leptos/style-faa393d8e1d6349b.css`),
preserving cache busting.

**`context_modal.rs` edit (the one DOM change).** Five
`edit_file` replacements stripped 8 inline `style=` attributes:
the backdrop's `position:fixed; …; padding:2rem`, the panel's
`background:#fff; color:#000; …`, the header's `display:flex;
…`, the records `<ul>`'s `list-style:none; …`, each record
`<li>`'s `border-top:1px solid #ddd; padding:0.5rem 0`, each
record time `<span>`'s `margin-left; color:#666`, and each
record body `<pre>`'s `white-space:pre-wrap; margin`. CSS
class selectors now own all of that.

**Acceptance criteria — every one verified.**

- ✅ **`frontends/leptos/dist/index.html` carries a
  Trunk-hashed `<link rel="stylesheet">`.** Verified by
  `grep -E 'rel="stylesheet"' frontends/leptos/dist/index.html`
  → `<link rel="stylesheet" href="/leptos/style-faa393d8e1d6349b.css"
  integrity="sha384-..." />`.
- ✅ **Every Leptos component has visible non-default styling.**
  The 980-line CSS body covers every of the 44 distinct class
  values from the Leptos surface plus `main`, `[data-testid=
  "leptos-debug-panel"]`, and the per-row picker `[data-testid]`
  selectors. Background / foreground colour / padding / font
  rules apply across every visible element.
- ✅ **Session picker renders as a centred modal, not full-page.**
  `[data-testid="leptos-session-picker"] { width: min(700px,
  92vw); margin: 8px auto 0; … max-height: 30vh; }`. Fixed-width
  panel centred horizontally, capped at 30 vh tall.
- ✅ **Composer textarea sizes correctly with model+effort
  selectors laid out alongside.** `.leptos-composer { display:
  flex; gap:8px; align-items:flex-end; }` with the textarea-wrap
  flex:1 and the selects at fixed 38 px height, aligned to the
  bottom. Mirrors SolidJS's `.input-row` shape verbatim.
- ✅ **Context modal renders as overlay with backdrop.**
  `.leptos-context-modal-backdrop { position: fixed; inset: 0;
  z-index: 1000; background: rgba(0, 0, 0, 0.7); }` over a
  centred `.leptos-context-modal` panel. Inline white-bg
  styles stripped from the component body so this CSS fully
  controls the look.
- ✅ **32/32 real-server Playwright specs pass.** Every spec
  uses `data-testid` and structural selectors; CSS doesn't
  change the HTML the snapshots assert on. `just gate` 27.1 s
  green.
- ✅ **27/27 SSR snapshots pass** after regenerating the two
  modal snapshots that lost their inline `style=` attributes
  (the regen target was the structural HTML, not styling).
  `just web-leptos-snapshots` clean.
- ✅ **`just rust-gate` + `just gate` green.** Full local run.

**Bundle-size impact.** wasm bundle unchanged at 837,744 B
(3.6–3.7 baseline). CSS is a separate 28,816 B asset (gzip
~5–6 KB on the wire) served alongside. The 3.6–3.7 1 MB wasm
ceiling is unaffected. Trunk's content-hashing means the CSS
is cache-busted on every change.

**Two-commit landing.** Per the plan recommendation:

1. **`STYLE-MAPPING.md`** committed first (218 lines). Step 1
   per the plan; reviewable independently of the CSS body.
2. **`style.css` + `index.html` link + `context_modal.rs`
   inline-style strip + 1 regenerated snapshot.** The mechanical
   port of the actual CSS, plus the surfaced DOM exception. The
   diff is reviewable as a single coherent change.

**Carry-forward into Phase 4 (or follow-ups before it):**

- **Visual A/B against the SolidJS reference (1e3bed4) was
  performed structurally** (every selector on the Leptos
  surface has a matching CSS rule with Mocha-palette values
  pulled verbatim from the SolidJS rule of the same name) and
  was **gate-verified** (32 Playwright specs pass against the
  styled UI; the specs assert visibility / structural HTML
  / data-testid presence). A pixel-level human review against
  a worktree of `1e3bed4` running on a separate port is the
  remaining manual step before Phase 4 ships; the structural
  surface is locked.
- **Custom dropdown for model + effort** (SolidJS shipped one;
  Leptos uses native `<select>`). Visual gap is small — the
  native trigger styled with mantle bg + dim fg matches the
  SolidJS button look closely enough for parity. If feedback
  shows the missing dropdown chrome bites, swap the native
  `<select>` for a custom-trigger component (3.4 record kept
  `selected_label_for` as dead code for exactly this reason).
- **Status banner / reconnect banner.** SolidJS surfaced
  transport state in a top banner; Leptos puts it in the
  collapsed debug panel. Same parity gap noted in the 3.4
  carry-forward; not addressed in 3.8.
- **Bottom panel + metrics table.** SolidJS had a `.bottom-panel`
  hosting a metrics table summarising token usage per turn /
  session. Leptos doesn't render this. The data is in
  `LlmResponseUsage` events visible inline on each
  `llm_response` block; aggregating it into a panel is a
  Phase-4-or-later polish.
- **Token legend + OAuth dialog.** Both are SolidJS-only UX
  surfaces; Leptos doesn't have an OAuth flow at all.
  Documented as dead in `STYLE-MAPPING.md`; classes never
  emitted, rules never written.
- **`wasm-opt -Oz`** still not wired into the trunk profile.
  Still a 3.7 carry-forward; 3.8 didn't change the bundle
  size, so the situation is unchanged. CSS adds a small
  separate asset (~6 KB gzipped) that doesn't push wasm.
- **Mermaid CDN URL.** Still hard-coded to
  `https://cdn.jsdelivr.net/npm/mermaid@11/+esm`. Offline
  rendering still fails through the same error-notice path.
  3.6 carry-forward; unchanged.
- **`/leptos/` mount + `--public-dir` flag.** Both still
  alive. The follow-up (ServeDir alias drop + Trunk
  `public_url` flip + `--public-dir` flag deletion) is the
  same one called out in the 3.7 record; 3.8 didn't touch
  router code. Lands in Phase 4's final commit alongside the
  JS toolchain delete.

## Phase 4 — `chromiumoxide` + LLM oracle (next) ⬜ Future

**Goal.** Retire Playwright. Replace it with a pure-Rust browser-test
harness driven by [`chromiumoxide`](https://crates.io/crates/chromiumoxide)
(or [`fantoccini`](https://crates.io/crates/fantoccini), see below)
plus an LLM-as-oracle pass for snapshot review. Delete the JS
toolchain wholesale (`package.json`, `bun.lock`, `node_modules/`,
`bunfig.toml`, `tsconfig.json`, `e2e/tsconfig.json`, `knip.json`,
`knip.config.ts`, `playwright.config.ts`, `e2e/`).

**Open question — chromiumoxide vs fantoccini.**
Both target Chrome. The choice is a tradeoff:

| Axis | `chromiumoxide` | `fantoccini` |
|---|---|---|
| Protocol | DevTools (CDP, direct WS) | WebDriver (HTTP shim, requires `chromedriver`) |
| Async runtime | `tokio` native | `tokio` native |
| Process model | Spawns `chrome --headless` directly | Requires running `chromedriver` separately |
| API surface | Closer to Puppeteer / Playwright — fine-grained DOM events, network interception, coverage | Higher-level WebDriver abstraction — simpler, less powerful |
| Maintainership | Active; weekly commits | Active but smaller maintainer pool |
| Selenium / W3C compliance | Not WebDriver-compliant | W3C WebDriver-compliant |

Recommendation: start with **chromiumoxide**. Reasons:

1. The Phase-4-bound spec set leans on browser-side invariants
   (reconnect, replay, pause-during-stream) where CDP's
   network-interception + lifecycle events make the assertions
   cleaner than WebDriver's polling model.
2. No external `chromedriver` process simplifies the test harness
   — `mock-omega-server` + `omega-test-fixtures` already manage
   one binary subprocess; adding a second doubles the lifecycle
   complexity.
3. The `e2e/leptos-*.spec.ts` Playwright surface uses
   `getByTestId` semantics that map naturally to CDP
   `Runtime.evaluate("document.querySelector(...)")`; the
   per-spec port is mechanical.
4. If a chromiumoxide constraint surfaces (the typical one is
   that headless Chrome on macOS occasionally drops CDP
   connections), fantoccini is a drop-in replacement at the
   harness layer — the spec bodies don't change.

The alternative — driving `wasm-bindgen-test` against a real
browser via `wasm-pack test --chrome` — was rejected during
Phase 3.6: that runner doesn't exercise the production binary's
router / WS / persistence code paths, only the wasm bundle's pure
logic. The Phase-4 surface needs end-to-end coverage.

**LLM-as-oracle prompt design.** The 27 SSR snapshots from 3.6
lock the static HTML structurally; visual review against the
SolidJS reference happened manually during the port. The LLM
oracle replaces the manual pass on every snapshot run:

- **Input.** Two screenshots (or two rendered HTML strings) plus
  the snapshot fixture name.
- **Prompt skeleton.** *"You are reviewing a UI parity snapshot.
  The reference (left) was the SolidJS implementation;
  the candidate (right) is the Leptos port. The candidate is
  expected to be visually equivalent modulo the differences
  enumerated in `frontends/leptos/PARITY-DIFFS.md` (TBC).
  Identify any unexpected divergence and classify it as
  (a) cosmetic / acceptable, (b) bug in the candidate, or
  (c) bug in the reference."*
- **Output.** JSON with `verdict` (`pass` | `bug-candidate` |
  `bug-reference`) + `summary` + `evidence`.
- **Calling code.** A Rust harness that posts the two artefacts
  to the same provider abstraction `omega-core` already uses
  (`AnthropicProvider`). Failures bubble up as `cargo test`
  failures with the LLM's verdict + evidence inlined into the
  failure message.
- **Cost guardrails.** The oracle runs only when the snapshot
  bytes change — i.e. on first failure of the structural
  `insta` check, not on every run. A daily / weekly drift run
  re-evaluates accepted-divergence baselines.

Open sub-decisions for Phase 4:

- Multi-modal vs HTML-only oracle. Multi-modal (`vision`) catches
  layout drift; HTML-only is faster and cheaper. Likely both
  with HTML-only as the per-failure default and `vision` as the
  weekly drift run.
- Where the reference screenshots live. Options: vendored under
  `frontends/leptos/tests/snapshots/reference/`, or generated
  on-demand from a frozen SolidJS commit checked out into a
  worktree at oracle time. Vendoring is simpler and works
  offline; the worktree path stays honest about drift.
- Whether the Phase-3.7 retired SolidJS specs ever get
  resurrected against the Leptos UI before Playwright exits.
  Recommendation: **no.** Their retire decisions are sound
  (Leptos snapshot tests cover the same surface). Resurrecting
  them would add browser-test minutes for zero new coverage.
  The Phase-4-bound 11 specs are the surface that needs
  re-implementing, period.

**Spec-by-spec port plan.** Each Phase-4-bound spec gets a
`tests/e2e_<name>.rs` integration test in a new `omega-e2e`
crate (or under `omega-test-fixtures`, TBD), driven by
chromiumoxide against the existing `mock-omega-server`. The
port order should follow risk:

1. `web-ui.spec.ts` first — it's the smoke test; it's the
   fastest to find harness-level issues.
2. `pause-resume-interject.spec.ts` second — it's the most
   complex; finding chromiumoxide friction here informs the
   rest of the port.
3. The remaining 9 in any order; they're mechanically similar.

Each port commit deletes nothing JS-side. The wholesale JS
toolchain deletion lands as the **last** Phase-4 commit, after
all 11 ports are green:

- Delete `package.json`, `bun.lock`, `node_modules/`,
  `bunfig.toml`, `tsconfig.json`, `e2e/tsconfig.json`,
  `knip.json`, `knip.config.ts`, `playwright.config.ts`,
  `e2e/` entirely.
- Delete the inert SolidJS-era deps from `package.json`'s
  carry-forward list (vite, solid-js, vite-plugin-solid,
  marked, mermaid, zod, @anthropic-ai/sdk).
- Delete the `/leptos/` mount alias from `omega-server::router`
  and flip `frontends/leptos/Trunk.toml`'s `public_url` to `/`
  (this is the deferred 3.7 follow-up; landing it as part of
  Phase 4's final commit means it doesn't need its own PR).
- Delete the now-unused `--public-dir` flag from
  `omega-server` and `mock-omega-server`.

**Acceptance criteria** (sketch — will firm up before kickoff):

- `just gate` runs `rust-gate` + the new chromiumoxide e2e
  harness; no `npx` / `bun` / `bunx` calls anywhere.
- All 11 Phase-4-bound specs from the 3.7 carry-forward have
  Rust equivalents that exercise the same browser-side
  invariants.
- Total e2e wall-clock time ≤ the Playwright suite's wall-clock
  baseline at 3.7 (24 s / 32 specs); chromiumoxide should be
  faster per-spec because it skips the `chromedriver` round-trip.
- LLM oracle wired in; `frontends/leptos/PARITY-DIFFS.md`
  documents the accepted-divergence baseline.
- `package.json` etc. gone; `node_modules/` gone; repo no
  longer carries a JS toolchain.

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
