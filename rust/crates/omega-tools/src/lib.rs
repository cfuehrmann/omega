//! Tool implementations and dispatch for the Omega agent.
//!
//! This crate provides:
//!
//! * [`tool_definitions`] — the JSON-Schema definitions sent to the LLM.
//! * [`execute_tool`] — dispatch by name to the tool body.
//! * [`format_tool_call`] — a stable human-readable rendering of
//!   `name(input)` for log lines.
//!
//! All tool bodies in **Phase 1d.0a** are stubs that return an error
//! `ToolResult`. They are filled in by **Phase 1d.0b**. The dispatch table,
//! schemas, and `format_tool_call` are real and tested today — they are the
//! contract that `omega-agent` depends on.

use serde_json::Value;
use tokio_util::sync::CancellationToken;

mod format;
mod schemas;
mod state;
mod tools;

pub use format::format_tool_call;
pub use schemas::tool_definitions;

/// Outcome of a single tool invocation.
///
/// `content` is the string the agent feeds back to the LLM as the
/// `tool_result` block content.  Stderr-style errors are NOT a separate
/// channel: when `is_error` is true, `content` carries the error message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResult {
    pub content: String,
    pub is_error: bool,
}

impl ToolResult {
    #[must_use]
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
        }
    }

    #[must_use]
    pub fn err(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
        }
    }
}

/// Dispatch by tool name. Unknown names produce an error result so the
/// agent never sees a panic on a misbehaving model.
///
/// `cancel` lets long-running tools (subprocess execution, network fetches)
/// abort early when the agent's turn is cancelled.
pub async fn execute_tool(
    name: &str,
    input: Value,
    cancel: Option<&CancellationToken>,
) -> ToolResult {
    let res: Result<String, String> = match name {
        "read_file" => tools::read_file::execute(input, cancel).await,
        "write_file" => tools::write_file::execute(input, cancel).await,
        "edit_file" => tools::edit_file::execute(input, cancel).await,
        "list_files" => tools::list_files::execute(input, cancel).await,
        "run_command" => tools::run_command::execute(input, cancel).await,
        "grep_files" => tools::grep_files::execute(input, cancel).await,
        "find_files" => tools::find_files::execute(input, cancel).await,
        "run_background" => tools::run_background::execute(input, cancel).await,
        "wait_for_output" => tools::wait_for_output::execute(input, cancel).await,
        "write_stdin" => tools::write_stdin::execute(input, cancel).await,
        "web_search" => tools::web_search::execute(input, cancel).await,
        "fetch_url" => tools::fetch_url::execute(input, cancel).await,
        other => Err(format!("Unknown tool: {other}")),
    };
    match res {
        Ok(content) => ToolResult::ok(content),
        Err(err) => ToolResult::err(err),
    }
}

#[cfg(test)]
mod dispatch_tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn unknown_tool_returns_error_result() {
        let r = execute_tool("does_not_exist", json!({}), None).await;
        assert!(r.is_error);
        assert!(r.content.contains("Unknown tool"));
    }

    #[tokio::test]
    async fn dispatch_table_routes_every_known_name() {
        // Every name listed in tool_definitions must dispatch to a body
        // (even if that body is currently a stub). Verifies the table
        // and the dispatch in execute_tool stay in sync.
        let defs = tool_definitions();
        for def in defs {
            let r = execute_tool(&def.name, json!({}), None).await;
            // Stubs return is_error=true with "not yet implemented".
            // Real tools may also error on missing args, but never with
            // "Unknown tool" — that would mean a missing dispatch arm.
            assert!(
                !r.content.contains("Unknown tool"),
                "tool {} not wired into dispatch",
                def.name,
            );
        }
    }
}
