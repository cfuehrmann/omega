//! `omega-server` binary entry point.
//!
//! Phase 1e.0: parses CLI flags, logs the resolved configuration, binds
//! `0.0.0.0:<port>`, and serves the router from `omega_server::build_router`.
//! All real session/agent/WebSocket handling lands in 1e.1 – 1e.4.

use clap::Parser as _;
use omega_server::{Args, build_router};

/// All logic lives in helpers in `lib.rs`; `main` is pure glue. Marked
/// `#[mutants::skip]` because mutating the bind/serve glue cannot be
/// caught without spawning a real process — `build_router` and `Args`
/// are exhaustively covered by integration tests instead.
#[mutants::skip]
#[tokio::main]
async fn main() -> std::io::Result<()> {
    let args = Args::parse();
    eprintln!(
        "omega-server: starting on 0.0.0.0:{} (sessions_root={}, public_dir={})",
        args.port,
        args.sessions_root.display(),
        args.public_dir.display(),
    );
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", args.port)).await?;
    let app = build_router(&args.public_dir);
    axum::serve(listener, app).await
}
