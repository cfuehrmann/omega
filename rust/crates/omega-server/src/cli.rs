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
/// Re-exports [`omega_store::SESSIONS_ROOT`] so that callers have a single
/// canonical constant and tests can assert they are identical.
pub const DEFAULT_SESSIONS_ROOT: &str = omega_store::SESSIONS_ROOT;

/// Default static-assets directory, relative to the process cwd.
/// Matches the directory `vite build` writes to (`src/web/public/`).
pub const DEFAULT_PUBLIC_DIR: &str = "src/web/public/";

/// Default Leptos `dist/` directory, relative to the process cwd.
/// Populated by `just web-leptos-build` (Phase 3.0). Mounted by
/// [`crate::build_router`] under `/leptos/`. If the directory does not
/// exist at runtime the route simply 404s — non-fatal.
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

    /// Directory containing the built static web client bundle.
    #[arg(long, default_value = DEFAULT_PUBLIC_DIR)]
    pub public_dir: PathBuf,

    /// Directory containing the built Leptos client bundle (Phase 3.0).
    /// Mounted under `/leptos/`; the existing `--public-dir` continues
    /// to serve `/`.
    #[arg(long, default_value = DEFAULT_LEPTOS_DIR)]
    pub leptos_dir: PathBuf,
}
