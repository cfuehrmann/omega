//! Command-line arguments for the `omega-server` binary.
//!
//! Defaults match the TS server (`src/web/server.ts`):
//! - `--port`          — `3000` (matches the TS `PORT` default).
//! - `--sessions-root` — `.omega/sessions` (matches `omega_store::SESSIONS_ROOT`).
//! - `--public-dir`    — `src/web/public/` relative to cwd; this is where
//!   `vite build` writes its bundle and where the TS server reads from.

use std::path::PathBuf;

use clap::Parser;

/// Default port. Matches `PORT` in `src/web/server.ts` (which itself
/// falls back to `3000` when no `--port` flag or `PORT` env is set).
pub const DEFAULT_PORT: u16 = 3000;

/// Default sessions root, relative to the process cwd.
/// Matches [`omega_store::SESSIONS_ROOT`] without the cross-crate dep —
/// the constant value is asserted against `omega_store` in an integration
/// test once 1e.1 wires up real session I/O.
pub const DEFAULT_SESSIONS_ROOT: &str = ".omega/sessions";

/// Default static-assets directory, relative to the process cwd.
/// Matches the directory `vite build` writes to (`src/web/public/`).
pub const DEFAULT_PUBLIC_DIR: &str = "src/web/public/";

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

    /// Directory containing the built static web client bundle.
    #[arg(long, default_value = DEFAULT_PUBLIC_DIR)]
    pub public_dir: PathBuf,
}
