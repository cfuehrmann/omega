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
}
