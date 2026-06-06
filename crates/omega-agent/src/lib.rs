//! Omega agent: orchestrates LLM calls and tool dispatch.
//!
//! The agentic loop core that drives one user turn through any
//! [`omega_core::Provider`] (typically wrapped by
//! [`omega_core::RetryingProvider`]) until the model returns a response
//! without tool calls — or an error / cancellation ends the turn.
//!
//! See [`Agent::send_message`] for the entry point.

pub mod agent;
pub mod config;
pub mod controls;
pub mod error_classify;
pub mod session_resume;
pub mod system_prompt;

pub use agent::{Agent, AgentConfig, DEFAULT_EFFORT, InputItem, ModelEffortHandle};
pub use config::max_output_tokens_for_model;
pub use controls::ControlHandle;
pub use error_classify::{is_context_too_long, is_invalid_tool_json};
pub use session_resume::{
    RESUMPTION_EFFORT, RESUMPTION_MAX_TOKENS, RESUMPTION_MODEL, RESUMPTION_SUMMARY_INSTRUCTIONS,
    extract_resumption_basis, extract_summary_from_response, extract_tool_selection,
};
pub use system_prompt::{
    AGENTS_FILE, InstructionFile, SystemBlock, build_system_blocks, discover_instruction_files,
    global_agents_md_path, join_blocks, repl_addendum, repo_agents_md_path,
};

/// Short git commit hash of the Omega binary, captured at compile time.
/// Falls back to `"unknown"` when git was unavailable at build time.
pub const OMEGA_GIT_COMMIT: &str = env!("OMEGA_GIT_COMMIT");
