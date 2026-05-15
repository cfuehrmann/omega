//! `run_command` — run a shell command with timeout, tee-on-truncate, and abort.
//!
//! Matches TypeScript `executeRunCommand`:
//! * Spawns `bash -c <command>` in a new process group so orphaned children
//!   are killed along with bash on timeout/abort.
//! * Captures stdout and stderr independently, each guarded by a 50 MB
//!   per-stream safety limit.  The combined output is tee'd to a session-cache
//!   log file and capped at 100 KB for the LLM.
//! * Default truncation bias: **tail** on non-zero exit (errors surface at the
//!   end), **head** on exit 0 (normal output starts at the top).  Override
//!   per-call with `truncation_bias: "head" | "tail" | "middle"`.
//! * Returns non-zero exit codes in a trailing `[exit code: N]` notice.

use std::path::PathBuf;
use std::process::Stdio;

use serde_json::Value;
use tokio::io::AsyncReadExt as _;
use tokio::process::Command;
use tokio::time::Duration;
use tokio_util::sync::CancellationToken;

use crate::cap_and_tee::{TruncationBias, cap_and_tee};
use crate::tool_ctx::ToolCtx;

/// LLM-facing cap: maximum bytes returned in the tool result.
const LLM_CAP: usize = 100_000;

/// Per-stream safety limit: abort reading if a single stream exceeds this.
/// Prevents OOM for runaway processes; the log file is still written first.
const STREAM_SAFETY_LIMIT: usize = 50_000_000;

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
    ctx: Option<&ToolCtx>,
) -> Result<String, String> {
    let command = input["command"]
        .as_str()
        .ok_or("run_command: command is required")?;
    let timeout_s = input["timeout"].as_u64().unwrap_or(DEFAULT_TIMEOUT_S);
    let bias_override = input["truncation_bias"]
        .as_str()
        .map(TruncationBias::parse_bias);

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
        let mut safety_capped = false;
        loop {
            tokio::select! {
                biased;
                () = io_cancel_out.cancelled() => break,
                n = stdout_pipe.read(&mut tmp) => {
                    match n {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            buf.extend_from_slice(&tmp[..n]);
                            if buf.len() >= STREAM_SAFETY_LIMIT {
                                buf.truncate(STREAM_SAFETY_LIMIT);
                                safety_capped = true;
                                break;
                            }
                        }
                    }
                }
            }
        }
        (buf, safety_capped)
    });

    let read_stderr = tokio::spawn(async move {
        let mut buf: Vec<u8> = Vec::new();
        let mut tmp = [0u8; 8_192];
        let mut safety_capped = false;
        loop {
            tokio::select! {
                biased;
                () = io_cancel_err.cancelled() => break,
                n = stderr_pipe.read(&mut tmp) => {
                    match n {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            buf.extend_from_slice(&tmp[..n]);
                            if buf.len() >= STREAM_SAFETY_LIMIT {
                                buf.truncate(STREAM_SAFETY_LIMIT);
                                safety_capped = true;
                                break;
                            }
                        }
                    }
                }
            }
        }
        (buf, safety_capped)
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

    let (stdout_bytes, _stdout_safety_capped, stderr_bytes, _stderr_safety_capped) = tokio::select! {
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

    // Determine truncation bias.  Override takes precedence; otherwise:
    // non-zero exit / timeout / abort → Tail (errors at end),
    // success → Head (interesting output starts at top).
    let bias = bias_override.unwrap_or_else(|| match &outcome {
        Outcome::Finished(Some(s)) if s.success() => TruncationBias::Head,
        _ => TruncationBias::Tail,
    });

    // Append a status suffix to the combined bytes so it appears in the log
    // and (with Tail bias) is always within the capped window.
    let suffix = match &outcome {
        Outcome::Aborted => "\n[killed by abort signal]".to_string(),
        Outcome::TimedOut => format!("\n[killed: timeout after {timeout_s}s]"),
        Outcome::Finished(Some(status)) if !status.success() => status.code().map_or_else(
            || "\n[process killed by signal]".to_string(),
            |code| format!("\n[exit code: {code}]"),
        ),
        Outcome::Finished(_) => String::new(),
    };

    // Build the combined byte buffer: stdout, then "[stderr]\n" header + stderr,
    // then the status suffix.
    let mut combined: Vec<u8> =
        Vec::with_capacity(stdout_bytes.len() + stderr_bytes.len() + suffix.len() + 16);
    combined.extend_from_slice(&stdout_bytes);
    if !stderr_bytes.is_empty() {
        if !stdout_bytes.is_empty() {
            combined.push(b'\n');
        }
        combined.extend_from_slice(b"[stderr]\n");
        combined.extend_from_slice(&stderr_bytes);
    }
    if !suffix.is_empty() {
        combined.extend_from_slice(suffix.as_bytes());
    }

    if combined.trim_ascii().is_empty() {
        return Ok("(no output)".to_string());
    }

    // Resolve the tee log path.
    let log_path = make_run_log_path(ctx, command);

    // Tee to disk and cap for the LLM.
    let capped = cap_and_tee(&combined, LLM_CAP, bias, &log_path)
        .await
        .map_err(|e| format!("run_command: failed to write tee log: {e}"))?;

    Ok(capped.body)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build the tee-log path for a `run_command` invocation.
///
/// With a session context: `<ctx.cache_dir>/run/<ts-ms>-<call_id>-<argv0>.log`.
/// Without context (test fallback): a per-process temp directory with a
/// timestamp-only name (no collision risk since tests run sequentially).
fn make_run_log_path(ctx: Option<&ToolCtx>, command: &str) -> PathBuf {
    let now = chrono::Utc::now();
    let ts = now.format("%Y-%m-%dT%H-%M-%S");
    let ms = now.timestamp_subsec_millis();
    let tag = sanitize_tag(command.split_whitespace().next().unwrap_or("cmd"));

    if let Some(c) = ctx {
        let filename = format!("{ts}-{ms:03}-{}-{tag}.log", c.call_id);
        c.cache_dir.join("run").join(filename)
    } else {
        let filename = format!("{ts}-{ms:03}-{tag}.log");
        std::env::temp_dir()
            .join(format!("omega-run-{}", std::process::id()))
            .join(filename)
    }
}

/// Truncate and sanitize `s` for use as a filename tag.
/// Keeps only ASCII alphanumeric characters and replaces others with `-`.
fn sanitize_tag(s: &str) -> String {
    let clean: String = s
        .chars()
        .take(20)
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    clean.trim_matches('-').to_string()
}

/// Send SIGKILL to the entire process group identified by `pgid`.
///
/// We go through the **shell's built-in** `kill` rather than the external
/// `util-linux kill` binary.  On systems where `/usr/bin/kill` is the
/// util-linux build, passing a negative PID (`-pgid`) causes it to do a
/// process-name search instead of a process-group signal, silently failing.
/// The POSIX shell built-in calls `kill(-pgid, SIGKILL)` correctly.
fn kill_group(pgid: u32) {
    let _ = std::process::Command::new("sh")
        .args(["-c", &format!("kill -9 -{pgid}")])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .stdin(std::process::Stdio::null())
        .status();
}
