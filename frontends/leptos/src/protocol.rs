//! Client-side wire-protocol types — typed mirrors of `omega-server`'s
//! `WsMessage` (server → client) and `ClientFrame` (client → server).
//!
//! ## Why a parallel enum (Phase 3.1 decision)
//!
//! The server-side `WsMessage` lives in `omega-server` (not `omega-types`)
//! because two of its variants are server-only wire shapes
//! (`SessionDeleted`, `SessionRenamed`, `ResetDone`) and `Item` carries a
//! `Box<AgentItem>` that is `#[serde(untagged)]` and `Serialize`-only by
//! design. Lifting the type into `omega-types` would either force a
//! redesign of `AgentItem` or pollute the shared-types crate with a
//! transport-level concern.
//!
//! Instead, we mirror the wire format with a single flat tagged enum
//! that re-uses every typed event/signal struct from `omega-types`.
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
//! - 5 stream-signal tags forwarded inside the server's `Item` variant:
//!   `text`, `thinking`, `thinking_block_complete`, `tool_use_block_start`,
//!   `tool_input`.
//! - 20 [`omega_types::OmegaEvent`] tags forwarded via `Item`. The
//!   `agent_error` event tag merges into the envelope variant via the
//!   payload-disambiguation trick above, so 19 dedicated event variants
//!   appear here.  SCHEMA-8 Phase 3 adds 6 block-grammar events;
//!   Phase 6.5 removes `llm_response` and `compacted`, bringing the
//!   total to 25 event variants.  Phase 2.0 (F11) adds `context_compacted`
//!   bringing the total to 26 event variants.

use omega_types::OmegaEvent;
use omega_types::events::AgentErrorEvent;
use omega_types::events::{
    ContextCompactedEvent, EffortChangedEvent, LlmCallEvent, LlmErrorEvent,
    LlmResponseDiscardedEvent, LlmResponseEndedEvent, LlmResponseStartedEvent, LlmRetryEvent,
    ModelChangedEvent, PauseRequestedEvent, PythonReplBootstrappedEvent, ResumingSessionEvent,
    ServerStartedEvent, ServerStoppedEvent, SessionResumedEvent, SessionStartedEvent,
    TextBlockEvent, ThinkingBlockEvent, ToolCallEvent, ToolResultEvent, ToolUseBlockEvent,
    TransportErrorEvent, TurnContinuedEvent, TurnEndEvent, TurnInterruptedEvent, TurnPausedEvent,
    UserMessageEvent,
};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Pending-changes warning payload
// ---------------------------------------------------------------------------

/// Mirror of the server's `PendingChangesIntent`: what the operator was
/// about to do when the dirty-tree gate fired.  The client uses this to
/// re-issue the original frame with `allow_dirty: true` after the
/// operator confirms via the dirty-warning modal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PendingChangesIntent {
    Reset {
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        effort: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    ResumeSession { session_dir: String },
}

/// Helper for `#[serde(skip_serializing_if = ...)]` on `bool` fields
/// where the default `false` should be omitted from the wire.
#[allow(clippy::trivially_copy_pass_by_ref)] // serde requires &T
fn is_false(b: &bool) -> bool {
    !*b
}

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
    SessionDeleted {
        session_dir: String,
    },
    /// A session was renamed on disk.
    #[serde(rename_all = "camelCase")]
    SessionRenamed {
        session_dir: String,
        name: String,
    },

    // --- Forwarded `StreamSignal` payloads -----------------------------------
    /// Streaming assistant text fragment.  `index` matches Anthropic's
    /// `content_block_start.index` so the per-block streaming buffer
    /// can route deltas to the correct slot (SCHEMA-8 Phase 5a).
    Text {
        index: usize,
        text: String,
    },
    /// Streaming thinking-block fragment.  `index` carries the same
    /// semantics as on [`Self::Text`].
    Thinking {
        index: usize,
        text: String,
    },
    /// End-of-thinking-block marker (carries cryptographic signature).
    /// The UI ignores the signature; `index` lets `apply` drop the
    /// matching slot from the thinking streaming buffer.
    ThinkingBlockComplete {
        index: usize,
        signature: String,
    },
    /// Streaming tool-use block opener.  Carries the LLM-issued
    /// `tool_use_id` and `name` so the UI can render the label
    /// immediately, before any `ToolInput` deltas arrive.
    ToolUseBlockStart {
        index: usize,
        tool_use_id: String,
        name: String,
    },
    /// Streaming partial-JSON fragment for the tool-use block at
    /// `index`.  Mid-stream content is NOT valid JSON; rendered raw.
    ToolInput {
        index: usize,
        partial_json: String,
    },

    // --- Forwarded `OmegaEvent` payloads (19 — `agent_error` merged above) ---
    SessionStarted(SessionStartedEvent),
    ServerStarted(ServerStartedEvent),
    ServerStopped(ServerStoppedEvent),
    UserMessage(UserMessageEvent),
    LlmCall(LlmCallEvent),
    ToolCall(ToolCallEvent),
    ToolResult(ToolResultEvent),
    TurnEnd(TurnEndEvent),
    LlmError(LlmErrorEvent),
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

    // --- SCHEMA-8 block-grammar event variants (Phase 3 commit 3b) ---------
    // Legacy `LlmResponse` and `Compacted` variants removed in Phase 6.5.
    LlmResponseStarted(LlmResponseStartedEvent),
    LlmResponseEnded(LlmResponseEndedEvent),
    LlmResponseDiscarded(LlmResponseDiscardedEvent),
    TextBlock(TextBlockEvent),
    ThinkingBlock(ThinkingBlockEvent),
    ToolUseBlock(ToolUseBlockEvent),

    // --- Phase 2.0 (F11) — server-side compaction event --------------------
    /// Server-side context compaction fired; always precedes `LlmResponseEnded`.
    ContextCompacted(ContextCompactedEvent),

    // --- python_repl bootstrap event ---------------------------------------
    /// Omega auto-installed python3 via apt-get because it was absent from PATH.
    PythonReplBootstrapped(PythonReplBootstrappedEvent),

    /// A `Reset` or `ResumeSession` frame was rejected because the
    /// working tree has uncommitted git changes and `allow_dirty` was
    /// not set.  The previous active session (if any) is untouched.
    /// `intent` echoes the original parameters so the client can
    /// re-issue with `allow_dirty: true` on operator confirmation.
    PendingChangesWarning {
        intent: PendingChangesIntent,
    },
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
            | Self::SessionRenamed { .. }
            | Self::PendingChangesWarning { .. } => return None,
            // Stream signals — never persisted as events.
            Self::Text { .. }
            | Self::Thinking { .. }
            | Self::ThinkingBlockComplete { .. }
            | Self::ToolUseBlockStart { .. }
            | Self::ToolInput { .. } => {
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
            Self::ToolCall(e) => OmegaEvent::ToolCall(e),
            Self::ToolResult(e) => OmegaEvent::ToolResult(e),
            Self::TurnEnd(e) => OmegaEvent::TurnEnd(e),
            Self::LlmError(e) => OmegaEvent::LlmError(e),
            Self::TurnInterrupted(e) => OmegaEvent::TurnInterrupted(e),
            Self::LlmRetry(e) => OmegaEvent::LlmRetry(e),
            Self::ModelChanged(e) => OmegaEvent::ModelChanged(e),
            Self::EffortChanged(e) => OmegaEvent::EffortChanged(e),
            Self::TransportError(e) => OmegaEvent::TransportError(e),
            Self::ResumingSession(e) => OmegaEvent::ResumingSession(e),
            Self::SessionResumed(e) => OmegaEvent::SessionResumed(e),
            Self::PauseRequested(e) => OmegaEvent::PauseRequested(e),
            Self::TurnPaused(e) => OmegaEvent::TurnPaused(e),
            Self::TurnContinued(e) => OmegaEvent::TurnContinued(e),
            // SCHEMA-8 block-grammar variants.
            Self::LlmResponseStarted(e) => OmegaEvent::LlmResponseStarted(e),
            Self::LlmResponseEnded(e) => OmegaEvent::LlmResponseEnded(e),
            Self::LlmResponseDiscarded(e) => OmegaEvent::LlmResponseDiscarded(e),
            Self::TextBlock(e) => OmegaEvent::TextBlock(e),
            Self::ThinkingBlock(e) => OmegaEvent::ThinkingBlock(e),
            Self::ToolUseBlock(e) => OmegaEvent::ToolUseBlock(e),
            // Phase 2.0 (F11).
            Self::ContextCompacted(e) => OmegaEvent::ContextCompacted(e),
            // python_repl bootstrap.
            Self::PythonReplBootstrapped(e) => OmegaEvent::PythonReplBootstrapped(e),
        })
    }
}

// ---------------------------------------------------------------------------
// Client → server: ClientFrame
// ---------------------------------------------------------------------------

/// One frame the client may send to `omega-server`. Mirrors
/// `omega-server::router::ClientFrame`. Phase 3.1 only requires the
/// type to exist (no UI sends frames yet); 3.2 wires up actual sends.
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
        #[serde(rename = "allowDirty", skip_serializing_if = "is_false")]
        allow_dirty: bool,
        // TODO(Phase 2.1): wire the UI's tool-selection picker through to
        // this field.  For now we always send `None`, which causes the
        // server to fall back to `omega_tools::DEFAULT_TOOL_NAMES`.
        #[serde(rename = "toolSelection", skip_serializing_if = "Option::is_none")]
        tool_selection: Option<Vec<String>>,
    },
    #[serde(rename_all = "camelCase")]
    ResumeSession {
        session_dir: String,
        #[serde(rename = "allowDirty", skip_serializing_if = "is_false")]
        allow_dirty: bool,
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
// Tool-selection presets (Phase 2.1 Commit B)
// ---------------------------------------------------------------------------

/// A named tool-selection preset surfaced by the picker UI.
///
/// SOURCE OF TRUTH: `crates/omega-tools/src/schemas.rs::PRESETS`.
///
/// The leptos crate compiles to `wasm32-unknown-unknown` and cannot
/// link against `omega-tools` (different target).  This mirror
/// duplicates the three preset definitions the UI needs (id, label,
/// tool list).  `Preset.description` is intentionally not mirrored;
/// the UI doesn't render it.  When `PRESETS` in `omega-tools` grows a
/// new entry, add a matching mirror here — both arrays must agree
/// (the server validates the wire `tool_selection` against
/// `ALL_TOOL_NAMES`, so a UI/preset mismatch surfaces as an
/// `agent_error` envelope rather than silent drift).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Preset {
    /// Stable preset id (matches `omega-tools::Preset::id`).
    pub id: &'static str,
    /// Display label for the chip.
    pub label: &'static str,
    /// Tool names this preset selects.
    pub tools: &'static [&'static str],
}

/// Standard preset — file ops + shell + web (no python_repl).
const STANDARD_TOOLS: &[&str] = &[
    "read_file",
    "write_file",
    "run_command",
    "edit_file",
    "list_files",
    "web_search",
    "fetch_url",
    "grep_files",
    "find_files",
    "run_background",
    "wait_for_output",
    "write_stdin",
];

/// All-tools preset — Standard plus python_repl.
const ALL_TOOLS: &[&str] = &[
    "read_file",
    "write_file",
    "run_command",
    "edit_file",
    "list_files",
    "web_search",
    "fetch_url",
    "grep_files",
    "find_files",
    "run_background",
    "wait_for_output",
    "write_stdin",
    "python_repl",
];

/// REPL-centric preset — python_repl plus web tools.
const REPL_CENTRIC_TOOLS: &[&str] = &["python_repl", "web_search", "fetch_url"];

/// All known presets, in chip-display order.  Mirrors
/// `omega_tools::PRESETS` — see [`Preset`] for the contract.
pub const PRESETS: &[Preset] = &[
    Preset {
        id: "standard",
        label: "Standard",
        tools: STANDARD_TOOLS,
    },
    Preset {
        id: "all",
        label: "+ Python REPL",
        tools: ALL_TOOLS,
    },
    Preset {
        id: "repl-centric",
        label: "REPL-centric",
        tools: REPL_CENTRIC_TOOLS,
    },
];

/// localStorage key persisting the operator's last tool selection.
/// Restored as the initial selection on the next "+ New session".
pub const TOOL_SELECTION_STORAGE_KEY: &str = "omega.toolSelection.lastChoice";

/// Standard preset materialised as a fresh `Vec<String>` — the
/// fallback when localStorage is empty / corrupt, and the initial
/// state of a tool picker on first ever use.
#[must_use]
pub fn default_tool_selection() -> Vec<String> {
    PRESETS[0].tools.iter().map(|s| (*s).to_owned()).collect()
}

/// Resolve a checkbox selection back to a preset id by **set equality**
/// (order doesn't matter).  Returns `None` when the selection matches
/// no named preset; the UI surfaces this as the *Custom* chip.
#[must_use]
pub fn resolve_preset(selection: &[String]) -> Option<&'static str> {
    PRESETS
        .iter()
        .find(|p| {
            p.tools.len() == selection.len()
                && p.tools.iter().all(|t| selection.iter().any(|s| s == t))
        })
        .map(|p| p.id)
}

/// Parse a localStorage payload (JSON array of strings) into a tool
/// selection.  Returns the Standard preset on any of:
///
/// * `raw == None`               — storage empty
/// * not valid JSON              — corrupt
/// * not a JSON array of strings — corrupt
/// * empty array                 — the UI requires ≥1 tool
///
/// Pure function — testable without a browser.
#[must_use]
pub fn parse_stored_selection(raw: Option<&str>) -> Vec<String> {
    raw.and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(default_tool_selection)
}

/// Serialise a tool selection for localStorage.  Cannot fail for
/// `Vec<String>`; the caller gets a plain `String`.
#[must_use]
pub fn serialize_selection(selection: &[String]) -> String {
    serde_json::to_string(selection).expect("Vec<String> always serialises")
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
        let msg = parse(r#"{"type":"session_renamed","sessionDir":"d","name":"my-name"}"#);
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
        match parse(r#"{"type":"text","index":0,"text":"hello"}"#) {
            WsMessage::Text { index, text } => {
                assert_eq!(index, 0);
                assert_eq!(text, "hello");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[wasm_bindgen_test]
    fn text_signal_with_nonzero_index() {
        // Interleaved-thinking can revisit older indices; the wire
        // value must round-trip 1:1.  SCHEMA-8 Phase 5a.
        match parse(r#"{"type":"text","index":3,"text":"world"}"#) {
            WsMessage::Text { index, text } => {
                assert_eq!(index, 3);
                assert_eq!(text, "world");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[wasm_bindgen_test]
    fn thinking_signal_round_trips() {
        match parse(r#"{"type":"thinking","index":1,"text":"musing"}"#) {
            WsMessage::Thinking { index, text } => {
                assert_eq!(index, 1);
                assert_eq!(text, "musing");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[wasm_bindgen_test]
    fn thinking_block_complete_carries_index_and_signature() {
        match parse(r#"{"type":"thinking_block_complete","index":2,"signature":"sig"}"#) {
            WsMessage::ThinkingBlockComplete { index, signature } => {
                assert_eq!(index, 2);
                assert_eq!(signature, "sig");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[wasm_bindgen_test]
    fn tool_use_block_start_round_trips() {
        match parse(
            r#"{"type":"tool_use_block_start","index":3,"tool_use_id":"tu_1","name":"bash"}"#,
        ) {
            WsMessage::ToolUseBlockStart {
                index,
                tool_use_id,
                name,
            } => {
                assert_eq!(index, 3);
                assert_eq!(tool_use_id, "tu_1");
                assert_eq!(name, "bash");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[wasm_bindgen_test]
    fn tool_input_round_trips() {
        // partial_json carries raw partial-JSON fragments; verify
        // the field round-trips without modification.
        match parse(r#"{"type":"tool_input","index":3,"partial_json":"{cmd:"}"#) {
            WsMessage::ToolInput {
                index,
                partial_json,
            } => {
                assert_eq!(index, 3);
                assert_eq!(partial_json, "{cmd:");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[wasm_bindgen_test]
    fn tool_use_block_start_and_tool_input_return_none_from_into_omega_event() {
        assert!(
            WsMessage::ToolUseBlockStart {
                index: 0,
                tool_use_id: "tu".into(),
                name: "bash".into(),
            }
            .into_omega_event()
            .is_none()
        );
        assert!(
            WsMessage::ToolInput {
                index: 0,
                partial_json: "{\"a\"".into(),
            }
            .into_omega_event()
            .is_none()
        );
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
        let sig = WsMessage::Text {
            index: 0,
            text: "x".into(),
        };
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
        assert!(matches!(
            ev.into_omega_event(),
            Some(OmegaEvent::AgentError(_))
        ));
    }

    // ---- ClientFrame --------------------------------------------------------

    #[wasm_bindgen_test]
    fn client_frame_user_message_serialises_with_snake_case_tag() {
        let frame = ClientFrame::UserMessage {
            content: "hi".into(),
        };
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
            allow_dirty: false,
        };
        let json = serde_json::to_string(&frame).unwrap();
        assert_eq!(json, r#"{"type":"resume_session","sessionDir":"abc"}"#);
    }

    #[wasm_bindgen_test]
    fn client_frame_reset_omits_absent_model_and_effort() {
        let frame = ClientFrame::Reset {
            model: None,
            effort: None,
            allow_dirty: false,
            tool_selection: None,
        };
        let json = serde_json::to_string(&frame).unwrap();
        assert_eq!(json, r#"{"type":"reset"}"#);
    }

    #[wasm_bindgen_test]
    fn client_frame_reset_serialises_tool_selection_under_camel_case_key() {
        let frame = ClientFrame::Reset {
            model: None,
            effort: None,
            allow_dirty: false,
            tool_selection: Some(vec!["python_repl".into(), "web_search".into()]),
        };
        let json = serde_json::to_string(&frame).unwrap();
        assert_eq!(
            json,
            r#"{"type":"reset","toolSelection":["python_repl","web_search"]}"#
        );
    }

    // ---- Tool-selection presets (Phase 2.1 Commit B) -----------------------

    #[wasm_bindgen_test]
    #[test]
    fn presets_mirror_omega_tools_in_display_order() {
        let ids: Vec<&str> = PRESETS.iter().map(|p| p.id).collect();
        assert_eq!(ids, vec!["standard", "all", "repl-centric"]);
    }

    #[wasm_bindgen_test]
    #[test]
    fn presets_standard_has_twelve_tools_without_python_repl() {
        let p = &PRESETS[0];
        assert_eq!(p.id, "standard");
        assert_eq!(p.tools.len(), 12);
        assert!(!p.tools.contains(&"python_repl"));
    }

    #[wasm_bindgen_test]
    #[test]
    fn presets_all_has_thirteen_tools_with_python_repl() {
        let p = &PRESETS[1];
        assert_eq!(p.id, "all");
        assert_eq!(p.tools.len(), 13);
        assert!(p.tools.contains(&"python_repl"));
    }

    #[wasm_bindgen_test]
    #[test]
    fn presets_repl_centric_is_python_repl_plus_web_tools() {
        let p = &PRESETS[2];
        assert_eq!(p.id, "repl-centric");
        assert_eq!(p.tools, &["python_repl", "web_search", "fetch_url"]);
    }

    #[wasm_bindgen_test]
    #[test]
    fn default_tool_selection_matches_standard_preset_by_set_equality() {
        let sel = default_tool_selection();
        assert_eq!(resolve_preset(&sel), Some("standard"));
    }

    #[wasm_bindgen_test]
    #[test]
    fn resolve_preset_finds_standard_in_canonical_order() {
        let sel: Vec<String> = PRESETS[0].tools.iter().map(|s| (*s).to_owned()).collect();
        assert_eq!(resolve_preset(&sel), Some("standard"));
    }

    #[wasm_bindgen_test]
    #[test]
    fn resolve_preset_finds_standard_ignoring_order() {
        // Set equality — reversed input still matches.
        let mut sel: Vec<String> = PRESETS[0].tools.iter().map(|s| (*s).to_owned()).collect();
        sel.reverse();
        assert_eq!(resolve_preset(&sel), Some("standard"));
    }

    #[wasm_bindgen_test]
    #[test]
    fn resolve_preset_finds_all_thirteen() {
        let sel: Vec<String> = PRESETS[1].tools.iter().map(|s| (*s).to_owned()).collect();
        assert_eq!(resolve_preset(&sel), Some("all"));
    }

    #[wasm_bindgen_test]
    #[test]
    fn resolve_preset_finds_repl_centric() {
        let sel: Vec<String> = PRESETS[2].tools.iter().map(|s| (*s).to_owned()).collect();
        assert_eq!(resolve_preset(&sel), Some("repl-centric"));
    }

    #[wasm_bindgen_test]
    #[test]
    fn resolve_preset_returns_none_for_unchecking_one_tool_from_standard() {
        // Standard minus run_command — diverges from every preset.
        let sel: Vec<String> = PRESETS[0]
            .tools
            .iter()
            .filter(|t| **t != "run_command")
            .map(|s| (*s).to_owned())
            .collect();
        assert_eq!(resolve_preset(&sel), None);
    }

    #[wasm_bindgen_test]
    #[test]
    fn resolve_preset_returns_none_for_empty_selection() {
        assert_eq!(resolve_preset(&[]), None);
    }

    #[wasm_bindgen_test]
    #[test]
    fn resolve_preset_returns_none_for_superset_of_a_preset() {
        // REPL-centric plus one extra — superset, not equal, so Custom.
        let mut sel: Vec<String> = PRESETS[2].tools.iter().map(|s| (*s).to_owned()).collect();
        sel.push("run_command".into());
        assert_eq!(resolve_preset(&sel), None);
    }

    #[wasm_bindgen_test]
    #[test]
    fn parse_stored_selection_returns_standard_on_none() {
        let sel = parse_stored_selection(None);
        assert_eq!(resolve_preset(&sel), Some("standard"));
    }

    #[wasm_bindgen_test]
    #[test]
    fn parse_stored_selection_returns_standard_on_invalid_json() {
        let sel = parse_stored_selection(Some("not-json"));
        assert_eq!(resolve_preset(&sel), Some("standard"));
    }

    #[wasm_bindgen_test]
    #[test]
    fn parse_stored_selection_returns_standard_on_wrong_shape() {
        // JSON object, not an array of strings.
        let sel = parse_stored_selection(Some(r#"{"foo":"bar"}"#));
        assert_eq!(resolve_preset(&sel), Some("standard"));
    }

    #[wasm_bindgen_test]
    #[test]
    fn parse_stored_selection_returns_standard_on_empty_array() {
        // Empty selection isn't a valid UI state (≥1 tool required) — fall back.
        let sel = parse_stored_selection(Some("[]"));
        assert_eq!(resolve_preset(&sel), Some("standard"));
    }

    #[wasm_bindgen_test]
    #[test]
    fn parse_stored_selection_round_trips_repl_centric() {
        let stored = serialize_selection(
            PRESETS[2]
                .tools
                .iter()
                .map(|s| (*s).to_owned())
                .collect::<Vec<_>>()
                .as_slice(),
        );
        let sel = parse_stored_selection(Some(&stored));
        assert_eq!(resolve_preset(&sel), Some("repl-centric"));
    }

    #[wasm_bindgen_test]
    #[test]
    fn serialize_selection_emits_json_array_of_strings() {
        let s = serialize_selection(&["a".into(), "b".into()]);
        assert_eq!(s, r#"["a","b"]"#);
    }
}
