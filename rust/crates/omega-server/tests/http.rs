//! Integration tests for the omega-server HTTP API.
//!
//! Phase 1e.1: tests cover the `/api/sessions` GET and POST endpoints,
//! session listing, newest-first ordering, and metadata propagation.
//! Phase 1e.0 tests (health, static files, CLI flags) are preserved.
//!
//! Each test binds `127.0.0.1:0` so parallel runs cannot collide on a port.
//! Session dirs accumulate in `TempDir` and are cleaned up when the test
//! scope ends.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use futures::stream::BoxStream;
use omega_core::{AgentItem, AgentItemStream, LlmError, LlmRequest, Provider};
use omega_server::{AppState, Args, build_router, serve};
use tempfile::TempDir;
use tokio::net::TcpListener;

// ---------------------------------------------------------------------------
// MockProvider — zero-response LLM stub for tests that construct agents.
// The session POST creates an Agent but never sends a message, so the mock
// never needs to produce real chunks.
// ---------------------------------------------------------------------------

struct MockProvider;

impl Provider for MockProvider {
    fn stream(&self, _req: LlmRequest) -> AgentItemStream {
        let stream: BoxStream<'static, Result<AgentItem, LlmError>> =
            Box::pin(futures::stream::empty());
        stream
    }
}

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Build an [`AppState`] backed by a [`MockProvider`] suitable for tests.
fn make_test_state(sessions_root: PathBuf, public_dir: PathBuf) -> AppState {
    AppState::new(Arc::new(MockProvider), sessions_root, public_dir)
}

/// Spawn `build_router(state)` on a random local port and return its bound
/// address.  The server task is detached; `TempDir` cleanup runs when the
/// caller drops the returned guard.
async fn spawn_server(state: AppState) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind 127.0.0.1:0");
    let addr = listener.local_addr().expect("local_addr");
    let app = build_router(state);
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("axum::serve");
    });
    addr
}

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .build()
        .expect("reqwest client build")
}

// ---------------------------------------------------------------------------
// /health (preserved from 1e.0)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn health_returns_200_with_json_status_ok() {
    let tmp = TempDir::new().expect("tempdir");
    let state = make_test_state(tmp.path().join("sessions"), tmp.path().to_path_buf());
    let addr = spawn_server(state).await;

    let resp = http_client()
        .get(format!("http://{addr}/health"))
        .send()
        .await
        .expect("GET /health");

    assert_eq!(resp.status().as_u16(), 200, "status");
    let ct = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    assert!(
        ct.starts_with("application/json"),
        "Content-Type was {ct:?}"
    );
    let body: serde_json::Value = resp.json().await.expect("decode json");
    assert_eq!(body, serde_json::json!({ "status": "ok" }));
}

// ---------------------------------------------------------------------------
// Placeholder routes — /context, /files still return 501.
// /api/sessions and /ws are NO LONGER placeholders.  /ws is exercised by
// the dedicated `tests/ws.rs` integration suite (Phase 1e.2).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn non_session_placeholder_routes_return_501() {
    let tmp = TempDir::new().expect("tempdir");
    let state = make_test_state(tmp.path().join("sessions"), tmp.path().to_path_buf());
    let addr = spawn_server(state).await;
    let client = http_client();

    for path in ["/context", "/files"] {
        let resp = client
            .get(format!("http://{addr}{path}"))
            .send()
            .await
            .unwrap_or_else(|e| panic!("GET {path}: {e}"));
        assert_eq!(
            resp.status().as_u16(),
            501,
            "expected 501 for {path}, got {}",
            resp.status(),
        );
    }
}

#[tokio::test]
async fn non_session_placeholders_accept_post_too() {
    let tmp = TempDir::new().expect("tempdir");
    let state = make_test_state(tmp.path().join("sessions"), tmp.path().to_path_buf());
    let addr = spawn_server(state).await;
    let client = http_client();

    for path in ["/context", "/files"] {
        let resp = client
            .post(format!("http://{addr}{path}"))
            .send()
            .await
            .unwrap_or_else(|e| panic!("POST {path}: {e}"));
        assert_eq!(
            resp.status().as_u16(),
            501,
            "expected 501 for POST {path}, got {}",
            resp.status(),
        );
    }
}

// ---------------------------------------------------------------------------
// Static file serving (preserved from 1e.0)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn servedir_serves_files_from_public_dir() {
    let tmp = TempDir::new().expect("tempdir");
    let file_path = tmp.path().join("hello.txt");
    std::fs::write(&file_path, "omega-rocks").expect("write tempfile");

    let state = make_test_state(tmp.path().join("sessions"), tmp.path().to_path_buf());
    let addr = spawn_server(state).await;

    let resp = http_client()
        .get(format!("http://{addr}/hello.txt"))
        .send()
        .await
        .expect("GET /hello.txt");
    assert_eq!(resp.status().as_u16(), 200);
    let body = resp.text().await.expect("body");
    assert_eq!(body, "omega-rocks");
}

#[tokio::test]
async fn servedir_returns_404_for_missing_file() {
    let tmp = TempDir::new().expect("tempdir");
    let state = make_test_state(tmp.path().join("sessions"), tmp.path().to_path_buf());
    let addr = spawn_server(state).await;

    let resp = http_client()
        .get(format!("http://{addr}/does-not-exist.html"))
        .send()
        .await
        .expect("GET missing");
    assert_eq!(resp.status().as_u16(), 404);
}

// ---------------------------------------------------------------------------
// CLI parsing (preserved from 1e.0, extended with omega_store assertion)
// ---------------------------------------------------------------------------

#[test]
fn args_defaults_match_documented_constants() {
    use clap::Parser as _;
    let args = Args::parse_from(["omega-server"]);
    assert_eq!(args.port, omega_server::cli::DEFAULT_PORT);
    assert_eq!(args.port, 3000);
    assert_eq!(
        args.sessions_root,
        PathBuf::from(omega_server::cli::DEFAULT_SESSIONS_ROOT),
    );
    assert_eq!(args.sessions_root, PathBuf::from(".omega/sessions"));
    // Phase 1e.1: DEFAULT_SESSIONS_ROOT must equal omega_store::SESSIONS_ROOT.
    assert_eq!(
        omega_server::cli::DEFAULT_SESSIONS_ROOT,
        omega_store::SESSIONS_ROOT,
        "DEFAULT_SESSIONS_ROOT must not duplicate the omega_store constant"
    );
    assert_eq!(
        args.public_dir,
        PathBuf::from(omega_server::cli::DEFAULT_PUBLIC_DIR),
    );
    assert_eq!(args.public_dir, PathBuf::from("src/web/public/"));
}

#[test]
fn args_accept_all_three_overrides() {
    use clap::Parser as _;
    let args = Args::parse_from([
        "omega-server",
        "--port",
        "4242",
        "--sessions-root",
        "/tmp/custom-sessions",
        "--public-dir",
        "/var/www/omega",
    ]);
    assert_eq!(args.port, 4242);
    assert_eq!(args.sessions_root, PathBuf::from("/tmp/custom-sessions"));
    assert_eq!(args.public_dir, PathBuf::from("/var/www/omega"));
}

#[test]
fn args_reject_invalid_port() {
    use clap::Parser as _;
    let res = Args::try_parse_from(["omega-server", "--port", "70000"]);
    assert!(res.is_err(), "expected port=70000 to be rejected");
}

// ---------------------------------------------------------------------------
// POST /api/sessions — Phase 1e.1
// ---------------------------------------------------------------------------

/// POST /api/sessions → 201 Created; returned dir exists on disk and
/// `events.jsonl` is non-empty (proof that `Agent::init()` ran).
#[tokio::test]
async fn post_session_creates_dir_and_returns_201() {
    let tmp = TempDir::new().expect("tempdir");
    let sessions_root = tmp.path().join("sessions");
    let state = make_test_state(sessions_root.clone(), tmp.path().to_path_buf());
    let addr = spawn_server(state).await;

    let resp = http_client()
        .post(format!("http://{addr}/api/sessions"))
        .send()
        .await
        .expect("POST /api/sessions");

    assert_eq!(resp.status().as_u16(), 201, "expected 201 Created");

    let body: serde_json::Value = resp.json().await.expect("decode json");
    let dir_name = body["dir"].as_str().expect("dir field missing");
    assert!(!dir_name.is_empty(), "dir name must not be empty");

    // Directory was created.
    let session_dir = sessions_root.join(dir_name);
    assert!(
        session_dir.is_dir(),
        "session dir must exist: {session_dir:?}"
    );

    // events.jsonl exists and is non-empty (Agent::init wrote events).
    let events_file = session_dir.join("events.jsonl");
    assert!(events_file.is_file(), "events.jsonl must exist");
    let content = std::fs::read_to_string(&events_file).expect("read events.jsonl");
    assert!(
        !content.trim().is_empty(),
        "events.jsonl must be non-empty after Agent::init()"
    );
}

/// `GET /api/sessions` against a sessions root that does not exist returns
/// `200 OK` with an empty JSON array — identical to the TS server behaviour.
#[tokio::test]
async fn get_sessions_returns_empty_array_for_nonexistent_root() {
    let tmp = TempDir::new().expect("tempdir");
    let nonexistent = tmp.path().join("no-such-dir");
    let state = make_test_state(nonexistent, tmp.path().to_path_buf());
    let addr = spawn_server(state).await;

    let resp = http_client()
        .get(format!("http://{addr}/api/sessions"))
        .send()
        .await
        .expect("GET /api/sessions");

    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await.expect("decode json");
    assert_eq!(body, serde_json::json!([]), "expected empty array");
}

/// Two POSTs → `GET /api/sessions` returns an array of exactly two items.
#[tokio::test]
async fn get_sessions_returns_two_after_two_posts() {
    let tmp = TempDir::new().expect("tempdir");
    let sessions_root = tmp.path().join("sessions");
    let state = make_test_state(sessions_root.clone(), tmp.path().to_path_buf());
    let addr = spawn_server(state).await;
    let client = http_client();

    // First session
    let r1 = client
        .post(format!("http://{addr}/api/sessions"))
        .send()
        .await
        .expect("POST 1");
    assert_eq!(r1.status().as_u16(), 201);

    // Small delay to ensure distinct directory timestamps.
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;

    // Second session
    let r2 = client
        .post(format!("http://{addr}/api/sessions"))
        .send()
        .await
        .expect("POST 2");
    assert_eq!(r2.status().as_u16(), 201);

    // List
    let resp = client
        .get(format!("http://{addr}/api/sessions"))
        .send()
        .await
        .expect("GET /api/sessions");
    assert_eq!(resp.status().as_u16(), 200);

    let body: serde_json::Value = resp.json().await.expect("decode json");
    let arr = body.as_array().expect("expected JSON array");
    assert_eq!(arr.len(), 2, "expected 2 sessions, got {}", arr.len());
}

/// After two POSTs the list is sorted newest-first.
#[tokio::test]
async fn get_sessions_newest_first() {
    let tmp = TempDir::new().expect("tempdir");
    let sessions_root = tmp.path().join("sessions");
    let state = make_test_state(sessions_root.clone(), tmp.path().to_path_buf());
    let addr = spawn_server(state).await;
    let client = http_client();

    let r1 = client
        .post(format!("http://{addr}/api/sessions"))
        .send()
        .await
        .expect("POST 1");
    let body1: serde_json::Value = r1.json().await.expect("json");
    let dir1 = body1["dir"].as_str().expect("dir").to_owned();

    tokio::time::sleep(std::time::Duration::from_millis(5)).await;

    let r2 = client
        .post(format!("http://{addr}/api/sessions"))
        .send()
        .await
        .expect("POST 2");
    let body2: serde_json::Value = r2.json().await.expect("json");
    let dir2 = body2["dir"].as_str().expect("dir").to_owned();

    // dir2 was created later → must appear first in the list.
    let resp = client
        .get(format!("http://{addr}/api/sessions"))
        .send()
        .await
        .expect("GET /api/sessions");
    let list: serde_json::Value = resp.json().await.expect("json");
    let arr = list.as_array().expect("array");
    assert_eq!(arr.len(), 2);

    let first_dir = arr[0]["dir"].as_str().expect("dir[0]");
    let second_dir = arr[1]["dir"].as_str().expect("dir[1]");

    assert_eq!(first_dir, dir2, "newest session must be first");
    assert_eq!(second_dir, dir1, "oldest session must be last");
}

/// Renaming a session via `omega_store::update_session_metadata` then listing
/// returns the updated `name` field in the response.
#[tokio::test]
async fn get_sessions_includes_metadata_after_rename() {
    let tmp = TempDir::new().expect("tempdir");
    let sessions_root = tmp.path().join("sessions");
    let state = make_test_state(sessions_root.clone(), tmp.path().to_path_buf());
    let addr = spawn_server(state).await;
    let client = http_client();

    // Create a session.
    let r = client
        .post(format!("http://{addr}/api/sessions"))
        .send()
        .await
        .expect("POST /api/sessions");
    assert_eq!(r.status().as_u16(), 201);
    let body: serde_json::Value = r.json().await.expect("json");
    let dir_name = body["dir"].as_str().expect("dir").to_owned();

    // Rename the session.
    let session_dir = sessions_root.join(&dir_name);
    omega_store::update_session_metadata(
        &session_dir,
        omega_store::SessionMetadata {
            name: Some("my-renamed-session".to_owned()),
            ..Default::default()
        },
    )
    .await
    .expect("update_session_metadata");

    // List and verify the name appears.
    let resp = client
        .get(format!("http://{addr}/api/sessions"))
        .send()
        .await
        .expect("GET /api/sessions");
    let list: serde_json::Value = resp.json().await.expect("json");
    let arr = list.as_array().expect("array");
    assert_eq!(arr.len(), 1);

    let entry = &arr[0];
    assert_eq!(entry["dir"].as_str().expect("dir"), dir_name);
    assert_eq!(
        entry["name"].as_str().expect("name field"),
        "my-renamed-session"
    );
}

// ---------------------------------------------------------------------------
// serve() function — called directly to catch the "replace with Ok(())" mutant
// ---------------------------------------------------------------------------

/// Calling `omega_server::serve` actually starts the Axum server.
///
/// This test exists solely to catch the `replace serve → Ok(())` mutant:
/// if `serve` short-circuits without calling `axum::serve`, the `TcpListener`
/// is dropped and the GET returns `ECONNREFUSED`.
#[tokio::test]
async fn serve_function_starts_real_http_listener() {
    let tmp = TempDir::new().expect("tempdir");
    let state = make_test_state(tmp.path().join("sessions"), tmp.path().to_path_buf());
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind 127.0.0.1:0");
    let addr = listener.local_addr().expect("local_addr");
    tokio::spawn(async move {
        serve(listener, state).await.expect("serve");
    });
    // The listener was already bound; the server task accepts on it.
    // No sleep needed — if serve() became Ok(()) the socket is dropped and
    // the request will fail with a connection error, catching the mutant.
    let resp = http_client()
        .get(format!("http://{addr}/health"))
        .send()
        .await
        .expect("GET /health via serve()");
    assert_eq!(
        resp.status().as_u16(),
        200,
        "serve() must accept connections"
    );
}
