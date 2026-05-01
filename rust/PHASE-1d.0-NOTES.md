# Phase 1d.0 — Working notes

## Workspace layout (existing)
- `/home/carsten/omega/dev/rust/` — workspace root (Cargo.toml at top)
- `/home/carsten/omega/dev/rust/crates/{omega-protocol,omega-core,omega-store}/` — done
- New crates to add: `omega-tools`, `omega-agent`, `omega-cli`
- Workspace deps with edition=2024, lints: clippy::pedantic warn, unwrap/expect/panic warn
- Gate: `cd rust && cargo fmt --check && cargo clippy -- -D warnings && cargo test && cargo machete`

## omega-protocol summary (rust/crates/omega-protocol/src/events.rs)
- `OmegaEvent` (enum) variants seen: `SessionStarted`, `ServerStarted`, `ServerStopped`,
  `UserMessage`, `LlmCall`, `LlmResponse`, `ToolCall`, `ToolResult`, `TurnEnd`,
  `LlmError`, `AgentError`, `TurnInterrupted`, `Compacted`, `LlmRetry`,
  `ModelChanged`, `EffortChanged`, `TransportError`, `ResumingSession`,
  `SessionResumed`, `PauseRequested`, `TurnPaused`, `TurnContinued`.
- `LlmCallEvent` fields: `time, url, model, context_hashes, cache_breakpoint_index,
   request_bytes, request_summary?` (camelCase serde).
- `LlmResponseEvent` fields: `time, stop_reason, cleared_tool_uses?, cleared_input_tokens?,
   usage, context_hash, text?, thinking?, streaming_start?, response_summary?`.
- `ToolCallEvent` fields: `time, id, name, input, context_hash`.
- `ToolResultEvent` fields: `time, id, name, is_error, duration_ms, output`.
- `TurnEndEvent` fields: `time, metrics:TurnMetrics`.
- `LlmErrorEvent` fields: `time, url, error, http_status?`.
- `LlmRetryEvent` fields: `time, attempt, http_status?, wait_ms, error, retry_at?,
   error_body?, thinking_fragment?, text_fragment?, reason?`.
- `UserMessageEvent`: `time, content`.
- `SessionStartedEvent`: `time, session_id, path, model, effort, system_prompt`.
- `TurnInterruptedEvent`: `time, reason?` where `InterruptReason` = enum
- `ServerStoppedEvent`: `time, outcome:ServerStopOutcome, reason?`
- `ISOTimestamp` is in omega-protocol; `ContextHash` is in omega-store.
- `StreamSignal` lives in omega-protocol::stream_signal — `Text { text }`, `Thinking { text }`.

## omega-core summary
- `Provider` trait: `fn stream(&self, request: LlmRequest) -> AgentItemStream`
- `AgentItemStream = BoxStream<'static, Result<AgentItem, LlmError>>`
- `AgentItem`: `Signal(StreamSignal)` or `Event(Box<OmegaEvent>)`, `From` impls
- `LlmRequest`: `model, messages, system?, tools, config:ModelConfig`
- `ModelConfig`: `max_tokens, temperature?, thinking_budget?`
- `LlmError`: `Http{status,body,retry_after}`, `Stream{message}`, `Transport{message}`,
   `Other{message}`. Has `is_retryable`, `retry_after`, `status`, `body`.
- `Message`: `role, content:Vec<ContentBlock>`
- `ContentBlock`: `Text{text}`, `Thinking{thinking,signature?}`,
   `ToolUse{id,name,input}`, `ToolResult{tool_use_id,content,is_error}`
- `Role`: User/Assistant
- `AnthropicProvider`: builder `with_base_url`, `with_beta`, `with_client`. The provider
  ALREADY:
   - emits `LlmResponse` event (with text, thinking, usage, stop_reason) on message_stop
   - emits `ToolCall` events (with empty context_hash) when content_block_stop fires for tool_use
   - emits `Text/Thinking` StreamSignals during deltas
   - context_hash is left empty — agent must rewrite events with the assistant's hash
   - **never** emits `LlmCall`, `LlmRetry`, `LlmError` — those are agent-level.
   - Errors come back as `Err(LlmError)`.

So the agent needs to: build the request, call provider.stream, receive a stream of
AgentItems, capture text/thinking/tool_calls, then synthesize llm_call/llm_response events
WITH proper hashes. Wait — provider already emits LlmResponse... but with empty context_hash.
The agent has to either replace context_hash on the event, or stop the provider from
emitting LlmResponse (and synthesize itself). The TS structure: agent itself owns LlmResponse/LlmCall events. Looking at omega-core/anthropic.rs more carefully, it DOES emit LlmResponse, so the agent just needs to fix-up context_hash before persisting/yielding.

Plan for events:
1. Agent builds and emits `LlmCall` event (with context_hashes from history).
2. Provider stream yields:
   - `Signal(Text{text})` — pass through to consumer
   - `Signal(Thinking{text})` — pass through
   - `Event(ToolCall)` — agent will fix `context_hash` later (can't assign yet because
     the assistant message hasn't been written; need to buffer until end)
   - `Event(LlmResponse)` — same; needs context_hash set to assistant record hash
3. After stream ends, agent reconstructs the full assistant `Message` from collected
   text/thinking/tool_calls (or uses what provider gave it).

Hmm, but provider already collects text/thinking and emits LlmResponse with text/thinking.
But to emit the *Message* we need ContentBlocks, and the provider doesn't expose that
re-assembled. Wait — provider emits ToolCall events one at a time per content_block_stop.
So agent collects:
- All text deltas (from Signal::Text) → one Text block
- All thinking content (we need full text + signature; from Signal::Thinking we get the
  thinking text but NOT the signature). LlmResponse event has `thinking: Option<String>` —
  but signatures aren't in the signal stream. Look — actually thinking is multiple blocks
  potentially each with their own signature. We need a richer interface.

Wait — let's read the signature handling in provider more carefully... I saw it
accumulates SignatureDelta into BlockAccum::Thinking { signature }. But it never emits
a Signal::ThinkingSignature, and it never emits the assembled thinking blocks to the
agent. The agent only gets the concatenated `all_thinking` string in LlmResponse.text/thinking.

This is a problem: to send a continuation request, we need the assistant's thinking
blocks WITH their signatures back. The provider only gives us concatenated text.

**Workaround:** for Phase 1d.0 we can either:
A. Extend the omega-core API to emit assembled assistant blocks alongside LlmResponse
   (clean but invasive — touches omega-core).
B. Don't preserve thinking blocks across turns (fine for headless run with thinking_budget=None).
C. Skip extended thinking entirely — set `thinking_budget: None`.

For Phase 1d.0 the spec says "no thinking" stuff isn't called out as omitted, but
simplest is: thinking_budget=None for the CLI initially. The TS code uses thinking via
"effort" on the API. The Anthropic Messages API takes `thinking: {type:enabled, budget_tokens:N}`.
If we don't pass thinking config, the model won't return thinking blocks, so we don't need
signatures. Effort levels in TS map to `output_config.effort` — but in the Rust core,
the only thinking knob is `thinking_budget`.

Actually the TS sets `thinking: {type:"enabled", budget_tokens: ...}` based on effort. Let
me grep — no, I'll re-read. Actually let me check — TS sets `output_config.effort`, not
thinking_budget.

For now, Phase 1d.0 plan: pass `effort` as a string-typed agent setting that maps to NO
thinking_budget in LlmRequest (the simplest path). Effort string is accepted but not used
to configure thinking. Mark as TODO for 1d.1. The CLI accepts --effort but it's currently
informational only.

**Better approach:** I'll punt extended thinking entirely in 1d.0. ModelConfig.thinking_budget
remains None. The Agent does NOT preserve thinking content across turns. Tests work.
session_started event records effort string, but model never thinks.

This IS a simplification vs TS, but the spec doc explicitly says: "What to omit from Phase 1d.0:
... setModel/setEffort - Phase 1d.1". So full effort handling is 1d.1.

Yet the LlmRequest already has thinking_budget — keep at None until 1d.1. effort is
recorded in session_started for compatibility.

## src/tools.ts — facts I need to preserve in Rust
- 13 tools: read_file, write_file, edit_file, run_command, list_files, web_search,
  fetch_url, grep_files, find_files, run_background, wait_for_output, write_stdin
- `eager_input_streaming: true` is passed for `write_file` and `edit_file` in
  toolDefinitions (Anthropic-specific). For Phase 1d.0 the omega-core ToolDefinition
  doesn't have this field — we omit it; behavior is the same, just slightly less
  responsive streaming. ACCEPTABLE.
- Output cap: 100_000 chars universally.
- run_command timeout default 120s, output cap 100KB per stream, killed via SIGKILL on PG.
- list_files MAX_ENTRIES=1000, skip node_modules, skip .* at top-level when not recursive.
- read_file: 2000 lines or 50KB cap; offset 1-indexed.
- edit_file: must match exactly once; multiple replacements applied sequentially.
- web_search: BRAVE_SEARCH_API_KEY required; 10 results; 8000-char cap.
- fetch_url: SHA-256(href) cache key in `tmpdir/omega-webcache-<pid>/`; HTML→text.
   Postprocess via `bash -c "$cmd < $cachefile"`. 8000-char cap on postprocess output.
- grep_files: try rg, else grep; default ctx=2; case_sensitive default false; max_results=200.
- find_files: try fd, else find; max_results=200.
- run_background: bash -c, detached, stdout+stderr → tmp logfile, returns {pid, logFile}.
   pid → ChildProcess tracked in module-level Map.
- wait_for_output: poll every 200ms, stop on pattern (regex)/minBytes/exit/timeout.
   If pattern is invalid regex, escape special chars and use literal.
- write_stdin: lookup pid; write text; optionally close stdin.

## src/cli.ts — CLI surface
```
omega-cli run --instruction <text> --model <id> [--effort low|medium|high|max|xhigh]
              [--session-dir <path>] [--max-turns <N>] [--help]
```
- Reads instruction from --instruction OR stdin (if not TTY).
- Session dir: explicit or `.omega/sessions/<timestamp>/`.
- Streams text deltas to stdout, structured logs to stderr.
- After turn_end / interrupted: emit ServerStopped("clean"|"error"), flush, exit 0/1.
- Counts llm_response events, aborts if reaches --max-turns.

## src/system-prompt
- `core.ts`: ~150 lines of literal prose template. Substitutes {cwd, maxOutputTokens}.
- `append.ts`: reads `.omega/system-prompt-append.md`, returns null on ENOENT.
- `index.ts`: concatenates `corePrompt(...) + "\n\n" + appendContent` if non-null.

## src/config.ts — values to constify
- `model: "claude-sonnet-4-6"` default
- `MODEL_MAX_OUTPUT_TOKENS`: sonnet-4-6→64k, opus-4-6→128k, opus-4-7→128k. Fallback 64k.
- `defaultEffort: "medium"`.
- `retryBaseMs: 1000, retryMaxMs: 60000`.
- `autoCompactThreshold: 750_000`, `toolResultClearTrigger: 100_000`,
   `toolResultClearKeep: 10`, `toolResultClearAtLeast: 15_000`. (Not used in 1d.0.)
- `COMPACTION_INSTRUCTIONS` — not used in 1d.0.
- `defaultPort: 3000` — not used in 1d.0.

## Approach, finalized

### Crate structure
Three new crates as the doc says:
1. **omega-tools** — pure tool dispatch. No agent dependency. Uses omega-protocol only
   for the ToolDefinition shape (actually it'll define its own ToolResult; tool defs
   build into omega_core::ToolDefinition).
2. **omega-agent** — Agent struct + send_message + system prompt + config consts.
3. **omega-cli** — clap binary.

### Scope cuts I'm making vs the doc
- **Extended thinking signatures**: `thinking_budget: None` for now; `effort` recorded
  in session_started but not wired into thinking config. This is a deliberate scope cut
  vs a hypothetical "feature parity" goal, but the doc says setEffort is Phase 1d.1, so
  we're just not pretending to implement what we omit.
- **No Compacted handling**: doc says omit; if API ever returns Compacted stop_reason,
  log + treat as turn_end.
- **No pause/resume/interject**.
- **System prompt append file**: implemented (it's small; no reason to skip).
- **Cache control on last message**: implemented (small; affects perf significantly).

### Sub-task order
1. Scaffold omega-tools (Cargo.toml + lib.rs skeleton + per-tool modules).
2. Implement tool schemas (raw JSON Schema constants) and `tool_definitions()`.
3. Implement each tool one by one with tests:
   read_file, write_file, edit_file, list_files, run_command, grep_files, find_files,
   run_background, wait_for_output, write_stdin, web_search, fetch_url.
4. Wire `execute_tool(name, input, signal)` dispatch.
5. Scaffold omega-agent (Cargo.toml + lib.rs + system_prompt + config + agent).
6. Port system prompt (literal text, two functions).
7. Port elision helpers + retry helpers.
8. Implement `Agent::send_message` as an async-stream returning `AgentItem`s.
9. Tests with a `MockProvider` (in `tests/common/`).
10. Scaffold omega-cli with clap and test the run subcommand.
11. Run `just rust-gate`.

### Public API design summary
- `omega_tools::ToolResult { content: String, is_error: bool }` (no duration_ms — agent times)
- `omega_tools::execute_tool(name: &str, input: Value, cancel: Option<&CancellationToken>)
   -> ToolResult`
- `omega_tools::tool_definitions() -> Vec<omega_core::ToolDefinition>`
- `omega_tools::format_tool_call(name: &str, input: &Value) -> String`
- `omega_agent::Agent::new(AgentConfig) -> Self`
- `omega_agent::Agent::init() -> Result<()>`  (emits server_started + session_started)
- `omega_agent::Agent::send_message(content, cancel) -> impl Stream<Item=AgentItem>`
- `omega_agent::Agent::emit_server_stopped(outcome) -> Result<()>`
- `omega_agent::config::DEFAULT_MODEL`, `max_output_tokens_for_model(&str) -> u32`,
   `RETRY_BASE_MS`, `RETRY_MAX_MS`.
- `omega_agent::system_prompt::build_system_prompt(cwd, max_output_tokens, append_content) -> String`
- `omega_agent::system_prompt::read_system_prompt_append(cwd) -> Option<String>`

### Background process state
Module-level `OnceLock<tokio::sync::Mutex<HashMap<u32, BgProcess>>>`. BgProcess holds
`tokio::process::Child` for stdin and the log path.

### Web cache dir
Module-level `OnceLock<PathBuf>` initialized as `std::env::temp_dir().join(format!("omega-webcache-{}", std::process::id()))`.

### Cancellation
Use `tokio_util::sync::CancellationToken` instead of bare AbortSignal. Already used in
omega-core.

### Test approach
- omega-tools: real tempdir, real subprocess. e.g. `assert_cmd` for run_command
  and `tempfile` for sandbox. Skip web tests if env vars missing.
- omega-agent: in-process MockProvider in tests/common/mock_provider.rs. Returns scripted
  AgentItem streams.
- omega-cli: integration test using `assert_cmd` and a MOCK_API_BASE env var? Actually
  no — for CLI, do a smoke test of --help only (no live API).

### Risks
- The agent loop is complex; getting the event ordering right (LlmCall before stream,
  LlmResponse fix-up after, ToolCall fix-up, ToolResult per call, persistence
  interleaved) needs care.
- The `addCacheControlToLastMessage` logic mutates the messages array — but our
  ContentBlock doesn't have cache_control. Adding it requires extending omega-core
  ContentBlock. **Decision:** SKIP cache control breakpoints in 1d.0. Add in 1d.1.
  This is a perf hit for Opus but functionally correct.
- thinking blocks across turns: skipping (see above).
- invalid tool JSON nudge: implement, mirroring TS.
- dangling tool-use repair: implement.

## Final agreement on mechanics

The agent's main loop, per turn:
```
emit user_message + persist + append history (User msg) + write to ContextStore
loop:
  build LlmRequest from history (with system prompt + tools + max_tokens for model)
  emit LlmCall event with context_hashes
  for each AgentItem from provider.stream(req):
    if Signal(text/thinking): yield through to consumer
    if Event(ToolCall): collect (set context_hash later)
    if Event(LlmResponse): collect for this iteration
  on stream end:
    write assistant Message to ContextStore -> get assistant_hash
    re-emit LlmResponse with context_hash=assistant_hash
    re-emit each ToolCall with context_hash=assistant_hash
    push assistant Message to history
    if no tool_calls -> emit turn_end + break
    else:
      for each ToolCall in parallel: execute_tool, build ToolResult, emit ToolResult
      build user Message of ContentBlock::ToolResult, write to ContextStore, push to history
      continue loop
on retryable LlmError: emit LlmRetry, sleep w/ backoff, repeat
on non-retryable: emit LlmError + turn_interrupted, break
on cancel: emit turn_interrupted(reason=user_abort), break
```

Aggregated TurnMetrics (input/output/cache_creation/cache_read tokens) accumulate
across the turn; emitted in turn_end.

OK — I have a clear plan. Now to scaffold and implement.
