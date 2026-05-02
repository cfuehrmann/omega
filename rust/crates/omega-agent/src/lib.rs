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
pub mod error_classify;
pub mod session_resume;
pub mod system_prompt;

pub use agent::{Agent, AgentConfig, DEFAULT_EFFORT};
pub use config::max_output_tokens_for_model;
pub use error_classify::{is_context_too_long, is_invalid_tool_json};
pub use session_resume::{
    extract_description_from_response, extract_last_model_and_effort, extract_resumption_basis,
    extract_summary_from_response,
};
pub use system_prompt::{
    build_system_prompt, read_system_prompt_append, system_prompt_append_path,
};
