//! Client-side wire-protocol types — the server → client frame
//! ([`WsMessage`]) and the client → server frame ([`ClientFrame`]).
//!
//! ## No event mirror (§16 — Contract Authority)
//!
//! Event-carrying frames are **not** re-typed in the frontend. The
//! canonical [`omega_types::OmegaEvent`] is already a dependency (the
//! History path parses `Vec<OmegaEvent>` directly), so [`WsMessage`] is
//! an `#[serde(untagged)]` enum over exactly two arms:
//!
//! - [`WsMessage::Envelope`] — the genuinely frontend-specific frames
//!   that are **not** `OmegaEvent`s: `ready`, `reset_done`, the
//!   `agent_error` *transport* envelope, `session_info`, `history`,
//!   `session_deleted`/`session_renamed`, `pending_changes_warning`,
//!   `monitor_roster`, `input_queue`, and the streaming signal deltas
//!   (`text`, `thinking`, `thinking_block_complete`,
//!   `tool_use_block_start`, `tool_input`). These live in the small,
//!   hand-written [`WsEnvelope`] enum.
//! - [`WsMessage::Event`] — a *transparent* [`OmegaEvent`]. The server
//!   forwards events generically
//!   (`omega-server::ws_message` → `serde_json::to_value(item)`), so
//!   every variant in `omega-types` reaches the UI with **zero**
//!   frontend edits. Adding a variant upstream can no longer drift the
//!   frontend out of sync — the mirror is gone.
//!
//! The drift-guard test (`drift_guard_*`, bottom of this file)
//! round-trips *every* `OmegaEvent` variant through the server
//! serialization → this parse → back to `OmegaEvent`, and fails to
//! compile if a future variant is added without a sample.
//!
//! ## Tag-collision check (envelope vs event)
//!
//! Under `#[serde(untagged)]` the `Envelope` arm is tried first and the
//! `Event` arm second. The only `type` tag shared by both layers is
//! `agent_error`:
//!
//! - **Envelope** — `{ "type": "agent_error", "message": "..." }` —
//!   a transport/handler error ([`WsEnvelope::AgentError`]). It requires
//!   a `message` field.
//! - **Event** — `{ "type": "agent_error", "time": "...", "error": "..." }`
//!   — the forwarded `OmegaEvent::AgentError`. It has no `message`, so
//!   the `Envelope` arm fails and parsing falls through to the
//!   transparent `Event` arm. The field sets are disjoint, so dispatch
//!   is deterministic. No other envelope/signal tag collides with any
//!   `OmegaEvent` tag.

use omega_types::OmegaEvent;
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
    HaltRequested,
    Halted,
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

// ---------------------------------------------------------------------------
// Server → client: WsEnvelope (the thin, frontend-specific frames)
// ---------------------------------------------------------------------------

/// The genuinely frontend-specific frames — everything the server emits
/// that is **not** an [`OmegaEvent`]. This is the only hand-written
/// per-variant enum left in the frontend protocol; the event-carrying
/// frames are deserialized straight into [`OmegaEvent`] (see
/// [`WsMessage`]).
///
/// The discriminator is the `type` field. Stream-signal frames (`text`,
/// `thinking`, …) and ephemeral snapshots (`monitor_roster`,
/// `input_queue`) live here because they are transport/UI concerns with
/// no persisted `OmegaEvent` backing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsEnvelope {
    /// Server is ready to receive client frames.
    Ready,
    /// Acknowledgement of a `reset` client frame.
    ResetDone,
    /// A transport/handler-level error (malformed client frame, missing
    /// session, …). Distinct from `OmegaEvent::AgentError`, which is the
    /// *agent-level* error written to `events.jsonl` and routed through
    /// [`WsMessage::Event`]. Disambiguated by the presence of `message`
    /// (this) vs `time`+`error` (the event) — see the module docs.
    AgentError { message: String },
    /// Session identity announcement.
    SessionInfo(SessionInfoPayload),
    /// Persisted history batch (carries `Vec<OmegaEvent>` directly).
    History(HistoryPayload),
    /// A session directory was deleted on disk.
    #[serde(rename_all = "camelCase")]
    SessionDeleted { session_dir: String },
    /// A session was renamed on disk.
    #[serde(rename_all = "camelCase")]
    SessionRenamed { session_dir: String, name: String },
    /// A `Reset` or `ResumeSession` frame was rejected because the
    /// working tree has uncommitted git changes and `allow_dirty` was
    /// not set.  `intent` echoes the original parameters so the client
    /// can re-issue with `allow_dirty: true` on operator confirmation.
    PendingChangesWarning { intent: PendingChangesIntent },
    /// Ephemeral roster snapshot pushed by the server on connect and
    /// after every monitor lifecycle event.  **Not** an `OmegaEvent`;
    /// never written to `events.jsonl`.
    MonitorRoster { monitors: Vec<MonitorRosterEntry> },
    /// Ephemeral input-queue snapshot pushed by the server after a human
    /// message is enqueued / drained, and on connect / reset / resume.
    /// **Not** an `OmegaEvent`.
    InputQueue { items: Vec<InputQueueItem> },

    // --- Forwarded `StreamSignal` payloads -----------------------------------
    /// Streaming assistant text fragment.  `index` matches Anthropic's
    /// `content_block_start.index` so the per-block streaming buffer
    /// can route deltas to the correct slot (SCHEMA-8 Phase 5a).
    Text { index: usize, text: String },
    /// Streaming thinking-block fragment.  `index` carries the same
    /// semantics as on [`Self::Text`].
    Thinking { index: usize, text: String },
    /// End-of-thinking-block marker (carries cryptographic signature).
    /// The UI ignores the signature; `index` lets `apply` drop the
    /// matching slot from the thinking streaming buffer.
    ThinkingBlockComplete { index: usize, signature: String },
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
    ToolInput { index: usize, partial_json: String },
}

// ---------------------------------------------------------------------------
// Server → client: WsMessage
// ---------------------------------------------------------------------------

/// One frame received over the WebSocket from `omega-server`.
///
/// `#[serde(untagged)]` over two arms (§16 — the event mirror is gone):
///
/// - [`WsMessage::Envelope`] — the hand-written [`WsEnvelope`] frames
///   (tried first).
/// - [`WsMessage::Event`] — a *transparent* [`OmegaEvent`]; any frame
///   whose `type` is not a known envelope tag is parsed as the canonical
///   event. Adding a variant to `omega-types` reaches the UI with no
///   edit here.
///
/// Tag-collision is impossible by construction: the only shared `type`
/// is `agent_error`, and the envelope arm requires a `message` field the
/// event form lacks, so the event falls through to the `Event` arm. See
/// the module docs and the `drift_guard_*` / `agent_error_*` tests.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WsMessage {
    /// A frontend-specific envelope / signal / ephemeral frame.
    Envelope(WsEnvelope),
    /// A canonical, persisted `OmegaEvent` forwarded by the server.
    Event(OmegaEvent),
}

impl From<WsEnvelope> for WsMessage {
    fn from(env: WsEnvelope) -> Self {
        Self::Envelope(env)
    }
}

impl From<OmegaEvent> for WsMessage {
    fn from(event: OmegaEvent) -> Self {
        Self::Event(event)
    }
}

// ---------------------------------------------------------------------------
// MonitorRosterEntry — one entry in a roster snapshot
// ---------------------------------------------------------------------------

/// One monitor entry in a [`WsEnvelope::MonitorRoster`] snapshot.
///
/// Transport-only projection of
/// [`omega_tools::MonitorInfo`](crates/omega-tools/src/monitors.rs);
/// field names are camelCase on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorRosterEntry {
    /// Stable per-session monitor id.
    pub id: String,
    /// Human description supplied to `monitor()`.
    pub description: String,
    /// The shell command being run.
    pub command: String,
    /// `"running"` or `"stopped"`.
    pub status: String,
    /// RFC 3339 start timestamp.
    pub started_at: String,
    /// Number of stdout lines delivered so far.
    pub fired_count: u64,
    /// Up to 20 most recent stderr lines.
    pub stderr_tail: Vec<String>,
}

// ---------------------------------------------------------------------------
// InputQueueItem — one entry in a queue snapshot
// ---------------------------------------------------------------------------

/// One pending item in a [`WsEnvelope::InputQueue`] snapshot.
///
/// Transport-only projection — never written to `events.jsonl`.
/// Structured to admit `"monitor:<id>"` sources in U2.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputQueueItem {
    /// Who queued this item.  Currently always `"human"`; will gain
    /// `"monitor:<id>"` variants in U2.
    pub source: String,
    /// First 120 characters of the content, for display.
    pub content_preview: String,
    /// RFC 3339 timestamp (millisecond precision) when the item was pushed.
    pub enqueued_at: String,
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
    /// Halt: §15 U3 — stop advancing at the next seam and WAIT (so the
    /// operator can compose a steering message at leisure).  Distinct from
    /// `Abort` (which cancels the in-flight block immediately).
    Halt,
    /// Resume a halted turn with **no** new input ("never mind, carry on").
    /// To resume *with* a steering message, send `UserMessage` instead —
    /// it wakes the halt seam and is injected before the loop continues.
    Resume,
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
// Tool-selection — single source of truth in omega-types
// ---------------------------------------------------------------------------

// The canonical [`Preset`] type, [`PRESETS`] registry, and all pure
// selection helpers now live in `omega-types::tools` — the only crate
// that is both wasm-safe (no native deps) and shared between the native
// backend and the wasm frontend.
//
// We re-export them here so that all picker.rs / sessions.rs call sites
// using `crate::protocol::{Preset, PRESETS, …}` keep compiling without
// change.
pub use omega_types::tools::{
    PRESETS, Preset, default_tool_selection, parse_stored_selection, resolve_preset,
    serialize_selection,
};

/// localStorage key persisting the operator's last tool selection.
/// Restored as the initial selection on the next "+ New session".
pub const TOOL_SELECTION_STORAGE_KEY: &str = "omega.toolSelection.lastChoice";

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

    /// Match an [`WsMessage::Envelope`] arm or panic with the wrong variant.
    fn envelope(msg: WsMessage) -> WsEnvelope {
        match msg {
            WsMessage::Envelope(env) => env,
            WsMessage::Event(ev) => panic!("expected envelope, got event: {ev:?}"),
        }
    }

    // ---- envelope -----------------------------------------------------------

    #[wasm_bindgen_test]
    fn ready_round_trips() {
        assert!(matches!(
            envelope(parse(r#"{"type":"ready"}"#)),
            WsEnvelope::Ready
        ));
    }

    #[wasm_bindgen_test]
    fn reset_done_round_trips() {
        assert!(matches!(
            envelope(parse(r#"{"type":"reset_done"}"#)),
            WsEnvelope::ResetDone
        ));
    }

    #[wasm_bindgen_test]
    fn session_deleted_camel_case_dir() {
        match envelope(parse(r#"{"type":"session_deleted","sessionDir":"abc"}"#)) {
            WsEnvelope::SessionDeleted { session_dir } => assert_eq!(session_dir, "abc"),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[wasm_bindgen_test]
    fn session_renamed_carries_dir_and_name() {
        let m = parse(r#"{"type":"session_renamed","sessionDir":"d","name":"my-name"}"#);
        match envelope(m) {
            WsEnvelope::SessionRenamed { session_dir, name } => {
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
        match envelope(parse(json)) {
            WsEnvelope::SessionInfo(p) => {
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
        match envelope(parse(json)) {
            WsEnvelope::SessionInfo(p) => {
                assert_eq!(p.turn_state, TurnState::Idle);
                assert!(!p.has_pending_changes);
                assert!(p.name.is_none());
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[wasm_bindgen_test]
    fn history_with_streaming_field() {
        match envelope(parse(r#"{"type":"history","events":[],"streaming":true}"#)) {
            WsEnvelope::History(p) => {
                assert!(p.events.is_empty());
                assert!(p.streaming);
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[wasm_bindgen_test]
    fn history_without_streaming_field_defaults_to_false() {
        match envelope(parse(r#"{"type":"history","events":[]}"#)) {
            WsEnvelope::History(p) => assert!(!p.streaming),
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
        match envelope(parse(json)) {
            WsEnvelope::History(p) => {
                assert_eq!(p.events.len(), 1);
                assert!(matches!(p.events[0], OmegaEvent::ServerStarted(_)));
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    // ---- agent_error disambiguation (envelope vs forwarded event) ----------

    #[wasm_bindgen_test]
    fn agent_error_envelope_disambiguates_by_message_field() {
        // `{message}` only → the transport envelope, NOT an OmegaEvent.
        match envelope(parse(r#"{"type":"agent_error","message":"bad frame"}"#)) {
            WsEnvelope::AgentError { message } => assert_eq!(message, "bad frame"),
            other => panic!("expected envelope agent_error, got {other:?}"),
        }
    }

    #[wasm_bindgen_test]
    fn agent_error_event_falls_through_to_transparent_event_arm() {
        // `{time, error}` (no `message`) → the canonical OmegaEvent, reached
        // via the transparent `Event` arm. This is the tag-collision case the
        // module docs call out: envelope tried first, fails on the missing
        // `message`, parsing falls through to `Event`.
        let json = r#"{"type":"agent_error","time":"2024-01-01T00:00:00.000Z","error":"oops"}"#;
        match parse(json) {
            WsMessage::Event(OmegaEvent::AgentError(e)) => {
                assert_eq!(e.error, "oops");
                assert_eq!(e.time, "2024-01-01T00:00:00.000Z");
            }
            other => panic!("expected Event(AgentError), got {other:?}"),
        }
    }

    // ---- stream signals -----------------------------------------------------

    #[wasm_bindgen_test]
    fn text_signal_round_trips() {
        match envelope(parse(r#"{"type":"text","index":0,"text":"hello"}"#)) {
            WsEnvelope::Text { index, text } => {
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
        match envelope(parse(r#"{"type":"text","index":3,"text":"world"}"#)) {
            WsEnvelope::Text { index, text } => {
                assert_eq!(index, 3);
                assert_eq!(text, "world");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[wasm_bindgen_test]
    fn thinking_signal_round_trips() {
        match envelope(parse(r#"{"type":"thinking","index":1,"text":"musing"}"#)) {
            WsEnvelope::Thinking { index, text } => {
                assert_eq!(index, 1);
                assert_eq!(text, "musing");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[wasm_bindgen_test]
    fn thinking_block_complete_carries_index_and_signature() {
        let m = parse(r#"{"type":"thinking_block_complete","index":2,"signature":"sig"}"#);
        match envelope(m) {
            WsEnvelope::ThinkingBlockComplete { index, signature } => {
                assert_eq!(index, 2);
                assert_eq!(signature, "sig");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[wasm_bindgen_test]
    fn tool_use_block_start_round_trips() {
        let m = parse(
            r#"{"type":"tool_use_block_start","index":3,"tool_use_id":"tu_1","name":"bash"}"#,
        );
        match envelope(m) {
            WsEnvelope::ToolUseBlockStart {
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
        match envelope(parse(
            r#"{"type":"tool_input","index":3,"partial_json":"{cmd:"}"#,
        )) {
            WsEnvelope::ToolInput {
                index,
                partial_json,
            } => {
                assert_eq!(index, 3);
                assert_eq!(partial_json, "{cmd:");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    // ---- forwarded events route to the transparent Event arm ----------------

    #[wasm_bindgen_test]
    fn user_message_event_routes_to_event_arm() {
        let json = r#"{"type":"user_message","time":"2024-01-01T00:00:00.000Z","content":"hi"}"#;
        match parse(json) {
            WsMessage::Event(OmegaEvent::UserMessage(e)) => assert_eq!(e.content, "hi"),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[wasm_bindgen_test]
    fn turn_end_event_routes_to_event_arm() {
        let json = r#"{
            "type":"turn_end","time":"2024-01-01T00:00:00.000Z",
            "metrics":{"inputTokens":1,"outputTokens":2}
        }"#;
        match parse(json) {
            WsMessage::Event(OmegaEvent::TurnEnd(e)) => assert_eq!(e.metrics.output_tokens, 2),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    // ---- live monitor path (the §16 bug) -----------------------------------

    #[wasm_bindgen_test]
    fn monitor_delivery_frame_routes_to_event_not_dropped() {
        // Regression for §16: a `monitor_delivery` frame arriving LIVE used
        // to fail to deserialise into the old per-event mirror and was
        // silently dropped. It must now reach the transparent Event arm so
        // the existing feed renderer shows it without a reload.
        let json = r#"{"type":"monitor_delivery","time":"2024-01-01T00:00:00.000Z",
            "items":[{"monitorId":"m1","lines":["hello","world"]}]}"#;
        match parse(json) {
            WsMessage::Event(OmegaEvent::MonitorDelivery(e)) => {
                assert_eq!(e.items.len(), 1);
                assert_eq!(e.items[0].monitor_id, "m1");
                assert_eq!(
                    e.items[0].lines,
                    vec!["hello".to_owned(), "world".to_owned()]
                );
            }
            other => panic!("monitor_delivery dropped / mis-routed: {other:?}"),
        }
    }

    #[wasm_bindgen_test]
    fn unparseable_frame_fails_loudly() {
        // An unknown `type` matches neither the envelope tags nor any
        // OmegaEvent tag, so parsing must ERROR. `ws.rs` surfaces this error
        // visibly (store error state) instead of silently dropping it.
        let r: Result<WsMessage, _> =
            serde_json::from_str(r#"{"type":"totally_unknown_frame","x":1}"#);
        assert!(r.is_err(), "unknown frame must fail to parse");
    }

    // ---- drift guard: every OmegaEvent variant round-trips -----------------

    /// Compile-time exhaustiveness guard. Every [`OmegaEvent`] variant must
    /// map to its wire tag here. Adding a variant to `omega-types` WITHOUT
    /// updating this match is a **compile error** — which is exactly the
    /// signal that forces a matching sample into [`drift_guard_samples`].
    /// This is the mechanism that makes a future omission impossible to
    /// merge silently (§16 drift guard).
    fn variant_tag(ev: &OmegaEvent) -> &'static str {
        match ev {
            OmegaEvent::SessionStarted(_) => "session_started",
            OmegaEvent::ServerStarted(_) => "server_started",
            OmegaEvent::ServerStopped(_) => "server_stopped",
            OmegaEvent::UserMessage(_) => "user_message",
            OmegaEvent::LlmCall(_) => "llm_call",
            OmegaEvent::ToolCall(_) => "tool_call",
            OmegaEvent::ToolResult(_) => "tool_result",
            OmegaEvent::TurnEnd(_) => "turn_end",
            OmegaEvent::LlmError(_) => "llm_error",
            OmegaEvent::AgentError(_) => "agent_error",
            OmegaEvent::TurnInterrupted(_) => "turn_interrupted",
            OmegaEvent::LlmRetry(_) => "llm_retry",
            OmegaEvent::ModelChanged(_) => "model_changed",
            OmegaEvent::EffortChanged(_) => "effort_changed",
            OmegaEvent::TransportError(_) => "transport_error",
            OmegaEvent::ResumingSession(_) => "resuming_session",
            OmegaEvent::SessionResumed(_) => "session_resumed",
            OmegaEvent::HaltRequested(_) => "halt_requested",
            OmegaEvent::TurnHalted(_) => "turn_halted",
            OmegaEvent::TurnResumed(_) => "turn_resumed",
            OmegaEvent::LlmResponseStarted(_) => "llm_response_started",
            OmegaEvent::LlmResponseEnded(_) => "llm_response_ended",
            OmegaEvent::LlmResponseDiscarded(_) => "llm_response_discarded",
            OmegaEvent::TextBlock(_) => "text_block",
            OmegaEvent::ThinkingBlock(_) => "thinking_block",
            OmegaEvent::ToolUseBlock(_) => "tool_use_block",
            OmegaEvent::ContextCompacted(_) => "context_compacted",
            OmegaEvent::PythonReplBootstrapped(_) => "python_repl_bootstrapped",
            OmegaEvent::HarnessRecovery(_) => "harness_recovery",
            OmegaEvent::MonitorStarted(_) => "monitor_started",
            OmegaEvent::MonitorDelivery(_) => "monitor_delivery",
            OmegaEvent::MonitorStderr(_) => "monitor_stderr",
            OmegaEvent::MonitorStopped(_) => "monitor_stopped",
        }
    }

    /// One canonical wire-form sample per [`OmegaEvent`] variant, in the
    /// camelCase shape the server emits. Built by deserialising the JSON the
    /// server would send so the round-trip exercises the real wire format.
    fn drift_guard_samples() -> Vec<OmegaEvent> {
        const SAMPLES: &[&str] = &[
            r#"{"type":"session_started","time":"t","sessionId":"00000000-0000-0000-0000-000000000000","path":"p","model":"m","effort":"e","systemPrompt":"sp","toolSelection":[]}"#,
            r#"{"type":"server_started","time":"t"}"#,
            r#"{"type":"server_stopped","time":"t","outcome":"clean"}"#,
            r#"{"type":"user_message","time":"t","content":"c"}"#,
            r#"{"type":"llm_call","time":"t","url":"u","model":"m","contextHashes":[],"cacheBreakpointIndex":null,"requestBytes":0}"#,
            r#"{"type":"tool_call","time":"t","toolCallId":"tc","name":"n","input":{},"contextHash":"h"}"#,
            r#"{"type":"tool_result","time":"t","toolCallId":"tc","name":"n","isError":false,"durationMs":1,"output":"o"}"#,
            r#"{"type":"turn_end","time":"t","metrics":{"inputTokens":1,"outputTokens":2}}"#,
            r#"{"type":"llm_error","time":"t","url":"u","error":"e"}"#,
            r#"{"type":"agent_error","time":"t","error":"e"}"#,
            r#"{"type":"turn_interrupted","time":"t"}"#,
            r#"{"type":"llm_retry","time":"t","attempt":1,"waitMs":100,"error":"e"}"#,
            r#"{"type":"model_changed","time":"t","model":"m"}"#,
            r#"{"type":"effort_changed","time":"t","effort":"e"}"#,
            r#"{"type":"transport_error","time":"t","error":"e"}"#,
            r#"{"type":"resuming_session","time":"t","resumedFrom":"r","basis":"b"}"#,
            r#"{"type":"session_resumed","time":"t","resumedFrom":"r","summary":"s"}"#,
            r#"{"type":"halt_requested","time":"t"}"#,
            r#"{"type":"turn_halted","time":"t"}"#,
            r#"{"type":"turn_resumed","time":"t"}"#,
            r#"{"type":"llm_response_started","time":"t"}"#,
            r#"{"type":"llm_response_ended","time":"t","stopReason":"end_turn","usage":{"input_tokens":1,"output_tokens":2},"contextHash":"h"}"#,
            r#"{"type":"llm_response_discarded","time":"t"}"#,
            r#"{"type":"text_block","time":"t","text":"x","partial":false}"#,
            r#"{"type":"thinking_block","time":"t","thinking":"x","partial":false}"#,
            r#"{"type":"tool_use_block","time":"t","toolCallId":"tc","toolUseId":"tu","name":"n","input":{},"partial":false}"#,
            r#"{"type":"context_compacted","time":"t","tokensBefore":1,"tokensAfter":2,"summaryTokens":3}"#,
            r#"{"type":"python_repl_bootstrapped","time":"t","durationMs":1,"success":true,"stderrExcerpt":""}"#,
            r#"{"type":"harness_recovery","time":"t","kind":"empty_response_continuation","content":"c"}"#,
            r#"{"type":"monitor_started","id":"m1","description":"d","command":"c","time":"t"}"#,
            r#"{"type":"monitor_delivery","time":"t","items":[{"monitorId":"m1","lines":["l"]}]}"#,
            r#"{"type":"monitor_stderr","id":"m1","chunk":"c","time":"t"}"#,
            r#"{"type":"monitor_stopped","id":"m1","reason":"process_exited","time":"t"}"#,
        ];
        SAMPLES
            .iter()
            .map(|j| serde_json::from_str(j).unwrap_or_else(|e| panic!("drift sample {j}: {e}")))
            .collect()
    }

    #[wasm_bindgen_test]
    fn drift_guard_every_omega_event_variant_round_trips_through_wsmessage() {
        let samples = drift_guard_samples();
        // One sample per variant. `variant_tag`'s match is the compile-time
        // guard; this count is the runtime reminder to add the sample too.
        assert_eq!(
            samples.len(),
            33,
            "add a drift-guard sample for the new OmegaEvent variant"
        );
        let mut seen = std::collections::BTreeSet::new();
        for ev in samples {
            let tag = variant_tag(&ev);
            assert!(seen.insert(tag), "duplicate drift sample for tag {tag}");
            // The server forwards events generically:
            //   omega-server::ws_message → serde_json::to_value(item).
            let wire = serde_json::to_value(&ev)
                .unwrap_or_else(|e| panic!("{tag} failed to serialize: {e}"));
            // The frontend parse path.
            let parsed: WsMessage = serde_json::from_value(wire)
                .unwrap_or_else(|e| panic!("{tag} failed to parse into WsMessage: {e}"));
            match parsed {
                WsMessage::Event(ev2) => {
                    assert_eq!(variant_tag(&ev2), tag, "{tag} routed to the wrong variant");
                    assert_eq!(ev2, ev, "{tag} did not round-trip by value");
                }
                WsMessage::Envelope(env) => {
                    panic!("{tag} mis-routed to the envelope arm: {env:?}")
                }
            }
        }
        assert_eq!(seen.len(), 33, "all 33 variant tags must be distinct");
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
    fn client_frame_halt_serialises_with_snake_case_tag() {
        let frame = ClientFrame::Halt;
        let json = serde_json::to_string(&frame).unwrap();
        assert_eq!(json, r#"{"type":"halt"}"#);
    }

    #[wasm_bindgen_test]
    fn client_frame_resume_serialises_with_snake_case_tag() {
        let frame = ClientFrame::Resume;
        let json = serde_json::to_string(&frame).unwrap();
        assert_eq!(json, r#"{"type":"resume"}"#);
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
    fn client_frame_reset_emits_allow_dirty_when_true() {
        // Guards `is_false`: a `true` flag MUST survive serialization. If
        // `is_false` ever returned `true` unconditionally the field would be
        // skipped here and this assertion would fail.
        let frame = ClientFrame::Reset {
            model: None,
            effort: None,
            allow_dirty: true,
            tool_selection: None,
        };
        let json = serde_json::to_string(&frame).unwrap();
        assert_eq!(json, r#"{"type":"reset","allowDirty":true}"#);
    }

    #[wasm_bindgen_test]
    fn client_frame_reset_omits_allow_dirty_when_false() {
        // Guards `is_false`: a `false` flag MUST be omitted. If `is_false`
        // returned `false` unconditionally the field would leak onto the wire.
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
    fn client_frame_resume_session_emits_allow_dirty_when_true() {
        let frame = ClientFrame::ResumeSession {
            session_dir: "abc".into(),
            allow_dirty: true,
        };
        let json = serde_json::to_string(&frame).unwrap();
        assert_eq!(
            json,
            r#"{"type":"resume_session","sessionDir":"abc","allowDirty":true}"#
        );
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
    fn presets_standard_has_fourteen_tools_without_python_repl() {
        let p = &PRESETS[0];
        assert_eq!(p.id, "standard");
        assert_eq!(p.tools.len(), 14);
        assert!(!p.tools.contains(&"python_repl"));
        // monitors are now in the standard/default set
        assert!(p.tools.contains(&"monitor"));
        assert!(p.tools.contains(&"stop_monitor"));
    }

    #[wasm_bindgen_test]
    #[test]
    fn presets_all_has_fifteen_tools_with_python_repl() {
        let p = &PRESETS[1];
        assert_eq!(p.id, "all");
        assert_eq!(p.tools.len(), 15);
        assert!(p.tools.contains(&"python_repl"));
        assert!(p.tools.contains(&"monitor"));
    }

    #[wasm_bindgen_test]
    #[test]
    fn presets_repl_centric_has_five_tools() {
        let p = &PRESETS[2];
        assert_eq!(p.id, "repl-centric");
        assert_eq!(p.tools.len(), 5);
        assert!(p.tools.contains(&"python_repl"));
        assert!(p.tools.contains(&"web_search"));
        assert!(p.tools.contains(&"fetch_url"));
        assert!(p.tools.contains(&"monitor"));
        assert!(p.tools.contains(&"stop_monitor"));
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
    fn resolve_preset_finds_all_fifteen() {
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
