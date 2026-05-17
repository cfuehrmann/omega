//! `run_background` — spawn a long-lived process, redirect its stdout+stderr
//! to a temp log file, and return the PID + log path.

use std::process::Stdio;

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::state::{BackgroundEntry, next_id, processes};

pub async fn execute(input: Value, _cancel: Option<&CancellationToken>) -> Result<String, String> {
    let command = input["command"]
        .as_str()
        .ok_or("run_background: command is required")?;
    let cwd = input["cwd"].as_str();

    // Build a unique log-file path in the system temp directory.
    let id = next_id();
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let log_file = std::env::temp_dir().join(format!("omega-bg-{ts}-{id}.log"));

    // Open the log file and clone the handle for stderr.
    let log_fd = std::fs::File::create(&log_file)
        .map_err(|e| format!("run_background: failed to create log file: {e}"))?;
    let log_fd2 = log_fd
        .try_clone()
        .map_err(|e| format!("run_background: failed to clone log fd: {e}"))?;

    let mut cmd = tokio::process::Command::new("bash");
    cmd.args(["-c", command])
        .stdin(Stdio::piped())
        .stdout(Stdio::from(log_fd))
        .stderr(Stdio::from(log_fd2))
        .kill_on_drop(false); // caller manages lifetime

    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("run_background: failed to spawn: {e}"))?;

    let pid = child
        .id()
        .ok_or("run_background: spawned process has no PID")?;

    let stdin = child.stdin.take();

    // Register in the global process table.
    let mut procs = processes().lock().await;
    procs.insert(
        pid,
        BackgroundEntry {
            child,
            stdin,
            stdin_closed: false,
        },
    );
    drop(procs);

    let log_str = log_file.to_string_lossy().into_owned();
    Ok(serde_json::json!({ "pid": pid, "logFile": log_str }).to_string())
}
