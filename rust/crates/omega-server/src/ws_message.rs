//! Server→client WebSocket message envelope.
//!
//! Phase 1e.2 replaces the placeholder `serde_json::Value` element type
//! used by [`ActiveSession::ws_tx`](crate::session::ActiveSession::ws_tx)
//! with this concrete enum.  Three wire shapes:
//!
//! - [`WsMessage::Ready`]            → `{"type":"ready"}`
//!   Sent after the upgrade handshake completes and again after a
//!   client-issued `reset` swaps the session slot.
//! - [`WsMessage::AgentError(msg)`]  → `{"type":"agent_error","message":msg}`
//!   Surfaces handler-level errors (malformed client frame, missing
//!   session, etc.) without closing the socket.
//! - [`WsMessage::Item`]             → forwarded `AgentItem` (signal or event),
//!   serialised verbatim because [`AgentItem`] is `#[serde(untagged)]`.
//!
//! The TS server's `broadcast()` helper takes the same three shapes; this
//! module is the Rust port of `src/web/server.ts`'s wire-construction
//! helpers.

use omega_core::AgentItem;
use omega_protocol::OmegaEvent;

/// One WebSocket frame the server can emit.
///
/// Constructed by the request handler, sent through the per-connection
/// `mpsc::UnboundedSender<WsMessage>` and serialised by the writer task.
#[derive(Debug)]
pub enum WsMessage {
    /// Server is ready to receive client frames (post-handshake or post-reset).
    Ready,
    /// Handler-level error surfaced to the client without closing the socket.
    AgentError(String),
    /// Forwarded agent item (`StreamSignal` or `OmegaEvent`).
    Item(Box<AgentItem>),
    /// Session identity announcement, sent before the history batch.
    /// Mirrors the TS server's `buildSessionInfo()` output.
    SessionInfo {
        /// Session directory name (basename of the session dir).
        dir: String,
        /// Active model id.
        model: String,
        /// Active thinking-effort level.
        effort: String,
        /// Server working directory.
        cwd: String,
        /// Optional human-readable session name; omitted when `None`.
        name: Option<String>,
        /// Current derived turn state (`idle` / `running` / `pause_requested` / `paused`).
        /// Projected to the JSON key `turnState` to match the TS contract.
        turn_state: String,
        /// Whether the working tree had uncommitted changes when this session
        /// was created.  Always present on the wire as `hasPendingChanges`.
        has_pending_changes: bool,
    },
    /// Persisted history batch sent on connect / reset / resume.
    /// `streaming` is omitted on the wire when `false` — matches the TS
    /// server's `...(isStreaming ? { streaming: true } : {})` pattern.
    History {
        /// Filtered persisted events for the current session.
        events: Vec<OmegaEvent>,
        /// True if a turn is in progress; the field is dropped when false.
        streaming: bool,
    },
    /// Acknowledgement that a `reset` client frame has been processed.
    ResetDone,
    /// Acknowledgement that a session directory has been deleted on disk.
    /// Mirrors the TS server's `{ type: "session_deleted", sessionDir }` frame.
    SessionDeleted {
        /// Directory name (basename) of the deleted session.
        session_dir: String,
    },
    /// Broadcast after a successful rename, so the client can update its
    /// display name without a full session reload.
    /// Mirrors the TS server's `{ type: "session_renamed", sessionDir, name }` frame.
    SessionRenamed {
        /// Directory name (basename) of the renamed session.
        session_dir: String,
        /// New human-readable name written into `session.jsonc`.
        name: String,
    },
}

impl WsMessage {
    /// Project this message to its wire JSON shape.
    ///
    /// Pure function so it can be exercised by direct unit tests; the
    /// writer task only ever calls [`Self::to_text`].
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Self::Ready => serde_json::json!({ "type": "ready" }),
            Self::AgentError(message) => serde_json::json!({
                "type": "agent_error",
                "message": message,
            }),
            Self::Item(item) => serde_json::to_value(item.as_ref()).unwrap_or_else(|_| {
                serde_json::json!({
                    "type": "agent_error",
                    "message": "internal: failed to serialise agent item",
                })
            }),
            Self::SessionInfo {
                dir,
                model,
                effort,
                cwd,
                name,
                turn_state,
                has_pending_changes,
            } => {
                let mut obj = serde_json::Map::with_capacity(8);
                obj.insert("type".to_owned(), serde_json::Value::from("session_info"));
                obj.insert("dir".to_owned(), serde_json::Value::from(dir.clone()));
                obj.insert("model".to_owned(), serde_json::Value::from(model.clone()));
                obj.insert("effort".to_owned(), serde_json::Value::from(effort.clone()));
                obj.insert("cwd".to_owned(), serde_json::Value::from(cwd.clone()));
                obj.insert(
                    "turnState".to_owned(),
                    serde_json::Value::from(turn_state.clone()),
                );
                obj.insert(
                    "hasPendingChanges".to_owned(),
                    serde_json::Value::from(*has_pending_changes),
                );
                if let Some(n) = name {
                    obj.insert("name".to_owned(), serde_json::Value::from(n.clone()));
                }
                serde_json::Value::Object(obj)
            }
            Self::History { events, streaming } => {
                let events_value = serde_json::to_value(events)
                    .unwrap_or_else(|_| serde_json::Value::Array(Vec::new()));
                let mut obj = serde_json::Map::with_capacity(3);
                obj.insert("type".to_owned(), serde_json::Value::from("history"));
                obj.insert("events".to_owned(), events_value);
                if *streaming {
                    obj.insert("streaming".to_owned(), serde_json::Value::from(true));
                }
                serde_json::Value::Object(obj)
            }
            Self::ResetDone => serde_json::json!({ "type": "reset_done" }),
            Self::SessionDeleted { session_dir } => serde_json::json!({
                "type": "session_deleted",
                "sessionDir": session_dir,
            }),
            Self::SessionRenamed { session_dir, name } => serde_json::json!({
                "type": "session_renamed",
                "sessionDir": session_dir,
                "name": name,
            }),
        }
    }

    /// Compact JSON string, ready to ship as a `Message::Text` frame.
    #[must_use]
    pub fn to_text(&self) -> String {
        self.to_json().to_string()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

    use super::WsMessage;
    use omega_core::AgentItem;
    use omega_protocol::{StreamSignal, events::TurnEndEvent, events::TurnMetrics};

    #[test]
    fn ready_serialises_to_type_ready_only() {
        let v = WsMessage::Ready.to_json();
        assert_eq!(v, serde_json::json!({ "type": "ready" }));
        assert_eq!(v.as_object().unwrap().len(), 1, "no extra fields");
    }

    #[test]
    fn ready_to_text_matches_to_json() {
        let m = WsMessage::Ready;
        assert_eq!(m.to_text(), m.to_json().to_string());
    }

    #[test]
    fn agent_error_carries_message_field() {
        let v = WsMessage::AgentError("boom".to_owned()).to_json();
        assert_eq!(
            v,
            serde_json::json!({ "type": "agent_error", "message": "boom" })
        );
    }

    #[test]
    fn agent_error_preserves_message_content_verbatim() {
        // Includes characters that JSON must escape: quote, backslash, newline.
        let raw = "a \"b\" \\ c\nd";
        let v = WsMessage::AgentError(raw.to_owned()).to_json();
        assert_eq!(v["message"].as_str().unwrap(), raw);
    }

    #[test]
    fn agent_error_serialised_text_is_valid_json_with_message_intact() {
        let raw = "a \"b\" \\ c\nd";
        let m = WsMessage::AgentError(raw.to_owned());
        let parsed: serde_json::Value = serde_json::from_str(&m.to_text()).unwrap();
        assert_eq!(parsed["type"], "agent_error");
        assert_eq!(parsed["message"], raw);
    }

    #[test]
    fn item_signal_text_serialises_with_type_text() {
        let sig = AgentItem::Signal(StreamSignal::Text {
            text: "hello".to_owned(),
        });
        let v = WsMessage::Item(Box::new(sig)).to_json();
        assert_eq!(v["type"], "text");
        assert_eq!(v["text"], "hello");
    }

    #[test]
    fn item_signal_thinking_serialises_with_type_thinking() {
        let sig = AgentItem::Signal(StreamSignal::Thinking {
            text: "musing".to_owned(),
        });
        let v = WsMessage::Item(Box::new(sig)).to_json();
        assert_eq!(v["type"], "thinking");
        assert_eq!(v["text"], "musing");
    }

    #[test]
    fn session_info_serialises_with_required_fields_only_when_name_absent() {
        let v = WsMessage::SessionInfo {
            dir: "2025-01-01T00-00-00-000-deadbeef".to_owned(),
            model: "claude-sonnet-4-6".to_owned(),
            effort: "medium".to_owned(),
            cwd: "/tmp".to_owned(),
            name: None,
            turn_state: "idle".to_owned(),
            has_pending_changes: false,
        }
        .to_json();
        assert_eq!(v["type"], "session_info");
        assert_eq!(v["dir"], "2025-01-01T00-00-00-000-deadbeef");
        assert_eq!(v["model"], "claude-sonnet-4-6");
        assert_eq!(v["effort"], "medium");
        assert_eq!(v["cwd"], "/tmp");
        let obj = v.as_object().unwrap();
        assert!(!obj.contains_key("name"), "name must be omitted when None");
        assert_eq!(obj.len(), 7, "unexpected extra fields: {obj:?}");
        assert_eq!(obj["turnState"], "idle");
        assert_eq!(obj["hasPendingChanges"], false);
    }

    #[test]
    fn session_info_has_pending_changes_true_appears_on_wire() {
        let v = WsMessage::SessionInfo {
            dir: "d".to_owned(),
            model: "m".to_owned(),
            effort: "e".to_owned(),
            cwd: "/c".to_owned(),
            name: None,
            turn_state: "idle".to_owned(),
            has_pending_changes: true,
        }
        .to_json();
        assert_eq!(v["hasPendingChanges"], true);
    }

    #[test]
    fn session_info_includes_name_when_some() {
        let v = WsMessage::SessionInfo {
            dir: "d".to_owned(),
            model: "m".to_owned(),
            effort: "e".to_owned(),
            cwd: "/c".to_owned(),
            name: Some("my-session".to_owned()),
            turn_state: "running".to_owned(),
            has_pending_changes: false,
        }
        .to_json();
        assert_eq!(v["name"], "my-session");
    }

    #[test]
    fn session_info_text_round_trips_through_json() {
        let m = WsMessage::SessionInfo {
            dir: "d".to_owned(),
            model: "m".to_owned(),
            effort: "e".to_owned(),
            cwd: "/c".to_owned(),
            name: Some("n".to_owned()),
            turn_state: "idle".to_owned(),
            has_pending_changes: false,
        };
        let parsed: serde_json::Value = serde_json::from_str(&m.to_text()).unwrap();
        assert_eq!(parsed, m.to_json());
    }

    #[test]
    fn history_omits_streaming_field_when_false() {
        let v = WsMessage::History {
            events: Vec::new(),
            streaming: false,
        }
        .to_json();
        assert_eq!(v["type"], "history");
        assert_eq!(v["events"], serde_json::json!([]));
        let obj = v.as_object().unwrap();
        assert!(
            !obj.contains_key("streaming"),
            "streaming must be omitted when false",
        );
        assert_eq!(obj.len(), 2);
    }

    #[test]
    fn history_includes_streaming_true() {
        let v = WsMessage::History {
            events: Vec::new(),
            streaming: true,
        }
        .to_json();
        assert_eq!(v["streaming"], true);
    }

    #[test]
    fn history_serialises_each_event_in_order() {
        let events = vec![omega_protocol::OmegaEvent::TurnEnd(TurnEndEvent {
            time: "2024-01-01T00:00:00.000Z".to_owned(),
            metrics: TurnMetrics {
                input_tokens: 1,
                output_tokens: 2,
                cache_creation_tokens: None,
                cache_read_tokens: None,
            },
        })];
        let v = WsMessage::History {
            events,
            streaming: false,
        }
        .to_json();
        let arr = v["events"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["type"], "turn_end");
    }

    #[test]
    fn reset_done_serialises_to_type_only() {
        let v = WsMessage::ResetDone.to_json();
        assert_eq!(v, serde_json::json!({ "type": "reset_done" }));
        assert_eq!(v.as_object().unwrap().len(), 1, "no extra fields");
    }

    #[test]
    fn reset_done_to_text_matches_to_json() {
        let m = WsMessage::ResetDone;
        assert_eq!(m.to_text(), m.to_json().to_string());
    }

    #[test]
    fn item_event_turn_end_serialises_with_type_turn_end() {
        let ev = AgentItem::Event(Box::new(omega_protocol::OmegaEvent::TurnEnd(
            TurnEndEvent {
                time: "2024-01-01T00:00:00.000Z".to_owned(),
                metrics: TurnMetrics {
                    input_tokens: 1,
                    output_tokens: 2,
                    cache_creation_tokens: None,
                    cache_read_tokens: None,
                },
            },
        )));
        let v = WsMessage::Item(Box::new(ev)).to_json();
        assert_eq!(v["type"], "turn_end");
        assert_eq!(v["time"], "2024-01-01T00:00:00.000Z");
    }

    #[test]
    fn session_deleted_serialises_with_session_dir_camel_case() {
        let v = WsMessage::SessionDeleted {
            session_dir: "2025-01-01T00-00-00-000-deadbeef".to_owned(),
        }
        .to_json();
        assert_eq!(v["type"], "session_deleted");
        assert_eq!(v["sessionDir"], "2025-01-01T00-00-00-000-deadbeef");
        assert_eq!(v.as_object().unwrap().len(), 2, "no extra fields");
    }

    #[test]
    fn session_renamed_serialises_with_session_dir_and_name() {
        let v = WsMessage::SessionRenamed {
            session_dir: "2025-01-01T00-00-00-000-deadbeef".to_owned(),
            name: "my-renamed-session".to_owned(),
        }
        .to_json();
        assert_eq!(v["type"], "session_renamed");
        assert_eq!(v["sessionDir"], "2025-01-01T00-00-00-000-deadbeef");
        assert_eq!(v["name"], "my-renamed-session");
        assert_eq!(v.as_object().unwrap().len(), 3, "no extra fields");
    }

    #[test]
    fn session_renamed_to_text_round_trips_through_json() {
        let m = WsMessage::SessionRenamed {
            session_dir: "d".to_owned(),
            name: "n".to_owned(),
        };
        let parsed: serde_json::Value = serde_json::from_str(&m.to_text()).unwrap();
        assert_eq!(parsed, m.to_json());
    }
}
