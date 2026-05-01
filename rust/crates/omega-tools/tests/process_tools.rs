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
    let out = exec("run_command", json!({ "command": "echo err >&2" }))
        .await
        .unwrap();
    assert!(out.contains("err"), "got: {out}");
}

#[tokio::test]
async fn run_command_exit_code_reported() {
    let out = exec("run_command", json!({ "command": "exit 42" }))
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
    assert!(
        out.contains("(no output)") || out.is_empty() || out.contains("exit"),
        "got: {out}"
    );
}

// ---------------------------------------------------------------------------
// run_command — boundary conditions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn run_command_success_does_not_show_exit_code() {
    // A command that exits 0 must not emit "[exit code: 0]" in its output.
    // Kills the `!status.success() → true` match-guard mutation, which would
    // cause even successful commands to append an exit-code notice.
    let out = exec("run_command", json!({ "command": "echo hello" }))
        .await
        .unwrap();
    assert!(out.contains("hello"), "stdout must appear: {out}");
    assert!(
        !out.contains("exit code: 0"),
        "successful command must not show exit-code notice: {out}"
    );
}

#[tokio::test]
async fn run_command_large_stdout_shows_truncation_notice() {
    // stdout > 100 KB with empty stderr → truncation notice must appear.
    // Kills the `|| → &&` mutation on `stdout_capped || stderr_capped`
    // (with &&, only-stdout-capped would produce no notice).
    let out = exec(
        "run_command",
        // Write exactly 100 001 bytes: just over the 100 000-byte cap but small
        // enough that the writer can finish before the reader caps and drops the
        // pipe (avoiding SIGPIPE, which would route into the non-success match arm
        // instead of the Finished(_) truncation arm).
        json!({ "command": "head -c 100001 /dev/zero" }),
    )
    .await
    .unwrap();
    assert!(
        out.contains("truncated") || out.contains("Truncated"),
        "stdout just over 100 KB must produce truncation notice: {out}"
    );
}

#[tokio::test]
async fn run_command_stdout_and_stderr_have_newline_separator() {
    // When stdout has no trailing newline and stderr is non-empty, a '\n'
    // must be inserted between them.  Without it, `[stderr]` would be glued
    // directly to the last stdout byte.
    // Kills the `!result.is_empty() → result.is_empty()` mutation, which
    // inverts the separator guard so the '\n' is omitted when stdout exists.
    let out = exec(
        "run_command",
        json!({ "command": "printf 'no_newline'; echo stderr_line >&2" }),
    )
    .await
    .unwrap();
    // The `[stderr]` marker must NOT be immediately glued to stdout.
    assert!(
        !out.contains("no_newline[stderr]"),
        "stdout and stderr sections must be separated by a newline: {out}"
    );
}

#[tokio::test]
async fn run_command_timeout_kills_entire_process_group() {
    // After a timeout the process *group* must be killed — not only the
    // direct bash child.  Verifies kill_group is actually called.
    // Kills the `kill_group → ()` (no-op) mutation.
    let dir = tempfile::tempdir().unwrap();
    let pidfile = dir.path().join("child.pid");
    // The command launches a background sleep, writes its PID, then sleeps
    // itself.  On timeout both processes must die.
    let cmd = format!(
        "sleep 100 & echo $! > {}; sleep 100",
        pidfile.to_str().unwrap()
    );

    let out = exec("run_command", json!({ "command": cmd, "timeout": 1 }))
        .await
        .unwrap();
    assert!(
        out.contains("timeout") || out.contains("killed"),
        "expected timeout notice: {out}"
    );

    // Give SIGKILL time to reach the process group and init to reap zombies.
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;

    if let Ok(pid_str) = std::fs::read_to_string(&pidfile) {
        if let Ok(pid) = pid_str.trim().parse::<u32>() {
            // Read /proc/<pid>/status: if the file is gone the process is fully
            // dead; if present and State is Z it is a zombie awaiting reaping by
            // init — either way the process is no longer runnable.  Using
            // `kill -0` alone would report zombies as alive, causing false failures.
            let proc_status =
                std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
            let actually_running = proc_status
                .lines()
                .find(|l| l.starts_with("State:"))
                .map(|l| !l.contains('Z'))
                .unwrap_or(false); // absent ⇒ fully reaped ⇒ not running
            assert!(
                !actually_running,
                "background child (pid={pid}) must not be running after process-group kill \
                 (State line: {:?})",
                proc_status.lines().find(|l| l.starts_with("State:"))
            );
        }
    }
    // If the pidfile was never written the command didn't get far enough; the
    // timeout notice check above is sufficient in that case.
}

// ---------------------------------------------------------------------------
// run_background / wait_for_output / write_stdin
// ---------------------------------------------------------------------------

#[tokio::test]
async fn run_background_produces_unique_log_files() {
    // Two concurrent run_background calls must get different log-file paths.
    // The only differentiator when timestamps collide is next_id().
    // Kills the `next_id → 0` and `next_id → 1` constant-replacement mutations.
    let (r1, r2) = tokio::join!(
        exec("run_background", json!({ "command": "true" })),
        exec("run_background", json!({ "command": "true" })),
    );
    let v1: serde_json::Value = serde_json::from_str(&r1.unwrap()).unwrap();
    let v2: serde_json::Value = serde_json::from_str(&r2.unwrap()).unwrap();
    let log1 = v1["logFile"].as_str().unwrap();
    let log2 = v2["logFile"].as_str().unwrap();
    assert_ne!(
        log1, log2,
        "concurrent run_background calls must produce unique log files"
    );
}

#[tokio::test]
async fn run_background_returns_pid_and_log() {
    let out = exec("run_background", json!({ "command": "sleep 1" }))
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
    assert!(
        rv["matched"].as_bool().unwrap_or(false),
        "expected matched: {result}"
    );
    assert!(
        rv["output"].as_str().unwrap_or("").contains("READY"),
        "{result}"
    );
}

#[tokio::test]
async fn wait_for_output_process_exit_detected() {
    let out = exec("run_background", json!({ "command": "echo DONE" }))
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
    let out = exec("run_background", json!({ "command": "sleep 60" }))
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
    assert!(
        rv["timedOut"].as_bool().unwrap_or(false),
        "expected timedOut: {result}"
    );

    // Clean up
    let _ = std::process::Command::new("kill")
        .arg(pid.to_string())
        .status();
}

#[tokio::test]
async fn wait_for_output_returns_on_any_output_when_no_pattern_given() {
    // When neither pattern nor minBytes is provided, effective_min_bytes
    // defaults to Some(1) so the call returns as soon as any byte appears.
    // Kills the `delete !` mutation on `} else if !has_pattern {`.
    //
    // Use a process that sleeps long enough to still be alive when we poll,
    // so we cannot hit the process-exit path instead.
    let out = exec(
        "run_background",
        json!({ "command": "echo OUTPUT_NOW; sleep 10" }),
    )
    .await
    .unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let pid = v["pid"].as_u64().unwrap();
    let log_file = v["logFile"].as_str().unwrap().to_owned();

    // Brief pause so echo has time to flush.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let result = exec(
        "wait_for_output",
        json!({
            "pid":       pid,
            "logFile":   log_file,
            "timeoutMs": 1000
            // no pattern, no minBytes
        }),
    )
    .await
    .unwrap();

    let rv: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert!(
        rv["minBytesReached"].as_bool().unwrap_or(false),
        "must return via minBytesReached when output is present: {result}"
    );
    assert!(
        !rv["timedOut"].as_bool().unwrap_or(false),
        "must not time out when output is already present: {result}"
    );

    let _ = std::process::Command::new("kill")
        .arg(pid.to_string())
        .status();
}

#[tokio::test]
async fn wait_for_output_min_bytes_threshold_fires_correctly() {
    // Verifies that `content.len() >= min` (not `<`) is used.
    // Kills the `>= → <` mutation on the minBytes check in the main loop.
    let out = exec(
        "run_background",
        json!({ "command": "printf 'ABCDEFGHIJ'; sleep 10" }), // 10 bytes
    )
    .await
    .unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let pid = v["pid"].as_u64().unwrap();
    let log_file = v["logFile"].as_str().unwrap().to_owned();

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let result = exec(
        "wait_for_output",
        json!({
            "pid":       pid,
            "logFile":   log_file,
            "timeoutMs": 1000,
            "minBytes":  5   // 10 bytes ≥ 5 → must fire
        }),
    )
    .await
    .unwrap();

    let rv: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert!(
        rv["minBytesReached"].as_bool().unwrap_or(false),
        "minBytesReached must be true when log has ≥ minBytes: {result}"
    );

    let _ = std::process::Command::new("kill")
        .arg(pid.to_string())
        .status();
}

#[tokio::test]
async fn wait_for_output_process_exit_reports_correct_exit_code() {
    // A process that exits 0 must produce processExited=true, exitCode=0.
    // Kills `check_exit → None` and `delete match arm Ok(Some(status))` mutations.
    let out = exec("run_background", json!({ "command": "exit 0" }))
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let pid = v["pid"].as_u64().unwrap();
    let log_file = v["logFile"].as_str().unwrap().to_owned();

    // Wait for the process to finish.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let result = exec(
        "wait_for_output",
        json!({
            "pid":       pid,
            "logFile":   log_file,
            "timeoutMs": 2000,
            "pattern":   "WILL_NOT_MATCH"
        }),
    )
    .await
    .unwrap();

    let rv: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert!(
        rv["processExited"].as_bool().unwrap_or(false),
        "processExited must be true after exit: {result}"
    );
    assert_eq!(
        rv["exitCode"].as_i64(),
        Some(0),
        "exitCode must be 0 for clean exit: {result}"
    );
}

#[tokio::test]
async fn wait_for_output_sigkill_reports_exit_code_minus_one() {
    // A process killed by SIGKILL has no OS exit code; the code path uses
    // `status.code().unwrap_or(-1)` → must report -1.
    // Kills the `delete -` mutation that changes `unwrap_or(-1)` to `unwrap_or(1)`.
    let out = exec("run_background", json!({ "command": "sleep 60" }))
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let pid = v["pid"].as_u64().unwrap();
    let log_file = v["logFile"].as_str().unwrap().to_owned();

    // Kill with SIGKILL — no exit code available to the OS.
    let _ = std::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status();

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let result = exec(
        "wait_for_output",
        json!({
            "pid":       pid,
            "logFile":   log_file,
            "timeoutMs": 1000,
            "pattern":   "WILL_NOT_MATCH"
        }),
    )
    .await
    .unwrap();

    let rv: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert!(
        rv["processExited"].as_bool().unwrap_or(false),
        "processExited must be true after SIGKILL: {result}"
    );
    assert_eq!(
        rv["exitCode"].as_i64(),
        Some(-1),
        "SIGKILL must report exitCode=-1 (unwrap_or(-1)): {result}"
    );
}

#[tokio::test]
async fn write_stdin_basic() {
    // Start a cat process that echoes stdin to stdout (via log file).
    let out = exec("run_background", json!({ "command": "cat" }))
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
    let out = exec("run_background", json!({ "command": "cat" }))
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
    let err = exec("write_stdin", json!({ "pid": pid, "text": "oops" }))
        .await
        .unwrap_err();
    assert!(
        err.contains("closed") || err.contains("stdin"),
        "got: {err}"
    );
}

#[tokio::test]
async fn write_stdin_unknown_pid_returns_error() {
    let err = exec(
        "write_stdin",
        json!({ "pid": 999_999_999_u32, "text": "x" }),
    )
    .await
    .unwrap_err();
    assert!(
        err.contains("999999999") || err.contains("No tracked"),
        "got: {err}"
    );
}
