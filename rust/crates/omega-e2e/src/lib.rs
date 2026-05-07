//! End-to-end test harness for the Leptos web UI.
//!
//! Each test owns a [`TestHarness`]:
//! - a fresh `mock-omega-server` subprocess on a random `127.0.0.1:0`
//!   port, with a `TempDir` sessions root, the production
//!   `frontends/leptos/dist` bundle as `--leptos-dir`, and a separate
//!   random control-API port,
//! - a headless Chrome instance launched via `chromiumoxide::Browser`,
//! - a `Page` already at `/leptos/`, the WS connection up
//!   (`<main data-connected="true">`),
//! - a control-API client for scripting the mock LLM and capturing
//!   `/v1/messages` calls.
//!
//! Drop kills the subprocess; `TempDir` cleans the sessions root.
//!
//! Tests in `tests/*.rs` are individually marked `#[ignore = "browser"]`
//! so the workspace `cargo test` (run by `just rust-gate`) compiles
//! them but does not launch Chrome. The `just rust-e2e` recipe runs
//! the full suite with `-- --ignored`.

// This crate is an internal test harness: every public surface is
// only consumed by `tests/*.rs` in the same crate. The pedantic doc
// and cast lints add no value here; allow them.
#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use chromiumoxide::Page;
use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::handler::HandlerConfig;
use futures::StreamExt;
pub use omega_test_fixtures::{MockResponse, ToolUseSpec};
use serde::Deserialize;
use serde_json::Value;
use tempfile::TempDir;
use tokio::task::JoinHandle;

/// Default timeout used by the `wait_*` helpers. Generous on purpose:
/// the slow paths (CDN-loaded mermaid, multi-tool turns) need it, and
/// fast assertions complete in a few hundred ms regardless.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// One captured `/v1/messages` request as returned by
/// `mock-omega-server`'s `/control/llm-calls` endpoint. Mirrors
/// `omega_test_fixtures::CapturedCall` (which is `Serialize`-only) so
/// we can deserialize it client-side.
#[derive(Debug, Clone, Deserialize)]
pub struct CapturedCall {
    #[serde(rename = "systemKind")]
    pub system_kind: String,
    pub at: u128,
    pub messages: Vec<CapturedMessage>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CapturedMessage {
    pub role: String,
    pub content: String,
}

// ---------------------------------------------------------------------------
// TestHarness
// ---------------------------------------------------------------------------

/// One-test fixture: server subprocess + headless Chrome page.
///
/// Construct with [`TestHarness::launch`]. Drop tears down both.
pub struct TestHarness {
    /// Page already at `/leptos/`, WS connected.
    pub page: Page,
    /// `http://127.0.0.1:<main_port>` — the mock-omega-server.
    pub base_url: String,
    /// `http://127.0.0.1:<ctrl_port>` — control API.
    pub ctrl_url: String,
    /// reqwest client reused for control calls.
    http: reqwest::Client,
    /// Holds the sessions tempdir alive for the test's lifetime.
    _sessions_dir: TempDir,
    /// Mock-omega-server child. Killed in `Drop`.
    server_child: Option<Child>,
    /// chromiumoxide handler driver. Aborted in `Drop` after the
    /// browser is closed.
    handler_task: Option<JoinHandle<()>>,
    /// Browser handle held until drop so the underlying Chromium
    /// process is owned for the test duration.
    _browser: Browser,
}

impl TestHarness {
    /// Spawn the server, wait for `/health`, launch headless Chrome,
    /// open `/leptos/`, wait for WS connect.
    pub async fn launch() -> Result<Self> {
        let main_port = pick_free_port()?;
        let ctrl_port = pick_free_port()?;
        let sessions_dir = TempDir::new().context("create tempdir for sessions")?;

        let server_child = spawn_mock_server(main_port, ctrl_port, sessions_dir.path())?;
        let base_url = format!("http://127.0.0.1:{main_port}");
        let ctrl_url = format!("http://127.0.0.1:{ctrl_port}");
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .context("build reqwest client")?;

        wait_for_health(&http, &base_url).await?;

        let (browser, handler_task) = launch_browser().await?;
        let page = browser
            .new_page(format!("{base_url}/leptos/"))
            .await
            .context("open /leptos/")?;
        page.wait_for_navigation().await.context("wait for nav")?;

        let harness = Self {
            page,
            base_url,
            ctrl_url,
            http,
            _sessions_dir: sessions_dir,
            server_child: Some(server_child),
            handler_task: Some(handler_task),
            _browser: browser,
        };

        // Wait for the WebSocket to actually connect — every spec
        // depends on this.
        harness
            .wait_for_attr("main", "data-connected", "true", DEFAULT_TIMEOUT)
            .await
            .context("wait for WS data-connected=true")?;

        Ok(harness)
    }

    // ---------------- control API ----------------

    /// Replace the mock LLM's scripted response queue.
    pub async fn load_script(&self, steps: Vec<MockResponse>) -> Result<()> {
        // `MockResponse` is `Deserialize` only — the mock's HTTP
        // surface accepts the camelCase JSON shape directly. We
        // serialize via `serde_json::Value` to reuse Rust types
        // without re-deriving `Serialize` on the upstream enum.
        let body = script_to_json(&steps);
        let resp = self
            .http
            .post(format!("{}/control/script", self.ctrl_url))
            .json(&body)
            .send()
            .await
            .context("POST /control/script")?;
        if !resp.status().is_success() {
            bail!("script load failed: {}", resp.status());
        }
        Ok(())
    }

    /// Clear the captured-call history.
    pub async fn reset_calls(&self) -> Result<()> {
        let resp = self
            .http
            .post(format!("{}/control/reset-calls", self.ctrl_url))
            .send()
            .await
            .context("POST /control/reset-calls")?;
        if !resp.status().is_success() {
            bail!("reset-calls failed: {}", resp.status());
        }
        Ok(())
    }

    /// Snapshot the captured `/v1/messages` calls.
    pub async fn captured_calls(&self) -> Result<Vec<CapturedCall>> {
        let resp = self
            .http
            .get(format!("{}/control/llm-calls", self.ctrl_url))
            .send()
            .await
            .context("GET /control/llm-calls")?;
        let calls: Vec<CapturedCall> = resp.json().await.context("parse llm-calls")?;
        Ok(calls)
    }

    // ---------------- DOM helpers ----------------

    /// Poll `document.querySelector(sel).getAttribute(attr)` until it
    /// equals `expected`, or the timeout elapses.
    pub async fn wait_for_attr(
        &self,
        sel: &str,
        attr: &str,
        expected: &str,
        timeout: Duration,
    ) -> Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            let script = format!(
                "(() => {{ const el = document.querySelector({sel}); \
                  return el ? el.getAttribute({attr}) : null; }})()",
                sel = json_str(sel),
                attr = json_str(attr),
            );
            let v: Option<String> = self
                .page
                .evaluate(script)
                .await
                .ok()
                .and_then(|r| r.into_value().ok());
            if v.as_deref() == Some(expected) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(anyhow!(
                    "timed out waiting for {sel}@{attr} == {expected:?}; last={v:?}"
                ));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Wait for at least one element matching `sel` to attach to the
    /// DOM. Returns when `document.querySelector(sel)` is truthy.
    pub async fn wait_for_selector(&self, sel: &str, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            let exists: bool = self
                .page
                .evaluate(format!("!!document.querySelector({})", json_str(sel)))
                .await
                .ok()
                .and_then(|r| r.into_value().ok())
                .unwrap_or(false);
            if exists {
                return Ok(());
            }
            if Instant::now() >= deadline {
                bail!("timed out waiting for selector {sel}");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Wait until `document.querySelector(sel)` returns null.
    pub async fn wait_for_detached(&self, sel: &str, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            let exists: bool = self
                .page
                .evaluate(format!("!!document.querySelector({})", json_str(sel)))
                .await
                .ok()
                .and_then(|r| r.into_value().ok())
                .unwrap_or(true);
            if !exists {
                return Ok(());
            }
            if Instant::now() >= deadline {
                bail!("timed out waiting for {sel} to detach");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Poll `document.querySelectorAll(sel).length` until it equals
    /// `n`.
    pub async fn wait_for_count(&self, sel: &str, n: usize, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            let count: i64 = self
                .page
                .evaluate(format!(
                    "document.querySelectorAll({}).length",
                    json_str(sel)
                ))
                .await
                .ok()
                .and_then(|r| r.into_value().ok())
                .unwrap_or(-1);
            if count == n as i64 {
                return Ok(());
            }
            if Instant::now() >= deadline {
                bail!("timed out waiting for {sel} count == {n}; last={count}");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Read `el.textContent` (or empty string) for the first match.
    pub async fn text_content(&self, sel: &str) -> Result<String> {
        let v: Option<String> = self
            .page
            .evaluate(format!(
                "(() => {{ const el = document.querySelector({}); \
                  return el ? (el.textContent || '') : null; }})()",
                json_str(sel)
            ))
            .await
            .context("evaluate textContent")?
            .into_value()
            .ok();
        v.ok_or_else(|| anyhow!("element not found: {sel}"))
    }

    /// Read the value of an attribute on the first match (or `None`
    /// if the element/attribute is missing).
    pub async fn attr(&self, sel: &str, attr: &str) -> Result<Option<String>> {
        let v: Option<String> = self
            .page
            .evaluate(format!(
                "(() => {{ const el = document.querySelector({}); \
                  return el ? el.getAttribute({}) : null; }})()",
                json_str(sel),
                json_str(attr),
            ))
            .await
            .context("evaluate getAttribute")?
            .into_value()
            .ok();
        Ok(v)
    }

    /// Click the first match. Errors if the element is not found.
    pub async fn click(&self, sel: &str) -> Result<()> {
        self.wait_for_selector(sel, DEFAULT_TIMEOUT).await?;
        let el = self.page.find_element(sel).await.context("find_element")?;
        el.click().await.context("click")?;
        Ok(())
    }

    /// Focus an `<input>`/`<textarea>`, clear it, type `value`, and
    /// dispatch the matching `input` event so Leptos signals fire.
    pub async fn fill(&self, sel: &str, value: &str) -> Result<()> {
        self.wait_for_selector(sel, DEFAULT_TIMEOUT).await?;
        let script = format!(
            "(() => {{ \
              const el = document.querySelector({sel}); \
              if (!el) return false; \
              el.focus(); \
              el.value = ''; \
              el.dispatchEvent(new Event('input', {{ bubbles: true }})); \
              return true; \
            }})()",
            sel = json_str(sel),
        );
        let _ = self.page.evaluate(script).await?;
        // Now type each character so Leptos sees a stream of keydowns
        // (used by composer @-completion).
        let el = self.page.find_element(sel).await?;
        el.click().await?;
        if !value.is_empty() {
            el.type_str(value).await?;
        }
        Ok(())
    }

    /// Set `<select>` value programmatically, then dispatch the
    /// `change` event.
    pub async fn select_option(&self, sel: &str, value: &str) -> Result<()> {
        self.wait_for_selector(sel, DEFAULT_TIMEOUT).await?;
        let script = format!(
            "(() => {{ \
              const el = document.querySelector({sel}); \
              if (!el) return false; \
              el.value = {value}; \
              el.dispatchEvent(new Event('change', {{ bubbles: true }})); \
              el.dispatchEvent(new Event('input', {{ bubbles: true }})); \
              return true; \
            }})()",
            sel = json_str(sel),
            value = json_str(value),
        );
        let ok: bool = self
            .page
            .evaluate(script)
            .await?
            .into_value()
            .unwrap_or(false);
        if !ok {
            bail!("select_option: {sel} not found");
        }
        Ok(())
    }

    /// Press a single keyboard key on the focused element (e.g.
    /// `"Enter"`, `"ArrowDown"`). Uses the page-level dispatcher.
    pub async fn press_key(&self, sel: &str, key: &str) -> Result<()> {
        let el = self.page.find_element(sel).await?;
        el.press_key(key).await?;
        Ok(())
    }

    /// Run an arbitrary JS expression and return the JSON value.
    pub async fn eval<T: serde::de::DeserializeOwned>(&self, expr: &str) -> Result<T> {
        let v = self
            .page
            .evaluate(expr.to_string())
            .await?
            .into_value()
            .map_err(|e| anyhow!("eval into_value: {e}"))?;
        Ok(v)
    }

    /// Install a JS WS-message recorder so tests can inspect the
    /// stream of server-pushed events. Must be called before the WS
    /// connects — or right after page navigation, whichever the test
    /// needs. The recorder stores `JSON.parse`d objects in
    /// `window.__omegaWsEvents` (an Array).
    pub async fn install_ws_recorder(&self) -> Result<()> {
        // We rely on the bundle exposing the WS as the only one the
        // page opens. We monkey-patch `WebSocket` so every instance
        // pushes parsed messages into a global array. Idempotent:
        // re-installation just resets the buffer and replaces the
        // hook.
        let script = r"
            (() => {
                if (!window.__omegaWsHooked) {
                    const Orig = window.WebSocket;
                    window.WebSocket = function(url, protos) {
                        const ws = new Orig(url, protos);
                        ws.addEventListener('message', (ev) => {
                            try {
                                const obj = JSON.parse(ev.data);
                                window.__omegaWsEvents.push(obj);
                            } catch (_e) { /* ignore non-JSON */ }
                        });
                        return ws;
                    };
                    window.WebSocket.prototype = Orig.prototype;
                    window.__omegaWsHooked = true;
                }
                window.__omegaWsEvents = [];
                return true;
            })()
        ";
        let _ = self.page.evaluate(script).await?;
        Ok(())
    }

    /// Return the recorded WS events as raw JSON values. Pair with
    /// [`Self::install_ws_recorder`].
    pub async fn ws_events(&self) -> Result<Vec<Value>> {
        let v: Vec<Value> = self
            .page
            .evaluate("window.__omegaWsEvents || []")
            .await?
            .into_value()
            .unwrap_or_default();
        Ok(v)
    }

    /// Wait for a recorded WS event matching a JSON `type` field.
    pub async fn wait_for_ws_event(&self, type_: &str, timeout: Duration) -> Result<Value> {
        let deadline = Instant::now() + timeout;
        loop {
            for ev in self.ws_events().await? {
                if ev.get("type").and_then(Value::as_str) == Some(type_) {
                    return Ok(ev);
                }
            }
            if Instant::now() >= deadline {
                bail!("timed out waiting for WS event type={type_:?}");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    // ---------------- session helpers ----------------

    /// Read `<main data-active-session-dir>`.
    pub async fn active_dir(&self) -> Result<String> {
        self.attr("main", "data-active-session-dir")
            .await?
            .ok_or_else(|| anyhow!("data-active-session-dir not set"))
    }

    /// Open the session picker if it isn't already open. The picker
    /// is mounted/unmounted, not just hidden — so we test for the
    /// element's presence in the DOM.
    pub async fn open_picker(&self) -> Result<()> {
        let exists: bool = self
            .eval::<bool>("!!document.querySelector('[data-testid=\"leptos-session-picker\"]')")
            .await
            .unwrap_or(false);
        if exists {
            return Ok(());
        }
        self.click("[data-testid='leptos-composer-sessions']")
            .await?;
        self.wait_for_selector("[data-testid='leptos-session-picker']", DEFAULT_TIMEOUT)
            .await?;
        Ok(())
    }

    /// Click `+ new session` (assumes the picker is open) and wait
    /// for `<main data-active-session-dir>` to flip to a new value.
    pub async fn new_session(&self) -> Result<String> {
        let before = self.active_dir().await.unwrap_or_default();
        self.click("[data-testid='leptos-session-new']").await?;
        let deadline = Instant::now() + DEFAULT_TIMEOUT;
        loop {
            if let Ok(now) = self.active_dir().await
                && now != before
                && !now.is_empty()
            {
                return Ok(now);
            }
            if Instant::now() >= deadline {
                bail!("new_session: active dir did not change from {before:?}");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Override `window.confirm` and `window.alert` to always accept,
    /// so confirm-protected actions (delete row) don't block. The
    /// override survives until the next page navigation.
    pub async fn auto_accept_dialogs(&self) -> Result<()> {
        let _ = self
            .page
            .evaluate("window.confirm = () => true; window.alert = () => undefined;")
            .await?;
        Ok(())
    }
}

impl Drop for TestHarness {
    fn drop(&mut self) {
        if let Some(task) = self.handler_task.take() {
            task.abort();
        }
        if let Some(mut child) = self.server_child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn pick_free_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").context("bind :0")?;
    let port = listener.local_addr().context("local_addr")?.port();
    drop(listener);
    Ok(port)
}

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR for omega-e2e is rust/crates/omega-e2e — go
    // up two levels for the rust/ root, then up once more for the
    // repo root (where frontends/leptos/dist lives).
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .ancestors()
        .nth(3)
        .unwrap_or(manifest)
        .to_path_buf()
}

fn mock_server_binary() -> PathBuf {
    // The Justfile recipe `rust-e2e` builds it first; this resolves
    // the resulting release binary path inside the workspace.
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .ancestors()
        .nth(2)
        .unwrap_or(manifest)
        .join("target/release/mock-omega-server")
}

fn spawn_mock_server(main_port: u16, ctrl_port: u16, sessions_root: &Path) -> Result<Child> {
    let bin = mock_server_binary();
    if !bin.exists() {
        bail!(
            "mock-omega-server binary not found at {}: build it first via \
             `cargo build --release -p omega-mock-server`",
            bin.display()
        );
    }
    let leptos_dir = workspace_root().join("frontends/leptos/dist");
    let child = Command::new(&bin)
        .arg("--port")
        .arg(main_port.to_string())
        .arg("--ctrl-port")
        .arg(ctrl_port.to_string())
        .arg("--sessions-root")
        .arg(sessions_root)
        .arg("--leptos-dir")
        .arg(&leptos_dir)
        // Run with cwd = workspace root so agent file-completion
        // sees real subdirectories (e.g. `rust/`, `frontends/`).
        .current_dir(workspace_root())
        .env("OMEGA_ALLOW_DIRTY", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("spawn {}", bin.display()))?;
    Ok(child)
}

async fn wait_for_health(http: &reqwest::Client, base_url: &str) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(resp) = http.get(format!("{base_url}/health")).send().await
            && resp.status().is_success()
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!("server at {base_url} did not become ready within 10 s");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn launch_browser() -> Result<(Browser, JoinHandle<()>)> {
    // BrowserConfig::builder defaults to headless=true on 0.9.x.
    let config = BrowserConfig::builder()
        .arg("--no-sandbox")
        .arg("--disable-dev-shm-usage")
        .build()
        .map_err(|e| anyhow!("BrowserConfig::build: {e}"))?;
    let _ = Arc::new(HandlerConfig::default()); // keep import live
    let (browser, mut handler) = Browser::launch(config)
        .await
        .context("Browser::launch (need chromium/chrome installed)")?;
    let task = tokio::spawn(async move {
        while let Some(ev) = handler.next().await {
            if ev.is_err() {
                break;
            }
        }
    });
    Ok((browser, task))
}

/// Quote a Rust `&str` as a JS-safe string literal. Useful when
/// embedding selectors into `eval` expressions in tests.
#[must_use]
pub fn js_string(s: &str) -> String {
    json_str(s)
}

fn json_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// Convert `MockResponse` Rust enum to the camelCase JSON shape the
/// control endpoint expects. The upstream type is `Deserialize`-only
/// so we mirror its wire format here.
fn script_to_json(steps: &[MockResponse]) -> Vec<Value> {
    steps.iter().map(mock_response_to_json).collect()
}

fn mock_response_to_json(r: &MockResponse) -> Value {
    match r {
        MockResponse::Text {
            text,
            input_tokens,
            output_tokens,
        } => serde_json::json!({
            "kind": "text",
            "text": text,
            "inputTokens": input_tokens,
            "outputTokens": output_tokens,
        }),
        MockResponse::SlowText {
            text,
            chunks,
            delay_ms,
        } => serde_json::json!({
            "kind": "slowText",
            "text": text,
            "chunks": chunks,
            "delayMs": delay_ms,
        }),
        MockResponse::ToolUse { id, name, input } => serde_json::json!({
            "kind": "toolUse",
            "id": id,
            "name": name,
            "input": input,
        }),
        MockResponse::ToolUseMulti { tools } => serde_json::json!({
            "kind": "toolUseMulti",
            "tools": tools.iter().map(|t| serde_json::json!({
                "id": t.id, "name": t.name, "input": t.input,
            })).collect::<Vec<_>>(),
        }),
        MockResponse::HttpError { status, body } => serde_json::json!({
            "kind": "httpError",
            "status": status,
            "body": body,
        }),
    }
}
