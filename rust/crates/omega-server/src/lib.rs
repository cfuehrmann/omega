//! omega-server — HTTP + WebSocket server for the Omega web UI.
//!
//! Phase 1e.1: adds `AppState`, `ActiveSession`, and the `/api/sessions`
//! GET + POST endpoints.  The `serve` function is extracted here so it can be
//! exercised directly by integration tests without spawning a process.
//!
//! Route map:
//! - `GET  /health`        — liveness probe (1e.0)
//! - `GET  /api/sessions`  — list sessions  (1e.1)
//! - `POST /api/sessions`  — create session (1e.1)
//! - `/ws`                 — WebSocket upgrade placeholder (1e.2)
//! - `/context`            — context-record lookup placeholder (1e.4)
//! - `/files`              — file-completion placeholder (1e.4)

pub mod cli;
pub mod router;
pub mod session;

use std::path::PathBuf;
use std::sync::Arc;

use tokio::net::TcpListener;
use tokio::sync::Mutex;

pub use cli::Args;
pub use router::build_router;
pub use session::ActiveSession;

/// Shared state threaded through the Axum router via [`Router::with_state`].
///
/// `Clone` is required by Axum — `Arc` fields are cheaply cloned.
#[derive(Clone)]
pub struct AppState {
    /// The currently-active session slot.  `None` until the first
    /// `POST /api/sessions`; replaced on every subsequent call.
    pub active_session: Arc<Mutex<Option<ActiveSession>>>,
    /// Root directory that contains the per-session sub-folders.
    pub sessions_root: PathBuf,
    /// Directory served as static files by the fallback `ServeDir` handler.
    pub public_dir: PathBuf,
    /// LLM provider.  `Arc<dyn Provider>` lets tests inject a
    /// `MockProvider` while the binary uses the real Anthropic provider.
    pub provider: Arc<dyn omega_core::Provider>,
}

impl AppState {
    /// Construct a fresh `AppState` with an empty session slot.
    pub fn new(
        provider: Arc<dyn omega_core::Provider>,
        sessions_root: PathBuf,
        public_dir: PathBuf,
    ) -> Self {
        Self {
            active_session: Arc::new(Mutex::new(None)),
            sessions_root,
            public_dir,
            provider,
        }
    }
}

/// Bind the router to `listener` and serve it until the process exits.
///
/// Extracted from `main` so integration tests can call it directly without
/// spawning a separate process.  `main` is pure glue and carries
/// `#[mutants::skip]`; this function is covered by the test suite.
///
/// # Errors
///
/// Propagates any I/O error returned by [`axum::serve`].
pub async fn serve(listener: TcpListener, state: AppState) -> std::io::Result<()> {
    let app = build_router(state);
    axum::serve(listener, app).await
}
