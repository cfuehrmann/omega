//! [`OmegaEvent`] — the unified discriminated union for Omega.
//!
//! Every `OmegaEvent` is both written to
//! `.omega/sessions/<timestamp>/events.jsonl` and streamed to UI consumers
//! over WebSocket.
//!
//! # JSON representation
//!
//! The outer discriminator field is `"type"` with `snake_case` values
//! (e.g. `"session_started"`).  Most struct fields use camelCase to match
//! existing `events.jsonl` files.  The nested `usage` object inside
//! `LlmResponseEvent` keeps the Anthropic API's original `snake_case` field
//! names (`input_tokens`, `output_tokens`, etc.).
//!
//! # Naming authority
//!
//! Persisted names win.  The `events.jsonl` file is the single source of
//! truth; stream-facing names conform to it.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::feature_flags::FeatureFlags;
use crate::ids::SessionId;
use crate::{ContextHash, ISOTimestamp};

// ---------------------------------------------------------------------------
// Sub-type enums used by specific variants
// ---------------------------------------------------------------------------

/// `ServerStoppedEvent.outcome` discriminator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServerStopOutcome {
    Clean,
    Error,
}

/// `TurnInterruptedEvent.reason` discriminator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterruptReason {
    Aborted,
    Error,
}

/// `LlmRetryEvent.reason` discriminator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LlmRetryReason {
    /// The provider sent a `retry-after` response header.
    #[serde(rename = "retry-after")]
    RetryAfter,
}

// ---------------------------------------------------------------------------
// Shared sub-structs
// ---------------------------------------------------------------------------

/// Per-turn aggregate token and cache metrics.  Used by [`TurnEndEvent`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnMetrics {
    pub input_tokens: i64,
    pub output_tokens: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<i64>,
}

/// Token usage from an LLM response envelope.  Field names intentionally
/// kept in `snake_case` to match the Anthropic API's wire format.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmResponseUsage {
    pub input_tokens: i64,
    pub output_tokens: i64,
    /// Tokens written to the prompt cache this call (billed at 1.25× base).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<i64>,
    /// Tokens served from the prompt cache this call (billed at 0.1× base).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<i64>,
    /// Service tier used; absent or `"standard"` is the baseline.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    /// Per-iteration breakdown when server-side compaction fires.
    /// Anthropic's response usage object exposes this as `iterations`,
    /// each entry tagged with `type` (e.g. `"compaction"`, `"message"`).
    /// Absent on responses that did not hit compaction; baseline
    /// non-compaction responses serialise without this key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iterations: Option<Vec<UsageIteration>>,
}

/// One entry in [`LlmResponseUsage::iterations`].  Mirrors Anthropic's
/// per-iteration usage shape verbatim.  The wire `type` discriminator
/// (e.g. `"compaction"`, `"message"`) is held in [`Self::iteration_type`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageIteration {
    /// Iteration kind, e.g. `"compaction"` or `"message"`. Stored under
    /// the wire field name `type`.
    #[serde(rename = "type")]
    pub iteration_type: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
}

// ---------------------------------------------------------------------------
// Per-variant event structs
// ---------------------------------------------------------------------------

/// Fallback for the `omega_commit` serde default: sessions written before
/// this field was added deserialise with `"unknown"` instead of being dropped.
fn default_omega_commit() -> String {
    "unknown".to_owned()
}

/// Default value for the `agent_time_zone` field on `SessionStartedEvent` when
/// deserialising a session recorded before the field existed.  `"UTC"` is a
/// valid IANA zone name; the UI's `Intl.DateTimeFormat` consumes it directly
/// and renders times with a UTC offset, matching the pre-migration display
/// behaviour without any conditional path.
fn default_agent_tz() -> String {
    "UTC".to_owned()
}

/// Default value for the `origin` field on [`SessionStartedEvent`] when
/// deserialising a session recorded before Phase 1.
///
/// Every pre-Phase-1 session is by construction a root session, because
/// subagents did not exist yet.  This is a deliberate semantic default,
/// not a defensive accommodation to suppress a parse error.
fn default_origin() -> crate::ids::Origin {
    crate::ids::Origin::Root
}

/// The session started (first event in every session).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStartedEvent {
    pub time: ISOTimestamp,
    pub session_id: SessionId,
    /// Session directory path relative to the Omega root (cwd).
    pub path: String,
    pub model: String,
    pub effort: String,
    /// The full system prompt text at session start.
    pub system_prompt: String,
    /// Short git commit hash of the Omega source at the time the binary was built.
    /// `"unknown"` when git was unavailable at build time.
    /// Defaulted on deserialise so that events written before this field was
    /// added still parse successfully (backward compat).
    #[serde(default = "default_omega_commit")]
    pub omega_commit: String,
    /// IANA time-zone name of the agent host at the moment the session was
    /// started (e.g. `"Europe/Berlin"`, `"America/New_York"`, `"UTC"`).
    /// Captured via `iana_time_zone::get_timezone()`; falls back to `"UTC"`
    /// when the host TZ cannot be determined.  The UI uses this string with
    /// `Intl.DateTimeFormat` to render every event's `time` (UTC) in the
    /// agent host's local wall-clock time.
    ///
    /// Defaulted on deserialise so that sessions written before this field
    /// was added still parse successfully — they render in UTC, which is
    /// what the UI did pre-migration anyway.
    #[serde(default = "default_agent_tz")]
    pub agent_time_zone: String,
    /// How this session came to exist.
    ///
    /// Defaults to [`Origin::Root`](crate::ids::Origin::Root) when absent —
    /// every pre-Phase-1 session is by construction a root session, because
    /// subagents did not exist yet.  Not a defensive accommodation.
    #[serde(default = "default_origin")]
    pub origin: crate::ids::Origin,

    /// Runtime feature flags active for this session.
    ///
    /// Defaults to [`FeatureFlags::default`] (`subagents = false`) when absent
    /// on deserialisation.  This is a deliberate semantic default, not a
    /// defensive accommodation: every session recorded before feature flags
    /// were introduced had subagents off by construction — that code path did
    /// not exist yet.
    #[serde(default)]
    pub features: FeatureFlags,

    /// Names of the tools exposed to the model in this session, in the
    /// canonical order they appear in `tool_definitions`.
    ///
    /// **Required field** — no `#[serde(default)]`.  Old `events.jsonl` files
    /// that lack it will fail to deserialise loudly; that is intended (per
    /// the schema-loud rule in AGENTS.md).  The toolset is part of the
    /// session's identity; silently inferring it from older flags would mask
    /// exactly the kind of semantic drift we want to see.
    pub tool_selection: Vec<String>,
}

/// The server process started.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerStartedEvent {
    pub time: ISOTimestamp,
}

/// The server process stopped cleanly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerStoppedEvent {
    pub time: ISOTimestamp,
    pub outcome: ServerStopOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// A user message submitted to the agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserMessageEvent {
    pub time: ISOTimestamp,
    pub content: String,
}

/// An outgoing API call to an LLM.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmCallEvent {
    pub time: ISOTimestamp,
    pub url: String,
    pub model: String,
    /// Ordered hashes of every context record in the sent context.
    pub context_hashes: Vec<ContextHash>,
    /// Index (0-based) of the message that received the cache breakpoint.
    /// Always serialized (as `null` when absent) — not `skip_serializing_if`.
    pub cache_breakpoint_index: Option<i64>,
    /// Serialized byte size of the full request payload.
    pub request_bytes: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_summary: Option<Value>,
}

// ---------------------------------------------------------------------------
// SCHEMA-8 — event structs (Phase 1b+6.5)
//
// The legacy `LlmResponseEvent` / `CompactedEvent` / `text_fragment` /
// `thinking_fragment` shapes were removed in Phase 6.5 after every
// consumer migrated to the new block-grammar events.
// ---------------------------------------------------------------------------

/// Opener emitted on the first signal of any kind from a freshly-started
/// provider stream within a turn iteration. Pairs with either
/// [`LlmResponseEndedEvent`] (success) or [`LlmResponseDiscardedEvent`]
/// (abandonment).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmResponseStartedEvent {
    pub time: ISOTimestamp,
}

/// Successful close of a provider stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmResponseEndedEvent {
    pub time: ISOTimestamp,
    pub stop_reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cleared_tool_uses: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cleared_input_tokens: Option<i64>,
    pub usage: LlmResponseUsage,
    /// FK into `context.jsonl` for the assistant record written for this response.
    pub context_hash: ContextHash,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_summary: Option<Value>,
}

/// Pure marker.  Closer for [`LlmResponseStartedEvent`] when the
/// response is abandoned mid-stream.  Always immediately precedes
/// `LlmRetry`, `LlmError`, or `TurnInterrupted`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmResponseDiscardedEvent {
    pub time: ISOTimestamp,
}

/// One text content block from a streamed assistant response.  Emitted
/// at the provider's `content_block_stop` for a `text` block.  `partial`
/// is `true` only when the block was cut off by abandonment
/// ([`LlmResponseDiscardedEvent`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextBlockEvent {
    pub time: ISOTimestamp,
    pub text: String,
    pub partial: bool,
}

/// One thinking content block from a streamed assistant response.
///
/// Invariant: `signature.is_none() iff partial == true`.  Both fields
/// are kept on the wire even though they are redundant — the explicit
/// `partial` flag matches sibling block events and avoids a special-case
/// at every consumer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThinkingBlockEvent {
    pub time: ISOTimestamp,
    pub thinking: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    pub partial: bool,
}

/// One `tool_use` content block from a streamed assistant response.
/// Emitted at the provider's `content_block_stop`.  When `partial:
/// true`, `input` may be malformed JSON; the agent does not dispatch
/// partial blocks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolUseBlockEvent {
    pub time: ISOTimestamp,
    /// Omega-issued, provider-agnostic identifier for this tool invocation.
    /// Minted by the agent on `StreamSignal::ToolUseBlockStart` and shared
    /// across the corresponding `ToolCallEvent` and `ToolResultEvent` for
    /// correlation across the three events.
    pub tool_call_id: String,
    /// LLM-issued identifier from the `tool_use` content block.  Faithfully
    /// recorded here as the LLM transcript field; the protocol layer
    /// (`ContentBlock::ToolResult.tool_use_id`) carries the same value as the
    /// FK back to this tool use.
    pub tool_use_id: String,
    pub name: String,
    pub input: Value,
    pub partial: bool,
}

// ---------------------------------------------------------------------------
// End of SCHEMA-8 additive structs.
// ---------------------------------------------------------------------------

/// A tool invocation by the agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallEvent {
    pub time: ISOTimestamp,
    /// Omega-issued, provider-agnostic identifier for this tool invocation.
    /// Shared with the originating `ToolUseBlockEvent` and the resulting
    /// `ToolResultEvent`; also used as the stem of the tee-log filename
    /// (`cache/<tool>/<tool_call_id>-<tag>.log`) so `events.jsonl` and
    /// the cache directory are bidirectionally cross-referenceable.
    pub tool_call_id: String,
    pub name: String,
    /// Tool input parameters (arbitrary JSON from the LLM).
    pub input: Value,
    /// Hash of the assistant context.jsonl record containing this `tool_use` block.
    pub context_hash: ContextHash,
}

/// The result of a tool invocation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultEvent {
    pub time: ISOTimestamp,
    /// Omega-issued identifier of the tool call this result is for.  Matches
    /// the `tool_call_id` of the originating `ToolCallEvent` (and the
    /// `ToolUseBlockEvent` before it).
    pub tool_call_id: String,
    pub name: String,
    pub is_error: bool,
    pub duration_ms: i64,
    /// Full text output of the tool.
    pub output: String,
}

/// Omega automatically bootstrapped `python3` via `apt-get` because the binary
/// was absent from `$PATH` when `python_repl` was first called.
///
/// Emitted at most once per Omega process (the first successful `python_repl`
/// invocation that triggered the bootstrap).  The event tells forensics
/// "Omega had to install python3 here" without requiring the reader to scan
/// the `ToolResult` output for apt-get log lines.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PythonReplBootstrappedEvent {
    pub time: ISOTimestamp,
    /// Elapsed time of the full bootstrap (both `apt-get update` and
    /// `apt-get install`) in milliseconds.
    pub duration_ms: i64,
    /// Whether the bootstrap succeeded (python3 is now available).
    pub success: bool,
    /// First 500 characters of combined apt-get stderr output.
    pub stderr_excerpt: String,
}

/// Server-side context compaction fired during this turn.
///
/// Emitted immediately before the accompanying [`LlmResponseEndedEvent`].
/// Anthropic's `compact_20260112` edit condensed the LLM-visible context
/// into a summary; any fold that derives the LLM-visible context must
/// reset history at this event and start fresh from the baseline in the
/// adjacent `LlmResponseEnded.context_hash`.
///
/// # Token semantics
/// - `tokens_before` — input tokens of the old context fed to the
///   compaction summariser (the "before" figure).
/// - `tokens_after`  — input tokens of the new, compacted context
///   (the "after" figure; baseline cost of subsequent turns).
/// - `summary_tokens` — output tokens produced by the summariser (the
///   size of the generated summary block).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextCompactedEvent {
    pub time: ISOTimestamp,
    /// Input tokens consumed by the compaction summariser ("before").
    pub tokens_before: i64,
    /// Input tokens of the new compacted baseline ("after").
    pub tokens_after: i64,
    /// Output tokens produced by the compaction summariser.
    pub summary_tokens: i64,
}

/// End of a user turn — aggregate metrics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnEndEvent {
    pub time: ISOTimestamp,
    pub metrics: TurnMetrics,
}

/// A non-retryable LLM provider call error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmErrorEvent {
    pub time: ISOTimestamp,
    pub url: String,
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
}

/// A generic agent-level error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentErrorEvent {
    pub time: ISOTimestamp,
    pub error: String,
}

/// The user interrupted an in-flight turn, or the turn ended due to an error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnInterruptedEvent {
    pub time: ISOTimestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<InterruptReason>,
}

/// LLM provider call retried after a transient error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmRetryEvent {
    pub time: ISOTimestamp,
    /// Retry attempt number, 1-based.
    pub attempt: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    /// Milliseconds to wait before the next attempt.
    pub wait_ms: i64,
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_at: Option<ISOTimestamp>,
    /// Full structured error body from the provider, kept verbatim.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_body: Option<Value>,
    /// Why the retry fired.  Absent for ordinary policy-driven retries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<LlmRetryReason>,
}

/// The operator switched the active model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelChangedEvent {
    pub time: ISOTimestamp,
    pub model: String,
}

/// The operator changed the thinking effort level.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffortChangedEvent {
    pub time: ISOTimestamp,
    pub effort: String,
}

/// A transport-layer error emitted by the web server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransportErrorEvent {
    pub time: ISOTimestamp,
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

/// Session resumption has started — basis extracted, LLM call about to fire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumingSessionEvent {
    pub time: ISOTimestamp,
    pub resumed_from: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub basis: String,
}

/// The session was seeded with a summary of a previous session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionResumedEvent {
    pub time: ISOTimestamp,
    pub resumed_from: String,
    pub summary: String,
}

/// The user has requested a halt (§15 Unified Input Model, U3).
///
/// Halt = "stop advancing at the next seam and WAIT" so the user can
/// compose a steering message while the agent is parked. This replaces the
/// retired pause-for-injection `PauseRequested` event; the semantics are
/// distinct (Halt parks at the seam rather than offering a content-carrying
/// continue), so the syntax changed too (Contract Authority).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HaltRequestedEvent {
    pub time: ISOTimestamp,
}

/// The agent reached a clean seam after a halt request and parked there,
/// waiting for the user to resume (with a queued steering message or an
/// explicit `Resume`). Replaces the retired `TurnPaused` event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnHaltedEvent {
    pub time: ISOTimestamp,
}

/// The halted turn is resuming and the run loop continues the block.
///
/// Carries no `mode`: the old `ContinueMode` (Manual/Auto) distinguished a
/// pre-commit race in the retired pause-for-injection machinery that no
/// longer exists. Resume is just resume — either a queued steering message
/// woke the park, or the user clicked Resume with no new input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnResumedEvent {
    pub time: ISOTimestamp,
}

// ---------------------------------------------------------------------------
// Harness-recovery event types (§15 — forensics gap close)
// ---------------------------------------------------------------------------

/// Discriminates the two harness-authored mid-loop recovery prompts.
///
/// Both are injected as `role: user` context records so the model sees
/// them as prompts; this enum tells forensics tools *why* the injection
/// happened, closing the gap described in §15 of `monitors-design.html`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessRecoveryKind {
    /// Injected when the model returns a response with zero content blocks.
    /// Follows Anthropic's documented handling for the `end_turn` stop reason;
    /// see <https://platform.claude.com/docs/en/build-with-claude/handling-stop-reasons>.
    EmptyResponseContinuation,
    /// Injected when the SSE parser surfaces a `malformed tool_use JSON`
    /// error.  The harness asks the model to retry with correct escaping.
    InvalidToolJson,
}

/// A harness-authored recovery prompt injected as `role: user`.
///
/// These are synchronous mid-loop self-repairs — NOT async user input.
/// The `role: user` context record is a projection; this event is the
/// authoritative backing entry in `events.jsonl`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessRecoveryEvent {
    pub time: ISOTimestamp,
    /// Which recovery path fired.
    pub kind: HarnessRecoveryKind,
    /// The verbatim text injected into the context (the prompt the model sees).
    pub content: String,
}

// ---------------------------------------------------------------------------
// Phase 0 — Async Monitors event types
// ---------------------------------------------------------------------------

/// Reason a monitor process stopped.
///
/// Used by [`MonitorStoppedEvent`].  Unexpected reasons
/// (`StoppedByUser`, `ProcessExited`, `ProcessCrashed`) are projected
/// into the LLM context so the agent learns and does not wait forever.
/// `StoppedByAgent` is NOT projected — the agent issued the stop
/// itself, so a notice would be redundant noise.
/// `StoppedBySessionEnd` is also NOT projected — the session is
/// terminating, so there is no agent loop left to notify.
///
/// §12 locked decision:  `StoppedByAgent` → not projected;  all other
/// variants → projected, EXCEPT `StoppedBySessionEnd` which is also not
/// projected (session teardown, no running agent loop).
///
/// Loud-schema-change note: variants are renamed (not aliased) so that
/// old log files that still carry `agent_stopped` / `user_killed` /
/// `crashed` fail loudly on deserialization rather than silently mapping
/// to the wrong variant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MonitorStopReason {
    /// The agent stopped the monitor explicitly via `stop_monitor`.
    /// Wire value: `stopped_by_agent`.
    StoppedByAgent,
    /// The user killed the monitor via the UI.
    /// Wire value: `stopped_by_user`.
    StoppedByUser,
    /// The monitor process exited naturally (ran to completion).
    /// Wire value: `process_exited`.
    ProcessExited,
    /// The monitor process crashed unexpectedly (signal or wait error).
    /// Wire value: `process_crashed`.
    ProcessCrashed,
    /// The session ended with the monitor still running; the shutdown
    /// path reaped the process tree and wrote this event so the log
    /// has a matching `MonitorStopped` for every `MonitorStarted`.
    /// Wire value: `stopped_by_session_end`.
    StoppedBySessionEnd,
}

impl MonitorStopReason {
    /// Returns `true` for stop reasons that should be projected into the
    /// LLM context.
    ///
    /// `StoppedByAgent` returns `false` — the agent already knows
    /// because it issued the stop.  `StoppedBySessionEnd` also returns
    /// `false` — the session is terminating and there is no agent loop
    /// left to notify.  Every other reason returns `true` so the agent
    /// learns about unexpected terminations and does not wait forever
    /// on a dead monitor.
    #[must_use]
    pub fn should_project(&self) -> bool {
        !matches!(self, Self::StoppedByAgent | Self::StoppedBySessionEnd)
    }
}

/// One monitor source contributing to a [`MonitorDeliveryEvent`] batch.
///
/// Each item carries the id of the monitor that produced the lines plus
/// the stdout lines themselves.  Human-typed messages are NOT included
/// here — they stay as `UserMessage` events and are merged by the
/// projection (§12 LEAN: separate canonical events, merged at projection).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorDeliveryItem {
    pub monitor_id: String,
    pub lines: Vec<String>,
}

/// A monitor was registered.  Causality / forensics only.
///
/// Per §12 locked decision: `MonitorStarted` does NOT re-surface into
/// the LLM context — the `monitor()` tool result already informs the
/// agent; this event exists for log attribution and forensics only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorStartedEvent {
    pub id: String,
    pub description: String,
    pub command: String,
    pub time: ISOTimestamp,
}

/// Batched monitor stdout delivered at a turn boundary.  Projects to
/// `role: user` in the LLM context.
///
/// Carries only MONITOR-sourced items (§12 LEAN).  Human-typed messages
/// stay as `UserMessage` events; the context projection merges
/// consecutive `role: user` messages into one API message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorDeliveryEvent {
    pub time: ISOTimestamp,
    pub items: Vec<MonitorDeliveryItem>,
}

/// A chunk of stderr from a monitor process.
///
/// **DIAGNOSTIC** — present in `events.jsonl`, **never** projected into
/// `context.jsonl` or the in-memory history.  Zero token cost; does not
/// wake the agent.  (§6 decision 3.)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorStderrEvent {
    pub id: String,
    pub chunk: String,
    pub time: ISOTimestamp,
}

/// A monitor process stopped.
///
/// `reason` determines projection: `StoppedByAgent` and
/// `StoppedBySessionEnd` are NOT projected; `StoppedByUser`,
/// `ProcessExited`, and `ProcessCrashed` ARE projected into the LLM
/// context so the agent learns and does not block waiting on a dead
/// monitor.  See [`MonitorStopReason::should_project`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorStoppedEvent {
    pub id: String,
    pub reason: MonitorStopReason,
    pub exit_code: Option<i32>,
    pub time: ISOTimestamp,
}

// ---------------------------------------------------------------------------
// OmegaEvent — the unified discriminated union
// ---------------------------------------------------------------------------

/// The single unified event type for Omega.
///
/// Every `OmegaEvent` is both streamed to UI consumers and written to
/// `.omega/sessions/<timestamp>/events.jsonl`.
///
/// The `"type"` JSON field is the discriminator; values are `snake_case`
/// (e.g. `"session_started"`, `"tool_call"`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OmegaEvent {
    SessionStarted(SessionStartedEvent),
    ServerStarted(ServerStartedEvent),
    ServerStopped(ServerStoppedEvent),
    UserMessage(UserMessageEvent),
    LlmCall(LlmCallEvent),
    ToolCall(ToolCallEvent),
    ToolResult(ToolResultEvent),
    TurnEnd(TurnEndEvent),
    LlmError(LlmErrorEvent),
    AgentError(AgentErrorEvent),
    TurnInterrupted(TurnInterruptedEvent),
    LlmRetry(LlmRetryEvent),
    ModelChanged(ModelChangedEvent),
    EffortChanged(EffortChangedEvent),
    TransportError(TransportErrorEvent),
    ResumingSession(ResumingSessionEvent),
    SessionResumed(SessionResumedEvent),
    HaltRequested(HaltRequestedEvent),
    TurnHalted(TurnHaltedEvent),
    TurnResumed(TurnResumedEvent),

    // --- SCHEMA-8 block-grammar variants (Phase 1b) -------------------------
    LlmResponseStarted(LlmResponseStartedEvent),
    LlmResponseEnded(LlmResponseEndedEvent),
    LlmResponseDiscarded(LlmResponseDiscardedEvent),
    TextBlock(TextBlockEvent),
    ThinkingBlock(ThinkingBlockEvent),
    ToolUseBlock(ToolUseBlockEvent),

    // --- Phase 2.0 — server-side compaction event (F11 gap close) ----------
    /// Server-side context compaction fired; history was reset.
    /// Always immediately precedes the corresponding `LlmResponseEnded`.
    ContextCompacted(ContextCompactedEvent),

    // --- python_repl bootstrap event ---------------------------------------
    /// Omega auto-installed python3 via apt-get because it was absent.
    /// Emitted at most once per process, on the first `python_repl` call that
    /// triggered a successful bootstrap.
    PythonReplBootstrapped(PythonReplBootstrappedEvent),

    // --- Harness-recovery events (§15 — forensics gap close) ---------------
    /// A harness-authored recovery prompt was injected as `role: user`.
    ///
    /// Two repair paths produce this event:
    /// - `EmptyResponseContinuation` — model returned zero content blocks.
    /// - `InvalidToolJson` — SSE parser surfaced a malformed-JSON error.
    ///
    /// The `role: user` context record is a projection; this event is the
    /// backing entry in `events.jsonl`.
    HarnessRecovery(HarnessRecoveryEvent),

    // --- Phase 0 — Async Monitors schema ----------------------------------
    /// A monitor was registered.  Log / causality only; NOT projected into
    /// LLM context (the tool result already informs the agent).
    MonitorStarted(MonitorStartedEvent),

    /// Batched monitor stdout delivered at a turn boundary.  Projects to
    /// `role: user` in the LLM context.  Consecutive user-role messages
    /// are merged into one API message by the context projection.
    MonitorDelivery(MonitorDeliveryEvent),

    /// A chunk of stderr from a monitor process.  **DIAGNOSTIC only** —
    /// present in `events.jsonl`, never in `context.jsonl` or history.
    MonitorStderr(MonitorStderrEvent),

    /// A monitor process stopped.  Unexpected reasons (`StoppedByUser`,
    /// `ProcessExited`, `ProcessCrashed`) are projected into LLM context;
    /// `StoppedByAgent` and `StoppedBySessionEnd` are not.
    MonitorStopped(MonitorStoppedEvent),
}

impl OmegaEvent {
    /// Returns the timestamp of this event.
    ///
    /// Every event variant carries a `time: ISOTimestamp` field; this
    /// method provides uniform access without a `match` at every call
    /// site.  The compiler enforces exhaustiveness, so adding a new
    /// variant without a `time` field will fail here.
    #[must_use]
    pub fn time(&self) -> &ISOTimestamp {
        match self {
            Self::SessionStarted(e) => &e.time,
            Self::ServerStarted(e) => &e.time,
            Self::ServerStopped(e) => &e.time,
            Self::UserMessage(e) => &e.time,
            Self::LlmCall(e) => &e.time,
            Self::ToolCall(e) => &e.time,
            Self::ToolResult(e) => &e.time,
            Self::TurnEnd(e) => &e.time,
            Self::LlmError(e) => &e.time,
            Self::AgentError(e) => &e.time,
            Self::TurnInterrupted(e) => &e.time,
            Self::LlmRetry(e) => &e.time,
            Self::ModelChanged(e) => &e.time,
            Self::EffortChanged(e) => &e.time,
            Self::TransportError(e) => &e.time,
            Self::ResumingSession(e) => &e.time,
            Self::SessionResumed(e) => &e.time,
            Self::HaltRequested(e) => &e.time,
            Self::TurnHalted(e) => &e.time,
            Self::TurnResumed(e) => &e.time,
            Self::LlmResponseStarted(e) => &e.time,
            Self::LlmResponseEnded(e) => &e.time,
            Self::LlmResponseDiscarded(e) => &e.time,
            Self::TextBlock(e) => &e.time,
            Self::ThinkingBlock(e) => &e.time,
            Self::ToolUseBlock(e) => &e.time,
            Self::ContextCompacted(e) => &e.time,
            Self::PythonReplBootstrapped(e) => &e.time,
            Self::HarnessRecovery(e) => &e.time,
            Self::MonitorStarted(e) => &e.time,
            Self::MonitorDelivery(e) => &e.time,
            Self::MonitorStderr(e) => &e.time,
            Self::MonitorStopped(e) => &e.time,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]

    use super::*;
    use crate::feature_flags::FeatureFlags;
    use crate::ids::{Origin, SessionId};

    // -----------------------------------------------------------------------
    // Discriminator / type-field round-trips
    // -----------------------------------------------------------------------

    /// Verify the `"type"` discriminator is `snake_case` and inlined correctly.
    #[test]
    fn session_started_type_field() {
        let sid: SessionId = "018f4c2e-3a1b-7d00-8000-abcdef012345".parse().unwrap();
        let ev = OmegaEvent::SessionStarted(SessionStartedEvent {
            time: "2024-01-15T12:00:00.000Z".into(),
            session_id: sid,
            path: String::new(),
            model: "claude-sonnet-4-6".into(),
            effort: "medium".into(),
            system_prompt: "You are Omega.".into(),
            omega_commit: "abc1234".into(),
            agent_time_zone: "Europe/Berlin".into(),
            origin: Origin::Root,
            features: FeatureFlags::default(),
            tool_selection: vec!["read_file".into(), "write_file".into()],
        });
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["type"], "session_started");
        assert_eq!(json["sessionId"], "018f4c2e-3a1b-7d00-8000-abcdef012345");
        assert_eq!(json["systemPrompt"], "You are Omega.");
        assert_eq!(json["agentTimeZone"], "Europe/Berlin");
        assert_eq!(json["origin"]["type"], "root");
        // tool_selection is camelCased on the wire and round-trips losslessly.
        assert_eq!(json["toolSelection"][0], "read_file");
        assert_eq!(json["toolSelection"][1], "write_file");
        let back: OmegaEvent = serde_json::from_value(json).unwrap();
        assert_eq!(ev, back);
    }

    /// Required-field rejection: `SessionStartedEvent` JSON without
    /// `toolSelection` must fail to deserialise loudly (per AGENTS.md
    /// schema-loud rule).
    #[test]
    fn session_started_missing_tool_selection_fails_loudly() {
        let json = serde_json::json!({
            "type": "session_started",
            "time": "2024-01-15T12:00:00.000Z",
            "sessionId": "018f4c2e-3a1b-7d00-8000-abcdef012345",
            "path": "",
            "model": "claude-sonnet-4-6",
            "effort": "medium",
            "systemPrompt": "You are Omega.",
            // toolSelection deliberately omitted
        });
        let err = serde_json::from_value::<OmegaEvent>(json).unwrap_err();
        assert!(
            err.to_string().contains("toolSelection"),
            "error must mention the missing field, got: {err}"
        );
    }

    /// Backward-compat: sessions written before the `omegaCommit` field
    /// existed must still deserialise cleanly. Kills both
    /// `replace default_omega_commit -> String with String::new()` and
    /// `... with "xyzzy".into()` — the only production caller of the
    /// default is the serde deserialiser path triggered when an old
    /// `events.jsonl` line lacks the field.
    #[test]
    fn session_started_uses_default_omega_commit_when_field_missing() {
        let json = serde_json::json!({
            "type": "session_started",
            "time": "2024-01-15T12:00:00.000Z",
            "sessionId": "018f4c2e-3a1b-7d00-8000-abcdef012345",
            "path": "",
            "model": "claude-sonnet-4-6",
            "effort": "medium",
            "systemPrompt": "You are Omega.",
            "toolSelection": [],
            // omegaCommit deliberately omitted
        });
        let parsed: OmegaEvent = serde_json::from_value(json).unwrap();
        match parsed {
            OmegaEvent::SessionStarted(ev) => {
                assert_eq!(ev.omega_commit, "unknown");
            }
            other => panic!("expected SessionStarted, got {other:?}"),
        }
    }

    /// Backward-compat: sessions written before the `agentTimeZone` field
    /// existed must still deserialise cleanly — they default to `"UTC"`,
    /// which renders unchanged from the pre-migration UI behaviour.
    #[test]
    fn session_started_uses_default_agent_time_zone_when_field_missing() {
        let json = serde_json::json!({
            "type": "session_started",
            "time": "2024-01-15T12:00:00.000Z",
            "sessionId": "018f4c2e-3a1b-7d00-8000-abcdef012345",
            "path": "",
            "model": "claude-sonnet-4-6",
            "effort": "medium",
            "systemPrompt": "You are Omega.",
            "omegaCommit": "abc1234",
            "toolSelection": [],
            // agentTimeZone deliberately omitted
        });
        let parsed: OmegaEvent = serde_json::from_value(json).unwrap();
        match parsed {
            OmegaEvent::SessionStarted(ev) => {
                assert_eq!(ev.agent_time_zone, "UTC");
            }
            other => panic!("expected SessionStarted, got {other:?}"),
        }
    }

    /// Backward-compat: sessions written before the `origin` field existed
    /// must still deserialise cleanly — they default to `Origin::Root`,
    /// which is the correct semantic for every pre-Phase-1 session.
    #[test]
    fn session_started_uses_default_origin_root_when_field_missing() {
        let json = serde_json::json!({
            "type": "session_started",
            "time": "2024-01-15T12:00:00.000Z",
            "sessionId": "018f4c2e-3a1b-7d00-8000-abcdef012345",
            "path": "",
            "model": "claude-sonnet-4-6",
            "effort": "medium",
            "systemPrompt": "You are Omega.",
            "omegaCommit": "abc1234",
            "agentTimeZone": "Europe/Berlin",
            "toolSelection": [],
            // origin deliberately omitted
        });
        let parsed: OmegaEvent = serde_json::from_value(json).unwrap();
        match parsed {
            OmegaEvent::SessionStarted(ev) => {
                assert_eq!(ev.origin, Origin::Root);
            }
            other => panic!("expected SessionStarted, got {other:?}"),
        }
    }

    /// Backward-compat: sessions written before the `features` field existed
    /// must still deserialise cleanly — they default to `FeatureFlags::default()`
    /// (subagents off), which is correct: the subagent code path did not
    /// exist before this field was introduced.
    #[test]
    fn session_started_uses_default_features_when_field_missing() {
        let json = serde_json::json!({
            "type": "session_started",
            "time": "2024-01-15T12:00:00.000Z",
            "sessionId": "018f4c2e-3a1b-7d00-8000-abcdef012345",
            "path": "",
            "model": "claude-sonnet-4-6",
            "effort": "medium",
            "systemPrompt": "You are Omega.",
            "omegaCommit": "abc1234",
            "agentTimeZone": "Europe/Berlin",
            "origin": {"type": "root"},
            "toolSelection": [],
            // features deliberately omitted — pre-flag-era session
        });
        let parsed: OmegaEvent = serde_json::from_value(json).unwrap();
        match parsed {
            OmegaEvent::SessionStarted(ev) => {
                assert_eq!(ev.features, FeatureFlags::default());
                assert!(
                    !ev.features.subagents,
                    "pre-flag-era subagents must be false"
                );
            }
            other => panic!("expected SessionStarted, got {other:?}"),
        }
    }

    /// Serde round-trip for `SessionStartedEvent` with features set.
    #[test]
    fn session_started_features_round_trip() {
        let sid: SessionId = "018f4c2e-3a1b-7d00-8000-abcdef012345".parse().unwrap();
        let ev = OmegaEvent::SessionStarted(SessionStartedEvent {
            time: "2024-01-15T12:00:00.000Z".into(),
            session_id: sid,
            path: String::new(),
            model: "claude-sonnet-4-6".into(),
            effort: "medium".into(),
            system_prompt: "You are Omega.".into(),
            omega_commit: "abc1234".into(),
            agent_time_zone: "UTC".into(),
            origin: Origin::Root,
            features: FeatureFlags { subagents: true },
            tool_selection: vec!["python_repl".into(), "web_search".into()],
        });
        let v = serde_json::to_value(&ev).unwrap();
        // features field is present with expected values
        assert_eq!(v["features"]["subagents"], true);
        // round-trip
        let back: OmegaEvent = serde_json::from_value(v).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn user_message_round_trip() {
        let ev = OmegaEvent::UserMessage(UserMessageEvent {
            time: "2024-01-15T12:00:01.000Z".into(),
            content: "Hello, agent!".into(),
        });
        let json = serde_json::to_string(&ev).unwrap();
        let back: OmegaEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
        // Check field name
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["type"], "user_message");
        assert_eq!(v["content"], "Hello, agent!");
    }

    #[test]
    fn tool_call_camel_case_fields() {
        let ev = OmegaEvent::ToolCall(ToolCallEvent {
            time: "2024-01-15T12:00:02.000Z".into(),
            tool_call_id: "a1b2c3d4".into(),
            name: "read_file".into(),
            input: serde_json::json!({"path": "foo.txt"}),
            context_hash: "aabbccddeeff0011".into(),
        });
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["type"], "tool_call");
        assert_eq!(v["toolCallId"], "a1b2c3d4");
        assert_eq!(v["contextHash"], "aabbccddeeff0011");
        // input should be inlined as-is
        assert_eq!(v["input"]["path"], "foo.txt");
    }

    #[test]
    fn tool_result_camel_case_fields() {
        let ev = OmegaEvent::ToolResult(ToolResultEvent {
            time: "2024-01-15T12:00:03.000Z".into(),
            tool_call_id: "a1b2c3d4".into(),
            name: "read_file".into(),
            is_error: false,
            duration_ms: 42,
            output: "file contents".into(),
        });
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["type"], "tool_result");
        assert_eq!(v["toolCallId"], "a1b2c3d4");
        assert_eq!(v["isError"], false);
        assert_eq!(v["durationMs"], 42);
    }

    #[test]
    fn llm_response_ended_usage_snake_case() {
        // Phase 6.5: verifies LlmResponseEnded (the sole response terminal
        // event) serialises usage fields in snake_case as expected.
        let ev = OmegaEvent::LlmResponseEnded(LlmResponseEndedEvent {
            time: "2024-01-15T12:00:04.000Z".into(),
            stop_reason: "end_turn".into(),
            cleared_tool_uses: None,
            cleared_input_tokens: None,
            usage: LlmResponseUsage {
                input_tokens: 100,
                output_tokens: 50,
                cache_creation_input_tokens: Some(10),
                cache_read_input_tokens: None,
                service_tier: None,
                iterations: None,
            },
            context_hash: "aabbccddeeff0011".into(),
            response_summary: None,
        });
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["type"], "llm_response_ended");
        assert_eq!(v["stopReason"], "end_turn");
        // usage fields: snake_case (Anthropic API native)
        assert_eq!(v["usage"]["input_tokens"], 100);
        assert_eq!(v["usage"]["output_tokens"], 50);
        assert_eq!(v["usage"]["cache_creation_input_tokens"], 10);
        // None fields are absent, not null
        assert!(
            v["usage"].get("cache_read_input_tokens").is_none()
                || v["usage"]["cache_read_input_tokens"].is_null()
        );
    }

    #[test]
    fn llm_call_cache_breakpoint_null() {
        // cacheBreakpointIndex is nullable (not optional) — must serialize as null
        let ev = OmegaEvent::LlmCall(LlmCallEvent {
            time: "2024-01-15T12:00:05.000Z".into(),
            url: "https://api.anthropic.com/v1/messages".into(),
            model: "claude-sonnet-4-6".into(),
            context_hashes: vec!["aabbccddeeff0011".into()],
            cache_breakpoint_index: None,
            request_bytes: 1234,
            request_summary: None,
        });
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["type"], "llm_call");
        assert_eq!(v["contextHashes"][0], "aabbccddeeff0011");
        // cacheBreakpointIndex: None → null in JSON (no skip_serializing_if)
        assert!(v["cacheBreakpointIndex"].is_null());
    }

    #[test]
    fn server_stopped_outcome_enum() {
        let ev = OmegaEvent::ServerStopped(ServerStoppedEvent {
            time: "2024-01-15T12:00:06.000Z".into(),
            outcome: ServerStopOutcome::Clean,
            reason: None,
        });
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["outcome"], "clean");
    }

    #[test]
    fn turn_interrupted_reason_enum() {
        let ev = OmegaEvent::TurnInterrupted(TurnInterruptedEvent {
            time: "2024-01-15T12:00:07.000Z".into(),
            reason: Some(InterruptReason::Aborted),
        });
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["reason"], "aborted");
    }

    #[test]
    fn turn_resumed_serialises() {
        let ev = OmegaEvent::TurnResumed(TurnResumedEvent {
            time: "2024-01-15T12:00:08.000Z".into(),
        });
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["type"], "turn_resumed");
    }

    #[test]
    fn turn_halted_serialises() {
        let ev = OmegaEvent::TurnHalted(TurnHaltedEvent {
            time: "2024-01-15T12:00:08.000Z".into(),
        });
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["type"], "turn_halted");
    }

    #[test]
    fn halt_requested_serialises() {
        let ev = OmegaEvent::HaltRequested(HaltRequestedEvent {
            time: "2024-01-15T12:00:08.000Z".into(),
        });
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["type"], "halt_requested");
    }

    #[test]
    fn llm_retry_reason_retry_after() {
        let ev = OmegaEvent::LlmRetry(LlmRetryEvent {
            time: "2024-01-15T12:00:09.000Z".into(),
            attempt: 1,
            http_status: Some(429),
            wait_ms: 5000,
            error: "rate limited".into(),
            retry_at: None,
            error_body: None,
            reason: Some(LlmRetryReason::RetryAfter),
        });
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["type"], "llm_retry");
        assert_eq!(v["reason"], "retry-after");
        assert_eq!(v["httpStatus"], 429);
    }

    #[test]
    fn resuming_session_camel_case() {
        let ev = OmegaEvent::ResumingSession(ResumingSessionEvent {
            time: "2024-01-15T12:00:10.000Z".into(),
            resumed_from: "20240115_120000".into(),
            name: Some("my session".into()),
            basis: "The previous session did X.".into(),
        });
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["type"], "resuming_session");
        assert_eq!(v["resumedFrom"], "20240115_120000");
        assert_eq!(v["name"], "my session");
    }

    // -----------------------------------------------------------------------
    // Full round-trip — deserialize from JSON string produced by TypeScript
    // -----------------------------------------------------------------------

    /// Verify that a JSON line as written by the TypeScript implementation
    /// parses successfully into the Rust type.
    #[test]
    fn deserialize_ts_session_started() {
        // Typical line from events.jsonl written by the TS agent
        let line = r#"{"type":"session_started","time":"2024-01-15T12:00:00.000Z","sessionId":"018f4c2e-3a1b-7d00-8000-abcdef012345","path":".omega/sessions/abc","model":"claude-sonnet-4-6","effort":"medium","systemPrompt":"You are Omega.","omegaCommit":"abc1234","toolSelection":["read_file"]}"#;
        let ev: OmegaEvent = serde_json::from_str(line).unwrap();
        match ev {
            OmegaEvent::SessionStarted(s) => {
                let expected: SessionId = "018f4c2e-3a1b-7d00-8000-abcdef012345".parse().unwrap();
                assert_eq!(s.session_id, expected);
                assert_eq!(s.model, "claude-sonnet-4-6");
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn deserialize_ts_tool_result() {
        let line = r#"{"type":"tool_result","time":"2024-01-15T12:00:03.000Z","toolCallId":"tool_1","name":"read_file","isError":false,"durationMs":12,"output":"contents"}"#;
        let ev: OmegaEvent = serde_json::from_str(line).unwrap();
        match ev {
            OmegaEvent::ToolResult(r) => {
                assert!(!r.is_error);
                assert_eq!(r.duration_ms, 12);
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn deserialize_ts_llm_retry_with_reason() {
        let line = r#"{"type":"llm_retry","time":"2024-01-15T12:00:09.000Z","attempt":1,"httpStatus":429,"waitMs":5000,"error":"rate limited","reason":"retry-after"}"#;
        let ev: OmegaEvent = serde_json::from_str(line).unwrap();
        match ev {
            OmegaEvent::LlmRetry(r) => {
                assert_eq!(r.reason, Some(LlmRetryReason::RetryAfter));
                assert_eq!(r.http_status, Some(429));
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // SCHEMA-8 — Phase 1b additive types: round-trip coverage.
    // -----------------------------------------------------------------------

    #[test]
    fn llm_response_started_round_trip() {
        let ev = OmegaEvent::LlmResponseStarted(LlmResponseStartedEvent {
            time: "2024-01-15T12:00:00.000Z".into(),
        });
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["type"], "llm_response_started");
        assert_eq!(v["time"], "2024-01-15T12:00:00.000Z");
        let back: OmegaEvent = serde_json::from_value(v).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn llm_response_ended_round_trip() {
        let ev = OmegaEvent::LlmResponseEnded(LlmResponseEndedEvent {
            time: "2024-01-15T12:00:01.000Z".into(),
            stop_reason: "end_turn".into(),
            cleared_tool_uses: Some(2),
            cleared_input_tokens: Some(123),
            usage: LlmResponseUsage {
                input_tokens: 100,
                output_tokens: 50,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
                service_tier: None,
                iterations: None,
            },
            context_hash: "aabbccddeeff0011".into(),
            response_summary: None,
        });
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["type"], "llm_response_ended");
        assert_eq!(v["stopReason"], "end_turn");
        assert_eq!(v["clearedToolUses"], 2);
        assert_eq!(v["clearedInputTokens"], 123);
        assert_eq!(v["contextHash"], "aabbccddeeff0011");
        // No legacy interval-summary fields on the new event.
        assert!(v.get("text").is_none() || v["text"].is_null());
        assert!(v.get("thinking").is_none() || v["thinking"].is_null());
        assert!(v.get("streamingStart").is_none() || v["streamingStart"].is_null());
        // Usage `iterations` absent when None (skip_serializing_if).
        assert!(v["usage"].get("iterations").is_none() || v["usage"]["iterations"].is_null());
        let back: OmegaEvent = serde_json::from_value(v).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn llm_response_discarded_round_trip() {
        let ev = OmegaEvent::LlmResponseDiscarded(LlmResponseDiscardedEvent {
            time: "2024-01-15T12:00:02.000Z".into(),
        });
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["type"], "llm_response_discarded");
        assert_eq!(v["time"], "2024-01-15T12:00:02.000Z");
        let back: OmegaEvent = serde_json::from_value(v).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn text_block_round_trip() {
        let ev = OmegaEvent::TextBlock(TextBlockEvent {
            time: "2024-01-15T12:00:03.000Z".into(),
            text: "hello, world".into(),
            partial: false,
        });
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["type"], "text_block");
        assert_eq!(v["text"], "hello, world");
        assert_eq!(v["partial"], false);
        let back: OmegaEvent = serde_json::from_value(v).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn text_block_partial_serialises_explicitly() {
        let ev = OmegaEvent::TextBlock(TextBlockEvent {
            time: "2024-01-15T12:00:03.000Z".into(),
            text: "par".into(),
            partial: true,
        });
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["partial"], true);
    }

    #[test]
    fn thinking_block_round_trip_full() {
        let ev = OmegaEvent::ThinkingBlock(ThinkingBlockEvent {
            time: "2024-01-15T12:00:04.000Z".into(),
            thinking: "hmm".into(),
            signature: Some("sig_abc".into()),
            partial: false,
        });
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["type"], "thinking_block");
        assert_eq!(v["thinking"], "hmm");
        assert_eq!(v["signature"], "sig_abc");
        assert_eq!(v["partial"], false);
        let back: OmegaEvent = serde_json::from_value(v).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn thinking_block_partial_drops_signature() {
        // Invariant: signature.is_none() iff partial == true.
        let ev = OmegaEvent::ThinkingBlock(ThinkingBlockEvent {
            time: "2024-01-15T12:00:04.000Z".into(),
            thinking: "hm".into(),
            signature: None,
            partial: true,
        });
        let v = serde_json::to_value(&ev).unwrap();
        assert!(v.get("signature").is_none() || v["signature"].is_null());
        assert_eq!(v["partial"], true);
        let back: OmegaEvent = serde_json::from_value(v).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn tool_use_block_round_trip() {
        let ev = OmegaEvent::ToolUseBlock(ToolUseBlockEvent {
            time: "2024-01-15T12:00:05.000Z".into(),
            tool_call_id: "a1b2c3d4".into(),
            tool_use_id: "toolu_xyz".into(),
            name: "read_file".into(),
            input: serde_json::json!({"path": "foo.txt"}),
            partial: false,
        });
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["type"], "tool_use_block");
        assert_eq!(v["toolCallId"], "a1b2c3d4");
        assert_eq!(v["toolUseId"], "toolu_xyz");
        assert_eq!(v["name"], "read_file");
        assert_eq!(v["input"]["path"], "foo.txt");
        assert_eq!(v["partial"], false);
        let back: OmegaEvent = serde_json::from_value(v).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn usage_iteration_round_trip() {
        let it = UsageIteration {
            iteration_type: "compaction".into(),
            input_tokens: 80,
            output_tokens: 0,
            cache_creation_input_tokens: Some(40),
            cache_read_input_tokens: None,
            service_tier: None,
        };
        let v = serde_json::to_value(&it).unwrap();
        // Wire field name is `type` (Anthropic shape).
        assert_eq!(v["type"], "compaction");
        assert_eq!(v["input_tokens"], 80);
        assert_eq!(v["output_tokens"], 0);
        assert_eq!(v["cache_creation_input_tokens"], 40);
        assert!(
            v.get("cache_read_input_tokens").is_none() || v["cache_read_input_tokens"].is_null()
        );
        let back: UsageIteration = serde_json::from_value(v).unwrap();
        assert_eq!(it, back);
    }

    #[test]
    fn llm_response_usage_with_iterations_serialises() {
        let usage = LlmResponseUsage {
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
            service_tier: None,
            iterations: Some(vec![
                UsageIteration {
                    iteration_type: "compaction".into(),
                    input_tokens: 80,
                    output_tokens: 0,
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                    service_tier: None,
                },
                UsageIteration {
                    iteration_type: "message".into(),
                    input_tokens: 20,
                    output_tokens: 50,
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                    service_tier: None,
                },
            ]),
        };
        let v = serde_json::to_value(&usage).unwrap();
        assert_eq!(v["iterations"][0]["type"], "compaction");
        assert_eq!(v["iterations"][1]["type"], "message");
        let back: LlmResponseUsage = serde_json::from_value(v).unwrap();
        assert_eq!(usage, back);
    }

    // -----------------------------------------------------------------------
    // ContextCompactedEvent
    // -----------------------------------------------------------------------

    #[test]
    fn context_compacted_round_trip() {
        let ev = OmegaEvent::ContextCompacted(ContextCompactedEvent {
            time: "2024-01-15T12:00:10.000Z".into(),
            tokens_before: 80_000,
            tokens_after: 500,
            summary_tokens: 300,
        });
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["type"], "context_compacted");
        assert_eq!(v["tokensBefore"], 80_000);
        assert_eq!(v["tokensAfter"], 500);
        assert_eq!(v["summaryTokens"], 300);
        assert_eq!(v["time"], "2024-01-15T12:00:10.000Z");
        let back: OmegaEvent = serde_json::from_value(v).unwrap();
        assert_eq!(ev, back);
    }

    /// The discriminator must be `context_compacted` (`snake_case` of the
    /// variant name) and the fields must be `camelCase` as per the struct
    /// annotation.
    #[test]
    fn context_compacted_field_names_are_camel_case() {
        let ev = OmegaEvent::ContextCompacted(ContextCompactedEvent {
            time: "2024-01-15T12:00:11.000Z".into(),
            tokens_before: 1,
            tokens_after: 2,
            summary_tokens: 3,
        });
        let s = serde_json::to_string(&ev).unwrap();
        assert!(
            s.contains("\"tokensBefore\""),
            "tokensBefore missing in: {s}"
        );
        assert!(s.contains("\"tokensAfter\""), "tokensAfter missing in: {s}");
        assert!(
            s.contains("\"summaryTokens\""),
            "summaryTokens missing in: {s}"
        );
        assert!(
            !s.contains("\"tokens_before\""),
            "snake_case leaked in: {s}"
        );
    }

    /// `time()` must return the embedded timestamp for `ContextCompacted`.
    #[test]
    fn time_returns_embedded_timestamp_for_context_compacted() {
        let ts = "2025-07-01T10:00:00.000Z";
        let ev = OmegaEvent::ContextCompacted(ContextCompactedEvent {
            time: ts.into(),
            tokens_before: 0,
            tokens_after: 0,
            summary_tokens: 0,
        });
        assert_eq!(ev.time().as_str(), ts);
    }

    /// Backward-compat: a usage object written before `iterations` was
    /// added must still deserialise, with `iterations: None`.
    #[test]
    fn llm_response_usage_without_iterations_round_trips() {
        let json = serde_json::json!({
            "input_tokens": 10,
            "output_tokens": 5
        });
        let usage: LlmResponseUsage = serde_json::from_value(json).unwrap();
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 5);
        assert!(usage.iterations.is_none());
        // And re-serialising omits the `iterations` key entirely (None
        // — skip_serializing_if).
        let v = serde_json::to_value(&usage).unwrap();
        assert!(v.get("iterations").is_none());
    }

    // -----------------------------------------------------------------------
    // OmegaEvent::time() — pin the accessor so the mutant that replaces the
    // entire body with `Box::leak(Box::new(Default::default()))` is caught.
    // Two variants are sufficient: the exhaustive match enforces that all
    // arms delegate to the real field, so covering any real variant kills
    // the whole-body replacement.
    // -----------------------------------------------------------------------

    #[test]
    fn time_returns_embedded_timestamp_for_session_started() {
        let ts = "2025-01-01T00:00:00.000Z";
        let ev = OmegaEvent::SessionStarted(SessionStartedEvent {
            time: ts.into(),
            session_id: "018f4c2e-3a1b-7d00-8000-abcdef012345"
                .parse::<SessionId>()
                .unwrap(),
            path: String::new(),
            model: "claude-sonnet-4-6".into(),
            effort: "medium".into(),
            system_prompt: String::new(),
            omega_commit: "abc".into(),
            agent_time_zone: "UTC".into(),
            origin: Origin::Root,
            features: FeatureFlags::default(),
            tool_selection: Vec::new(),
        });
        assert_eq!(ev.time().as_str(), ts);
    }

    #[test]
    fn time_returns_embedded_timestamp_for_turn_end() {
        let ts = "2025-06-15T12:34:56.789Z";
        let ev = OmegaEvent::TurnEnd(TurnEndEvent {
            time: ts.into(),
            metrics: TurnMetrics {
                input_tokens: 1,
                output_tokens: 2,
                cache_creation_tokens: None,
                cache_read_tokens: None,
            },
        });
        assert_eq!(ev.time().as_str(), ts);
    }

    // -----------------------------------------------------------------------
    // Phase 0 — Async Monitors: MonitorStopReason::should_project()
    // Phase 4 — adds StoppedBySessionEnd (not projected) and renames
    //            AgentStopped→StoppedByAgent, UserKilled→StoppedByUser,
    //            Crashed→ProcessCrashed.
    // -----------------------------------------------------------------------

    #[test]
    fn monitor_stop_reason_stopped_by_agent_is_not_projected() {
        assert!(
            !MonitorStopReason::StoppedByAgent.should_project(),
            "StoppedByAgent must NOT project — agent already knows it stopped the monitor"
        );
    }

    #[test]
    fn monitor_stop_reason_stopped_by_session_end_is_not_projected() {
        assert!(
            !MonitorStopReason::StoppedBySessionEnd.should_project(),
            "StoppedBySessionEnd must NOT project — session is terminating, no loop to notify"
        );
    }

    #[test]
    fn monitor_stop_reason_stopped_by_user_is_projected() {
        assert!(
            MonitorStopReason::StoppedByUser.should_project(),
            "StoppedByUser must project so agent learns the monitor was killed by the user"
        );
    }

    #[test]
    fn monitor_stop_reason_process_exited_is_projected() {
        assert!(
            MonitorStopReason::ProcessExited.should_project(),
            "ProcessExited must project so agent learns the monitor finished naturally"
        );
    }

    #[test]
    fn monitor_stop_reason_process_crashed_is_projected() {
        assert!(
            MonitorStopReason::ProcessCrashed.should_project(),
            "ProcessCrashed must project so agent learns the monitor crashed"
        );
    }

    // -----------------------------------------------------------------------
    // Phase 0 — Async Monitors: serde round-trips + wire-format assertions
    // -----------------------------------------------------------------------

    #[test]
    fn monitor_started_serde_round_trip_and_wire_format() {
        let ev = OmegaEvent::MonitorStarted(MonitorStartedEvent {
            id: "mon-1".into(),
            description: "watch build log".into(),
            command: "tail -f build.log".into(),
            time: "2024-01-15T12:00:00.000Z".into(),
        });
        let json = serde_json::to_value(&ev).unwrap();
        // type discriminator
        assert_eq!(json["type"], "monitor_started");
        // camelCase field names on the wire
        assert_eq!(json["id"], "mon-1");
        assert_eq!(json["description"], "watch build log");
        assert_eq!(json["command"], "tail -f build.log");
        assert_eq!(json["time"], "2024-01-15T12:00:00.000Z");
        // round-trip
        let back: OmegaEvent = serde_json::from_value(json).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn monitor_delivery_serde_round_trip_and_wire_format() {
        let ev = OmegaEvent::MonitorDelivery(MonitorDeliveryEvent {
            time: "2024-01-15T12:00:00.000Z".into(),
            items: vec![MonitorDeliveryItem {
                monitor_id: "mon-1".into(),
                lines: vec!["line 1".into(), "line 2".into()],
            }],
        });
        let json = serde_json::to_value(&ev).unwrap();
        // type discriminator
        assert_eq!(json["type"], "monitor_delivery");
        // camelCase on items
        assert_eq!(json["items"][0]["monitorId"], "mon-1");
        assert_eq!(json["items"][0]["lines"][0], "line 1");
        assert_eq!(json["items"][0]["lines"][1], "line 2");
        // round-trip
        let back: OmegaEvent = serde_json::from_value(json).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn monitor_stderr_serde_round_trip_and_wire_format() {
        let ev = OmegaEvent::MonitorStderr(MonitorStderrEvent {
            id: "mon-1".into(),
            chunk: "stderr output".into(),
            time: "2024-01-15T12:00:00.000Z".into(),
        });
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["type"], "monitor_stderr");
        assert_eq!(json["id"], "mon-1");
        assert_eq!(json["chunk"], "stderr output");
        assert_eq!(json["time"], "2024-01-15T12:00:00.000Z");
        let back: OmegaEvent = serde_json::from_value(json).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn monitor_stopped_serde_round_trip_stopped_by_agent() {
        let ev = OmegaEvent::MonitorStopped(MonitorStoppedEvent {
            id: "mon-1".into(),
            reason: MonitorStopReason::StoppedByAgent,
            exit_code: Some(0),
            time: "2024-01-15T12:00:00.000Z".into(),
        });
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["type"], "monitor_stopped");
        assert_eq!(json["id"], "mon-1");
        assert_eq!(json["reason"], "stopped_by_agent");
        assert_eq!(json["exitCode"], 0);
        let back: OmegaEvent = serde_json::from_value(json).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn monitor_stopped_serde_round_trip_stopped_by_session_end() {
        let ev = OmegaEvent::MonitorStopped(MonitorStoppedEvent {
            id: "mon-99".into(),
            reason: MonitorStopReason::StoppedBySessionEnd,
            exit_code: None,
            time: "2024-01-15T12:00:00.000Z".into(),
        });
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["reason"], "stopped_by_session_end");
        assert!(
            json["exitCode"].is_null(),
            "session-end stop has no exit code"
        );
        let back: OmegaEvent = serde_json::from_value(json).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn monitor_stopped_serde_round_trip_process_crashed_no_exit_code() {
        let ev = OmegaEvent::MonitorStopped(MonitorStoppedEvent {
            id: "mon-2".into(),
            reason: MonitorStopReason::ProcessCrashed,
            exit_code: None,
            time: "2024-01-15T12:00:00.000Z".into(),
        });
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["reason"], "process_crashed");
        assert!(json["exitCode"].is_null(), "absent exit code must be null");
        let back: OmegaEvent = serde_json::from_value(json).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn monitor_stopped_stopped_by_user_wire_format() {
        let ev = OmegaEvent::MonitorStopped(MonitorStoppedEvent {
            id: "mon-3".into(),
            reason: MonitorStopReason::StoppedByUser,
            exit_code: Some(137),
            time: "2024-01-15T12:00:00.000Z".into(),
        });
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["reason"], "stopped_by_user");
        assert_eq!(json["exitCode"], 137);
        let back: OmegaEvent = serde_json::from_value(json).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn monitor_stopped_process_exited_wire_format() {
        let ev = OmegaEvent::MonitorStopped(MonitorStoppedEvent {
            id: "mon-4".into(),
            reason: MonitorStopReason::ProcessExited,
            exit_code: Some(0),
            time: "2024-01-15T12:00:00.000Z".into(),
        });
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["reason"], "process_exited");
        let back: OmegaEvent = serde_json::from_value(json).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn monitor_started_time_accessor() {
        let ts = "2025-01-01T00:00:00.000Z";
        let ev = OmegaEvent::MonitorStarted(MonitorStartedEvent {
            id: "m".into(),
            description: String::new(),
            command: String::new(),
            time: ts.into(),
        });
        assert_eq!(ev.time().as_str(), ts);
    }

    #[test]
    fn monitor_delivery_time_accessor() {
        let ts = "2025-02-01T00:00:00.000Z";
        let ev = OmegaEvent::MonitorDelivery(MonitorDeliveryEvent {
            time: ts.into(),
            items: vec![],
        });
        assert_eq!(ev.time().as_str(), ts);
    }

    #[test]
    fn monitor_stderr_time_accessor() {
        let ts = "2025-03-01T00:00:00.000Z";
        let ev = OmegaEvent::MonitorStderr(MonitorStderrEvent {
            id: "m".into(),
            chunk: String::new(),
            time: ts.into(),
        });
        assert_eq!(ev.time().as_str(), ts);
    }

    #[test]
    fn monitor_stopped_time_accessor() {
        let ts = "2025-04-01T00:00:00.000Z";
        let ev = OmegaEvent::MonitorStopped(MonitorStoppedEvent {
            id: "m".into(),
            reason: MonitorStopReason::ProcessCrashed,
            exit_code: None,
            time: ts.into(),
        });
        assert_eq!(ev.time().as_str(), ts);
    }

    // -----------------------------------------------------------------------
    // HarnessRecovery serde round-trip and wire-format tests
    // -----------------------------------------------------------------------

    #[test]
    fn harness_recovery_empty_response_continuation_wire_format() {
        let ev = OmegaEvent::HarnessRecovery(HarnessRecoveryEvent {
            time: "2024-01-15T12:00:00.000Z".into(),
            kind: HarnessRecoveryKind::EmptyResponseContinuation,
            content: "Please continue.".into(),
        });
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["type"], "harness_recovery");
        assert_eq!(json["time"], "2024-01-15T12:00:00.000Z");
        assert_eq!(json["kind"], "empty_response_continuation");
        assert_eq!(json["content"], "Please continue.");
        let back: OmegaEvent = serde_json::from_value(json).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn harness_recovery_invalid_tool_json_wire_format() {
        let ev = OmegaEvent::HarnessRecovery(HarnessRecoveryEvent {
            time: "2024-01-15T12:00:00.000Z".into(),
            kind: HarnessRecoveryKind::InvalidToolJson,
            content: "The tool_use JSON could not be parsed.".into(),
        });
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["type"], "harness_recovery");
        assert_eq!(json["kind"], "invalid_tool_json");
        assert_eq!(json["content"], "The tool_use JSON could not be parsed.");
        let back: OmegaEvent = serde_json::from_value(json).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn harness_recovery_time_accessor() {
        let ts = "2025-05-01T00:00:00.000Z";
        let ev = OmegaEvent::HarnessRecovery(HarnessRecoveryEvent {
            time: ts.into(),
            kind: HarnessRecoveryKind::EmptyResponseContinuation,
            content: String::new(),
        });
        assert_eq!(ev.time().as_str(), ts);
    }
}
