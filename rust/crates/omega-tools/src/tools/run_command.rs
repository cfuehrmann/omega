//! `run_command` — run a shell command with timeout, output cap, and abort.
//!
//! Matches TypeScript `executeRunCommand`:
//! * Spawns `bash -c <command>` in a new process group so orphaned children
//!   are killed along with bash on timeout/abort.
//! * Captures stdout and stderr independently, each capped at 100 KB.
//! * Returns non-zero exit codes in a trailing `[exit code: N]` notice.

use std::process::Stdio;

use serde_json::Value;
use tokio::io::AsyncReadExt as _;
use tokio::process::Command;
use tokio::time::Duration;
use tokio_util::sync::CancellationToken;

const OUTPUT_CAP: usize = 100_000;
const DEFAULT_TIMEOUT_S: u64 = 120;

// Outcome must be defined before any statements in `execute` to satisfy
// the `items_after_statements` lint.
#[derive(Debug)]
enum Outcome {
    Finished(Option<std::process::ExitStatus>),
    TimedOut,
    Aborted,
}

#[allow(clippy::too_many_lines)] // inherent complexity of a subprocess tool
pub async fn execute(
    input: Value,
    cancel: Option<&CancellationToken>,
) -> Result<String, String> {
    let command = input["command"]
        .as_str()
        .ok_or("run_command: command is required")?;
    let timeout_s = input["timeout"].as_u64().unwrap_or(DEFAULT_TIMEOUT_S);

    let mut cmd = Command::new("bash");
    cmd.args(["-c", command])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .kill_on_drop(true);

    #[cfg(unix)]
    cmd.process_group(0);

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("run_command: failed to spawn: {e}"))?;

    let pgid = child.id();

    let mut stdout_pipe = child
        .stdout
        .take()
        .ok_or_else(|| "run_command: no stdout pipe".to_string())?;
    let mut stderr_pipe = child
        .stderr
        .take()
        .ok_or_else(|| "run_command: no stderr pipe".to_string())?;

    // Internal token to stop I/O readers once the process group is killed.
    let io_cancel = CancellationToken::new();
    let io_cancel_out = io_cancel.clone();
    let io_cancel_err = io_cancel.clone();

    let read_stdout = tokio::spawn(async move {
        let mut buf: Vec<u8> = Vec::new();
        let mut tmp = [0u8; 8_192];
        let mut capped = false;
        loop {
            tokio::select! {
                biased;
                () = io_cancel_out.cancelled() => break,
                n = stdout_pipe.read(&mut tmp) => {
                    match n {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            buf.extend_from_slice(&tmp[..n]);
                            if buf.len() >= OUTPUT_CAP {
                                buf.truncate(OUTPUT_CAP);
                                capped = true;
                                break;
                            }
                        }
                    }
                }
            }
        }
        (buf, capped)
    });

    let read_stderr = tokio::spawn(async move {
        let mut buf: Vec<u8> = Vec::new();
        let mut tmp = [0u8; 8_192];
        let mut capped = false;
        loop {
            tokio::select! {
                biased;
                () = io_cancel_err.cancelled() => break,
                n = stderr_pipe.read(&mut tmp) => {
                    match n {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            buf.extend_from_slice(&tmp[..n]);
                            if buf.len() >= OUTPUT_CAP {
                                buf.truncate(OUTPUT_CAP);
                                capped = true;
                                break;
                            }
                        }
                    }
                }
            }
        }
        (buf, capped)
    });

    let timeout_dur = Duration::from_secs(timeout_s);

    let outcome = if let Some(ct) = cancel {
        tokio::select! {
            res = child.wait() => Outcome::Finished(res.ok()),
            () = tokio::time::sleep(timeout_dur) => Outcome::TimedOut,
            () = ct.cancelled() => Outcome::Aborted,
        }
    } else {
        tokio::select! {
            res = child.wait() => Outcome::Finished(res.ok()),
            () = tokio::time::sleep(timeout_dur) => Outcome::TimedOut,
        }
    };

    // Only cancel I/O tasks (and kill the process group) on timeout/abort.
    // On a normal exit the tasks drain remaining pipe data via EOF naturally.
    match outcome {
        Outcome::TimedOut | Outcome::Aborted => {
            if let Some(gid) = pgid {
                kill_group(gid);
            }
            io_cancel.cancel();
        }
        Outcome::Finished(_) => {}
    }

    let (stdout_bytes, stdout_capped, stderr_bytes, stderr_capped) = tokio::select! {
        res = async { (read_stdout.await, read_stderr.await) } => {
            let (rs, re) = res;
            let (sb, sc) = rs.unwrap_or_default();
            let (eb, ec) = re.unwrap_or_default();
            (sb, sc, eb, ec)
        }
        // Grace period — should complete almost immediately after kill_group.
        // For normal exits the pipe EOF arrives quickly; 500 ms is a safety net.
        () = tokio::time::sleep(Duration::from_millis(500)) => {
            io_cancel.cancel();
            (Vec::new(), false, Vec::new(), false)
        }
    };

    let stdout = String::from_utf8_lossy(&stdout_bytes).into_owned();
    let stderr = String::from_utf8_lossy(&stderr_bytes).into_owned();

    let suffix = match &outcome {
        Outcome::Aborted => "\n[killed by abort signal]".to_string(),
        Outcome::TimedOut => format!("\n[killed: timeout after {timeout_s}s]"),
        Outcome::Finished(Some(status)) if !status.success() => {
            status.code().map_or_else(
                || "\n[process killed by signal]".to_string(),
                |code| format!("\n[exit code: {code}]"),
            )
        }
        Outcome::Finished(_) => {
            if stdout_capped || stderr_capped {
                "\n[Output truncated at 100KB]".to_string()
            } else {
                String::new()
            }
        }
    };

    let mut result = String::new();
    if !stdout.is_empty() {
        result.push_str(&stdout);
    }
    if !stderr.is_empty() {
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str("[stderr]\n");
        result.push_str(&stderr);
    }
    result.push_str(&suffix);

    if result.trim().is_empty() {
        result = "(no output)".to_string();
    }

    Ok(result)
}

/// Send SIGKILL to the entire process group identified by `pgid`.
fn kill_group(pgid: u32) {
    let _ = std::process::Command::new("kill")
        .args(["-KILL", &format!("-{pgid}")])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}
