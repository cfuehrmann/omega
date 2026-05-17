//! [`StreamSignal`] — ephemeral streaming primitives.
//!
//! These are never written to `events.jsonl`.  They are yielded by the agent
//! loop to drive live rendering in the UI.
//!
//! # SCHEMA-8 indices
//!
//! Every signal carries an `index: usize` matching Anthropic's
//! `content_block_start.index` so the agent's order-preserving accumulator
//! can route deltas to the correct slot. For non-interleaved streams the
//! index increments monotonically per content block; for interleaved streams
//! (with the `interleaved-thinking` beta) it can revisit older blocks.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A raw streaming fragment from the LLM.  Never persisted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamSignal {
    /// A text token fragment for the content block at `index`.
    Text { index: usize, text: String },
    /// A thinking (extended reasoning) token fragment for the content block at
    /// `index`.
    Thinking { index: usize, text: String },
    /// Emitted at `content_block_stop` for a text content block. Carries the
    /// full assembled text so the agent can finalise its slot in one step
    /// rather than relying on the per-delta accumulation having matched.
    TextBlockComplete { index: usize, text: String },
    /// Emitted at `content_block_stop` for a thinking content block. Carries
    /// the cryptographic signature Anthropic requires when the thinking block
    /// is echoed back in the next API call.  Never forwarded to the UI.
    ThinkingBlockComplete { index: usize, signature: String },
    /// Emitted at `content_block_start` for a `tool_use` content block.
    /// Carries the LLM-issued `tool_use_id` and `name` so the UI can render
    /// the label immediately, before any `ToolInput` deltas arrive.  The
    /// Omega-issued `tool_call_id` is minted by the agent (not the provider)
    /// when this signal is observed, and lives only at the event layer.
    ToolUseBlockStart {
        index: usize,
        tool_use_id: String,
        name: String,
    },
    /// A partial JSON fragment for the tool-use block at `index`.
    /// Mid-stream `partial_json` is NOT valid JSON; the UI displays it
    /// raw.  Only `ToolUseBlockComplete` carries the fully-assembled
    /// parsed input.
    ToolInput { index: usize, partial_json: String },
    /// Emitted at `content_block_stop` for a `tool_use` content block. The
    /// `input` is parsed JSON; the agent dispatches the tool only after the
    /// surrounding `LlmResponseEnded` event has been emitted.  Never
    /// forwarded to the UI.
    ToolUseBlockComplete {
        index: usize,
        tool_use_id: String,
        name: String,
        input: Value,
    },
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn text_signal_round_trips() {
        let s = StreamSignal::Text {
            index: 0,
            text: "hello".into(),
        };
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, r#"{"type":"text","index":0,"text":"hello"}"#);
        let back: StreamSignal = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn thinking_signal_round_trips() {
        let s = StreamSignal::Thinking {
            index: 1,
            text: "reasoning...".into(),
        };
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(
            json,
            r#"{"type":"thinking","index":1,"text":"reasoning..."}"#
        );
        let back: StreamSignal = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn text_block_complete_round_trips() {
        let s = StreamSignal::TextBlockComplete {
            index: 2,
            text: "full text".into(),
        };
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(
            json,
            r#"{"type":"text_block_complete","index":2,"text":"full text"}"#
        );
        let back: StreamSignal = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn thinking_block_complete_round_trips() {
        let s = StreamSignal::ThinkingBlockComplete {
            index: 3,
            signature: "sig-abc".into(),
        };
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(
            json,
            r#"{"type":"thinking_block_complete","index":3,"signature":"sig-abc"}"#
        );
        let back: StreamSignal = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn tool_use_block_start_round_trips() {
        let s = StreamSignal::ToolUseBlockStart {
            index: 5,
            tool_use_id: "tu_99".into(),
            name: "bash".into(),
        };
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(
            json,
            r#"{"type":"tool_use_block_start","index":5,"tool_use_id":"tu_99","name":"bash"}"#
        );
        let back: StreamSignal = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn tool_input_round_trips() {
        let s = StreamSignal::ToolInput {
            index: 5,
            partial_json: r#"{"path": "foo"#.into(),
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: StreamSignal = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["type"], "tool_input");
        assert_eq!(v["index"], 5);
    }

    #[test]
    fn tool_use_block_complete_round_trips() {
        let s = StreamSignal::ToolUseBlockComplete {
            index: 4,
            tool_use_id: "tu_1".into(),
            name: "read_file".into(),
            input: serde_json::json!({"path": "foo.txt"}),
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: StreamSignal = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["type"], "tool_use_block_complete");
        assert_eq!(v["index"], 4);
        assert_eq!(v["tool_use_id"], "tu_1");
        assert_eq!(v["name"], "read_file");
        assert_eq!(v["input"]["path"], "foo.txt");
    }
}
