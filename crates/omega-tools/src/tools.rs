//! One submodule per tool — Phase 1d.0b real implementations.
//!
//! Each submodule contains a single `execute(input, cancel)` async function
//! that is dispatched from [`crate::execute_tool`].

pub mod edit_file;
pub mod fetch_url;
pub mod find_files;
pub mod grep_files;
pub mod list_files;
pub mod read_file;
pub mod run_background;
pub mod run_command;
pub mod wait_for_output;
pub mod web_search;
pub mod write_file;
pub mod write_stdin;
