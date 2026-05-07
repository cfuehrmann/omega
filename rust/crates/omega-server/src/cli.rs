//! Command-line arguments for the `omega-server` binary.
//!
//! Defaults:
//! - `--port`          — `3000`.
//! - `--sessions-root` — `.omega/sessions` (matches `omega_store::SESSIONS_ROOT`).
//! - `--leptos-dir`    — `frontends/leptos/dist` (Trunk's output directory).

use std::path::PathBuf;

use clap::Parser;

/// Default port. Matches `PORT` in `src/web/server.ts` (which itself
/// falls back to `3000` when no `--port` flag or `PORT` env is set).
pub const DEFAULT_PORT: u16 = 3000;

/// Default sessions root, relative to the process cwd.
/// Re-exports [`omega_store::SESSIONS_ROOT`] so that callers have a single
/// canonical constant and tests can assert they are identical.
pub const DEFAULT_SESSIONS_ROOT: &str = omega_store::SESSIONS_ROOT;

/// Default Leptos `dist/` directory, relative to the process cwd.
/// Populated by `just web-leptos-build`. Served by [`crate::build_router`]
/// as the fallback `ServeDir`. If the directory does not exist at runtime
/// the route simply 404s — non-fatal.
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
    /// Served as the fallback `ServeDir` at `/`.
    #[arg(long, default_value = DEFAULT_LEPTOS_DIR)]
    pub leptos_dir: PathBuf,
}
