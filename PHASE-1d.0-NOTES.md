# Phase 1d.0 Notes — RESUMPTION CHECKLIST FIRST

## CURRENT STATE (resume here)

**Phase 1d.0a DONE.**

- `cargo test -p omega-agent`: 13 unit + 6 integration green.
- `just rust-gate`: green.
- `cargo mutants -p omega-agent`: **29 mutants — 20 caught, 6 unviable, 3 missed.** Misses are all in low-value helpers, not core logic:
  1. `now_iso()` body replaced — timestamp helper not directly asserted; acceptable.
  2. Same again with a different replacement.
  3. `read_system_prompt_append` `match guard err.kind() == NotFound` — the graceful-fallback branch isn't exercised by a unit test (would need a permission-denied directory).

  These are acceptable for a v1 slice. Tighten later if/when the helpers grow.

### How far along
- `cargo test -p omega-agent` is **GREEN** — 13 unit + 6 integration tests pass on first try.
- `just rust-gate` **fails** with clippy lints, but only style — no real bugs.
- Build of agent.rs lib: GREEN.

### Resume here: fix the remaining clippy lints, then commit.

#### Lib lints (in `crates/omega-agent/src/`):
1. `agent.rs` line ~302: `serde_json::to_vec(&request).map(...).unwrap_or(0)` should become `.map_or(0, |v| ...)`. **Already attempted in last edit_file call; if not applied, redo.**
2. `agent.rs`: tracking `Vec<Option<...>>` with `expect(...)` for tool results was replaced with `HashMap<String, (String, bool)>` keyed by tool_use id. **Already attempted in last edit_file call; if not applied, redo.**
3. `agent.rs::send_message`: add `#[allow(clippy::too_many_lines)]` above pub fn. **Already attempted.**
4. `error_classify.rs` line 34: `agent_error` should be backticked: `´agent_error´ message`.
5. `system_prompt.rs::build_system_prompt`: collapse the nested `if let Some(extra) = ... { if !extra.is_empty() { ... } }` to `if let Some(extra) = ... && !extra.is_empty() { ... }`. **Already attempted.**

#### Test lints (each test file + tests/common/mod.rs):
Add at the top of each integration test file AND tests/common/mod.rs:
```rust
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, clippy::doc_markdown, clippy::too_many_lines, clippy::wildcard_enum_match_arm)]
```
Files needing this: `tests/common/mod.rs`, `tests/single_text_turn.rs`, `tests/parallel_tools.rs`, `tests/retry_then_success.rs`, `tests/non_retryable.rs`, `tests/invalid_tool_json_nudge.rs`, `tests/dangling_tool_use_repair.rs`.

Note: `tests/common/mod.rs` already has `#![allow(dead_code)]` — extend that line to add the clippy allows.

Also possibly: `parallel_tools.rs` had a `using contains() instead of iter().any() is more efficient` warning — might need to switch `t.iter().filter(...).count()` style or just allow it.

#### After fixes:
1. `cd rust && cargo fmt --all && just rust-gate` — must be green.
2. `cd rust && cargo mutants -p omega-agent` — record results (timeout 600s probably).
3. `cd /home/carsten/omega/dev && git add -A && git commit -m "Phase 1d.0a: omega-agent core loop + MockProvider tests"`
4. STOP. Report results to user. Tell them to start fresh Sonnet session for 1d.0b.

### Files written this session (Opus, 1d.0a)
- `rust/crates/omega-agent/Cargo.toml` — deps: omega-{protocol,core,store,tools}, async-stream, chrono, futures, serde_json, thiserror, tokio (sync/time/macros/rt/fs), tokio-util (rt). dev: tempfile, tokio-full, async-trait.
- `rust/crates/omega-agent/src/config.rs` — `max_output_tokens_for_model` + tests. DONE.
- `rust/crates/omega-agent/src/system_prompt.rs` — `build_system_prompt`, `read_system_prompt_append`, `system_prompt_append_path`. Verbatim port of TS core.ts. DONE + tests.
- `rust/crates/omega-agent/src/error_classify.rs` — `is_invalid_tool_json` (Stream prefix `"malformed tool_use JSON"`), `is_context_too_long` (HTTP 429 with `"Extra usage is required for long context requests"` in body). DONE + tests.
- `rust/crates/omega-agent/src/agent.rs` — Agent struct + AgentConfig + send_message stream. **HAS COMPILE BUG**: the LlmRetry arm in the provider stream loop uses `boxed_clone(boxed)` where `boxed` has already been moved by `match *boxed`. Must replace the entire match block with: `let event = *boxed; match event { ... OmegaEvent::LlmRetry(retry) => { text_buf.clear(); thinking_buf.clear(); let ev = OmegaEvent::LlmRetry(retry); let _ = self.event_store.append(&ev).await; yield AgentItem::event(ev); } ... }` and DELETE the `boxed_clone` helper at the bottom.

### Still TODO this session before declaring 1d.0a done
1. Fix agent.rs LlmRetry arm (above).
2. Write `rust/crates/omega-agent/src/lib.rs` with: `pub mod agent; pub mod config; pub mod error_classify; pub mod system_prompt;` plus `pub use agent::{Agent, AgentConfig};`.
3. Write `rust/crates/omega-agent/tests/common/mod.rs` with a `MockProvider` that implements `omega_core::Provider`. Holds `Mutex<VecDeque<Vec<Result<AgentItem, LlmError>>>>` — each call pops the next prepared transcript and converts to a `BoxStream` via `futures::stream::iter`. Plus a helper `make_test_agent(provider) -> (Agent, TempDir)` that builds tempdir + ContextStore + EventStore + AgentConfig{model:"claude-sonnet-4-6", cwd: tempdir, system_prompt_append: None}.
4. Write the 6 test files listed under "Tests" further down. Each is `tests/<name>.rs` with `mod common;` at top.
5. `cd rust && cargo fmt && just rust-gate` — gate must be green.
6. `cd rust && cargo mutants -p omega-agent` — record results.
7. Commit with `git add -A && git commit -m "Phase 1d.0a: omega-agent core loop + MockProvider tests"`.
8. STOP. Report results to user. Tell them to start fresh Sonnet session for 1d.0b.

### MockProvider sketch
```rust
pub struct MockProvider {
    responses: Mutex<VecDeque<Vec<Result<AgentItem, LlmError>>>>,
}
impl MockProvider {
    pub fn new() -> Self { Self { responses: Mutex::new(VecDeque::new()) } }
    pub fn push(&self, items: Vec<Result<AgentItem, LlmError>>) {
        self.responses.lock().unwrap().push_back(items);
    }
}
impl Provider for MockProvider {
    fn stream(&self, _req: LlmRequest) -> AgentItemStream {
        let items = self.responses.lock().unwrap().pop_front()
            .unwrap_or_default();
        Box::pin(futures::stream::iter(items))
    }
}
```

### The 6 tests (checklist)
1. **single_text_turn.rs** — single text reply.
2. **parallel_tools.rs** — two ToolCalls dispatched in parallel, then second turn finishes.
3. **retry_then_success.rs** — LlmRetry event in stream forwarded; turn completes.
4. **non_retryable.rs** — HTTP 400 → LlmError + AgentError + TurnInterrupted{Error}.
5. **invalid_tool_json_nudge.rs** — Stream error with `malformed tool_use JSON` prefix → nudge UserMessage appended, turn retries, succeeds.
6. **dangling_tool_use_repair.rs** — history pre-seeded with dangling assistant tool_use; first event of new turn is synthetic ToolResult{is_error:true}.

Use `omega_store::random_hash()` to forge ContextHashes for seed_history.

- `omega-tools/` — schemas, format_tool_call, dispatch all REAL + tested. Tool bodies STUBBED until 1d.0b.
- `omega-agent/` — was empty stub. NOW being implemented per the algorithm below.
- `omega-cli/` — clap stub, will be wired in 1d.0b.
- `just rust-gate` is green at the start of this session (post-commit `1c01f15`).

**Resume point if context fills mid-implementation:** look in `rust/crates/omega-agent/src/` for what's been written. The structure is:
```
src/lib.rs            — pub use Agent, AgentConfig, MockProvider hooks
src/config.rs         — max_output_tokens_for_model, OMEGA_VERSION
src/system_prompt.rs  — corePrompt port + append loader
src/error_classify.rs — is_invalid_tool_json, is_context_too_long
src/agent.rs          — Agent struct, send_message stream
tests/common/mod.rs   — MockProvider helper
tests/{6 files}.rs    — the six MockProvider tests
```

**CRITICAL ARCHITECTURE NOTE I LEARNED reading omega-core:**
- `RetryingProvider` (omega-core/src/retry.rs) ALREADY handles retry + emits `LlmRetry` events as `AgentItem::Event` items in the stream.
- The agent does NOT implement its own retry loop. It just consumes the provider stream — `LlmRetry` events flow through transparently. Tests inject a mock provider that yields `LlmRetry` then `LlmResponse` to test the retry-then-success path.
- This means Agent does NOT need `max_retry_attempts/retry_base_ms/retry_max_ms` fields (the original notes had them; ignore that part).
- `is_invalid_tool_json` detection in Rust: AnthropicProvider in omega-core surfaces malformed tool JSON as `LlmError::Stream { message: "malformed tool_use JSON: ..." }` (see anthropic.rs line ~190). Match `message.starts_with("malformed tool_use JSON")`.
- `is_context_too_long` detection: HTTP 429 with body containing `"Extra usage is required for long context requests"` (already in `LlmError::is_retryable` as a non-retryable case).

**Provider/store types reference (verified by reading source):**
- `omega_core::Provider::stream(&self, LlmRequest) -> AgentItemStream` (BoxStream<'static, Result<AgentItem, LlmError>>)
- `AgentItem::{Signal(StreamSignal), Event(Box<OmegaEvent>)}`. Use `AgentItem::event(ev)` constructor.
- `LlmRequest { model, messages: Vec<Message>, system: Option<String>, tools: Vec<ToolDefinition>, config: ModelConfig }`
- `Message { role: Role, content: Vec<ContentBlock> }`. Role::{User, Assistant}.
- `ContentBlock::{Text{text}, Thinking{thinking, signature: Option<String>}, ToolUse{id, name, input: Value}, ToolResult{tool_use_id, content, is_error}}`.
- `omega_store::ContextStore::append(role, content) -> Result<ContextHash>` (async).
- `omega_store::EventStore::append(&event) -> Result<()>` (async).
- `omega_store::random_hash()` returns `ContextHash` (newtype wrapping 12-hex String, has `.as_ref() -> &str`).
- `omega_tools::execute_tool(name, input, Option<&CancellationToken>) -> ToolResult { content: String, is_error: bool }`. NO duration_ms — agent times it itself with `Instant::now()`.
- `omega_tools::tool_definitions() -> Vec<ToolDefinition>`.
- `OmegaEvent` field naming: outer `type` snake_case; struct fields camelCase via serde.
- `LlmResponseEvent.context_hash: ContextHash` (a String). Mutate after writing assistant record.
- `TurnMetrics { input_tokens, output_tokens, cache_creation_tokens: Option<i64>, cache_read_tokens: Option<i64> }`.

**TS reference algorithm captured in detail in section "send_message algorithm" below — port verbatim minus retry (RetryingProvider owns it).**

### Files written so far
```
rust/Cargo.toml                                          (added 3 members)
rust/crates/omega-tools/Cargo.toml
rust/crates/omega-tools/src/lib.rs                       (dispatch + tests)
rust/crates/omega-tools/src/schemas.rs                   (12 tools, real schemas, tests)
rust/crates/omega-tools/src/format.rs                    (format_tool_call, real, tests)
rust/crates/omega-tools/src/tools.rs                     (mod registry, #![allow(unused_async)])
rust/crates/omega-tools/src/tools/{12 stub files}.rs
rust/crates/omega-agent/Cargo.toml                       (deps unused; trim)
rust/crates/omega-agent/src/lib.rs                       (one-line stub)
rust/crates/omega-cli/Cargo.toml                         (deps mostly unused; trim)
rust/crates/omega-cli/src/main.rs                        (clap stub)
```

### Quick fix to get gate green
Edit `rust/crates/omega-agent/Cargo.toml` to drop everything except keep the section header:
```toml
[dependencies]
# Phase 1d.0a: dependencies are introduced as agent.rs grows.
# Will be added: omega-{protocol,core,store,tools}, serde, serde_json,
# tokio (sync/time/macros/rt/fs/io-util), tokio-util, async-stream,
# futures, chrono, rand.
```
Same for `omega-cli/Cargo.toml`:
```toml
[dependencies]
clap  = { version = "4", features = ["derive"] }
# Phase 1d.0a: agent wiring deps will be re-added in 1d.0b.
```
(omega-cli main.rs only uses clap right now; tokio is only needed once we actually `tokio::main` an agent run.)

After that: `cd rust && cargo fmt && just rust-gate` should pass.

---

## PLAN (STILL VALID — agreed with user)

Three new crates as specified:
1. **omega-tools** — pure tool dispatch (DONE for 1d.0a as stubs; bodies in 1d.0b).
2. **omega-agent** — Agent struct + send_message + system_prompt + config (1d.0a Opus work, in progress).
3. **omega-cli** — clap binary (1d.0b).

User AGREED to split:
- **1d.0a (this session, Opus)**: scaffolding + omega-agent core loop with MockProvider tests + mutants on omega-agent. Tool bodies STAY STUBS.
- **1d.0b (next session, Sonnet)**: implement 12 tool bodies + integration tests + mutants on omega-tools + finalize omega-cli + Harbor adapter + manual e2e smoke.

Mutants checkpoints: `cargo mutants -p omega-agent` end of 1d.0a; `cargo mutants -p omega-tools` end of 1d.0b.

### Adjustments (still valid):
- Extended-thinking signatures: SKIP (omega-core stream doesn't surface them; effort in 1d.0a is recorded but doesn't change request).
- cache_control on last message: SKIP (1d.1).
- eager_input_streaming flag: SKIP (omega-core doesn't model it).
- system-prompt append file: IMPLEMENT (30 lines, parity-preserving).
- Compacted stop reason: log + treat as end_turn.
- Pause/resume/interject/abort: SKIP (1d.1) — but plumb CancellationToken through.

---

## WHAT TO BUILD IN omega-agent (1d.0a remainder)

### Module layout
```
src/lib.rs          — pub use Agent, AgentConfig, etc.
src/config.rs       — constants: OMEGA_VERSION, MAX_OUTPUT_TOKENS_*, COMPACTION thresholds
src/system_prompt.rs — core prompt text + append loader + builder
src/elide.rs        — request/response summarisation (for llm_call/llm_response audit fields)
src/error_classify.rs — is_invalid_tool_json, is_context_too_long
src/agent.rs        — Agent struct, init(), send_message()
tests/common/mod.rs — MockProvider helper
tests/*.rs          — six tests (see "Tests" below)
```

### Agent struct fields (1d.0a minimal)
```rust
pub struct Agent {
    provider: Box<dyn Provider + Send + Sync>,
    context_store: ContextStore,
    event_store: EventStore,
    session_dir: PathBuf,
    session_id: String,                  // 12 hex chars
    model: String,
    effort: String,                      // recorded only
    max_retry_attempts: Option<u32>,
    retry_base_ms: u64,                  // 1000
    retry_max_ms: u64,                   // 60_000
    history: Vec<Message>,
    context_hashes: Vec<ContextHash>,
    session_input_tokens: i64,
    session_output_tokens: i64,
    session_cache_creation_tokens: i64,
    session_cache_read_tokens: i64,
    system_prompt_append: Option<String>,
}
```

### Verified protocol facts
- `TurnMetrics { input_tokens: i64, output_tokens: i64, cache_creation_tokens: Option<i64>, cache_read_tokens: Option<i64> }` (camelCase).
- `LlmResponseUsage` keeps Anthropic snake_case: `input_tokens`, `output_tokens`, `cache_creation_input_tokens` (Option), `cache_read_input_tokens` (Option), `service_tier` (Option).
- `InterruptReason::{Aborted, Error}` (snake_case).
- `ServerStopOutcome::{Clean, Error}`.
- `ContentBlock::{Text{text}, Thinking{thinking, signature:Option<String>}, ToolUse{id,name,input}, ToolResult{tool_use_id,content,is_error}}`.
- `ContextHash` is a newtype around 12-hex-char String. `random_hash()` and `hash_from_str()`.
- `ContextStore::append(role, content) -> Result<ContextHash>` (async).
- `EventStore::append(&event)` (async).
- `make_session_dir(root) -> Result<SessionPaths { dir, context_file, events_file }>` (async); `SESSIONS_ROOT = ".omega/sessions"`.
- AnthropicProvider's stream emits: `Signal(Text)/Signal(Thinking)` deltas, then `Event(ToolCall)` per tool block (with empty context_hash), then ONE `Event(LlmResponse)` on message_stop (also empty context_hash), and includes `text`, `thinking`, `streaming_start`, `usage`, `stop_reason` fields.

### send_message algorithm (full pseudocode in earlier notes — kept for resume)

```
async-stream! {
  // 1. Dangling tool_use repair: if last assistant has tool_use blocks
  //    without matching tool_result, append synthetic tool_results to history
  //    and emit ToolResult(is_error=true) events.

  // 2. Append user message to context_store, emit UserMessage event.

  // 3. Outer agentic loop:
  let mut continue_loop = true;
  let mut feedback_attempts = 0u32;
  let mut tot_input=0; tot_output=0; tot_cache_creation=0; tot_cache_read=0;
  while continue_loop {
    continue_loop = false;
    // 3a. Build LlmRequest (model, messages=history, system=core+append,
    //     tools=tool_definitions(), config{max_tokens, thinking:None}).
    // 3b. Emit LlmCall event with context_hashes snapshot, request_summary.
    // 3c. Drain provider stream with retry:
    //     - LlmError retryable → emit LlmRetry, sleep with jittered backoff, loop.
    //     - LlmError invalid_tool_json → break with err, handled below.
    //     - LlmError other non-retryable → break with err.
    //     - Stream completes → collect text, thinking, tool_uses[], llm_response_proto, usage.
    // 3d. Error handling:
    //     - aborted → emit TurnInterrupted(Aborted), return.
    //     - invalid_tool_json AND feedback_attempts < 2:
    //         feedback_attempts++; append nudge user-msg ("Your tool input was malformed... fix and retry");
    //         emit UserMessage(nudge); continue_loop = true; continue.
    //     - context_too_long → emit AgentError("Context too large..."), TurnInterrupted(Error), return.
    //     - retryable_exhausted → emit AgentError("Anthropic rate limit..."), TurnInterrupted(Error), return.
    //     - other → emit AgentError("API error: ..."), TurnInterrupted(Error), return.
    // 3e. Build assistant Message blocks: thinking? + text? + tool_uses (in order).
    //     Append to context_store → assistant_hash. Push to history + context_hashes.
    // 3f. Mutate llm_response_proto.context_hash = assistant_hash; store + yield.
    //     Accumulate token totals.
    // 3g. If stop_reason == "tool_use" and tool_uses non-empty:
    //     - Emit ToolCall events with assistant_hash filled in.
    //     - Dispatch all tool_uses concurrently (FuturesUnordered).
    //     - As each completes: measure duration via Instant, emit ToolResult event.
    //     - Build tool_result Message of ContentBlock::ToolResult{tool_use_id,content,is_error}.
    //     - Append to context_store + history + context_hashes.
    //     - continue_loop = true; continue.
    // 3h. Else: emit TurnEnd with TurnMetrics; loop ends.
  }
}
```

### Tests (target ≥ 1 per branch)
1. `tests/single_text_turn.rs` — mock returns one text turn; assert events:
   UserMessage, LlmCall, LlmResponse, TurnEnd. Verify context_hash on
   LlmResponse equals assistant message hash.
2. `tests/parallel_tools.rs` — mock turn-1 returns two ToolUse blocks (stop=tool_use);
   turn-2 returns text (stop=end_turn). Assert: ToolCall x2, ToolResult x2 (in any
   order but all four events present), then second LlmResponse + TurnEnd.
3. `tests/retry_then_success.rs` — mock returns retryable error, then success.
   Assert LlmRetry event before LlmResponse.
4. `tests/non_retryable.rs` — mock returns non-retryable LlmError.
   Assert LlmError, AgentError, TurnInterrupted(Error).
5. `tests/invalid_tool_json_nudge.rs` — mock returns invalid_tool_json error
   then text-success. Assert nudge UserMessage emitted, eventually TurnEnd.
6. `tests/dangling_tool_use_repair.rs` — pre-seed Agent.history with assistant
   message containing ToolUse, then call send_message. Assert synthetic
   ToolResult(is_error=true) events emitted before user message event.

### Key TS reference points (line numbers in src/agent.ts)
- 1112–1130: send_message signature + abort plumbing.
- 1142–1190: dangling tool_use repair.
- 1192–1199: append user msg to history + emit user_message event.
- 1290–1340: build streamParams, stream call.
- 1350–1410: error handling + feedback recovery.
- 1473–1510: append assistant message, emit llm_response with context_hash.
- 1670–1716: tool_call emission + concurrent dispatch + tool_result.
- 1758–1770: append tool_results to history; continue_loop = true.

### invalid_tool_json detection (port from agent.ts errFields/policy)
TS condition: error message contains `messages.` AND `input` AND one of:
- `did not match the expected pattern`
- `invalid_request_error.*tool_input` (regex)
- `JSON parse error` (case-insensitive)
And HTTP status is 400. Map to LlmError variant inspection.

### context_too_long detection
HTTP 429 with body containing `prompt is too long` (TS isContextTooLong helper).
Or HTTP 400 with `prompt is too long`.

### System prompt
Core prompt is in `src/system-prompt/core.ts`. Append in `src/system-prompt/append.ts`
loads from `<cwd>/.omega/system-prompt-append.md` if present (return `Ok(None)` if not).
Actual content of core prompt: I haven't transcribed it yet but it's a multi-paragraph
description of Omega's role + tool guidance. Just port verbatim — read the TS file and
turn it into a Rust `const CORE_PROMPT: &str = "...";` or `include_str!`.

`buildSystemPrompt(maxOutputTokens) → String` substitutes `${MAX_OUTPUT_TOKENS}`
placeholder in core prompt with the actual number, then concatenates append (if any)
with a `\n\n---\n\n` separator.

### Config constants (port from src/config.ts)
- `max_output_tokens_for_model(model)` — sonnet=8192, opus=16384 (verify).
- `tool_result_clear_trigger`, `tool_result_clear_keep`, `tool_result_clear_at_least`
  — used by context_management edits in TS but irrelevant for 1d.0a since we're not
  passing context_management. SKIP for 1d.0a.

---

## NEXT-MODEL HANDOFF SUGGESTION

Once 1d.0a is committed: STOP this Opus session. The remaining work is:
- 1d.0b in a new Sonnet session with the same notes file. Tools are pure I/O ports;
  one tool at a time + integration test.
- Or, finish 1d.0a in this Opus session before stopping (if user wants).

When to stop within 1d.0a: I expect to need a fresh Opus session if context fills.
If that happens, mark progress in this notes file under "CURRENT STATE" and tell the
user to start a fresh Opus session pointing at this file.
