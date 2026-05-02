//! Axum router construction and route handlers.
//!
//! Phase 1e.1 implements `GET /api/sessions` and `POST /api/sessions`.
//! The three remaining placeholder routes (`/ws`, `/context`, `/files`)
//! still return `501 Not Implemented`.

use std::path::Path;
use std::sync::{Arc, OnceLock};

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{any, get},
};
use omega_agent::{Agent, AgentConfig};
use omega_store::{ContextStore, EventStore, session_dir_re};
use regex::Regex;
use serde::Serialize;
use tower_http::services::ServeDir;

use crate::AppState;
use crate::session::ActiveSession;

// ---------------------------------------------------------------------------
// Router construction
// ---------------------------------------------------------------------------

/// Build the top-level [`Router`] using `state` for all stateful handlers.
///
/// The `public_dir` inside `state` is wrapped in [`ServeDir`] as the
/// fallback service so any unmatched path is served from disk (404 if
/// the file doesn't exist).
pub fn build_router(state: AppState) -> Router {
    let public_dir = state.public_dir.clone();
    Router::new()
        .route("/health", get(health))
        .route("/api/sessions", get(get_sessions).post(post_session))
        .route("/ws", any(not_implemented))
        .route("/context", any(not_implemented))
        .route("/files", any(not_implemented))
        .fallback_service(ServeDir::new(public_dir))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Shared handlers
// ---------------------------------------------------------------------------

/// `GET /health` — liveness probe.
async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

/// Placeholder for routes whose real implementation lands in later sub-phases.
async fn not_implemented() -> StatusCode {
    StatusCode::NOT_IMPLEMENTED
}

// ---------------------------------------------------------------------------
// Session list item — `GET /api/sessions`
// ---------------------------------------------------------------------------

/// One entry in the `GET /api/sessions` JSON array.
///
/// Mirrors the `SessionListItem` interface in `src/web/server.ts`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionListItem {
    /// Session directory name, e.g. `"2025-07-11T09-14-22-037-a8c3f1b2"`.
    pub dir: String,
    /// ISO-8601 timestamp derived from `dir` by re-inserting colons/dots.
    pub last_activity: String,
    /// Human-readable name set via `omega_store::update_session_metadata`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Optional description field from session metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Dir name of the session this one was resumed from, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resumed_from: Option<String>,
}

// ---------------------------------------------------------------------------
// `GET /api/sessions`
// ---------------------------------------------------------------------------

/// Convert a session folder name to an ISO-8601 timestamp string.
///
/// `"2025-07-11T09-14-22-037-a8c3f1b2"` → `"2025-07-11T09:14:22.037Z"`
///
/// Returns the original `name` unchanged if it does not match the expected
/// pattern (i.e. the function is total and never panics).
fn folder_name_to_timestamp(name: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        #[allow(clippy::unwrap_used)]
        Regex::new(r"^(\d{4}-\d{2}-\d{2})T(\d{2})-(\d{2})-(\d{2})(?:-(\d{3}))?").unwrap()
    });

    if let Some(caps) = re.captures(name) {
        let date = caps.get(1).map_or("", |m| m.as_str());
        let h = caps.get(2).map_or("", |m| m.as_str());
        let min = caps.get(3).map_or("", |m| m.as_str());
        let s = caps.get(4).map_or("", |m| m.as_str());
        match caps.get(5) {
            Some(ms) => format!("{date}T{h}:{min}:{s}.{}Z", ms.as_str()),
            None => format!("{date}T{h}:{min}:{s}Z"),
        }
    } else {
        name.to_owned()
    }
}

/// Enumerate session directories under `sessions_root`, sort newest-first,
/// and attach metadata.
///
/// Returns an empty vec if `sessions_root` does not exist or cannot be read.
///
/// This is a Rust port of `listSessions()` in `src/web/server.ts`.
pub async fn list_sessions(sessions_root: &Path) -> Vec<SessionListItem> {
    let Ok(mut dir_reader) = tokio::fs::read_dir(sessions_root).await else {
        return Vec::new();
    };

    let mut names: Vec<String> = Vec::new();
    while let Ok(Some(entry)) = dir_reader.next_entry().await {
        if let Some(name) = entry.file_name().to_str()
            && session_dir_re().is_match(name)
        {
            names.push(name.to_owned());
        }
    }

    // Sort lexicographically; because the folder names are ISO-like timestamps
    // with the same character width, lexicographic order equals chronological
    // order.  Reverse for newest-first.
    names.sort_unstable();
    names.reverse();

    let mut items = Vec::with_capacity(names.len());
    for name in &names {
        let full_path = sessions_root.join(name);
        let meta = omega_store::read_session_metadata(&full_path).await;
        items.push(SessionListItem {
            dir: name.clone(),
            last_activity: folder_name_to_timestamp(name),
            name: meta.name,
            description: meta.description,
            resumed_from: meta.resumed_from,
        });
    }
    items
}

async fn get_sessions(State(state): State<AppState>) -> Response {
    let items = list_sessions(&state.sessions_root).await;
    (StatusCode::OK, Json(items)).into_response()
}

// ---------------------------------------------------------------------------
// `POST /api/sessions`
// ---------------------------------------------------------------------------

async fn post_session(State(state): State<AppState>) -> Response {
    // 1. Create the session directory and canonical file paths.
    let paths = match omega_store::make_session_dir(&state.sessions_root).await {
        Ok(p) => p,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("make_session_dir failed: {e}"),
            )
                .into_response();
        }
    };

    let dir_name = paths
        .dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    // 2. Build the agent.
    let context_store = ContextStore::new(paths.context_file.clone());
    let event_store = EventStore::new(paths.events_file.clone());
    let cwd = std::env::current_dir().unwrap_or_default();
    let config = AgentConfig {
        model: "claude-sonnet-4-6".to_owned(),
        cwd,
        system_prompt_append: None,
        session_dir: paths.dir.clone(),
    };
    let agent = Agent::new(
        Arc::clone(&state.provider),
        context_store,
        event_store,
        config,
    );

    // 3. Write server_started + session_started events.
    if let Err(e) = agent.init().await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("agent.init() failed: {e}"),
        )
            .into_response();
    }

    let controls = agent.controls();

    // 4. Replace the active session slot (single-session model).
    let active = ActiveSession {
        agent: Arc::new(tokio::sync::Mutex::new(agent)),
        controls,
        paths,
        ws_tx: None,
    };
    *state.active_session.lock().await = Some(active);

    // 5. Return 201 Created with the directory name.
    (
        StatusCode::CREATED,
        Json(serde_json::json!({ "dir": dir_name })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Unit tests for the pure helpers
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::folder_name_to_timestamp;

    #[test]
    fn timestamp_conversion_with_millis() {
        assert_eq!(
            folder_name_to_timestamp("2025-07-11T09-14-22-037-a8c3f1b2"),
            "2025-07-11T09:14:22.037Z"
        );
    }

    #[test]
    fn timestamp_conversion_without_millis() {
        assert_eq!(
            folder_name_to_timestamp("2025-07-11T09-14-22"),
            "2025-07-11T09:14:22Z"
        );
    }

    #[test]
    fn timestamp_conversion_passthrough_for_non_matching() {
        assert_eq!(folder_name_to_timestamp("not-a-date"), "not-a-date");
    }
}
