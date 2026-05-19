//! `omega-server` binary entry point.
//!
//! Phase 1e.1: parses CLI flags, constructs the real Anthropic provider,
//! builds [`AppState`], and delegates to [`omega_server::serve`].
//!
//! All logic lives in `lib.rs` helpers; `main` is pure glue marked
//! `#[mutants::skip]` because mutation-testing the bind/serve wiring would
//! require a real process spawn — `build_router`, `AppState`, and all
//! handlers are covered by integration tests instead.

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser as _;
use omega_core::{AnthropicProvider, RetryConfig, RetryingProvider};
use omega_server::{AppState, Args, cli, serve};

/// All logic lives in helpers in `lib.rs`; `main` is pure glue.
#[mutants::skip]
#[tokio::main]
async fn main() -> std::io::Result<()> {
    // Load .env files in priority order (first writer wins for each key):
    //   1. CWD .env  — project-level overrides (e.g. local mock API URL)
    //   2. ~/.config/omega/.env — user-level secrets (API keys, etc.)
    //   3. Real environment variables — always highest priority (never overridden)
    //
    // Loading CWD first means project overrides beat user config; neither
    // file can override a variable already set in the real environment.
    dotenvy::dotenv().ok();
    if let Ok(home) = std::env::var("HOME") {
        dotenvy::from_path(std::path::Path::new(&home).join(".config/omega/.env")).ok();
    }

    let args = Args::parse();

    // Apply --working-dir before any relative-path I/O.
    if let Some(ref dir) = args.working_dir {
        std::env::set_current_dir(dir)
            .map_err(|e| std::io::Error::other(format!("--working-dir {}: {e}", dir.display())))?;
    }

    // Resolve leptos dir: explicit flag → binary-relative → CWD fallback.
    let leptos_dir = args.leptos_dir.unwrap_or_else(|| {
        std::env::current_exe()
            .ok()
            .and_then(|p| {
                // Binary is at <repo>/target/release/omega-server.
                // Three parent() calls reach the repo root.
                let root = p.parent()?.parent()?.parent()?;
                Some(root.join("frontends/leptos/dist"))
            })
            .unwrap_or_else(|| PathBuf::from(cli::DEFAULT_LEPTOS_DIR))
    });

    eprintln!(
        "omega-server: starting on 0.0.0.0:{} (sessions_root={}, leptos_dir={})",
        args.port,
        args.sessions_root.display(),
        leptos_dir.display(),
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

    let state = AppState::new(
        provider,
        args.sessions_root,
        std::env::current_dir().unwrap_or_default(),
    )
    .with_leptos_dir(leptos_dir);
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", args.port)).await?;
    serve(listener, state).await
}
