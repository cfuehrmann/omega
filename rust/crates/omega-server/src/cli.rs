//! Command-line arguments for the `omega-server` binary.
//!
//! Defaults:
//! - `--port`          — `3000`.
//! - `--sessions-root` — `.omega/sessions` relative to the working directory.
//! - `--leptos-dir`    — `frontends/leptos/dist` relative to the binary location.
//!   Resolved at runtime via `current_exe()`; falls back to CWD-relative if
//!   resolution fails.
//! - `--working-dir`   — not set (defaults to the process CWD).

use std::path::PathBuf;

use clap::Parser;

/// Default port. Matches `PORT` in `src/web/server.ts` (which itself
/// falls back to `3000` when no `--port` flag or `PORT` env is set).
pub const DEFAULT_PORT: u16 = 3000;

/// Default sessions root, relative to the process cwd.
/// Re-exports [`omega_store::SESSIONS_ROOT`] so that callers have a single
/// canonical constant and tests can assert they are identical.
pub const DEFAULT_SESSIONS_ROOT: &str = omega_store::SESSIONS_ROOT;

/// Fallback Leptos `dist/` directory used when binary-relative resolution
/// fails.  The real default is computed at runtime in `main.rs` from
/// `current_exe()` (four parent directories up, then `frontends/leptos/dist`).
/// This constant is only reached if the binary cannot determine its own path.
pub const DEFAULT_LEPTOS_DIR: &str = "frontends/leptos/dist";

/// Parsed `omega-server` command-line arguments.
#[derive(Parser, Debug, Clone)]
#[command(
    name = "omega-server",
    about = "Omega web UI server (Rust port of src/web/server.ts)"
)]
pub struct Args {
    /// TCP port to bind on `0.0.0.0`.
    #[arg(long, default_value_t = DEFAULT_PORT)]
    pub port: u16,

    /// Root directory containing per-session folders (`<root>/<timestamp>-<rand>/`).
    #[arg(long, default_value = DEFAULT_SESSIONS_ROOT)]
    pub sessions_root: PathBuf,

    /// Directory containing the built Leptos client bundle.
    /// Defaults to `frontends/leptos/dist` relative to the omega-server binary
    /// (resolved at runtime). Only needed if the binary is moved away from the
    /// omega repo tree.
    #[arg(long)]
    pub leptos_dir: Option<PathBuf>,

    /// Working directory for the agent. The server changes its CWD to this
    /// path at startup, so sessions are stored there and the agent operates
    /// on that folder. Defaults to the current directory.
    #[arg(long)]
    pub working_dir: Option<PathBuf>,
}
