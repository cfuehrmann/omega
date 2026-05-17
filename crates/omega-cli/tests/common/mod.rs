//! Test fixtures for the `omega-cli` integration tests.
//!
//! The Anthropic-shaped axum SSE fake itself lives in the workspace's
//! `omega-test-fixtures` crate (the single source of the LLM HTTP fake).
//! This module re-exports it and adds path-normalisation helpers used
//! by the snapshot tests in this crate.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;

pub use omega_test_fixtures::{MockResponse, MockServer};

/// Replace temp-dir paths with a stable placeholder for snapshots.
pub fn normalize_temp_paths(s: &str, temp_dir: &Path) -> String {
    let p = temp_dir.to_string_lossy().into_owned();
    s.replace(&p, "[TEMP_DIR]")
}

/// Replace the session-dir line `Session: <root>/<timestamp>-<hex>` with
/// a stable placeholder. The session-dir name embeds wallclock + random
/// bytes, so the literal path is never reproducible across runs.
pub fn normalize_session_line(s: &str) -> String {
    let mut out = String::new();
    for line in s.split_inclusive('\n') {
        if let Some(rest) = line.strip_prefix("Session: ") {
            let _ = rest;
            out.push_str("Session: [SESSION_DIR]\n");
        } else {
            out.push_str(line);
        }
    }
    out
}
