//! `mock-omega-server` — Playwright test fixture binary.
//!
//! Wraps [`omega_server::serve`] with a hard-coded mock LLM provider so the
//! real-server e2e suite is fully deterministic and never touches a real LLM
//! API.  Replaces the historical `e2e/fixtures/real-server.ts`.
//!
//! See [`provider`] for the routing rules and [`control`] for the small
//! HTTP API the tests use to inspect captured LLM calls.

mod control;
mod provider;

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use tokio::net::TcpListener;

use crate::provider::{CallHistory, MockProvider};

/// CLI shape mirrors `omega-server`'s, plus `--ctrl-port` for the control API.
#[derive(Parser, Debug)]
#[command(about = "Mock omega-server for Playwright e2e tests (deterministic LLM).")]
struct Args {
    /// Port for the main HTTP + WebSocket server.
    #[arg(long, default_value_t = 3003)]
    port: u16,

    /// Port for the control HTTP API used by tests to inspect captured calls.
    #[arg(long, default_value_t = 3004)]
    ctrl_port: u16,

    /// Sessions root directory.
    #[arg(long, default_value = ".omega/test-sessions")]
    sessions_root: PathBuf,

    /// Static asset directory served by the fallback `ServeDir` handler.
    #[arg(long, default_value = "src/web/public")]
    public_dir: PathBuf,
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let args = Args::parse();

    // Capture history shared between the provider and the control server.
    let history = CallHistory::new();
    let provider = Arc::new(MockProvider::new(history.clone()));

    // Bind both listeners up-front so Playwright's port-readiness probe
    // succeeds the moment we hit `serve`.
    let main_addr = format!("0.0.0.0:{}", args.port);
    let ctrl_addr = format!("0.0.0.0:{}", args.ctrl_port);
    let main_listener = TcpListener::bind(&main_addr).await?;
    let ctrl_listener = TcpListener::bind(&ctrl_addr).await?;

    let state = omega_server::AppState::new(provider, args.sessions_root, args.public_dir);

    // Run the control server alongside the main server.
    let ctrl_app = control::router(history);
    let ctrl_handle = tokio::spawn(async move { axum::serve(ctrl_listener, ctrl_app).await });

    eprintln!("mock-omega-server: main on {main_addr}, control on {ctrl_addr}");

    // omega_server::serve installs SIGINT/SIGTERM handlers and runs the
    // graceful-shutdown sequence — exactly what Playwright triggers between
    // projects via `webServer.gracefulShutdown`.
    let result = omega_server::serve(main_listener, state).await;

    // After the main server exits, abort the control server and propagate.
    ctrl_handle.abort();
    result
}
