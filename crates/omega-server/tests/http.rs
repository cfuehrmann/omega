#![allow(clippy::collapsible_if)]

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
fn make_test_state(sessions_root: PathBuf) -> AppState {
    AppState::new(Arc::new(MockProvider), sessions_root, PathBuf::from("."))
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
    let state = make_test_state(tmp.path().join("sessions"));
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
// GET /api/files — file-completion suggestions (Phase 1e.4)
// ---------------------------------------------------------------------------

/// `GET /api/files?prefix=<absolute-prefix>` returns the matching entries
/// in the prefix's directory, with directories first.
///
/// Uses an absolute prefix so the test is independent of the (process-global)
/// current working directory.
#[tokio::test]
async fn get_api_files_returns_completions_for_absolute_prefix() {
    let tmp = TempDir::new().expect("tempdir");
    let seed = tmp.path().join("seed");
    std::fs::create_dir(&seed).expect("create seed dir");
    std::fs::write(seed.join("hello.txt"), "").expect("write hello.txt");
    std::fs::write(seed.join("help.md"), "").expect("write help.md");
    std::fs::create_dir(seed.join("helpers")).expect("mkdir helpers");
    std::fs::write(seed.join("world.txt"), "").expect("write world.txt");

    let state = make_test_state(tmp.path().join("sessions"));
    let addr = spawn_server(state).await;

    let prefix = format!("{}/hel", seed.display());
    let resp = http_client()
        .get(format!("http://{addr}/api/files"))
        .query(&[("prefix", &prefix)])
        .send()
        .await
        .expect("GET /api/files");
    assert_eq!(resp.status().as_u16(), 200);
    let body: Vec<String> = resp.json().await.expect("json");

    // Directories first, then alphabetical files.
    let dir_part = format!("{}/", seed.display());
    assert_eq!(
        body,
        vec![
            format!("{dir_part}helpers/"),
            format!("{dir_part}hello.txt"),
            format!("{dir_part}help.md"),
        ],
    );
}

// ---------------------------------------------------------------------------
// GET /api/context — context-record lookup by hash (Phase 1e.4)
// ---------------------------------------------------------------------------

/// `GET /api/context?hashes=h1,h2` returns exactly the records whose
/// hashes are listed, in request order.
#[tokio::test]
async fn get_api_context_returns_records_in_request_order() {
    use omega_core::ContentBlock;
    use omega_core::Role;
    use omega_store::ContextStore;

    let tmp = TempDir::new().expect("tempdir");
    let sessions_root = tmp.path().join("sessions");
    let state = make_test_state(sessions_root.clone());
    let addr = spawn_server(state).await;
    let client = http_client();

    // Create a session so AppState has a context_file pointer.
    let r = client
        .post(format!("http://{addr}/api/sessions"))
        .send()
        .await
        .expect("POST /api/sessions");
    assert_eq!(r.status().as_u16(), 201);
    let body: serde_json::Value = r.json().await.expect("json");
    let dir_name = body["dir"].as_str().expect("dir").to_owned();

    // Append three records directly via ContextStore so the test does
    // not depend on any specific Anthropic-side hashing scheme.
    let context_file = sessions_root.join(&dir_name).join("context.jsonl");
    let store = ContextStore::new(context_file);
    let h_a = store
        .append(
            Role::User,
            vec![ContentBlock::Text {
                text: "alpha".to_owned(),
            }],
        )
        .await
        .expect("append alpha");
    let h_b = store
        .append(
            Role::User,
            vec![ContentBlock::Text {
                text: "bravo".to_owned(),
            }],
        )
        .await
        .expect("append bravo");
    let h_c = store
        .append(
            Role::User,
            vec![ContentBlock::Text {
                text: "charlie".to_owned(),
            }],
        )
        .await
        .expect("append charlie");

    // Request c, a (in that order); b must not appear, and order must be c-then-a.
    let hashes = format!("{},{}", h_c.as_ref(), h_a.as_ref());
    let resp = client
        .get(format!("http://{addr}/api/context"))
        .query(&[("hashes", hashes.as_str())])
        .send()
        .await
        .expect("GET /api/context");
    assert_eq!(resp.status().as_u16(), 200);
    let arr: Vec<serde_json::Value> = resp.json().await.expect("json");
    assert_eq!(arr.len(), 2, "expected 2 records, got {arr:?}");
    assert_eq!(arr[0]["hash"].as_str().unwrap(), h_c.as_ref());
    assert_eq!(arr[1]["hash"].as_str().unwrap(), h_a.as_ref());
    assert!(
        !arr.iter().any(|v| v["hash"].as_str() == Some(h_b.as_ref())),
        "b must not be present",
    );

    // No hashes parameter → empty array.
    let resp = client
        .get(format!("http://{addr}/api/context"))
        .send()
        .await
        .expect("GET /api/context (no hashes)");
    assert_eq!(resp.status().as_u16(), 200);
    let arr: Vec<serde_json::Value> = resp.json().await.expect("json");
    assert!(arr.is_empty(), "empty hashes must yield []");
}

// ---------------------------------------------------------------------------
// Static file serving (Phase 3.7 — fallback now serves the Leptos bundle)
// ---------------------------------------------------------------------------
//
// `make_test_state` defaults `leptos_dir` to the production constant
// (`frontends/leptos/dist`); tests that exercise the fallback `ServeDir`
// override it via `with_leptos_dir(...)` so reads resolve to the
// test-scoped tempdir.

#[tokio::test]
async fn servedir_serves_files_from_fallback_dir() {
    let tmp = TempDir::new().expect("tempdir");
    let file_path = tmp.path().join("hello.txt");
    std::fs::write(&file_path, "omega-rocks").expect("write tempfile");

    let state =
        make_test_state(tmp.path().join("sessions")).with_leptos_dir(tmp.path().to_path_buf());
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
    let state =
        make_test_state(tmp.path().join("sessions")).with_leptos_dir(tmp.path().to_path_buf());
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
    // leptos_dir is None when not supplied; the real default is resolved
    // at runtime in main.rs from the binary path.
    assert!(args.leptos_dir.is_none());
    assert!(args.working_dir.is_none());
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
        "--leptos-dir",
        "/var/www/omega-leptos",
    ]);
    assert_eq!(args.port, 4242);
    assert_eq!(args.sessions_root, PathBuf::from("/tmp/custom-sessions"));
    assert_eq!(
        args.leptos_dir,
        Some(PathBuf::from("/var/www/omega-leptos"))
    );
}

#[test]
fn args_working_dir_is_accepted() {
    use clap::Parser as _;
    let args = Args::parse_from(["omega-server", "--working-dir", "/srv/myproject"]);
    assert_eq!(args.working_dir, Some(PathBuf::from("/srv/myproject")));
    assert!(args.leptos_dir.is_none());
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
    let state = make_test_state(sessions_root.clone());
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
    let state = make_test_state(nonexistent);
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
    let state = make_test_state(sessions_root.clone());
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

/// `GET /api/sessions` reports each session's **absolute** directory path so
/// the picker's "Copy @path" button yields a fully-qualified reference rather
/// than a `.omega/sessions/...` relative one. The path is the configured
/// sessions root joined with the directory name.
#[tokio::test]
async fn get_sessions_item_path_is_absolute() {
    let tmp = TempDir::new().expect("tempdir");
    let sessions_root = tmp.path().join("sessions");
    let state = make_test_state(sessions_root.clone());
    let addr = spawn_server(state).await;
    let client = http_client();

    let r = client
        .post(format!("http://{addr}/api/sessions"))
        .send()
        .await
        .expect("POST");
    assert_eq!(r.status().as_u16(), 201);

    let resp = client
        .get(format!("http://{addr}/api/sessions"))
        .send()
        .await
        .expect("GET /api/sessions");
    assert_eq!(resp.status().as_u16(), 200);

    let body: serde_json::Value = resp.json().await.expect("decode json");
    let arr = body.as_array().expect("expected JSON array");
    assert_eq!(arr.len(), 1, "expected 1 session");

    let item = &arr[0];
    let dir = item["dir"].as_str().expect("dir field");
    let path = item["path"].as_str().expect("path field");

    assert!(
        std::path::Path::new(path).is_absolute(),
        "path must be absolute, got {path:?}"
    );
    assert_eq!(
        path,
        sessions_root.join(dir).to_string_lossy(),
        "path must be the configured root joined with the dir name"
    );
}

/// After two POSTs the list is sorted newest-first.
#[tokio::test]
async fn get_sessions_newest_first() {
    let tmp = TempDir::new().expect("tempdir");
    let sessions_root = tmp.path().join("sessions");
    let state = make_test_state(sessions_root.clone());
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
    let state = make_test_state(sessions_root.clone());
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
// Graceful shutdown — Phase 1e.4
// ---------------------------------------------------------------------------

/// Send SIGTERM to a real `omega-server` child process and verify that:
///
/// 1. The process exits cleanly (status code 0 — no panic, no abort).
/// 2. The active session's `events.jsonl` ends with a `server_stopped`
///    event, written by [`omega_server::perform_shutdown`].
#[tokio::test]
async fn graceful_shutdown_writes_server_stopped_and_exits_clean() {
    use std::process::Stdio;
    use std::time::Duration;
    use tokio::process::Command;

    // Pick a free port — the binary does not advertise its bound port,
    // so we choose one explicitly and accept the small race window.
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("local_addr").port();
    drop(listener);

    let tmp = TempDir::new().expect("tempdir");
    let sessions_root = tmp.path().join("sessions");

    let bin = env!("CARGO_BIN_EXE_omega-server");
    let mut child = Command::new(bin)
        .args(["--port", &port.to_string()])
        .arg("--sessions-root")
        .arg(&sessions_root)
        .env("ANTHROPIC_API_KEY", "dummy")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn omega-server");

    // Wait for the server to be ready (poll /health).
    let url = format!("http://127.0.0.1:{port}");
    let mut ready = false;
    for _ in 0..100 {
        if let Ok(r) = reqwest::get(format!("{url}/health")).await {
            if r.status().is_success() {
                ready = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(ready, "server did not become ready in 5 s");

    // Create a session — server_stopped is only written when one exists.
    let resp = reqwest::Client::new()
        .post(format!("{url}/api/sessions"))
        .send()
        .await
        .expect("POST /api/sessions");
    assert_eq!(resp.status().as_u16(), 201);
    let body: serde_json::Value = resp.json().await.expect("json");
    let dir_name = body["dir"].as_str().expect("dir").to_owned();

    // SIGTERM → graceful shutdown.
    let pid = nix::unistd::Pid::from_raw(
        i32::try_from(child.id().expect("child has pid")).expect("pid fits in i32"),
    );
    nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGTERM).expect("send SIGTERM");
    let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .expect("server did not exit within 10 s of SIGTERM")
        .expect("child wait");
    assert!(status.success(), "server exit was not clean: {status:?}");

    // events.jsonl must contain the server_stopped record.
    let events_file = sessions_root.join(&dir_name).join("events.jsonl");
    let content = std::fs::read_to_string(&events_file).expect("read events.jsonl");
    assert!(
        content.contains("\"type\":\"server_stopped\""),
        "events.jsonl must contain server_stopped record; got:\n{content}",
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
    let state = make_test_state(tmp.path().join("sessions"));
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

// ---------------------------------------------------------------------------
// POST /api/compose — external-editor prompt composition
// ---------------------------------------------------------------------------

/// `POST /api/compose` launches the configured editor (`OMEGA_EDITOR`) on a
/// temp file seeded with the request's `draft`, waits for it to exit, and
/// returns the edited contents.
///
/// Run against the real binary (subprocess) so the editor command comes from
/// the child's own environment — no global `set_var` race with parallel
/// in-process tests. The fake editor wraps the seeded draft in `EDITED[…]`,
/// proving both that the draft was written to the file (seeding) and that the
/// editor's changes flow back to the response.
#[tokio::test]
async fn post_compose_runs_configured_editor_and_returns_edited_text() {
    use std::time::Duration;
    use tokio::process::Command;

    let tmp = TempDir::new().expect("tempdir");

    // Fake editor: $1 is the temp file. Replace its contents with a marker
    // that embeds the seed, so the assertion verifies seeding + round-trip.
    let editor = tmp.path().join("fake-editor.sh");
    std::fs::write(
        &editor,
        "#!/bin/sh\nprintf 'EDITED[%s]' \"$(cat \"$1\")\" > \"$1\"\n",
    )
    .expect("write fake editor");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&editor).expect("metadata").permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&editor, perms).expect("chmod +x");
    }

    // Pick a free port, then release it for the child to bind.
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("local_addr").port();
    drop(listener);

    let sessions_root = tmp.path().join("sessions");
    let bin = env!("CARGO_BIN_EXE_omega-server");
    let mut child = Command::new(bin)
        .args(["--port", &port.to_string()])
        .arg("--sessions-root")
        .arg(&sessions_root)
        // Hermetic: point HOME + CWD at the empty tempdir so no real
        // `.env` / `~/.config/omega/.env` leaks an editor var into the test.
        .current_dir(tmp.path())
        .env("HOME", tmp.path())
        .env("ANTHROPIC_API_KEY", "dummy")
        .env("OMEGA_EDITOR", &editor)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn omega-server");

    let url = format!("http://127.0.0.1:{port}");
    let mut ready = false;
    for _ in 0..100 {
        if let Ok(r) = reqwest::get(format!("{url}/health")).await {
            if r.status().is_success() {
                ready = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(ready, "server did not become ready in 5 s");

    let resp = reqwest::Client::new()
        .post(format!("{url}/api/compose"))
        .json(&serde_json::json!({ "draft": "hello world" }))
        .send()
        .await
        .expect("POST /api/compose");
    assert_eq!(resp.status().as_u16(), 200, "expected 200 OK");
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(
        body["outcome"], "send",
        "a normal (exit 0) editor close must map to the send outcome",
    );
    assert_eq!(
        body["content"].as_str().expect("content field"),
        "EDITED[hello world]",
        "content must reflect the seeded draft wrapped by the fake editor",
    );

    let _ = child.kill().await;
}

/// The seed temp file is created **inside the server's working directory**
/// (the project CWD), not the system temp dir. Helix-style path completion
/// resolves relative paths against the *current document's* directory, so a
/// temp file in `/tmp` would complete against `/tmp` rather than the project.
/// Anchoring it at the cwd lets the operator tab-complete project files while
/// composing a prompt.
///
/// The fake editor writes the canonical directory of its file argument back
/// into the file; the test asserts that directory is the server's cwd.
#[tokio::test]
async fn post_compose_seeds_temp_file_in_server_cwd() {
    use std::time::Duration;
    use tokio::process::Command;

    let tmp = TempDir::new().expect("tempdir");

    // Fake editor: replace the file contents with the canonical path of the
    // directory the temp file lives in (`cd $(dirname $1) && pwd`).
    let editor = tmp.path().join("fake-editor.sh");
    std::fs::write(
        &editor,
        "#!/bin/sh\nprintf '%s' \"$(cd \"$(dirname \"$1\")\" && pwd)\" > \"$1\"\n",
    )
    .expect("write fake editor");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&editor).expect("metadata").permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&editor, perms).expect("chmod +x");
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("local_addr").port();
    drop(listener);

    let sessions_root = tmp.path().join("sessions");
    let bin = env!("CARGO_BIN_EXE_omega-server");
    let mut child = Command::new(bin)
        .args(["--port", &port.to_string()])
        .arg("--sessions-root")
        .arg(&sessions_root)
        .current_dir(tmp.path())
        .env("HOME", tmp.path())
        .env("ANTHROPIC_API_KEY", "dummy")
        .env("OMEGA_EDITOR", &editor)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn omega-server");

    let url = format!("http://127.0.0.1:{port}");
    let mut ready = false;
    for _ in 0..100 {
        if let Ok(r) = reqwest::get(format!("{url}/health")).await {
            if r.status().is_success() {
                ready = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(ready, "server did not become ready in 5 s");

    let resp = reqwest::Client::new()
        .post(format!("{url}/api/compose"))
        .json(&serde_json::json!({ "draft": "anything" }))
        .send()
        .await
        .expect("POST /api/compose");
    assert_eq!(resp.status().as_u16(), 200, "expected 200 OK");
    let body: serde_json::Value = resp.json().await.expect("json");

    let reported_dir = body["content"].as_str().expect("content field");
    // The server's cwd is `tmp.path()` (passed via `.current_dir`).
    // Canonicalize for a symlink-stable comparison (e.g. /tmp vs /private/tmp).
    let expected_cwd = std::fs::canonicalize(tmp.path()).expect("canonicalize cwd");
    assert_eq!(
        std::path::Path::new(reported_dir),
        expected_cwd,
        "temp file must be created in the server cwd so path completion is \
         anchored at the project, not the system temp dir",
    );

    let _ = child.kill().await;
}

/// `POST /api/compose` returns the file **verbatim** even when the editor is
/// quit without saving (the file still holds the seed). The server does no
/// send/suppress logic: the client drops the result into the textarea for
/// review, so the operator can back out. Emptying server-side would instead
/// wipe the draft on refill — the opposite of what we want.
#[tokio::test]
async fn post_compose_quit_without_saving_returns_seed_verbatim() {
    use std::time::Duration;
    use tokio::process::Command;

    let tmp = TempDir::new().expect("tempdir");

    // Fake editor that exits 0 without touching the file — i.e. the operator
    // opened the buffer and quit without writing (`:q!`).
    let editor = tmp.path().join("fake-editor.sh");
    std::fs::write(&editor, "#!/bin/sh\nexit 0\n").expect("write fake editor");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&editor).expect("metadata").permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&editor, perms).expect("chmod +x");
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("local_addr").port();
    drop(listener);

    let sessions_root = tmp.path().join("sessions");
    let bin = env!("CARGO_BIN_EXE_omega-server");
    let mut child = Command::new(bin)
        .args(["--port", &port.to_string()])
        .arg("--sessions-root")
        .arg(&sessions_root)
        .current_dir(tmp.path())
        .env("HOME", tmp.path())
        .env("ANTHROPIC_API_KEY", "dummy")
        .env("OMEGA_EDITOR", &editor)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn omega-server");

    let url = format!("http://127.0.0.1:{port}");
    let mut ready = false;
    for _ in 0..100 {
        if let Ok(r) = reqwest::get(format!("{url}/health")).await {
            if r.status().is_success() {
                ready = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(ready, "server did not become ready in 5 s");

    let resp = reqwest::Client::new()
        .post(format!("{url}/api/compose"))
        .json(&serde_json::json!({ "draft": "please keep my draft" }))
        .send()
        .await
        .expect("POST /api/compose");
    assert_eq!(resp.status().as_u16(), 200, "expected 200 OK");
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(
        body["content"].as_str().expect("content field"),
        "please keep my draft",
        "quitting without saving must return the seed verbatim (the client \
         refills the textarea for review rather than sending)",
    );

    let _ = child.kill().await;
}

/// Spawn `omega-server` with `OMEGA_EDITOR` pointing at a fake editor whose
/// shell body is `script`, POST `/api/compose` with `draft`, and return the
/// HTTP status code and response body text. Keeps the exit-code tests below
/// concise (they differ only in the fake editor's body and the assertions).
async fn compose_with_fake_editor(script: &str, draft: &str) -> (u16, String) {
    use std::time::Duration;
    use tokio::process::Command;

    let tmp = TempDir::new().expect("tempdir");
    let editor = tmp.path().join("fake-editor.sh");
    std::fs::write(&editor, script).expect("write fake editor");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&editor).expect("metadata").permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&editor, perms).expect("chmod +x");
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("local_addr").port();
    drop(listener);

    let sessions_root = tmp.path().join("sessions");
    let bin = env!("CARGO_BIN_EXE_omega-server");
    let mut child = Command::new(bin)
        .args(["--port", &port.to_string()])
        .arg("--sessions-root")
        .arg(&sessions_root)
        .current_dir(tmp.path())
        .env("HOME", tmp.path())
        .env("ANTHROPIC_API_KEY", "dummy")
        .env("OMEGA_EDITOR", &editor)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn omega-server");

    let url = format!("http://127.0.0.1:{port}");
    let mut ready = false;
    for _ in 0..100 {
        if let Ok(r) = reqwest::get(format!("{url}/health")).await {
            if r.status().is_success() {
                ready = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(ready, "server did not become ready in 5 s");

    let resp = reqwest::Client::new()
        .post(format!("{url}/api/compose"))
        .json(&serde_json::json!({ "draft": draft }))
        .send()
        .await
        .expect("POST /api/compose");
    let status = resp.status().as_u16();
    let text = resp.text().await.expect("text");
    let _ = child.kill().await;
    (status, text)
}

/// Normal editor exit (code 0) maps to `outcome: "send"` — the client sends
/// the composed text immediately, as if the Send button were clicked.
#[tokio::test]
async fn post_compose_normal_exit_yields_send_outcome() {
    let (status, text) =
        compose_with_fake_editor("#!/bin/sh\nprintf 'final prompt' > \"$1\"\n", "seed").await;
    assert_eq!(status, 200, "expected 200; got body {text:?}");
    let body: serde_json::Value = serde_json::from_str(&text).expect("json");
    assert_eq!(
        body["outcome"], "send",
        "exit 0 must map to the send outcome"
    );
    assert_eq!(body["content"], "final prompt");
}

/// Backout exit (code 1, Helix `:cq!`) maps to `outcome: "backout"` — the
/// client keeps the text as a draft without sending. The file content is
/// returned so the backout is loss-free.
#[tokio::test]
async fn post_compose_backout_exit_yields_backout_outcome() {
    let (status, text) = compose_with_fake_editor(
        "#!/bin/sh\nprintf 'work in progress' > \"$1\"\nexit 1\n",
        "seed",
    )
    .await;
    assert_eq!(
        status, 200,
        "a deliberate backout is a 200, not an error; got body {text:?}"
    );
    let body: serde_json::Value = serde_json::from_str(&text).expect("json");
    assert_eq!(
        body["outcome"], "backout",
        "exit 1 must map to the backout outcome"
    );
    assert_eq!(
        body["content"], "work in progress",
        "backout must return the saved file content so no work is lost",
    );
}

/// An unexpected non-zero exit (neither `0` nor the backout code `1`) is a
/// genuine launch/runtime error → `500` with the terminal-wrapper hint.
#[tokio::test]
async fn post_compose_unexpected_exit_code_returns_500() {
    let (status, text) = compose_with_fake_editor("#!/bin/sh\nexit 2\n", "seed").await;
    assert_eq!(status, 500, "exit 2 must be an error; got body {text:?}");
    assert!(
        text.contains("foot") || text.contains("terminal"),
        "error must carry the terminal-wrapper hint; got: {text:?}",
    );
}

/// `POST /api/compose` with no editor configured responds `500` with an
/// actionable plaintext error mentioning `OMEGA_EDITOR`.
#[tokio::test]
async fn post_compose_without_editor_configured_returns_500() {
    use std::time::Duration;
    use tokio::process::Command;

    let tmp = TempDir::new().expect("tempdir");

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("local_addr").port();
    drop(listener);

    let sessions_root = tmp.path().join("sessions");
    let bin = env!("CARGO_BIN_EXE_omega-server");
    // Explicitly clear all three editor vars so the host environment can't
    // leak one in and make the editor launch succeed.
    let mut child = Command::new(bin)
        .args(["--port", &port.to_string()])
        .arg("--sessions-root")
        .arg(&sessions_root)
        // Hermetic: empty HOME + CWD so dotenvy can't re-inject an editor var
        // from a real `.env` / `~/.config/omega/.env` after the removes below.
        .current_dir(tmp.path())
        .env("HOME", tmp.path())
        .env("ANTHROPIC_API_KEY", "dummy")
        .env_remove("OMEGA_EDITOR")
        .env_remove("VISUAL")
        .env_remove("EDITOR")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn omega-server");

    let url = format!("http://127.0.0.1:{port}");
    let mut ready = false;
    for _ in 0..100 {
        if let Ok(r) = reqwest::get(format!("{url}/health")).await {
            if r.status().is_success() {
                ready = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(ready, "server did not become ready in 5 s");

    let resp = reqwest::Client::new()
        .post(format!("{url}/api/compose"))
        .json(&serde_json::json!({ "draft": "hi" }))
        .send()
        .await
        .expect("POST /api/compose");
    assert_eq!(resp.status().as_u16(), 500, "expected 500");
    let text = resp.text().await.expect("text");
    assert!(
        text.contains("OMEGA_EDITOR"),
        "error must mention OMEGA_EDITOR; got: {text:?}",
    );

    let _ = child.kill().await;
}

/// Regression: `POST /api/compose` must not hang when the configured editor
/// reads from stdin (as every TUI editor does).  The server has no
/// controlling terminal, so the editor is launched with a detached stdin
/// (`Stdio::null`); without that, a bare `nvim`/`vim`/`hx` reads the
/// server's inherited non-tty stdin and blocks forever, leaving the browser
/// button "doing nothing".
///
/// The harness spawns the server with a *piped* stdin and keeps the write
/// end open for the whole test, so the editor would inherit an open,
/// never-closing pipe.  The fake editor blocks on `cat` until its stdin
/// reaches EOF before editing the file — which only happens if stdin is
/// detached.  A timeout around the request turns the would-be hang into a
/// clean failure.
#[tokio::test]
async fn post_compose_does_not_hang_when_editor_reads_stdin() {
    use std::time::Duration;
    use tokio::process::Command;

    let tmp = TempDir::new().expect("tempdir");

    // Fake editor that reads its stdin to EOF *before* editing. With the
    // editor's stdin detached (the fix) `cat` sees EOF at once; without it,
    // `cat` blocks on the inherited open pipe and the request never returns.
    let editor = tmp.path().join("fake-editor.sh");
    std::fs::write(
        &editor,
        "#!/bin/sh\ncat > /dev/null\nprintf 'EDITED[%s]' \"$(cat \"$1\")\" > \"$1\"\n",
    )
    .expect("write fake editor");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&editor).expect("metadata").permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&editor, perms).expect("chmod +x");
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("local_addr").port();
    drop(listener);

    let sessions_root = tmp.path().join("sessions");
    let bin = env!("CARGO_BIN_EXE_omega-server");
    // Piped stdin, with the write end held open by `child` for the whole
    // test: this is the open, never-closing pipe the editor would inherit
    // were its stdin not detached.
    let mut child = Command::new(bin)
        .args(["--port", &port.to_string()])
        .arg("--sessions-root")
        .arg(&sessions_root)
        .current_dir(tmp.path())
        .env("HOME", tmp.path())
        .env("ANTHROPIC_API_KEY", "dummy")
        .env("OMEGA_EDITOR", &editor)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn omega-server");
    // Hold the pipe's write end open; dropping it would send EOF and mask
    // the bug.
    let _server_stdin = child.stdin.take().expect("piped stdin");

    let url = format!("http://127.0.0.1:{port}");
    let mut ready = false;
    for _ in 0..100 {
        if let Ok(r) = reqwest::get(format!("{url}/health")).await {
            if r.status().is_success() {
                ready = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(ready, "server did not become ready in 5 s");

    let resp = tokio::time::timeout(
        Duration::from_secs(10),
        reqwest::Client::new()
            .post(format!("{url}/api/compose"))
            .json(&serde_json::json!({ "draft": "seed" }))
            .send(),
    )
    .await
    .expect("POST /api/compose hung — editor blocked on inherited stdin")
    .expect("POST /api/compose");

    assert_eq!(resp.status().as_u16(), 200, "expected 200");
    let body: serde_json::Value = resp.json().await.expect("json body");
    assert_eq!(
        body["content"].as_str(),
        Some("EDITED[seed]"),
        "editor must have run to completion after EOF on stdin",
    );

    let _ = child.kill().await;
}
