//! Verifies that `omega-server` resolves its `leptos_dir` correctly when
//! started **without** an explicit `--leptos-dir` flag.
//!
//! This exercises the `current_exe()` ancestor-navigation in `main.rs`.
//! The test caught the regression introduced by the `rust/` → root refactor
//! (four `parent()` calls were needed before; three are needed now that the
//! binary lives at `target/release/omega-server` instead of
//! `rust/target/release/omega-server`).
//!
//! Requires:
//!   - `target/release/omega-server` built before running
//!     (`cargo build --release -p omega-server`, done by `_rust-e2e-run`).
//!   - `frontends/leptos/dist/` present (done by `web-leptos-build`).

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use anyhow::{Result, bail};

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is `crates/omega-e2e`; two levels up is the repo root.
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .ancestors()
        .nth(2)
        .unwrap_or(manifest)
        .to_path_buf()
}

fn server_binary() -> PathBuf {
    repo_root().join("target/release/omega-server")
}

fn pick_free_port() -> Result<u16> {
    let l = TcpListener::bind("127.0.0.1:0")?;
    Ok(l.local_addr()?.port())
    // `l` dropped here — port is free by the time the server binds it
}

#[tokio::test]
#[ignore = "browser"]
async fn server_serves_leptos_bundle_without_leptos_dir_flag() -> Result<()> {
    let bin = server_binary();
    if !bin.exists() {
        bail!(
            "omega-server binary not found at {}; build it first with \
             `cargo build --release -p omega-server`",
            bin.display()
        );
    }

    let port = pick_free_port()?;

    let mut child: Child = Command::new(&bin)
        .arg("--port")
        .arg(port.to_string())
        // A syntactically valid key is required; no real API call is made.
        .env("ANTHROPIC_API_KEY", "test-key-not-used")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    // Poll /health until the server is accepting connections (up to 5 s).
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{port}");
    let mut ready = false;
    for _ in 0..50 {
        if client
            .get(format!("{base}/health"))
            .send()
            .await
            .is_ok_and(|r| r.status().is_success())
        {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    if !ready {
        child.kill().ok();
        child.wait().ok();
        bail!("omega-server did not become ready within 5 s on port {port}");
    }

    let resp = client.get(format!("{base}/")).send().await?;

    let status = resp.status();
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    child.kill().ok();
    child.wait().ok();

    assert_eq!(status, 200, "GET / should return 200 OK");
    assert!(
        ct.contains("text/html"),
        "GET / content-type should be text/html, got {ct:?}"
    );
    Ok(())
}
