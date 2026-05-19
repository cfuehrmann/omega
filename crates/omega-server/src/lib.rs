//! omega-server — HTTP + WebSocket server for the Omega web UI.
//!
//! Phase 1e.1: adds `AppState`, `ActiveSession`, and the `/api/sessions`
//! GET + POST endpoints.  The `serve` function is extracted here so it can be
//! exercised directly by integration tests without spawning a process.
//!
//! Phase 1e.4: `serve` now installs SIGINT/SIGTERM handlers.  On signal it
//! aborts any running turn (with a 2 s drain deadline), appends a
//! `server_stopped` event to the active session's `events.jsonl`, then
//! lets `axum::serve` finish gracefully.
//!
//! Route map:
//! - `GET  /health`        — liveness probe (1e.0)
//! - `GET  /api/sessions`  — list sessions  (1e.1)
//! - `POST /api/sessions`  — create session (1e.1)
//! - `GET  /api/context`   — context-record lookup (1e.4)
//! - `GET  /api/files`     — file-completion (1e.4)
//! - `/ws`                 — WebSocket upgrade (1e.2)

pub mod cli;
pub mod router;
pub mod session;
pub mod ws_message;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use omega_store::EventStore;
use omega_types::OmegaEvent;
use omega_types::events::{ServerStopOutcome, ServerStoppedEvent};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

pub use cli::Args;
pub use router::build_router;
pub use session::ActiveSession;
pub use ws_message::WsMessage;

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
    /// Directory containing the built Leptos client bundle, served as
    /// the fallback `ServeDir` at `/`. Defaults to
    /// [`cli::DEFAULT_LEPTOS_DIR`]; override with
    /// [`AppState::with_leptos_dir`].
    pub leptos_dir: PathBuf,
    /// LLM provider.  `Arc<dyn Provider>` lets tests inject a
    /// `MockProvider` while the binary uses the real Anthropic provider.
    pub provider: Arc<dyn omega_core::Provider>,
    /// Server working directory, captured once at startup.
    ///
    /// Immutable property of the server process — captured in `main` via
    /// `std::env::current_dir()` and stored here so handlers can use it
    /// without repeating the syscall (and so tests can inject a custom
    /// path, e.g. a temporary git repository for dirty-tree tests).
    pub cwd: PathBuf,
}

impl AppState {
    /// Construct a fresh `AppState` with an empty session slot.
    ///
    /// `leptos_dir` defaults to [`cli::DEFAULT_LEPTOS_DIR`]; override
    /// with [`AppState::with_leptos_dir`] before calling [`serve`] /
    /// [`build_router`].
    pub fn new(
        provider: Arc<dyn omega_core::Provider>,
        sessions_root: PathBuf,
        cwd: PathBuf,
    ) -> Self {
        Self {
            active_session: Arc::new(Mutex::new(None)),
            sessions_root,
            leptos_dir: PathBuf::from(cli::DEFAULT_LEPTOS_DIR),
            provider,
            cwd,
        }
    }

    /// Override the directory containing the built Leptos client bundle.
    /// Returns `self` for builder-style chaining.
    #[must_use]
    pub fn with_leptos_dir(mut self, leptos_dir: PathBuf) -> Self {
        self.leptos_dir = leptos_dir;
        self
    }
}

/// Bind the router to `listener` and serve it until SIGINT/SIGTERM is
/// received.
///
/// On signal, [`perform_shutdown`] runs first — it aborts any in-flight
/// turn (with a 2 s drain deadline) and appends a `server_stopped`
/// event to the active session's `events.jsonl` — and only then does
/// `axum::serve` complete its graceful shutdown.
///
/// Extracted from `main` so integration tests can call it directly
/// without spawning a separate process.
///
/// # Errors
///
/// Propagates any I/O error returned by [`axum::serve`].
pub async fn serve(listener: TcpListener, state: AppState) -> std::io::Result<()> {
    let app = build_router(state.clone());
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(state))
        .await
}

/// Wait for SIGINT or SIGTERM (Unix) or Ctrl-C (other platforms).
///
/// Pulled out of `shutdown_signal` so it can be reused in tests.
async fn wait_for_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let Ok(mut term) = signal(SignalKind::terminate()) else {
            return;
        };
        let Ok(mut int_) = signal(SignalKind::interrupt()) else {
            return;
        };
        tokio::select! {
            _ = term.recv() => {},
            _ = int_.recv() => {},
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// Composed shutdown future passed to [`axum::serve::with_graceful_shutdown`].
///
/// Waits for a signal, then runs [`perform_shutdown`] before returning.
async fn shutdown_signal(state: AppState) {
    wait_for_signal().await;
    perform_shutdown(&state).await;
}

/// Maximum time the graceful-shutdown path waits for the running turn
/// task to finish after [`omega_agent::ControlHandle::request_abort`].
pub const TURN_DRAIN_DEADLINE: Duration = Duration::from_secs(2);

/// Perform the graceful-shutdown sequence:
///
/// 1. Snapshot the active session's [`omega_agent::ControlHandle`],
///    `events.jsonl` path, and `current_turn` `JoinHandle` (taking ownership
///    of the handle so we can `join` it without holding the lock).
/// 2. Call `request_abort` so the turn winds down at the next seam.
/// 3. Await the turn handle bounded by [`TURN_DRAIN_DEADLINE`].
/// 4. Append a `server_stopped` event to `events.jsonl`.
///
/// Public so tests can drive it without spawning a real process.
pub async fn perform_shutdown(state: &AppState) {
    let (controls, events_file, turn_handle) = {
        let mut slot = state.active_session.lock().await;
        match slot.as_mut() {
            Some(active) => (
                Some(active.controls.clone()),
                Some(active.paths.events_file.clone()),
                active.current_turn.take(),
            ),
            None => (None, None, None),
        }
    };
    if let Some(c) = controls {
        c.request_abort();
    }
    if let Some(handle) = turn_handle {
        let _ = tokio::time::timeout(TURN_DRAIN_DEADLINE, handle).await;
    }
    if let Some(events_file) = events_file {
        let store = EventStore::new(events_file);
        let ev = OmegaEvent::ServerStopped(ServerStoppedEvent {
            time: now_iso(),
            outcome: ServerStopOutcome::Clean,
            reason: None,
        });
        let _ = store.append(&ev).await;
    }
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    // Justification for inline test block: `now_iso` is a private formatting
    // helper that produces a timestamp for `server_stopped` events.  Its
    // output format (ISO-8601, millisecond precision, trailing `Z`) is part
    // of the persistence contract but cannot be asserted by WebSocket
    // integration tests — those tests would need to observe the timestamp
    // written into events.jsonl by the shutdown path, which requires
    // orchestrating a real SIGTERM.  A direct unit test is the right level.
    #![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

    use super::now_iso;

    /// `now_iso` must produce an ISO-8601 timestamp with millisecond
    /// precision and a trailing `Z` — the exact shape required by the
    /// `server_stopped` event schema.  Pinning the format here catches
    /// mutations that replace the helper with a constant or an empty
    /// string.
    #[test]
    fn now_iso_is_iso8601_utc_millis_with_trailing_z() {
        let s = now_iso();
        // YYYY-MM-DDTHH:MM:SS.sssZ — exactly 24 characters.
        assert_eq!(s.len(), 24, "unexpected length: {s:?}");
        assert!(s.ends_with('Z'), "must end with Z; got {s:?}");
        assert_eq!(s.as_bytes()[4], b'-', "date sep at idx 4: {s:?}");
        assert_eq!(s.as_bytes()[7], b'-', "date sep at idx 7: {s:?}");
        assert_eq!(s.as_bytes()[10], b'T', "date/time sep at idx 10: {s:?}");
        assert_eq!(s.as_bytes()[13], b':', "time sep at idx 13: {s:?}");
        assert_eq!(s.as_bytes()[16], b':', "time sep at idx 16: {s:?}");
        assert_eq!(s.as_bytes()[19], b'.', "frac sep at idx 19: {s:?}");
        // chrono::Utc::now() returns the system clock; tests will run in
        // a year that starts with "2" for the foreseeable future.
        assert!(s.starts_with('2'), "year should start with 2; got {s:?}");
        // The character classes for digits.
        for (i, b) in s.bytes().enumerate() {
            if matches!(i, 4 | 7 | 10 | 13 | 16 | 19 | 23) {
                continue;
            }
            assert!(
                b.is_ascii_digit(),
                "byte {i} ({b}) must be a digit in {s:?}",
            );
        }
    }
}
