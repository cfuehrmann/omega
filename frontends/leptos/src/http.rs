//! HTTP fetch helpers — the JS-interop edge of the Leptos client.
//!
//! ## Mutation-testing carve-out
//!
//! Following the `ws.rs` precedent (Phase 3.1), this module is treated
//! as a JS-interop edge: thin glue around `gloo-net`, untestable
//! without a real browser HTTP stack. `cargo mutants` runs against it
//! are expected to leave the network-bound mutants surviving — same
//! gap as `ws.rs::WsClient::send` / `ws_url_from_window`.
//!
//! Picker-side code that needs to *react* to fetch results goes into
//! [`crate::sessions::SessionListStore`] (pure), not here.
//!
//! ## Why `gloo-net`
//!
//! `gloo-net` 0.6 is already a transitive dep via `leptos`'s
//! `server_fn`. Pinning it as a direct dep with the same version costs
//! ~zero bytes (LTO already linked the relevant code paths). The
//! ergonomics win is significant — see `ws_url_from_window` for the
//! `web_sys::Request` alternative we'd otherwise be writing.

use gloo_net::http::Request;

use crate::sessions::SessionListItem;

/// `GET /api/sessions` → `Vec<SessionListItem>`.
///
/// # Errors
///
/// Returns `Err(message)` for any network-level failure (request build,
/// fetch error, non-2xx response, JSON decode failure). The message
/// is intended for direct display in `SessionListStore::last_error`.
pub async fn get_sessions() -> Result<Vec<SessionListItem>, String> {
    let resp = Request::get("/api/sessions")
        .send()
        .await
        .map_err(|e| format!("GET /api/sessions failed: {e}"))?;

    if !resp.ok() {
        return Err(format!("GET /api/sessions: HTTP {}", resp.status()));
    }

    resp.json::<Vec<SessionListItem>>()
        .await
        .map_err(|e| format!("GET /api/sessions: decode failed: {e}"))
}

/// `GET /api/files?prefix=...` → `Vec<String>`.
///
/// Each returned entry is a path completion ready to be pasted after
/// the `@` in the composer (directories include their trailing `/`,
/// subdir prefixes preserve the leading path component). The server
/// caps the response at `MAX_FILE_COMPLETIONS = 50`.
///
/// # Errors
///
/// Returns `Err(message)` for any network-level failure. Same
/// JS-interop carve-out as [`get_sessions`].
pub async fn get_files(prefix: &str) -> Result<Vec<String>, String> {
    // Build the URL via `Request::get` with a query parameter so
    // we get URL-encoding for free (e.g. spaces, `&`).
    let resp = Request::get("/api/files")
        .query([("prefix", prefix)])
        .send()
        .await
        .map_err(|e| format!("GET /api/files failed: {e}"))?;

    if !resp.ok() {
        return Err(format!("GET /api/files: HTTP {}", resp.status()));
    }

    resp.json::<Vec<String>>()
        .await
        .map_err(|e| format!("GET /api/files: decode failed: {e}"))
}
