//! Integration tests for the Phase 1e.0 router.
//!
//! Each test binds `127.0.0.1:0` so parallel runs cannot collide on a
//! port, spawns the router on that listener, then drives it with a real
//! `reqwest` HTTP client.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use omega_server::{Args, build_router};
use tempfile::TempDir;
use tokio::net::TcpListener;

/// Spawn `build_router(public_dir)` on a random local port and return
/// its bound address.  The server task is detached; `TempDir` cleanup
/// runs when the caller drops the returned guard.
async fn spawn_router(public_dir: &Path) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind 127.0.0.1:0");
    let addr = listener.local_addr().expect("local_addr");
    let app = build_router(public_dir);
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
// /health
// ---------------------------------------------------------------------------

#[tokio::test]
async fn health_returns_200_with_json_status_ok() {
    let tmp = TempDir::new().expect("tempdir");
    let addr = spawn_router(tmp.path()).await;

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
// Placeholder routes — every one must return 501.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn placeholder_routes_return_501() {
    let tmp = TempDir::new().expect("tempdir");
    let addr = spawn_router(tmp.path()).await;
    let client = http_client();

    for path in ["/api/sessions", "/ws", "/context", "/files"] {
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
async fn placeholder_routes_accept_post_too() {
    // The placeholders use `any(...)` so non-GET methods must also 501,
    // not 405 Method Not Allowed.  This pins the chosen routing helper.
    let tmp = TempDir::new().expect("tempdir");
    let addr = spawn_router(tmp.path()).await;
    let client = http_client();

    for path in ["/api/sessions", "/ws", "/context", "/files"] {
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
// Static file serving via `tower_http::services::ServeDir`.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn servedir_serves_files_from_public_dir() {
    let tmp = TempDir::new().expect("tempdir");
    let file_path = tmp.path().join("hello.txt");
    std::fs::write(&file_path, "omega-rocks").expect("write tempfile");

    let addr = spawn_router(tmp.path()).await;

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
    let addr = spawn_router(tmp.path()).await;

    let resp = http_client()
        .get(format!("http://{addr}/does-not-exist.html"))
        .send()
        .await
        .expect("GET missing");
    assert_eq!(resp.status().as_u16(), 404);
}

// ---------------------------------------------------------------------------
// CLI parsing — defaults and overrides.
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
    // 70000 is out of range for u16 — clap must reject.
    let res = Args::try_parse_from(["omega-server", "--port", "70000"]);
    assert!(res.is_err(), "expected port=70000 to be rejected");
}
