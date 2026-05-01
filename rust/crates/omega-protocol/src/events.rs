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
}

// ---------------------------------------------------------------------------
// Per-variant event structs
// ---------------------------------------------------------------------------

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

/// An LLM response received by the agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmResponseEvent {
    pub time: ISOTimestamp,
    pub stop_reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cleared_tool_uses: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cleared_input_tokens: Option<i64>,
    pub usage: LlmResponseUsage,
    /// FK into `context.jsonl` for the assistant record written for this response.
    pub context_hash: ContextHash,
    /// Full assembled assistant text, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Full assembled thinking content, if any (multiple blocks concatenated).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    /// ISO timestamp of the first streaming text delta.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub streaming_start: Option<ISOTimestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_summary: Option<Value>,
}

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

/// Server-side compaction fired during this turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompactedEvent {
    pub time: ISOTimestamp,
    /// Full usage object from the API response (structure varies; kept verbatim).
    pub usage: Value,
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
    /// Partial thinking content accumulated before the error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_fragment: Option<String>,
    /// Partial text content accumulated before the error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_fragment: Option<String>,
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
    LlmResponse(LlmResponseEvent),
    ToolCall(ToolCallEvent),
    ToolResult(ToolResultEvent),
    TurnEnd(TurnEndEvent),
    LlmError(LlmErrorEvent),
    AgentError(AgentErrorEvent),
    TurnInterrupted(TurnInterruptedEvent),
    Compacted(CompactedEvent),
    LlmRetry(LlmRetryEvent),
    ModelChanged(ModelChangedEvent),
    EffortChanged(EffortChangedEvent),
    TransportError(TransportErrorEvent),
    ResumingSession(ResumingSessionEvent),
    SessionResumed(SessionResumedEvent),
    PauseRequested(PauseRequestedEvent),
    TurnPaused(TurnPausedEvent),
    TurnContinued(TurnContinuedEvent),
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

    /// Verify the `"type"` discriminator is snake_case and inlined correctly.
    #[test]
    fn session_started_type_field() {
        let ev = OmegaEvent::SessionStarted(SessionStartedEvent {
            time: "2024-01-15T12:00:00.000Z".into(),
            session_id: "abc123".into(),
            path: "".into(),
            model: "claude-sonnet-4-6".into(),
            effort: "medium".into(),
            system_prompt: "You are Omega.".into(),
        });
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["type"], "session_started");
        assert_eq!(json["sessionId"], "abc123");
        assert_eq!(json["systemPrompt"], "You are Omega.");
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
            context_hash: "aabbccddeeff".into(),
        });
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["type"], "tool_call");
        assert_eq!(v["contextHash"], "aabbccddeeff");
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
    fn llm_response_usage_snake_case() {
        let ev = OmegaEvent::LlmResponse(LlmResponseEvent {
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
            },
            context_hash: "aabbccddeeff".into(),
            text: Some("Hello!".into()),
            thinking: None,
            streaming_start: None,
            response_summary: None,
        });
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["type"], "llm_response");
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
        // Optional event fields absent when None
        assert!(v.get("thinking").is_none() || v["thinking"].is_null());
    }

    #[test]
    fn llm_call_cache_breakpoint_null() {
        // cacheBreakpointIndex is nullable (not optional) — must serialize as null
        let ev = OmegaEvent::LlmCall(LlmCallEvent {
            time: "2024-01-15T12:00:05.000Z".into(),
            url: "https://api.anthropic.com/v1/messages".into(),
            model: "claude-sonnet-4-6".into(),
            context_hashes: vec!["aabbccddeeff".into()],
            cache_breakpoint_index: None,
            request_bytes: 1234,
            request_summary: None,
        });
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["type"], "llm_call");
        assert_eq!(v["contextHashes"][0], "aabbccddeeff");
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
            thinking_fragment: None,
            text_fragment: None,
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

    #[test]
    fn compacted_usage_is_opaque() {
        let raw_usage = serde_json::json!({
            "input_tokens": 100,
            "output_tokens": 50,
            "iterations": [
                {"type": "compaction", "input_tokens": 80},
                {"type": "message",    "input_tokens": 20}
            ]
        });
        let ev = OmegaEvent::Compacted(CompactedEvent {
            time: "2024-01-15T12:00:11.000Z".into(),
            usage: raw_usage.clone(),
        });
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["usage"], raw_usage);
    }

    // -----------------------------------------------------------------------
    // Full round-trip — deserialize from JSON string produced by TypeScript
    // -----------------------------------------------------------------------

    /// Verify that a JSON line as written by the TypeScript implementation
    /// parses successfully into the Rust type.
    #[test]
    fn deserialize_ts_session_started() {
        // Typical line from events.jsonl written by the TS agent
        let line = r#"{"type":"session_started","time":"2024-01-15T12:00:00.000Z","sessionId":"abc","path":".omega/sessions/abc","model":"claude-sonnet-4-6","effort":"medium","systemPrompt":"You are Omega."}"#;
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
}
