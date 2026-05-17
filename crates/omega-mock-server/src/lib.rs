//! Library facet of the `omega-mock-server` crate.
//!
//! Exists so that integration tests under `tests/` can drive the
//! control router directly via `tower::ServiceExt::oneshot` without
//! binding TCP ports. Production wiring (clap parsing, listeners,
//! `omega_server::serve`) lives in `main.rs`.

pub mod control;
