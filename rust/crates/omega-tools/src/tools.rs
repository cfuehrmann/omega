//! One submodule per tool. Bodies are stubs in Phase 1d.0a; they are filled
//! in by Phase 1d.0b. Keeping them in separate files keeps the diff for
//! 1d.0b localised — tool by tool.
//!
//! The stubs are declared `async` so 1d.0b can fill them in without changing
//! the dispatch signature. Until then, clippy would complain about "unused
//! async"; suppress that here.
#![allow(clippy::unused_async)]

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
