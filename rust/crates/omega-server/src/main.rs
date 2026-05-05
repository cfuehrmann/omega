//! `omega-server` binary entry point.
//!
//! Phase 1e.1: parses CLI flags, constructs the real Anthropic provider,
//! builds [`AppState`], and delegates to [`omega_server::serve`].
//!
//! All logic lives in `lib.rs` helpers; `main` is pure glue marked
//! `#[mutants::skip]` because mutation-testing the bind/serve wiring would
//! require a real process spawn — `build_router`, `AppState`, and all
//! handlers are covered by integration tests instead.

use std::sync::Arc;

use clap::Parser as _;
use omega_core::{AnthropicProvider, RetryConfig, RetryingProvider};
use omega_server::{AppState, Args, serve};

/// All logic lives in helpers in `lib.rs`; `main` is pure glue.
#[mutants::skip]
#[tokio::main]
async fn main() -> std::io::Result<()> {
    let args = Args::parse();
    eprintln!(
        "omega-server: starting on 0.0.0.0:{} (sessions_root={}, public_dir={}, leptos_dir={})",
        args.port,
        args.sessions_root.display(),
        args.public_dir.display(),
        args.leptos_dir.display(),
    );

    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|e| std::io::Error::other(format!("ANTHROPIC_API_KEY env var: {e}")))?;

    // ANTHROPIC_BASE_URL: documented Anthropic-SDK env var — lets tests (and
    // corporate proxies) point the provider at a local SSE fake instead of
    // the real API.  Mirror of the same hook in omega-cli/src/main.rs.
    let inner = if let Ok(url) = std::env::var("ANTHROPIC_BASE_URL") {
        AnthropicProvider::new(api_key).with_base_url(url)
    } else {
        AnthropicProvider::new(api_key)
    }
    // BUG-D: context-management betas required for `clear_tool_uses_20250919`,
    // `clear_thinking_20251015`, and `compact_20260112` edit types.
    .with_beta("compact-2026-01-12")
    .with_beta("context-management-2025-06-27");
    let provider = Arc::new(RetryingProvider::new(
        inner,
        RetryConfig {
            max_attempts: 4,
            ..RetryConfig::default()
        },
    ));

    let state = AppState::new(provider, args.sessions_root, args.public_dir)
        .with_leptos_dir(args.leptos_dir);
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", args.port)).await?;
    serve(listener, state).await
}
