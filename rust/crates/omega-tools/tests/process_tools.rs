//! Integration tests for the process tools:
//! run_command, run_background, wait_for_output, write_stdin.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::json;

async fn exec(name: &str, input: serde_json::Value) -> Result<String, String> {
    let result = omega_tools::execute_tool(name, input, None).await;
    if result.is_error {
        Err(result.content)
    } else {
        Ok(result.content)
    }
}

// ---------------------------------------------------------------------------
// run_command
// ---------------------------------------------------------------------------

#[tokio::test]
async fn run_command_basic() {
    let out = exec("run_command", json!({ "command": "echo hello" }))
        .await
        .unwrap();
    assert!(out.contains("hello"), "got: {out}");
}

#[tokio::test]
async fn run_command_stderr_captured() {
    let out = exec(
        "run_command",
        json!({ "command": "echo err >&2" }),
    )
    .await
    .unwrap();
    assert!(out.contains("err"), "got: {out}");
}

#[tokio::test]
async fn run_command_exit_code_reported() {
    let out = exec(
        "run_command",
        json!({ "command": "exit 42" }),
    )
    .await
    .unwrap();
    assert!(out.contains("42"), "expected exit code in output: {out}");
}

#[tokio::test]
async fn run_command_timeout_kills_process() {
    let out = exec(
        "run_command",
        json!({ "command": "sleep 60", "timeout": 1 }),
    )
    .await
    .unwrap();
    assert!(
        out.contains("timeout") || out.contains("killed"),
        "expected timeout notice: {out}"
    );
}

#[tokio::test]
async fn run_command_no_output_shows_placeholder() {
    let out = exec("run_command", json!({ "command": "true" }))
        .await
        .unwrap();
    assert!(out.contains("(no output)") || out.is_empty() || out.contains("exit"),
        "got: {out}");
}

// ---------------------------------------------------------------------------
// run_background / wait_for_output / write_stdin
// ---------------------------------------------------------------------------

#[tokio::test]
async fn run_background_returns_pid_and_log() {
    let out = exec(
        "run_background",
        json!({ "command": "sleep 1" }),
    )
    .await
    .unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(v["pid"].as_u64().is_some(), "missing pid: {out}");
    assert!(v["logFile"].as_str().is_some(), "missing logFile: {out}");

    // Clean up
    let pid = v["pid"].as_u64().unwrap() as u32;
    let _ = std::process::Command::new("kill")
        .arg(pid.to_string())
        .status();
}

#[tokio::test]
async fn wait_for_output_pattern_match() {
    // Spawn a process that prints output after a brief delay.
    let out = exec(
        "run_background",
        json!({ "command": "sleep 0.1 && echo READY" }),
    )
    .await
    .unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let pid = v["pid"].as_u64().unwrap();
    let log_file = v["logFile"].as_str().unwrap().to_owned();

    let result = exec(
        "wait_for_output",
        json!({
            "pid":       pid,
            "logFile":   log_file,
            "timeoutMs": 3000,
            "pattern":   "READY"
        }),
    )
    .await
    .unwrap();

    let rv: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert!(rv["matched"].as_bool().unwrap_or(false), "expected matched: {result}");
    assert!(rv["output"].as_str().unwrap_or("").contains("READY"), "{result}");
}

#[tokio::test]
async fn wait_for_output_process_exit_detected() {
    let out = exec(
        "run_background",
        json!({ "command": "echo DONE" }),
    )
    .await
    .unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let pid = v["pid"].as_u64().unwrap();
    let log_file = v["logFile"].as_str().unwrap().to_owned();

    // Wait briefly for the process to finish, then poll
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let result = exec(
        "wait_for_output",
        json!({
            "pid":       pid,
            "logFile":   log_file,
            "timeoutMs": 2000,
        }),
    )
    .await
    .unwrap();

    let rv: serde_json::Value = serde_json::from_str(&result).unwrap();
    // Either matched (any output) or process exited.
    assert!(
        rv["minBytesReached"].as_bool().unwrap_or(false)
            || rv["processExited"].as_bool().unwrap_or(false),
        "expected exit or minBytes: {result}"
    );
}

#[tokio::test]
async fn wait_for_output_timeout() {
    let out = exec(
        "run_background",
        json!({ "command": "sleep 60" }),
    )
    .await
    .unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let pid = v["pid"].as_u64().unwrap();
    let log_file = v["logFile"].as_str().unwrap().to_owned();

    let result = exec(
        "wait_for_output",
        json!({
            "pid":       pid,
            "logFile":   log_file,
            "timeoutMs": 300,
            "pattern":   "WILL_NEVER_APPEAR"
        }),
    )
    .await
    .unwrap();

    let rv: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert!(rv["timedOut"].as_bool().unwrap_or(false), "expected timedOut: {result}");

    // Clean up
    let _ = std::process::Command::new("kill")
        .arg(pid.to_string())
        .status();
}

#[tokio::test]
async fn write_stdin_basic() {
    // Start a cat process that echoes stdin to stdout (via log file).
    let out = exec(
        "run_background",
        json!({ "command": "cat" }),
    )
    .await
    .unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let pid = v["pid"].as_u64().unwrap();
    let log_file = v["logFile"].as_str().unwrap().to_owned();

    // Write to stdin and close it.
    let write_out = exec(
        "write_stdin",
        json!({
            "pid":       pid,
            "text":      "hello from stdin\n",
            "end_stdin": true
        }),
    )
    .await
    .unwrap();
    assert!(write_out.contains("Wrote"), "got: {write_out}");

    // Wait for cat to finish and emit output.
    let result = exec(
        "wait_for_output",
        json!({
            "pid":       pid,
            "logFile":   log_file,
            "timeoutMs": 2000,
            "pattern":   "hello from stdin"
        }),
    )
    .await
    .unwrap();
    let rv: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert!(
        rv["matched"].as_bool().unwrap_or(false),
        "expected stdin echo in log: {result}"
    );
}

#[tokio::test]
async fn write_stdin_after_close_returns_error() {
    let out = exec(
        "run_background",
        json!({ "command": "cat" }),
    )
    .await
    .unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let pid = v["pid"].as_u64().unwrap();

    // Close stdin
    exec(
        "write_stdin",
        json!({ "pid": pid, "text": "", "end_stdin": true }),
    )
    .await
    .unwrap();

    // Second write should error
    let err = exec(
        "write_stdin",
        json!({ "pid": pid, "text": "oops" }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("closed") || err.contains("stdin"), "got: {err}");
}

#[tokio::test]
async fn write_stdin_unknown_pid_returns_error() {
    let err = exec(
        "write_stdin",
        json!({ "pid": 999_999_999_u32, "text": "x" }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("999999999") || err.contains("No tracked"), "got: {err}");
}
