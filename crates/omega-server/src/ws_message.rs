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
use omega_types::FeatureFlags;
use omega_types::ids::LoggedEvent;

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
        /// Runtime feature flags active for this session.
        /// Always present on the wire as `features` so the UI can display
        /// capability badges (e.g. "REPL on") without re-reading the event log.
        features: FeatureFlags,
    },
    /// Persisted history batch sent on connect / reset / resume.
    /// `streaming` is omitted on the wire when `false` — matches the TS
    /// server's `...(isStreaming ? { streaming: true } : {})` pattern.
    History {
        /// Filtered persisted events for the current session.
        /// Each element is a [`LoggedEvent`] envelope carrying the stable
        /// `eventId` assigned at write time alongside the event payload.
        events: Vec<LoggedEvent>,
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
    /// Refusal frame: the operator attempted a `Reset` or `ResumeSession`
    /// against a working tree with uncommitted git changes, *without* the
    /// `allow_dirty` opt-in.  The server has done nothing — the previous
    /// active session (if any) is untouched.  The client is expected to
    /// surface a confirmation modal; on "Proceed" it re-issues the same
    /// frame with `allow_dirty: true`, on "Cancel" it discards the intent.
    ///
    /// Mirrors the CLI's deny-by-default `--allow-dirty` semantics.
    PendingChangesWarning { intent: PendingChangesIntent },
}

/// What the operator was about to do when the dirty-tree gate fired.
///
/// Echoed back inside [`WsMessage::PendingChangesWarning`] so the client
/// can re-issue the exact same frame with `allow_dirty: true` after the
/// operator confirms — no client-side bookkeeping required.
#[derive(Debug, Clone)]
pub enum PendingChangesIntent {
    /// `Reset { model, effort }` was attempted.
    Reset {
        model: Option<String>,
        effort: Option<String>,
    },
    /// `ResumeSession { session_dir }` was attempted.
    ResumeSession { session_dir: String },
}

impl PendingChangesIntent {
    fn to_json(&self) -> serde_json::Value {
        match self {
            Self::Reset { model, effort } => {
                let mut obj = serde_json::Map::new();
                obj.insert("kind".to_owned(), serde_json::Value::from("reset"));
                if let Some(m) = model {
                    obj.insert("model".to_owned(), serde_json::Value::from(m.clone()));
                }
                if let Some(e) = effort {
                    obj.insert("effort".to_owned(), serde_json::Value::from(e.clone()));
                }
                serde_json::Value::Object(obj)
            }
            Self::ResumeSession { session_dir } => serde_json::json!({
                "kind": "resume_session",
                "sessionDir": session_dir,
            }),
        }
    }
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
                features,
            } => {
                let mut obj = serde_json::Map::with_capacity(9);
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
                obj.insert(
                    "features".to_owned(),
                    serde_json::to_value(features).unwrap_or_else(|_| serde_json::Value::Null),
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
            Self::PendingChangesWarning { intent } => serde_json::json!({
                "type": "pending_changes_warning",
                "intent": intent.to_json(),
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
    // Justification for inline test block: these tests pin the WebSocket
    // wire protocol — exact JSON field names, camelCase keys, and
    // omit-when-None semantics.  Integration tests in tests/ws.rs connect
    // real WebSocket clients and assert on message *types*, but they do not
    // parse every field of every message shape.  Covering the full wire
    // contract at the unit level is cheaper and more precise.
    //
    // `PendingChangesIntent::to_json` is a private function only reachable
    // from this module, so tests must live here.
    #![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

    use super::{PendingChangesIntent, WsMessage};
    use omega_core::AgentItem;
    use omega_types::FeatureFlags;
    use omega_types::ids::LoggedEvent;
    use omega_types::{StreamSignal, events::TurnEndEvent, events::TurnMetrics};

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
            index: 0,
            text: "hello".to_owned(),
        });
        let v = WsMessage::Item(Box::new(sig)).to_json();
        assert_eq!(v["type"], "text");
        assert_eq!(v["text"], "hello");
        assert_eq!(v["index"], 0);
    }

    #[test]
    fn item_signal_thinking_serialises_with_type_thinking() {
        let sig = AgentItem::Signal(StreamSignal::Thinking {
            index: 0,
            text: "musing".to_owned(),
        });
        let v = WsMessage::Item(Box::new(sig)).to_json();
        assert_eq!(v["type"], "thinking");
        assert_eq!(v["text"], "musing");
        assert_eq!(v["index"], 0);
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
            features: FeatureFlags::default(),
        }
        .to_json();
        assert_eq!(v["type"], "session_info");
        assert_eq!(v["dir"], "2025-01-01T00-00-00-000-deadbeef");
        assert_eq!(v["model"], "claude-sonnet-4-6");
        assert_eq!(v["effort"], "medium");
        assert_eq!(v["cwd"], "/tmp");
        let obj = v.as_object().unwrap();
        assert!(!obj.contains_key("name"), "name must be omitted when None");
        // type, dir, model, effort, cwd, turnState, hasPendingChanges, features = 8
        assert_eq!(obj.len(), 8, "unexpected extra fields: {obj:?}");
        assert_eq!(obj["turnState"], "idle");
        assert_eq!(obj["hasPendingChanges"], false);
        // features is always present
        assert!(obj.contains_key("features"), "features must be present");
        assert_eq!(obj["features"]["repl"], false);
        assert_eq!(obj["features"]["subagents"], false);
    }

    #[test]
    fn session_info_features_flags_on_appear_on_wire() {
        let v = WsMessage::SessionInfo {
            dir: "d".to_owned(),
            model: "m".to_owned(),
            effort: "e".to_owned(),
            cwd: "/c".to_owned(),
            name: None,
            turn_state: "idle".to_owned(),
            has_pending_changes: false,
            features: FeatureFlags {
                repl: true,
                subagents: false,
                repl_replaces_fileops: false,
            },
        }
        .to_json();
        assert_eq!(v["features"]["repl"], true);
        assert_eq!(v["features"]["subagents"], false);
    }

    #[test]
    fn session_info_includes_name_with_features_gives_nine_fields() {
        let v = WsMessage::SessionInfo {
            dir: "d".to_owned(),
            model: "m".to_owned(),
            effort: "e".to_owned(),
            cwd: "/c".to_owned(),
            name: Some("my-session".to_owned()),
            turn_state: "running".to_owned(),
            has_pending_changes: false,
            features: FeatureFlags::default(),
        }
        .to_json();
        assert_eq!(v["name"], "my-session");
        let obj = v.as_object().unwrap();
        // type, dir, model, effort, cwd, turnState, hasPendingChanges, features, name = 9
        assert_eq!(obj.len(), 9, "unexpected field count: {obj:?}");
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
            features: FeatureFlags::default(),
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
            features: FeatureFlags::default(),
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
            features: FeatureFlags::default(),
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
        // event_id: None represents a pre-Phase-1 log entry that has no ID.
        let events = vec![LoggedEvent {
            event_id: None,
            event: omega_types::OmegaEvent::TurnEnd(TurnEndEvent {
                time: "2024-01-01T00:00:00.000Z".to_owned(),
                metrics: TurnMetrics {
                    input_tokens: 1,
                    output_tokens: 2,
                    cache_creation_tokens: None,
                    cache_read_tokens: None,
                },
            }),
        }];
        let v = WsMessage::History {
            events,
            streaming: false,
        }
        .to_json();
        let arr = v["events"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["type"], "turn_end");
        // No eventId when None (pre-Phase-1 log entry).
        assert!(
            arr[0].get("eventId").is_none(),
            "eventId must be absent when None"
        );
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
        let ev = AgentItem::Event(Box::new(omega_types::OmegaEvent::TurnEnd(TurnEndEvent {
            time: "2024-01-01T00:00:00.000Z".to_owned(),
            metrics: TurnMetrics {
                input_tokens: 1,
                output_tokens: 2,
                cache_creation_tokens: None,
                cache_read_tokens: None,
            },
        })));
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

    // -----------------------------------------------------------------------
    // PendingChangesIntent::to_json — pin the wire shape so the mutant that
    // replaces the whole body with Default::default() is caught.
    // -----------------------------------------------------------------------

    #[test]
    fn pending_changes_intent_reset_with_model_and_effort() {
        let intent = PendingChangesIntent::Reset {
            model: Some("m".to_owned()),
            effort: Some("e".to_owned()),
        };
        // Build a PendingChangesWarning so we can call the public to_json path.
        let v = WsMessage::PendingChangesWarning { intent }.to_json();
        let obj = &v["intent"];
        assert_eq!(obj["kind"], "reset");
        assert_eq!(obj["model"], "m");
        assert_eq!(obj["effort"], "e");
    }

    #[test]
    fn pending_changes_intent_reset_without_model_or_effort_omits_keys() {
        let intent = PendingChangesIntent::Reset {
            model: None,
            effort: None,
        };
        let v = WsMessage::PendingChangesWarning { intent }.to_json();
        let obj = &v["intent"];
        assert_eq!(obj["kind"], "reset", "kind must be \"reset\"");
        assert!(
            obj.get("model").is_none() || obj["model"].is_null(),
            "model must be absent when None"
        );
        assert!(
            obj.get("effort").is_none() || obj["effort"].is_null(),
            "effort must be absent when None"
        );
    }

    #[test]
    fn pending_changes_intent_resume_session_serialises_correctly() {
        let intent = PendingChangesIntent::ResumeSession {
            session_dir: "2025-01-01T00-00-00-000-abc".to_owned(),
        };
        let v = WsMessage::PendingChangesWarning { intent }.to_json();
        let obj = &v["intent"];
        assert_eq!(obj["kind"], "resume_session");
        assert_eq!(obj["sessionDir"], "2025-01-01T00-00-00-000-abc");
    }
}
