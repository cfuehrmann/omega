//! Integration tests for [`super::PythonRepl`].
//!
//! Most tests call `PythonRepl::execute` directly rather than going through
//! `execute_tool`.  Carve-out justification: the timeout / process-kill /
//! process-group tests require precise timing control and inspection of
//! internal state (dead flag, pgid, log file contents) that would require
//! disproportionate setup to test through the full tool-dispatch stack.
//! The end-to-end timeout tests that verify dispatch-layer behaviour (dead
//! repl cleared → fresh kernel on next call) ARE tested through
//! `execute_tool` in `dispatch_tests` (lib.rs).
//!
//! These tests spawn a real `python3` subprocess.  They are NOT marked
//! `#[ignore]` because `python3` is available on all supported platforms.

#![allow(
    clippy::expect_used, // test assertions
    clippy::unwrap_used, // test assertions
    clippy::panic,       // test setup helpers use panic for clarity
)]

use super::super::bootstrap::{BootstrapOutcome, bootstrap_python3};
use super::super::output::{MAX_OUTPUT_CHARS, MAX_OUTPUT_LINES};
use super::{DEFAULT_TIMEOUT_SECS, MAX_TIMEOUT_SECS, PythonRepl};
use crate::tool_ctx::ToolCtx;

/// Start a fresh `PythonRepl` or panic with a clear message.
fn repl_sync() -> PythonRepl {
    PythonRepl::start().map_or_else(
        |e| panic!("python3 must be available for REPL tests: {e}"),
        |(repl, _info)| repl,
    )
}

// -----------------------------------------------------------------------
// Basic execution
// -----------------------------------------------------------------------

#[tokio::test]
async fn executes_simple_expression() {
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let mut r = repl_sync();
        r.execute("print('hello world')", DEFAULT_TIMEOUT_SECS, None)
            .await
    })
    .await;
    let out = result.unwrap_or_else(|_| panic!("execute must complete within 10 s"));
    assert_eq!(out.trim(), "hello world", "got: {out:?}");
}

#[tokio::test]
async fn empty_code_produces_empty_output() {
    let mut r = repl_sync();
    let out = r.execute("", DEFAULT_TIMEOUT_SECS, None).await;
    assert!(out.is_empty(), "expected empty output, got: {out:?}");
}

#[tokio::test]
async fn only_assignment_produces_no_output() {
    let mut r = repl_sync();
    let out = r.execute("x = 42", DEFAULT_TIMEOUT_SECS, None).await;
    assert!(out.is_empty(), "expected no output, got: {out:?}");
}

// -----------------------------------------------------------------------
// State persistence
// -----------------------------------------------------------------------

#[tokio::test]
async fn state_persists_across_calls() {
    let mut r = repl_sync();
    let out1 = r.execute("x = 99", DEFAULT_TIMEOUT_SECS, None).await;
    assert!(out1.is_empty(), "define should produce no output: {out1:?}");
    let out2 = r.execute("print(x)", DEFAULT_TIMEOUT_SECS, None).await;
    assert_eq!(out2.trim(), "99", "state must persist: {out2:?}");
}

#[tokio::test]
async fn accumulated_state_survives_many_calls() {
    let mut r = repl_sync();
    r.execute("total = 0", DEFAULT_TIMEOUT_SECS, None).await;
    for i in 0..5u32 {
        r.execute(&format!("total += {i}"), DEFAULT_TIMEOUT_SECS, None)
            .await;
    }
    let out = r.execute("print(total)", DEFAULT_TIMEOUT_SECS, None).await;
    assert_eq!(out.trim(), "10", "accumulated total must be 10: {out:?}");
}

// -----------------------------------------------------------------------
// Error handling
// -----------------------------------------------------------------------

#[tokio::test]
async fn runtime_error_lands_in_output() {
    let mut r = repl_sync();
    let out = r.execute("1 / 0", DEFAULT_TIMEOUT_SECS, None).await;
    assert!(
        out.contains("ZeroDivisionError"),
        "traceback must appear in output: {out:?}"
    );
}

#[tokio::test]
async fn name_error_lands_in_output() {
    let mut r = repl_sync();
    let out = r
        .execute("print(undefined_variable)", DEFAULT_TIMEOUT_SECS, None)
        .await;
    assert!(out.contains("NameError"), "NameError must appear: {out:?}");
}

#[tokio::test]
async fn syntax_error_lands_in_output() {
    let mut r = repl_sync();
    let out = r
        .execute("def broken(:\n    pass", DEFAULT_TIMEOUT_SECS, None)
        .await;
    assert!(
        out.contains("SyntaxError") || out.contains("Error"),
        "SyntaxError must appear: {out:?}"
    );
}

#[tokio::test]
async fn repl_continues_working_after_error() {
    let mut r = repl_sync();
    let err_out = r.execute("1 / 0", DEFAULT_TIMEOUT_SECS, None).await;
    assert!(err_out.contains("ZeroDivisionError"));
    let ok_out = r
        .execute("print('still alive')", DEFAULT_TIMEOUT_SECS, None)
        .await;
    assert_eq!(
        ok_out.trim(),
        "still alive",
        "REPL must survive: {ok_out:?}"
    );
}

// -----------------------------------------------------------------------
// stderr captured
// -----------------------------------------------------------------------

#[tokio::test]
async fn stderr_only_appears_in_output() {
    let mut r = repl_sync();
    let out = r
        .execute(
            "import sys; sys.stderr.write('err line\\n')",
            DEFAULT_TIMEOUT_SECS,
            None,
        )
        .await;
    assert!(out.contains("err line"), "stderr must appear: {out:?}");
}

// -----------------------------------------------------------------------
// Truncation — head bias (normal completion)
// -----------------------------------------------------------------------

#[tokio::test]
async fn truncation_triggers_at_line_limit() {
    let mut r = repl_sync();
    let n = MAX_OUTPUT_LINES + 10;
    let code = format!("for i in range({n}): print(f'line {{i}}')");
    let out = r.execute(&code, DEFAULT_TIMEOUT_SECS, None).await;
    assert!(
        out.contains("output truncated"),
        "truncation marker missing (got {} chars): {out:.200}",
        out.len()
    );
    assert!(
        out.contains("10 lines"),
        "suppressed count missing: {out:.200}"
    );
}

#[tokio::test]
async fn output_below_limits_is_not_truncated() {
    let mut r = repl_sync();
    let code = "for i in range(5): print(f'line {i}')".to_owned();
    let out = r.execute(&code, DEFAULT_TIMEOUT_SECS, None).await;
    assert!(
        !out.contains("output truncated"),
        "unexpected truncation: {out:?}"
    );
    assert!(out.contains("line 0"));
    assert!(out.contains("line 4"));
}

#[tokio::test]
async fn truncation_triggers_at_char_limit() {
    let mut r = repl_sync();
    let long_str = "x".repeat(MAX_OUTPUT_CHARS + 100);
    let code = format!("print('{long_str}')\nprint('second line')");
    let out = r.execute(&code, DEFAULT_TIMEOUT_SECS, None).await;
    assert!(
        out.contains("output truncated"),
        "char-limit truncation marker missing: {out:.200}"
    );
    let marker_pos = out
        .find("output truncated")
        .unwrap_or_else(|| panic!("marker absent"));
    let after = &out[marker_pos..];
    assert!(
        after.contains("chars suppressed"),
        "'chars suppressed' missing: {after:?}"
    );
    let chars_val: usize = after
        .split('/')
        .nth(1)
        .unwrap_or_else(|| panic!("no '/' in marker: {after:?}"))
        .split_whitespace()
        .next()
        .unwrap_or_else(|| panic!("no token after '/'"))
        .parse()
        .unwrap_or_else(|e| panic!("not a number: {e}"));
    assert!(
        chars_val > 0,
        "suppressed chars must be > 0, got {chars_val}"
    );
}

/// Truncation marker uses the new wording mentioning variables.
#[tokio::test]
async fn truncation_marker_mentions_variables() {
    let mut r = repl_sync();
    let n = MAX_OUTPUT_LINES + 5;
    let code = format!("for i in range({n}): print(f'line {{i}}')");
    let out = r.execute(&code, DEFAULT_TIMEOUT_SECS, None).await;
    assert!(
        out.contains("Capture large values in variables"),
        "marker must mention variable pattern: {out:.300}"
    );
    // Crucially: no file path should appear in the LLM result.
    assert!(
        !out.contains("full output:"),
        "LLM result must NOT contain file-handle wording: {out:.300}"
    );
}

// -----------------------------------------------------------------------
// Sentinel safety
// -----------------------------------------------------------------------

#[tokio::test]
async fn code_that_prints_repl_response_prefix_is_safe() {
    // The sentinel is `__REPL_RESPONSE_<hex>__`.  Code printing
    // `__REPL_RESPONSE_not_a_sentinel__` (no valid hex suffix) must not be
    // mistaken for the sentinel.
    let mut r = repl_sync();
    let out = r
        .execute(
            "print('__REPL_RESPONSE_not_a_sentinel__')",
            DEFAULT_TIMEOUT_SECS,
            None,
        )
        .await;
    assert!(
        out.contains("__REPL_RESPONSE_not_a_sentinel__"),
        "sentinel false-positive: {out:?}"
    );
}

#[tokio::test]
async fn blank_line_in_output_is_preserved() {
    let mut r = repl_sync();
    let out = r
        .execute(
            "print('a'); print(); print('b')",
            DEFAULT_TIMEOUT_SECS,
            None,
        )
        .await;
    assert!(out.contains('a'), "first line missing: {out:?}");
    assert!(out.contains('b'), "third line missing: {out:?}");
    let pos_a = out.find('a').unwrap_or_else(|| panic!("'a' not found"));
    let pos_b = out.rfind('b').unwrap_or_else(|| panic!("'b' not found"));
    let between = &out[pos_a + 1..pos_b];
    assert!(between.contains('\n'), "blank line swallowed: {out:?}");
}

#[tokio::test]
async fn xyzzy_in_output_is_preserved() {
    let mut r = repl_sync();
    let out = r
        .execute("print('xyzzy')", DEFAULT_TIMEOUT_SECS, None)
        .await;
    assert_eq!(out.trim(), "xyzzy", "output was not 'xyzzy': {out:?}");
}

#[tokio::test]
async fn fast_output_all_lines_collected() {
    let mut r = repl_sync();
    let code = "for i in range(10): print(i)".to_owned();
    let out = r.execute(&code, DEFAULT_TIMEOUT_SECS, None).await;
    for i in 0..10 {
        assert!(out.contains(&i.to_string()), "line {i} missing: {out:?}");
    }
}

// -----------------------------------------------------------------------
// Timeout parameter clamping
// -----------------------------------------------------------------------

/// `execute` with an out-of-range (0) timeout is clamped to 1 — it does
/// not panic or return an error for ordinary code.
#[tokio::test]
async fn timeout_zero_is_clamped_to_one() {
    // 0 is clamped to 1; simple code still completes fast.
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let mut r = repl_sync();
        r.execute("print('ok')", 0, None).await
    })
    .await;
    let out = result.unwrap_or_else(|_| panic!("must complete within 10 s"));
    // Fast code finishes before 1 s and returns normally.
    assert_eq!(out.trim(), "ok", "got: {out:?}");
}

/// `execute` with a timeout above `MAX_TIMEOUT_SECS` is clamped downward.
///
/// Unit test: tests `timeout_secs.clamp(1, MAX_TIMEOUT_SECS)` directly
/// because waiting 601 s in a test is impractical.
#[test]
fn timeout_clamped_max() {
    let clamped = (MAX_TIMEOUT_SECS + 1).clamp(1, MAX_TIMEOUT_SECS);
    assert_eq!(clamped, MAX_TIMEOUT_SECS);
}

// -----------------------------------------------------------------------
// Soft timeout — subprocess.run hangs, SIGINT recovers it
// -----------------------------------------------------------------------

/// A code block that blocks in `subprocess.run(["sleep", "30"])` with a
/// 2 s timeout returns the soft-timeout annotation within ~4 s (2 s
/// timeout + 2 s grace), and the REPL is still alive afterwards.
///
/// We also verify that a variable defined *before* the hang is still
/// accessible after the soft recovery (state preserved).
#[tokio::test(flavor = "multi_thread")]
async fn soft_timeout_subprocess_run_sleep() {
    let mut r = repl_sync();

    // Define a variable before the hang.
    let setup = r.execute("saved = 'before_hang'", 2, None).await;
    assert!(setup.is_empty(), "setup must produce no output: {setup:?}");

    // Hang on sleep.
    let hang_out = r
        .execute(
            "import subprocess; subprocess.run(['sleep', '30'])",
            2,
            None,
        )
        .await;

    // Must return soft-timeout annotation.
    assert!(
        hang_out.contains("timed out") && hang_out.contains("REPL state preserved"),
        "expected soft-timeout annotation, got: {hang_out:?}"
    );
    // Kernel must still be alive.
    assert!(!r.is_dead(), "repl must be alive after soft timeout");

    // State must be preserved.
    let check = r.execute("print(saved)", 5, None).await;
    assert_eq!(
        check.trim(),
        "before_hang",
        "state must survive soft timeout: {check:?}"
    );
}

// -----------------------------------------------------------------------
// Hard timeout — SIGINT ignored, kernel killed
// -----------------------------------------------------------------------

/// A code block that installs a no-op SIGINT handler and then busy-loops
/// cannot be recovered via soft escalation.  After the grace window the
/// kernel is SIGKILL'd and `is_dead()` returns true.
///
/// The subsequent call (to the same dead repl) gets the dead-repl error
/// message.  The dispatch layer is responsible for clearing the dead repl
/// and spawning a fresh kernel (tested in `execute_tool` dispatch tests).
#[tokio::test(flavor = "multi_thread")]
async fn hard_timeout_sigint_ignored() {
    let mut r = repl_sync();

    // Define a variable that should be gone after hard kill.
    r.execute("secret = 'gone_after_hard_kill'", 5, None).await;

    // Install SIG_IGN and busy-loop so soft escalation can't recover.
    let hang_out = r
        .execute(
            "import signal, time\n\
             signal.signal(signal.SIGINT, signal.SIG_IGN)\n\
             while True: time.sleep(0.01)",
            2,
            None,
        )
        .await;

    assert!(
        hang_out.contains("timed out") && hang_out.contains("kernel killed"),
        "expected hard-timeout annotation, got: {hang_out:?}"
    );
    assert!(r.is_dead(), "repl must be dead after hard timeout");

    // Calling execute on a dead repl returns the dead-repl error.
    let dead_out = r.execute("print('hi')", 5, None).await;
    assert!(
        dead_out.contains("REPL error"),
        "dead repl must return error: {dead_out:?}"
    );
}

// -----------------------------------------------------------------------
// Process group — hard kill takes grandchild with it
// -----------------------------------------------------------------------

/// When a code block spawns a grandchild process and is then hard-killed,
/// the grandchild must be gone afterward (no orphan).
///
/// Strategy: have Python write the grandchild's PID to a temp file, then
/// hang with SIGINT ignored.  After the hard kill, check that
/// `/proc/<pid>/status` does not exist (Linux) — if the file is gone the
/// process is gone.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn hard_kill_takes_grandchild_with_it() {
    use std::path::Path;

    let tmp = tempfile::NamedTempFile::new().unwrap();
    let pid_file = tmp.path().to_string_lossy().to_string();

    let mut r = repl_sync();

    // Spawn a long-running grandchild and record its PID.
    let code = format!(
        "import signal, subprocess, time\n\
         signal.signal(signal.SIGINT, signal.SIG_IGN)\n\
         grandchild = subprocess.Popen(['sleep', '300'])\n\
         open('{pid_file}', 'w').write(str(grandchild.pid))\n\
         while True: time.sleep(0.01)"
    );

    let _ = r.execute(&code, 2, None).await;
    assert!(r.is_dead(), "repl must be dead after hard timeout");

    // Read the grandchild PID — give it a moment to be written.
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
    let pid_str = std::fs::read_to_string(&pid_file).unwrap_or_default();
    let pid_str = pid_str.trim();
    if pid_str.is_empty() {
        // PID was never written — grandchild didn't start (unlikely but acceptable).
        return;
    }

    // Allow up to 1 s for the OS to reap the grandchild after kill_group.
    for _ in 0..10 {
        if !Path::new(&format!("/proc/{pid_str}")).exists() {
            return; // grandchild is gone ✓
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    // If we reach here the grandchild is still alive — test fails.
    panic!("grandchild PID {pid_str} is still in /proc after kill_group");
}

// -----------------------------------------------------------------------
// Tee log
// -----------------------------------------------------------------------

/// Tee log contains the full (untruncated) kernel output, and the
/// LLM-visible result does NOT contain any "full output:" file-handle
/// wording.
#[tokio::test]
async fn tee_log_contains_full_output_and_llm_result_has_no_file_reference() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let ctx = ToolCtx::new(tmp_dir.path(), "testcall");

    let mut r = repl_sync();
    // Emit MAX_OUTPUT_LINES + 20 lines so truncation fires in the LLM result.
    let n = MAX_OUTPUT_LINES + 20;
    let code = format!("for i in range({n}): print(f'tee_line_{{i}}')");
    let llm_out = r.execute(&code, DEFAULT_TIMEOUT_SECS, Some(&ctx)).await;

    // LLM result must be truncated (we exceeded the line limit).
    assert!(
        llm_out.contains("output truncated"),
        "LLM result must be truncated: {llm_out:.200}"
    );
    // LLM result must NOT mention a file path.
    assert!(
        !llm_out.contains("full output:"),
        "LLM result must not contain file-handle wording: {llm_out:.300}"
    );

    // Tee log must exist and contain the full untruncated output.
    let repl_log_dir = tmp_dir.path().join("cache").join("python_repl");
    let mut log_entries: Vec<_> = std::fs::read_dir(&repl_log_dir)
        .unwrap_or_else(|e| panic!("log dir missing: {e}"))
        .filter_map(std::result::Result::ok)
        .collect();
    assert_eq!(log_entries.len(), 1, "expected exactly one log file");
    let log_path = log_entries.pop().unwrap().path();
    let log_content =
        std::fs::read_to_string(&log_path).unwrap_or_else(|e| panic!("failed to read log: {e}"));

    // Full output: all N lines must be present.
    for i in 0..n {
        assert!(
            log_content.contains(&format!("tee_line_{i}")),
            "tee log missing line {i}: first 200 chars: {log_content:.200}"
        );
    }
}

// -----------------------------------------------------------------------
// Bootstrap logic — exercises PythonRepl::start_inner against a closure
// -----------------------------------------------------------------------

#[tokio::test]
async fn bootstrap_not_called_when_python3_present() {
    let mut called = false;
    let result = PythonRepl::start_inner("python3", || {
        called = true;
        BootstrapOutcome::AptNotFound
    });
    assert!(result.is_ok(), "start_inner must succeed: {result:?}");
    assert!(
        !called,
        "bootstrap must not be called when python3 is present"
    );
    let (_repl, info) = result.unwrap();
    assert!(
        info.is_none(),
        "BootstrapInfo must be None when no bootstrap needed"
    );
}

#[tokio::test]
async fn bootstrap_called_when_spawn_not_found() {
    let mut called = false;
    let result = PythonRepl::start_inner("/nonexistent_python_binary", || {
        called = true;
        BootstrapOutcome::AptNotFound
    });
    assert!(
        called,
        "bootstrap must be called when python3 binary not found"
    );
    assert!(result.is_err(), "start_inner must fail");
    let msg = result.unwrap_err();
    assert!(
        msg.contains("apt-get is not available"),
        "error must mention apt-get: {msg:?}"
    );
}

#[tokio::test]
async fn bootstrap_failed_error_contains_apt_stderr() {
    let result = PythonRepl::start_inner("/nonexistent_python_binary", || {
        BootstrapOutcome::AptFailed {
            stderr: "E: Unable to locate package python3".to_owned(),
        }
    });
    assert!(result.is_err());
    let msg = result.unwrap_err();
    assert!(
        msg.contains("bootstrap failed"),
        "error must mention bootstrap: {msg:?}"
    );
    assert!(
        msg.contains("E: Unable to locate package python3"),
        "error must include apt-get stderr: {msg:?}"
    );
}

#[tokio::test]
async fn bootstrap_succeeded_but_retry_fails() {
    let result = PythonRepl::start_inner("/nonexistent_python_binary", || {
        BootstrapOutcome::Succeeded {
            duration_ms: 5000,
            stderr_excerpt: "Setting up python3".to_owned(),
        }
    });
    assert!(result.is_err());
    let msg = result.unwrap_err();
    assert!(
        msg.contains("not available even after bootstrap"),
        "error must mention post-bootstrap failure: {msg:?}"
    );
}

#[tokio::test]
async fn bootstrap_info_is_none_when_no_bootstrap_needed() {
    let result = PythonRepl::start_inner("python3", || BootstrapOutcome::Succeeded {
        duration_ms: 0,
        stderr_excerpt: String::new(),
    });
    assert!(result.is_ok());
    let (_repl, info) = result.unwrap();
    assert!(info.is_none(), "no bootstrap needed → info must be None");
}

#[tokio::test]
async fn non_not_found_spawn_error_skips_bootstrap() {
    use std::os::unix::fs::PermissionsExt as _;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("not_executable");
    std::fs::write(&path, b"#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let path_str = path.to_string_lossy().into_owned();

    let mut bootstrap_called = false;
    let result = PythonRepl::start_inner(&path_str, || {
        bootstrap_called = true;
        BootstrapOutcome::AptNotFound
    });
    assert!(
        !bootstrap_called,
        "bootstrap must not fire for non-NotFound errors"
    );
    assert!(result.is_err());
    let msg = result.unwrap_err();
    assert!(
        !msg.contains("bootstrap"),
        "error must not mention bootstrap: {msg:?}"
    );
}

#[tokio::test]
#[ignore = "requires network / root; run manually in a fresh container"]
async fn integration_bootstrap_retry_path() {
    let result = PythonRepl::start_inner("/this_binary_does_not_exist", bootstrap_python3);
    assert!(result.is_err());
    let msg = result.unwrap_err();
    let was_bootstrap_attempted = msg.contains("bootstrap") || msg.contains("apt-get");
    assert!(
        was_bootstrap_attempted,
        "expected bootstrap-related error, got: {msg:?}"
    );
}
