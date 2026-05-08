//! [`StreamSignal`] — ephemeral streaming primitives.
//!
//! These are never written to `events.jsonl`.  They are yielded by the agent
//! loop to drive live rendering in the UI.

use serde::{Deserialize, Serialize};

/// A raw streaming fragment from the LLM.  Never persisted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamSignal {
    /// A text token fragment.
    Text { text: String },
    /// A thinking (extended reasoning) token fragment.
    Thinking { text: String },
    /// Emitted when a thinking block finishes streaming.  Carries the
    /// cryptographic signature Anthropic requires when the thinking block is
    /// echoed back in the next API call.  Never forwarded to the UI.
    ThinkingBlockComplete { signature: String },
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn text_signal_round_trips() {
        let s = StreamSignal::Text {
            text: "hello".into(),
        };
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, r#"{"type":"text","text":"hello"}"#);
        let back: StreamSignal = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn thinking_signal_round_trips() {
        let s = StreamSignal::Thinking {
            text: "reasoning...".into(),
        };
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, r#"{"type":"thinking","text":"reasoning..."}"#);
        let back: StreamSignal = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }
}
