//! Conversation-primitive types: [`Role`] and [`ContentBlock`].
//!
//! These are the building blocks shared by every LLM provider backend and
//! the persistence layer.  They intentionally mirror the Anthropic Messages
//! API shape.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Role
// ---------------------------------------------------------------------------

/// Role of a message in the conversation history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
}

// ---------------------------------------------------------------------------
// ContentBlock
// ---------------------------------------------------------------------------

/// A single content block inside a message.
///
/// Mirrors the Anthropic Messages API shape — the union of every block
/// type the agent sends or receives.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// A text block.
    Text { text: String },
    /// A thinking block (extended reasoning, returned by Anthropic
    /// when the model has thinking enabled).  The `signature` is the
    /// opaque token Anthropic requires when echoing the block back in
    /// a follow-up turn.
    Thinking {
        thinking: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    /// A tool invocation by the assistant.
    ToolUse {
        id: String,
        name: String,
        /// Arbitrary JSON input parameters supplied by the LLM.
        input: Value,
    },
    /// The result of a tool invocation, sent back as a user message.
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        is_error: bool,
    },
}
