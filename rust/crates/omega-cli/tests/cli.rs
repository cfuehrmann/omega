//! End-to-end CLI tests via subprocess + Anthropic-shaped HTTP fake.
//!
//! Mirrors the `nutriterm/tests/cli.rs` pattern: `assert_cmd::cargo_bin_cmd!`
//! invokes the real `omega` binary, `insta` snapshots normalise output,
//! `tempfile::TempDir` isolates the session root from the host.
//!
//! The fake LLM is an axum SSE server (see `common/mod.rs`) addressed via
//! `ANTHROPIC_BASE_URL`. `OMEGA_RETRY_INITIAL_MS=1` keeps the retry-path
//! test to single-digit milliseconds.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::cargo::cargo_bin_cmd;
use insta::assert_snapshot;
use serde_json::Value;
use tempfile::TempDir;

mod common;
use common::{MockResponse, MockServer, normalize_session_line, normalize_temp_paths};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn find_session_dir(root: &Path) -> PathBuf {
    let entry = fs::read_dir(root)
        .unwrap()
        .next()
        .expect("session-root contains no session directory")
        .unwrap();
    entry.path()
}

fn read_events(session_dir: &Path) -> Vec<Value> {
    let body = fs::read_to_string(session_dir.join("events.jsonl")).unwrap();
    body.lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

// ---------------------------------------------------------------------------
// 1. Help / arg parsing
// ---------------------------------------------------------------------------

/// Kills `replace main with ()` — without `main`, no help is printed.
#[test]
fn help_shows_usage() {
    let assert = cargo_bin_cmd!("omega").arg("--help").assert().success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert_snapshot!("help_stdout", stdout);
}

// ---------------------------------------------------------------------------
// 2. Empty / missing API key
// ---------------------------------------------------------------------------

/// Kills:
/// - `replace run -> i32 with 0` (failure path must exit non-zero).
/// - `replace match guard !k.trim().is_empty() with true in run`
///   (a `true` guard treats `""` as a valid key, sends it to the
///   provider, no `ANTHROPIC_API_KEY is not set` message).
#[test]
fn empty_api_key_exits_with_error() {
    let temp = TempDir::new().unwrap();
    let assert = cargo_bin_cmd!("omega")
        .env("ANTHROPIC_API_KEY", "   ") // whitespace-only also empty
        .args([
            "run",
            "--instruction",
            "hi",
            "--session-root",
            temp.path().to_str().unwrap(),
        ])
        .assert()
        .failure();
    let code = assert.get_output().status.code().unwrap_or(-1);
    assert_eq!(code, 1, "expected exit 1, got {code}");

    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(
        stderr.contains("ANTHROPIC_API_KEY is not set"),
        "stderr did not mention missing key: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// 3. Happy path: single text turn
// ---------------------------------------------------------------------------

/// One mock LLM call returning a text block, then `end_turn`. Kills:
/// - `replace run -> i32 with 1` and `replace run -> i32 with -1`
///   (success path must exit 0).
/// - `replace !k.trim().is_empty() with false` and `delete ! in run`
///   (both turn the API-key check into "always reject", so the happy
///   path would exit 1 instead).
/// - `delete match arm OmegaEvent::TurnEnd`
///   (no `[turn complete | …]` line on stderr).
/// - `delete match arm OmegaEvent::LlmCall(_)`
///   (no `.` per LLM call on stderr).
/// - `replace now_iso -> String with String::new()` and `… with "xyzzy".into()`
///   (the `session_started` event in events.jsonl carries an ISO-8601
///   timestamp; both mutants produce something that is not).
#[tokio::test(flavor = "multi_thread")]
async fn happy_path_single_text_turn() {
    let mock = MockServer::start(vec![MockResponse::Text {
        text: "Hello, world!".to_owned(),
        input_tokens: 10,
        output_tokens: 5,
    }])
    .await;

    let temp = TempDir::new().unwrap();
    let session_root = temp.path().join("sessions");
    fs::create_dir_all(&session_root).unwrap();

    let assert = cargo_bin_cmd!("omega")
        .env("ANTHROPIC_API_KEY", "sk-test")
        .env("ANTHROPIC_BASE_URL", &mock.base_url)
        .env("OMEGA_RETRY_INITIAL_MS", "1")
        .current_dir(temp.path())
        .args([
            "run",
            "--instruction",
            "say hi",
            "--session-root",
            session_root.to_str().unwrap(),
        ])
        .assert()
        .success();

    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();

    assert_eq!(stdout, "Hello, world!\n");

    // LlmCall arm renders a single `.`; TurnEnd arm renders the
    // `[turn complete | …]` summary.
    assert!(stderr.contains('.'), "stderr missing LlmCall dot: {stderr}");
    assert!(
        stderr.contains("[turn complete | in=10 out=5 cache_hit=0 cache_write=0]"),
        "stderr missing TurnEnd line: {stderr}"
    );

    // `now_iso` mutants: read events.jsonl and verify the
    // session_started time looks like an ISO-8601 timestamp.
    let session = find_session_dir(&session_root);
    let events = read_events(&session);
    assert!(!events.is_empty(), "no events written");
    let first = &events[0];
    let kind = first.get("type").and_then(|v| v.as_str()).unwrap_or("");
    assert_eq!(kind, "session_started");
    let time = first.get("time").and_then(|v| v.as_str()).unwrap_or("");
    assert!(!time.is_empty(), "session_started.time is empty: {first:?}");
    assert!(
        time.contains('T') && (time.ends_with('Z') || time.contains('+')),
        "session_started.time is not ISO-8601: {time:?}"
    );
    // Belt-and-braces against `… with "xyzzy".into()`.
    assert!(
        !time.contains("xyzzy"),
        "session_started.time was overridden: {time:?}"
    );
}

// ---------------------------------------------------------------------------
// 4. Tool-use round trip
// ---------------------------------------------------------------------------

/// Mock returns `tool_use(read_file, {path: <temp>/hello.txt})`, then a
/// text reply on the second call. Kills:
/// - `delete match arm OmegaEvent::ToolCall(tc)` → `[tool: read_file]`.
/// - `delete match arm OmegaEvent::ToolResult(tr)` → `[result: …]`.
#[tokio::test(flavor = "multi_thread")]
async fn tool_use_then_text() {
    let temp = TempDir::new().unwrap();
    let session_root = temp.path().join("sessions");
    fs::create_dir_all(&session_root).unwrap();

    let file_path = temp.path().join("hello.txt");
    fs::write(&file_path, "ciao\n").unwrap();

    let mock = MockServer::start(vec![
        MockResponse::ToolUse {
            id: "toolu_1".to_owned(),
            name: "read_file".to_owned(),
            input: serde_json::json!({ "path": file_path.to_str().unwrap() }),
        },
        MockResponse::Text {
            text: "done".to_owned(),
            input_tokens: 11,
            output_tokens: 1,
        },
    ])
    .await;

    let assert = cargo_bin_cmd!("omega")
        .env("ANTHROPIC_API_KEY", "sk-test")
        .env("ANTHROPIC_BASE_URL", &mock.base_url)
        .env("OMEGA_RETRY_INITIAL_MS", "1")
        .current_dir(temp.path())
        .args([
            "run",
            "--instruction",
            "read it",
            "--session-root",
            session_root.to_str().unwrap(),
        ])
        .assert()
        .success();

    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(
        stderr.contains("[tool: read_file]"),
        "stderr missing ToolCall line: {stderr}"
    );
    assert!(
        stderr.contains("[result"),
        "stderr missing ToolResult line: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// 5. Retry exhaustion (terminal after max_attempts)
// ---------------------------------------------------------------------------

/// Mock returns four retryable HTTP 500s, then a text response. With
/// `max_attempts: 4` (production wiring) the retry loop gives up on the
/// 4th attempt, the agent emits `AgentError` + `TurnInterrupted{Error}`,
/// the CLI exits non-zero. Kills:
///
/// - `delete field max_attempts from struct RetryConfig expression`
///   (deleting the field falls through to the 32-default; the 5th
///   attempt would succeed and exit code/stderr would change).
/// - `delete field initial_backoff from struct RetryConfig expression`
///   (deleting the field falls through to the 500 ms default; we read
///   `wait_ms` from the persisted `llm_retry` events to verify the
///   1 ms backoff is in effect).
/// - `delete match arm OmegaEvent::AgentError(ae)` → `[agent error: …]`.
/// - `delete match arm OmegaEvent::TurnInterrupted(ti)` → `[turn interrupted: …]`.
#[tokio::test(flavor = "multi_thread")]
async fn retry_exhaustion_emits_agent_error_and_turn_interrupted() {
    let mut script = Vec::new();
    for _ in 0..4 {
        script.push(MockResponse::HttpError {
            status: 500,
            body: "boom".to_owned(),
        });
    }
    // 5th — only reached if max_attempts mutant raises the cap.
    script.push(MockResponse::Text {
        text: "should-not-print".to_owned(),
        input_tokens: 1,
        output_tokens: 1,
    });
    let mock = MockServer::start(script).await;

    let temp = TempDir::new().unwrap();
    let session_root = temp.path().join("sessions");
    fs::create_dir_all(&session_root).unwrap();

    let assert = cargo_bin_cmd!("omega")
        .env("ANTHROPIC_API_KEY", "sk-test")
        .env("ANTHROPIC_BASE_URL", &mock.base_url)
        .env("OMEGA_RETRY_INITIAL_MS", "1")
        .current_dir(temp.path())
        .args([
            "run",
            "--instruction",
            "fail please",
            "--session-root",
            session_root.to_str().unwrap(),
        ])
        .assert()
        .failure();

    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();

    assert!(
        !stdout.contains("should-not-print"),
        "5th-attempt response leaked into stdout (max_attempts mutant survived?): \
         stdout={stdout:?}"
    );
    assert!(
        stderr.contains("[agent error:"),
        "stderr missing AgentError line: {stderr}"
    );
    assert!(
        stderr.contains("[turn interrupted:"),
        "stderr missing TurnInterrupted line: {stderr}"
    );

    // `initial_backoff` mutant: read events.jsonl and verify at least
    // one `llm_retry` event has `wait_ms` consistent with our 1 ms
    // override. The default (500 ms) would land well above this
    // threshold even with -10 % jitter.
    let session = find_session_dir(&session_root);
    let events = read_events(&session);
    let retry_waits: Vec<i64> = events
        .iter()
        .filter(|e| e.get("type").and_then(|v| v.as_str()) == Some("llm_retry"))
        .filter_map(|e| e.get("waitMs").and_then(serde_json::Value::as_i64))
        .collect();
    assert!(
        !retry_waits.is_empty(),
        "no llm_retry events recorded: {events:?}"
    );
    assert!(
        retry_waits.iter().all(|w| *w < 50),
        "retry wait_ms above 1 ms-override threshold (initial_backoff mutant?): {retry_waits:?}"
    );
}

// ---------------------------------------------------------------------------
// 6. Stable stderr snapshot (catches any new arm changing the wire format)
// ---------------------------------------------------------------------------

/// One last belt-and-braces snapshot of the happy-path stderr with paths
/// normalised. Anchors the output format so future drift in the
/// `Session: …` / `[turn complete | …]` lines is caught explicitly.
#[tokio::test(flavor = "multi_thread")]
async fn happy_path_stderr_snapshot() {
    let mock = MockServer::start(vec![MockResponse::Text {
        text: "Hi.".to_owned(),
        input_tokens: 7,
        output_tokens: 2,
    }])
    .await;

    let temp = TempDir::new().unwrap();
    let session_root = temp.path().join("sessions");
    fs::create_dir_all(&session_root).unwrap();

    let assert = cargo_bin_cmd!("omega")
        .env("ANTHROPIC_API_KEY", "sk-test")
        .env("ANTHROPIC_BASE_URL", &mock.base_url)
        .env("OMEGA_RETRY_INITIAL_MS", "1")
        .current_dir(temp.path())
        .args([
            "run",
            "--instruction",
            "hi",
            "--session-root",
            session_root.to_str().unwrap(),
        ])
        .assert()
        .success();

    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    let stderr = normalize_temp_paths(&stderr, temp.path());
    let stderr = normalize_session_line(&stderr);
    assert_snapshot!("happy_path_stderr", stderr);
}
