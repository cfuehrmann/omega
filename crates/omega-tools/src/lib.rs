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

use omega_types::OmegaEvent;
use omega_types::events::PythonReplBootstrappedEvent;

mod cap_and_tee;
mod format;
mod output_cleaner;
mod process_util;
pub mod python_repl;
mod schemas;
mod state;
mod tool_ctx;
mod tools;

pub use cap_and_tee::{CappedOutput, TruncationBias, cap_and_tee};
pub use format::format_tool_call;
pub use python_repl::PythonRepl;
pub use schemas::{ALL_TOOL_NAMES, DEFAULT_TOOL_NAMES, tool_definitions};
pub use tool_ctx::ToolCtx;

/// Outcome of a single tool invocation.
///
/// `content` is the string the agent feeds back to the LLM as the
/// `tool_result` block content.  Stderr-style errors are NOT a separate
/// channel: when `is_error` is true, `content` carries the error message.
///
/// `extra_events` carries side-band [`OmegaEvent`]s to be emitted by the
/// agent before the `ToolResultEvent`.  Normally empty; populated only by
/// `python_repl` dispatch when a successful bootstrap occurs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ToolResult {
    pub content: String,
    pub is_error: bool,
    /// Extra events to be emitted before the `ToolResultEvent`.  The agent
    /// layer persists these to `events.jsonl` and streams them to UI consumers
    /// just like any other event.
    pub extra_events: Vec<OmegaEvent>,
}

impl ToolResult {
    #[must_use]
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
            extra_events: vec![],
        }
    }

    #[must_use]
    pub fn err(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
            extra_events: vec![],
        }
    }
}

/// Returns `true` when `path_str` canonicalises to a path already embedded
/// in the system prompt.
///
/// Extracted from [`execute_tool`] so cargo-mutants can generate the
/// `→ true` / `→ false` body mutations.  Run targeted mutation testing with
/// `just mutants-system-prompt-guard` (which passes `--cap-lints=true` so
/// that the unused-parameter warning produced by those mutations does not
/// prevent compilation).
fn in_system_prompt(path_str: &str, paths: &std::collections::HashSet<std::path::PathBuf>) -> bool {
    std::path::Path::new(path_str)
        .canonicalize()
        .is_ok_and(|p| paths.contains(&p))
}

/// Dispatch by tool name. Unknown names produce an error result so the
/// agent never sees a panic on a misbehaving model.
///
/// `cancel` lets long-running tools (subprocess execution, network fetches)
/// abort early when the agent's turn is cancelled.
///
/// `ctx` carries the session's cache directory so tools can tee their output
/// to the session tree. Pass `None` only in unit tests; production code
/// always provides a `ToolCtx` constructed from `AgentConfig::session_dir`.
///
/// ## System-prompt guard
///
/// Before dispatching `read_file`, `execute_tool` checks whether the
/// requested path is already embedded in the system prompt (via
/// [`ToolCtx::system_prompt_paths`]). If so it returns a short
/// `ToolResult::ok` message immediately — the file content is already
/// present and the round-trip is unnecessary.
pub async fn execute_tool(
    name: &str,
    input: Value,
    cancel: Option<&CancellationToken>,
    ctx: Option<&ToolCtx>,
) -> ToolResult {
    let res: Result<String, String> = match name {
        "read_file" => {
            if let (Some(ctx), Some(path_str)) = (ctx, input["path"].as_str())
                && in_system_prompt(path_str, &ctx.system_prompt_paths)
            {
                return ToolResult::ok(
                    "This file is already included in your system prompt \
                     and is available there — no tool call needed.",
                );
            }
            tools::read_file::execute(input, cancel).await
        }
        "write_file" => tools::write_file::execute(input, cancel).await,
        "edit_file" => tools::edit_file::execute(input, cancel).await,
        "list_files" => tools::list_files::execute(input, cancel).await,
        "run_command" => tools::run_command::execute(input, cancel, ctx).await,
        "grep_files" => tools::grep_files::execute(input, cancel).await,
        "find_files" => tools::find_files::execute(input, cancel).await,
        "run_background" => tools::run_background::execute(input, cancel).await,
        "wait_for_output" => tools::wait_for_output::execute(input, cancel, ctx).await,
        "write_stdin" => tools::write_stdin::execute(input, cancel).await,
        "web_search" => tools::web_search::execute(input, cancel).await,
        "fetch_url" => tools::fetch_url::execute(input, cancel, ctx).await,
        "python_repl" => {
            // The REPL tool requires an active session context with the
            // python_repl Arc.  Early-return ToolResult directly so the
            // match arm type is `!` (compatible with Result<String,String>)
            // without needing a Result wrapper for every early exit.
            let Some(ctx) = ctx else {
                return ToolResult::err("python_repl: no session context — REPL is not available");
            };
            let Some(repl_arc) = &ctx.python_repl else {
                return ToolResult::err(
                    "python_repl: tool not present in this session's tool_selection \
                     (include \"python_repl\" when creating the session)",
                );
            };
            let Some(code) = input["code"].as_str() else {
                return ToolResult::err("python_repl: missing 'code' field in input");
            };
            let code = code.to_owned();
            let mut guard = repl_arc.lock().await;
            // bootstrap_info is Some when python3 was just installed via apt-get.
            let bootstrap_info: Option<python_repl::BootstrapInfo>;
            if guard.is_none() {
                match python_repl::PythonRepl::start() {
                    Ok((repl, info)) => {
                        *guard = Some(repl);
                        bootstrap_info = info;
                    }
                    Err(e) => {
                        return ToolResult::err(format!(
                            "python_repl: failed to start Python interpreter: {e}"
                        ));
                    }
                }
            } else {
                bootstrap_info = None;
            }
            // SAFETY: guard is Some — either it was already Some or we
            // just inserted a value above (or returned early on error).
            let Some(repl) = guard.as_mut() else {
                return ToolResult::err(
                    "python_repl: internal error: REPL not initialised after successful start",
                );
            };
            // Parse per-call timeout: default 60 s, clamped to [1, 600].
            let timeout_secs = input["timeout"]
                .as_u64()
                .unwrap_or(python_repl::DEFAULT_TIMEOUT_SECS)
                .min(python_repl::MAX_TIMEOUT_SECS);
            let output = repl.execute(&code, timeout_secs, Some(ctx)).await;
            // If the repl was hard-killed during execute, clear the cached
            // handle so the next call spawns a fresh kernel.
            let repl_dead = repl.is_dead();
            if repl_dead {
                *guard = None;
            }
            let mut result = ToolResult::ok(output);
            // If python3 was just bootstrapped via apt-get, emit a
            // PythonReplBootstrapped event so forensics can see it happened.
            if let Some(info) = bootstrap_info {
                result.extra_events.push(OmegaEvent::PythonReplBootstrapped(
                    PythonReplBootstrappedEvent {
                        time: chrono::Utc::now()
                            .format("%Y-%m-%dT%H:%M:%S%.3fZ")
                            .to_string(),
                        duration_ms: i64::try_from(info.duration_ms).unwrap_or(i64::MAX),
                        success: true,
                        stderr_excerpt: info.stderr_excerpt,
                    },
                ));
            }
            return result;
        }
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
        let r = execute_tool("does_not_exist", json!({}), None, None).await;
        assert!(r.is_error);
        assert!(r.content.contains("Unknown tool"));
    }

    #[tokio::test]
    async fn dispatch_table_routes_every_known_name() {
        // Every tool name in ALL_TOOL_NAMES must dispatch to a body — no
        // "Unknown tool" errors.  Verifies the table and the dispatch in
        // execute_tool stay in sync.
        use crate::schemas::ALL_TOOL_NAMES;
        let selection: Vec<String> = ALL_TOOL_NAMES.iter().map(|s| (*s).to_owned()).collect();
        let defs = tool_definitions(&selection);
        for def in defs {
            let r = execute_tool(&def.name, json!({}), None, None).await;
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
