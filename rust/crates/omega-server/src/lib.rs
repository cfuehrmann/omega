//! omega-server — HTTP + WebSocket server for the Omega web UI.
//!
//! Phase 1e.0 scaffolding: serves a `GET /health` probe, static files via
//! `tower_http::services::ServeDir`, and 501-Not-Implemented placeholders
//! for the routes that later sub-phases will fill in:
//!
//! - `/api/sessions` — session list + create (1e.1)
//! - `/ws`          — WebSocket upgrade (1e.2)
//! - `/context`     — context-record lookup (1e.4)
//! - `/files`       — file completion for the @-picker (1e.4)
//!
//! The single-session, single-WebSocket model documented in
//! `rust-migration.md` (Phase 1e — *Important: TS server is single-session,
//! single-WS*) lands in 1e.1; this crate currently exposes only a stateless
//! router so that the binary, CLI flags, and static-asset serving can be
//! validated independently.

pub mod cli;
pub mod router;

pub use cli::Args;
pub use router::build_router;
