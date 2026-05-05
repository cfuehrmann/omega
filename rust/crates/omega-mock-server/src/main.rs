//! `mock-omega-server` — Playwright test fixture binary.
//!
//! Hosts the production [`omega_server::serve`] driven by a real
//! [`omega_core::AnthropicProvider`] whose base URL points at an
//! internal Anthropic-shaped SSE fake on a random 127.0.0.1 port. Both
//! the fake and its `MockResponse` / `CallHistory` machinery live in
//! the workspace's `omega-test-fixtures` crate.
//!
//! Per-test scripted responses are loaded via the control HTTP API on
//! `--ctrl-port` (default 3004) — tests POST a script of `MockResponse`
//! to `/control/script` before triggering input and inspect captured
//! requests via `/control/llm-calls`. See [`control`] for the full route
//! list.
//!
//! Compared to the historical `Provider`-trait-injected mock, this
//! binary exercises the full HTTP/SSE code path: `AnthropicProvider`
//! serialises the request, `reqwest` sends it, the fake replies with a
//! streaming SSE body, the production parser reassembles it. Bugs in
//! the parser, the network layer, retry behaviour, or the
//! request/response wire shape are all reachable from Playwright tests.

mod control;

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use omega_core::AnthropicProvider;
use omega_test_fixtures::{CallHistory, new_script, router as fake_router};
use tokio::net::TcpListener;

/// CLI shape mirrors `omega-server`'s, plus `--ctrl-port` for the control API.
#[derive(Parser, Debug)]
#[command(about = "Mock omega-server for Playwright e2e tests (HTTP-fake LLM).")]
struct Args {
    /// Port for the main HTTP + WebSocket server.
    #[arg(long, default_value_t = 3003)]
    port: u16,

    /// Port for the control HTTP API used by tests to script the fake
    /// and inspect captured LLM calls.
    #[arg(long, default_value_t = 3004)]
    ctrl_port: u16,

    /// Sessions root directory.
    #[arg(long, default_value = ".omega/test-sessions")]
    sessions_root: PathBuf,

    /// Static asset directory served by the fallback `ServeDir` handler.
    #[arg(long, default_value = "src/web/public")]
    public_dir: PathBuf,

    /// Leptos `dist/` directory served under `/leptos/` (Phase 3.0).
    #[arg(long, default_value = "frontends/leptos/dist")]
    leptos_dir: PathBuf,
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let args = Args::parse();

    let history = CallHistory::new();
    let script = new_script();

    // Bind the public listeners up-front so Playwright's port-readiness
    // probe succeeds the moment we hit `serve`.
    let main_addr = format!("0.0.0.0:{}", args.port);
    let ctrl_addr = format!("0.0.0.0:{}", args.ctrl_port);
    let main_listener = TcpListener::bind(&main_addr).await?;
    let ctrl_listener = TcpListener::bind(&ctrl_addr).await?;

    // Internal-only Anthropic-shaped SSE fake: random port on
    // 127.0.0.1, never exposed to Playwright. The production
    // `AnthropicProvider` talks to it via reqwest just as it would talk
    // to api.anthropic.com.
    let fake_listener = TcpListener::bind("127.0.0.1:0").await?;
    let fake_addr = fake_listener.local_addr()?;
    let fake_url = format!("http://{fake_addr}");

    eprintln!(
        "mock-omega-server: main on {main_addr}, control on {ctrl_addr}, fake LLM on {fake_url}"
    );

    let provider = Arc::new(
        AnthropicProvider::new("sk-mock-test")
            .with_base_url(fake_url)
            // Mirror the production betas so the fake exercises the full
            // request path including context_management (BUG-D).
            .with_beta("compact-2026-01-12")
            .with_beta("context-management-2025-06-27"),
    );
    let state = omega_server::AppState::new(provider, args.sessions_root, args.public_dir)
        .with_leptos_dir(args.leptos_dir);

    let fake_app = fake_router(script.clone(), Some(history.clone()));
    let ctrl_app = control::router(history, script);

    let fake_handle = tokio::spawn(async move {
        let _ = axum::serve(fake_listener, fake_app).await;
    });
    let ctrl_handle = tokio::spawn(async move {
        let _ = axum::serve(ctrl_listener, ctrl_app).await;
    });

    // omega_server::serve installs SIGINT/SIGTERM handlers and runs the
    // graceful-shutdown sequence — exactly what Playwright triggers
    // between projects via `webServer.gracefulShutdown`.
    let result = omega_server::serve(main_listener, state).await;

    // After the main server exits, abort the side servers and propagate.
    fake_handle.abort();
    ctrl_handle.abort();
    result
}
