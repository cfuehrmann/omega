//! Test fixtures shared by the `omega-server` integration tests.
//!
//! The Anthropic-shaped axum SSE fake itself lives in the workspace's
//! `omega-test-fixtures` crate (the single source of the LLM HTTP fake).
//! This module is just a re-export shim so test code reads
//! `common::MockServer` rather than the longer crate path.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

pub use omega_test_fixtures::{MockResponse, MockServer};
