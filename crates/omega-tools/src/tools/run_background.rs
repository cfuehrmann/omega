//! `run_background` — start a long-lived process as a **background job**.
//!
//! Contract (Claude Code's `run_in_background`): the call returns
//! **immediately** with a job `id` and a `logFile` path; the process keeps
//! running detached.  Its stdout+stderr are redirected (inside the wrapped
//! command, via `exec > logFile 2>&1`) into that file, so the job streams
//! **zero** `MonitorDelivery` messages and emits exactly **one**
//! `MonitorStopped` when it exits.
//!
//! A background job *is* a monitor: it is spawned through the same
//! [`MonitorManager`](crate::monitors::MonitorManager) so it shows up in the
//! roster and the event log (`MonitorStarted` / `MonitorStopped`).  There is
//! **no** separate process table and **no** blocking wait — the agent reads
//! the `logFile` for output and is notified asynchronously on exit.

use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};

use omega_types::events::{MonitorStartedEvent, OmegaEvent};

use crate::ToolResult;
use crate::tool_ctx::ToolCtx;

/// Monotonic per-process counter that disambiguates log-file names when two
/// background jobs are launched within the same millisecond from the same
/// `ToolCtx` (e.g. concurrent calls in a test).  Production already gets a
/// unique `tool_call_id` per call; the counter is the belt to that braces.
static LOG_SEQ: AtomicU64 = AtomicU64::new(0);

/// Execute the `run_background` tool.  Synchronous: spawning is non-blocking
/// and the function early-returns a [`ToolResult`] (mirroring `monitor`).
pub fn execute(input: &Value, ctx: Option<&ToolCtx>) -> ToolResult {
    let Some(ctx) = ctx else {
        return ToolResult::err(
            "run_background: no session context — background jobs are not available",
        );
    };
    let Some(manager) = &ctx.monitors else {
        return ToolResult::err("run_background: monitors are not enabled for this session");
    };
    let Some(command) = input["command"].as_str() else {
        return ToolResult::err("run_background: 'command' is required");
    };
    let cwd = input["cwd"].as_str();

    // Build a unique log-file path inside the session cache (never $TMPDIR
    // and never the cwd — see the ToolCtx cache-dir invariant).
    let bg_dir = ctx.cache_dir.join("bg");
    if let Err(e) = std::fs::create_dir_all(&bg_dir) {
        return ToolResult::err(format!("run_background: failed to create bg dir: {e}"));
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let seq = LOG_SEQ.fetch_add(1, Ordering::Relaxed);
    let log_file = bg_dir.join(format!("{ts}-{id}-{seq}.log", id = ctx.tool_call_id));
    let log_str = log_file.to_string_lossy().into_owned();

    // Redirect the job's fds to the log file *before* running the user's
    // command.  `exec > 'log' 2>&1` rewires fd1/fd2 of the wrapping `bash`,
    // which closes the stdout/stderr pipes the manager set up → the reader
    // tasks see EOF immediately → zero MonitorDelivery messages.
    let wrapped = format!("exec > '{log_str}' 2>&1\n{command}");

    match manager.spawn(command, &wrapped, cwd) {
        Ok(spawned) => {
            let mut result = ToolResult::ok(
                json!({
                    "id": spawned.id,
                    "logFile": log_str,
                    "pid": spawned.pid,
                })
                .to_string(),
            );
            // Record the job in the event log so forensics + roster see it.
            // The logFile is implicitly recorded in the wrapped command
            // string — no OmegaEvent schema change.
            result
                .extra_events
                .push(OmegaEvent::MonitorStarted(MonitorStartedEvent {
                    id: spawned.id,
                    description: command.to_owned(),
                    command: wrapped,
                    time: spawned.started_at,
                }));
            result
        }
        Err(e) => ToolResult::err(e),
    }
}
