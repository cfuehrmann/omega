//! Axum router construction.
//!
//! `build_router` wires the routes documented in `lib.rs`. All non-static
//! routes except `/health` currently return `501 Not Implemented` — they
//! are placeholders for the handlers landing in 1e.1 – 1e.4.

use std::path::Path;

use axum::{
    Json, Router,
    http::StatusCode,
    routing::{any, get},
};
use tower_http::services::ServeDir;

/// Build the top-level `Router`.
///
/// `public_dir` is wrapped in [`ServeDir`] and installed as the fallback
/// service so that any path not matched by an explicit route is served
/// from disk (returning 404 if the file does not exist).
pub fn build_router(public_dir: &Path) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/api/sessions", any(not_implemented))
        .route("/ws", any(not_implemented))
        .route("/context", any(not_implemented))
        .route("/files", any(not_implemented))
        .fallback_service(ServeDir::new(public_dir))
}

/// `GET /health` — liveness probe.
///
/// Returns `200 OK` with body `{"status":"ok"}` and
/// `Content-Type: application/json` (set by `axum::Json`).
async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

/// Placeholder handler for routes whose real implementation lands in
/// later 1e sub-phases. Returns `501 Not Implemented` with no body.
async fn not_implemented() -> StatusCode {
    StatusCode::NOT_IMPLEMENTED
}
