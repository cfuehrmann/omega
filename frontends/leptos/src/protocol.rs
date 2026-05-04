//! Client-side wire-protocol types — typed mirrors of `omega-server`'s
//! `WsMessage` (server → client) and `ClientFrame` (client → server).
//!
//! ## Why a parallel enum (Phase 3.1 decision)
//!
//! The server-side `WsMessage` lives in `omega-server` (not `omega-protocol`)
//! because two of its variants are server-only wire shapes
//! (`SessionDeleted`, `SessionRenamed`, `ResetDone`) and `Item` carries a
//! `Box<AgentItem>` that is `#[serde(untagged)]` and `Serialize`-only by
//! design. Lifting the type into `omega-protocol` would either force a
//! redesign of `AgentItem` or pollute the protocol crate with a
//! transport-level concern.
//!
//! Instead, we mirror the wire format with a single flat tagged enum
//! that re-uses every typed event/signal struct from `omega-protocol`.
//! The duplication is purely at the variant-listing layer; field types
//! remain the single source of truth.
//!
//! ## Wire-shape collision: `agent_error`
//!
//! The server emits two distinct payloads under `type: "agent_error"`:
//!
//! - **Envelope** — `{ "type": "agent_error", "message": "..." }` —
//!   transport/handler-level error (malformed client frame, missing
//!   session, etc). Sent directly by the WS handler.
//! - **Event** — `{ "type": "agent_error", "time": "...", "error": "..." }`
//!   — agent-level error written to `events.jsonl` and forwarded as a
//!   `WsMessage::Item(OmegaEvent::AgentError(...))`.
//!
//! Resolved client-side by [`AgentErrorPayload`], an `#[serde(untagged)]`
//! enum that disambiguates by structure — no server change.
//!
//! ## Tag enumeration
//!
//! - 7 envelope tags: `ready`, `agent_error`, `session_info`, `history`,
//!   `reset_done`, `session_deleted`, `session_renamed`.
//! - 3 stream-signal tags forwarded inside the server's `Item` variant:
//!   `text`, `thinking`, `thinking_block_complete`.
//! - 22 [`omega_protocol::OmegaEvent`] tags forwarded via `Item`. The
//!   `agent_error` event tag merges into the envelope variant via the
//!   payload-disambiguation trick above, so 21 dedicated event variants
//!   appear here.

use omega_protocol::events::{
    CompactedEvent, EffortChangedEvent, LlmCallEvent, LlmErrorEvent, LlmResponseEvent,
    LlmRetryEvent, ModelChangedEvent, PauseRequestedEvent, ResumingSessionEvent,
    ServerStartedEvent, ServerStoppedEvent, SessionResumedEvent, SessionStartedEvent,
    ToolCallEvent, ToolResultEvent, TransportErrorEvent, TurnContinuedEvent, TurnEndEvent,
    TurnInterruptedEvent, TurnPausedEvent, UserMessageEvent,
};
use omega_protocol::events::AgentErrorEvent;
use omega_protocol::OmegaEvent;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Server-derived turn state
// ---------------------------------------------------------------------------

/// Server-reported turn state. Mirrors the values the server projects on
/// `WsMessage::SessionInfo.turnState` (see
/// `omega-server::router::next_turn_state_for`). Defaults to `Idle`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TurnState {
    #[default]
    Idle,
    Running,
    PauseRequested,
    Paused,
}

// ---------------------------------------------------------------------------
// Envelope payloads
// ---------------------------------------------------------------------------

/// Body of a `session_info` frame. Field-name projection mirrors the
/// server's `WsMessage::SessionInfo` JSON output (camelCase).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfoPayload {
    pub dir: String,
    pub model: String,
    pub effort: String,
    pub cwd: String,
    pub turn_state: TurnState,
    pub has_pending_changes: bool,
    /// Omitted on the wire when absent (server uses `Option::is_none`).
    #[serde(default)]
    pub name: Option<String>,
}

/// Body of a `history` frame. The `streaming` flag is omitted on the
/// wire when `false`; we reconstruct via `#[serde(default)]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryPayload {
    pub events: Vec<OmegaEvent>,
    #[serde(default)]
    pub streaming: bool,
}

/// `agent_error` payload disambiguator. See module docs for context.
///
/// Variant order matters for `#[serde(untagged)]`: the first variant
/// whose required fields all match wins. `Envelope` requires `message`;
/// `Event` requires `time` *and* `error`. The two field sets are
/// disjoint, so the dispatch is deterministic for any well-formed
/// server emission.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AgentErrorPayload {
    /// Server-side transport/handler error — no `time`, no `error` field.
    Envelope { message: String },
    /// Forwarded `OmegaEvent::AgentError` — has `time` and `error`.
    Event(AgentErrorEvent),
}

// ---------------------------------------------------------------------------
// Server → client: WsMessage
// ---------------------------------------------------------------------------

/// One frame received over the WebSocket from `omega-server`.
///
/// The outer discriminator is the `type` field. Every variant the server
/// emits today is enumerated explicitly so `serde::Deserialize` can
/// route each frame to a fully-typed payload — there is no
/// `serde_json::Value` in the parse path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsMessage {
    // --- Envelope (server-only wire shapes) ----------------------------------
    /// Server is ready to receive client frames.
    Ready,
    /// Acknowledgement of a `reset` client frame.
    ResetDone,
    /// Either a transport-layer error (envelope `{message}`) or a
    /// forwarded `OmegaEvent::AgentError` (`{time, error}`).
    AgentError(AgentErrorPayload),
    /// Session identity announcement.
    SessionInfo(SessionInfoPayload),
    /// Persisted history batch.
    History(HistoryPayload),
    /// A session directory was deleted on disk.
    #[serde(rename_all = "camelCase")]
    SessionDeleted { session_dir: String },
    /// A session was renamed on disk.
    #[serde(rename_all = "camelCase")]
    SessionRenamed { session_dir: String, name: String },

    // --- Forwarded `StreamSignal` payloads -----------------------------------
    /// Streaming assistant text fragment.
    Text { text: String },
    /// Streaming thinking-block fragment.
    Thinking { text: String },
    /// End-of-thinking-block marker (carries cryptographic signature).
    /// The UI ignores it; preserved so `apply` can reset the thinking
    /// accumulator.
    ThinkingBlockComplete { signature: String },

    // --- Forwarded `OmegaEvent` payloads (21 — `agent_error` merged above) ---
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

impl WsMessage {
    /// If this frame carries an [`OmegaEvent`] payload (forwarded `Item`
    /// or an `agent_error` *event*), reconstruct the original event so
    /// it can be appended to a UI's event log. Returns `None` for
    /// envelope-only frames and for raw stream signals.
    #[must_use]
    pub fn into_omega_event(self) -> Option<OmegaEvent> {
        Some(match self {
            // Envelope-only frames — no event payload.
            Self::Ready
            | Self::ResetDone
            | Self::SessionInfo(_)
            | Self::History(_)
            | Self::SessionDeleted { .. }
            | Self::SessionRenamed { .. } => return None,
            // Stream signals — never persisted as events.
            Self::Text { .. } | Self::Thinking { .. } | Self::ThinkingBlockComplete { .. } => {
                return None;
            }
            // `agent_error` envelope → not an event; envelope-side error.
            Self::AgentError(AgentErrorPayload::Envelope { .. }) => return None,
            // `agent_error` event → real OmegaEvent.
            Self::AgentError(AgentErrorPayload::Event(e)) => OmegaEvent::AgentError(e),

            // Genuine OmegaEvent variants.
            Self::SessionStarted(e) => OmegaEvent::SessionStarted(e),
            Self::ServerStarted(e) => OmegaEvent::ServerStarted(e),
            Self::ServerStopped(e) => OmegaEvent::ServerStopped(e),
            Self::UserMessage(e) => OmegaEvent::UserMessage(e),
            Self::LlmCall(e) => OmegaEvent::LlmCall(e),
            Self::LlmResponse(e) => OmegaEvent::LlmResponse(e),
            Self::ToolCall(e) => OmegaEvent::ToolCall(e),
            Self::ToolResult(e) => OmegaEvent::ToolResult(e),
            Self::TurnEnd(e) => OmegaEvent::TurnEnd(e),
            Self::LlmError(e) => OmegaEvent::LlmError(e),
            Self::TurnInterrupted(e) => OmegaEvent::TurnInterrupted(e),
            Self::Compacted(e) => OmegaEvent::Compacted(e),
            Self::LlmRetry(e) => OmegaEvent::LlmRetry(e),
            Self::ModelChanged(e) => OmegaEvent::ModelChanged(e),
            Self::EffortChanged(e) => OmegaEvent::EffortChanged(e),
            Self::TransportError(e) => OmegaEvent::TransportError(e),
            Self::ResumingSession(e) => OmegaEvent::ResumingSession(e),
            Self::SessionResumed(e) => OmegaEvent::SessionResumed(e),
            Self::PauseRequested(e) => OmegaEvent::PauseRequested(e),
            Self::TurnPaused(e) => OmegaEvent::TurnPaused(e),
            Self::TurnContinued(e) => OmegaEvent::TurnContinued(e),
        })
    }
}

// ---------------------------------------------------------------------------
// Client → server: ClientFrame
// ---------------------------------------------------------------------------

/// One frame the client may send to `omega-server`. Mirrors
/// `omega-server::router::ClientFrame`. Phase 3.1 only requires the
/// type to exist (no UI sends frames yet); 3.2 wires up actual sends.
#[allow(dead_code)] // variants constructed by 3.2+ composers
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientFrame {
    UserMessage {
        content: String,
    },
    Pause,
    #[serde(rename = "continue")]
    Continue {
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<String>,
    },
    Abort,
    Reset {
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        effort: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    ResumeSession {
        session_dir: String,
    },
    #[serde(rename_all = "camelCase")]
    RenameSession {
        session_dir: String,
        name: String,
    },
    SetModel {
        model: String,
    },
    SetEffort {
        effort: String,
    },
    #[serde(rename_all = "camelCase")]
    DeleteSession {
        session_dir: String,
    },
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]

    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test;

    fn parse(json: &str) -> WsMessage {
        serde_json::from_str(json).unwrap_or_else(|e| panic!("parse failed for {json}: {e}"))
    }

    // ---- envelope -----------------------------------------------------------

    #[wasm_bindgen_test]
    fn ready_round_trips() {
        let msg = parse(r#"{"type":"ready"}"#);
        assert!(matches!(msg, WsMessage::Ready));
    }

    #[wasm_bindgen_test]
    fn reset_done_round_trips() {
        let msg = parse(r#"{"type":"reset_done"}"#);
        assert!(matches!(msg, WsMessage::ResetDone));
    }

    #[wasm_bindgen_test]
    fn session_deleted_camel_case_dir() {
        let msg = parse(r#"{"type":"session_deleted","sessionDir":"abc"}"#);
        match msg {
            WsMessage::SessionDeleted { session_dir } => assert_eq!(session_dir, "abc"),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[wasm_bindgen_test]
    fn session_renamed_carries_dir_and_name() {
        let msg =
            parse(r#"{"type":"session_renamed","sessionDir":"d","name":"my-name"}"#);
        match msg {
            WsMessage::SessionRenamed { session_dir, name } => {
                assert_eq!(session_dir, "d");
                assert_eq!(name, "my-name");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[wasm_bindgen_test]
    fn session_info_with_name_and_pending_changes() {
        let json = r#"{
            "type":"session_info","dir":"d","model":"m","effort":"e",
            "cwd":"/c","turnState":"running","hasPendingChanges":true,
            "name":"alpha"
        }"#;
        match parse(json) {
            WsMessage::SessionInfo(p) => {
                assert_eq!(p.turn_state, TurnState::Running);
                assert!(p.has_pending_changes);
                assert_eq!(p.name.as_deref(), Some("alpha"));
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[wasm_bindgen_test]
    fn session_info_without_name_field() {
        let json = r#"{
            "type":"session_info","dir":"d","model":"m","effort":"e",
            "cwd":"/c","turnState":"idle","hasPendingChanges":false
        }"#;
        match parse(json) {
            WsMessage::SessionInfo(p) => {
                assert_eq!(p.turn_state, TurnState::Idle);
                assert!(!p.has_pending_changes);
                assert!(p.name.is_none());
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[wasm_bindgen_test]
    fn history_with_streaming_field() {
        let msg = parse(r#"{"type":"history","events":[],"streaming":true}"#);
        match msg {
            WsMessage::History(p) => {
                assert!(p.events.is_empty());
                assert!(p.streaming);
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[wasm_bindgen_test]
    fn history_without_streaming_field_defaults_to_false() {
        let msg = parse(r#"{"type":"history","events":[]}"#);
        match msg {
            WsMessage::History(p) => assert!(!p.streaming),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[wasm_bindgen_test]
    fn history_carries_typed_omega_events() {
        // Inner OmegaEvent must deserialise via its own tag = "type".
        let json = r#"{
            "type":"history",
            "events":[
                {"type":"server_started","time":"2024-01-01T00:00:00.000Z"}
            ]
        }"#;
        match parse(json) {
            WsMessage::History(p) => {
                assert_eq!(p.events.len(), 1);
                assert!(matches!(p.events[0], OmegaEvent::ServerStarted(_)));
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    // ---- agent_error disambiguation ----------------------------------------

    #[wasm_bindgen_test]
    fn agent_error_envelope_disambiguates_by_message_field() {
        let msg = parse(r#"{"type":"agent_error","message":"bad frame"}"#);
        match msg {
            WsMessage::AgentError(AgentErrorPayload::Envelope { message }) => {
                assert_eq!(message, "bad frame");
            }
            other => panic!("expected envelope, got {other:?}"),
        }
    }

    #[wasm_bindgen_test]
    fn agent_error_event_disambiguates_by_time_and_error_fields() {
        let json = r#"{"type":"agent_error","time":"2024-01-01T00:00:00.000Z","error":"oops"}"#;
        match parse(json) {
            WsMessage::AgentError(AgentErrorPayload::Event(e)) => {
                assert_eq!(e.error, "oops");
                assert_eq!(e.time, "2024-01-01T00:00:00.000Z");
            }
            other => panic!("expected event, got {other:?}"),
        }
    }

    // ---- stream signals -----------------------------------------------------

    #[wasm_bindgen_test]
    fn text_signal_round_trips() {
        match parse(r#"{"type":"text","text":"hello"}"#) {
            WsMessage::Text { text } => assert_eq!(text, "hello"),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[wasm_bindgen_test]
    fn thinking_signal_round_trips() {
        match parse(r#"{"type":"thinking","text":"musing"}"#) {
            WsMessage::Thinking { text } => assert_eq!(text, "musing"),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[wasm_bindgen_test]
    fn thinking_block_complete_carries_signature() {
        match parse(r#"{"type":"thinking_block_complete","signature":"sig"}"#) {
            WsMessage::ThinkingBlockComplete { signature } => assert_eq!(signature, "sig"),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    // ---- forwarded events ---------------------------------------------------

    #[wasm_bindgen_test]
    fn user_message_event_round_trips() {
        let json = r#"{"type":"user_message","time":"2024-01-01T00:00:00.000Z","content":"hi"}"#;
        match parse(json) {
            WsMessage::UserMessage(e) => assert_eq!(e.content, "hi"),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[wasm_bindgen_test]
    fn turn_end_event_round_trips() {
        let json = r#"{
            "type":"turn_end","time":"2024-01-01T00:00:00.000Z",
            "metrics":{"inputTokens":1,"outputTokens":2}
        }"#;
        match parse(json) {
            WsMessage::TurnEnd(e) => assert_eq!(e.metrics.output_tokens, 2),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    // ---- into_omega_event ---------------------------------------------------

    #[wasm_bindgen_test]
    fn into_omega_event_returns_none_for_envelope() {
        assert!(WsMessage::Ready.into_omega_event().is_none());
        assert!(WsMessage::ResetDone.into_omega_event().is_none());
    }

    #[wasm_bindgen_test]
    fn into_omega_event_returns_none_for_signals() {
        let sig = WsMessage::Text { text: "x".into() };
        assert!(sig.into_omega_event().is_none());
    }

    #[wasm_bindgen_test]
    fn into_omega_event_returns_none_for_envelope_agent_error() {
        let env = WsMessage::AgentError(AgentErrorPayload::Envelope {
            message: "boom".into(),
        });
        assert!(env.into_omega_event().is_none());
    }

    #[wasm_bindgen_test]
    fn into_omega_event_yields_typed_event_for_event_agent_error() {
        let ev = WsMessage::AgentError(AgentErrorPayload::Event(AgentErrorEvent {
            time: "t".into(),
            error: "e".into(),
        }));
        assert!(matches!(ev.into_omega_event(), Some(OmegaEvent::AgentError(_))));
    }

    // ---- ClientFrame --------------------------------------------------------

    #[wasm_bindgen_test]
    fn client_frame_user_message_serialises_with_snake_case_tag() {
        let frame = ClientFrame::UserMessage { content: "hi".into() };
        let json = serde_json::to_string(&frame).unwrap();
        assert_eq!(json, r#"{"type":"user_message","content":"hi"}"#);
    }

    #[wasm_bindgen_test]
    fn client_frame_continue_omits_optional_content() {
        let frame = ClientFrame::Continue { content: None };
        let json = serde_json::to_string(&frame).unwrap();
        assert_eq!(json, r#"{"type":"continue"}"#);
    }

    #[wasm_bindgen_test]
    fn client_frame_resume_session_uses_camel_case_field() {
        let frame = ClientFrame::ResumeSession {
            session_dir: "abc".into(),
        };
        let json = serde_json::to_string(&frame).unwrap();
        assert_eq!(json, r#"{"type":"resume_session","sessionDir":"abc"}"#);
    }

    #[wasm_bindgen_test]
    fn client_frame_reset_omits_absent_model_and_effort() {
        let frame = ClientFrame::Reset { model: None, effort: None };
        let json = serde_json::to_string(&frame).unwrap();
        assert_eq!(json, r#"{"type":"reset"}"#);
    }
}
