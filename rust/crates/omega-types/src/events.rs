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

/// `TurnContinuedEvent.mode` discriminator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContinueMode {
    Manual,
    Auto,
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

/// The session started (first event in every session).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStartedEvent {
    pub time: ISOTimestamp,
    pub session_id: String,
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
pub struct ToolUseBlockEvent {
    pub time: ISOTimestamp,
    pub id: String,
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
    pub id: String,
    pub name: String,
    /// Tool input parameters (arbitrary JSON from the LLM).
    pub input: Value,
    /// Hash of the assistant context.jsonl record containing this `tool_use` block.
    pub context_hash: ContextHash,
    /// Agent-assigned identifier for this tool invocation, independent of the
    /// LLM provider.  Used as the stem of the tee-log filename so that
    /// `events.jsonl` and `cache/<tool>/<call_id>-<tag>.log` are
    /// bidirectionally cross-referenceable without knowing the provider format.
    ///
    /// Absent on events written before this field was introduced (serde
    /// deserializes those as `None`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub call_id: Option<String>,
}

/// The result of a tool invocation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultEvent {
    pub time: ISOTimestamp,
    pub id: String,
    pub name: String,
    pub is_error: bool,
    pub duration_ms: i64,
    /// Full text output of the tool.
    pub output: String,
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

/// The user has requested a pause.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PauseRequestedEvent {
    pub time: ISOTimestamp,
}

/// The agent has reached a clean seam and the turn is now paused.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnPausedEvent {
    pub time: ISOTimestamp,
}

/// The paused turn is resuming.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnContinuedEvent {
    pub time: ISOTimestamp,
    pub mode: ContinueMode,
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
    PauseRequested(PauseRequestedEvent),
    TurnPaused(TurnPausedEvent),
    TurnContinued(TurnContinuedEvent),

    // --- SCHEMA-8 block-grammar variants (Phase 1b) -------------------------
    LlmResponseStarted(LlmResponseStartedEvent),
    LlmResponseEnded(LlmResponseEndedEvent),
    LlmResponseDiscarded(LlmResponseDiscardedEvent),
    TextBlock(TextBlockEvent),
    ThinkingBlock(ThinkingBlockEvent),
    ToolUseBlock(ToolUseBlockEvent),
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
            Self::PauseRequested(e) => &e.time,
            Self::TurnPaused(e) => &e.time,
            Self::TurnContinued(e) => &e.time,
            Self::LlmResponseStarted(e) => &e.time,
            Self::LlmResponseEnded(e) => &e.time,
            Self::LlmResponseDiscarded(e) => &e.time,
            Self::TextBlock(e) => &e.time,
            Self::ThinkingBlock(e) => &e.time,
            Self::ToolUseBlock(e) => &e.time,
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

    // -----------------------------------------------------------------------
    // Discriminator / type-field round-trips
    // -----------------------------------------------------------------------

    /// Verify the `"type"` discriminator is `snake_case` and inlined correctly.
    #[test]
    fn session_started_type_field() {
        let ev = OmegaEvent::SessionStarted(SessionStartedEvent {
            time: "2024-01-15T12:00:00.000Z".into(),
            session_id: "abc123".into(),
            path: String::new(),
            model: "claude-sonnet-4-6".into(),
            effort: "medium".into(),
            system_prompt: "You are Omega.".into(),
            omega_commit: "abc1234".into(),
            agent_time_zone: "Europe/Berlin".into(),
        });
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["type"], "session_started");
        assert_eq!(json["sessionId"], "abc123");
        assert_eq!(json["systemPrompt"], "You are Omega.");
        assert_eq!(json["agentTimeZone"], "Europe/Berlin");
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
            "sessionId": "abc123",
            "path": "",
            "model": "claude-sonnet-4-6",
            "effort": "medium",
            "systemPrompt": "You are Omega.",
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
            "sessionId": "abc123",
            "path": "",
            "model": "claude-sonnet-4-6",
            "effort": "medium",
            "systemPrompt": "You are Omega.",
            "omegaCommit": "abc1234",
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
            id: "tool_abc".into(),
            name: "read_file".into(),
            input: serde_json::json!({"path": "foo.txt"}),
            context_hash: "aabbccddeeff0011".into(),
            call_id: Some("a1b2c3d4".into()),
        });
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["type"], "tool_call");
        assert_eq!(v["contextHash"], "aabbccddeeff0011");
        // input should be inlined as-is
        assert_eq!(v["input"]["path"], "foo.txt");
    }

    #[test]
    fn tool_result_camel_case_fields() {
        let ev = OmegaEvent::ToolResult(ToolResultEvent {
            time: "2024-01-15T12:00:03.000Z".into(),
            id: "tool_abc".into(),
            name: "read_file".into(),
            is_error: false,
            duration_ms: 42,
            output: "file contents".into(),
        });
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["type"], "tool_result");
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
    fn turn_continued_mode_enum() {
        let ev = OmegaEvent::TurnContinued(TurnContinuedEvent {
            time: "2024-01-15T12:00:08.000Z".into(),
            mode: ContinueMode::Manual,
        });
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["mode"], "manual");
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
        let line = r#"{"type":"session_started","time":"2024-01-15T12:00:00.000Z","sessionId":"abc","path":".omega/sessions/abc","model":"claude-sonnet-4-6","effort":"medium","systemPrompt":"You are Omega.","omegaCommit":"abc1234"}"#;
        let ev: OmegaEvent = serde_json::from_str(line).unwrap();
        match ev {
            OmegaEvent::SessionStarted(s) => {
                assert_eq!(s.session_id, "abc");
                assert_eq!(s.model, "claude-sonnet-4-6");
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn deserialize_ts_tool_result() {
        let line = r#"{"type":"tool_result","time":"2024-01-15T12:00:03.000Z","id":"tool_1","name":"read_file","isError":false,"durationMs":12,"output":"contents"}"#;
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
            id: "toolu_xyz".into(),
            name: "read_file".into(),
            input: serde_json::json!({"path": "foo.txt"}),
            partial: false,
        });
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["type"], "tool_use_block");
        assert_eq!(v["id"], "toolu_xyz");
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
}
