//! [`PythonRepl`] — the long-lived Python 3 subprocess and its execute loop.
//!
//! This is the orchestrating layer.  It owns the child handle and the
//! end-to-end protocol: write `<code>\n__CODE_END__\n`, race the sentinel
//! against the per-call timeout, escalate (SIGINT → SIGKILL of the process
//! group) on hang, drain pending I/O, and produce a single `String` for
//! the LLM.
//!
//! The supporting concerns live in sibling modules:
//!
//! - [`super::bootstrap`] — apt-get install retry path.
//! - [`super::sentinel`] — per-instance sentinel hash.
//! - [`super::wrapper`]  — the Python-side wrapper script.
//! - [`super::output`]   — head/tail truncation for the LLM result.
//! - [`super::tee`]      — full-fidelity tee log (forensics only).

use std::fmt;

use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};

use crate::process_util::{kill_group, kill_soft};
use crate::tool_ctx::ToolCtx;

use super::bootstrap::{BootstrapInfo, BootstrapOutcome, cached_bootstrap, is_not_found};
use super::output::truncate_for_llm;
use super::sentinel::gen_sentinel;
use super::tee::{flush_log, make_repl_log_path, open_log_writer, tee_line};
use super::wrapper::{CODE_END_MARKER, PYTHON_WRAPPER};

// ---------------------------------------------------------------------------
// Timing constants
// ---------------------------------------------------------------------------

/// Default per-call timeout in seconds.
pub const DEFAULT_TIMEOUT_SECS: u64 = 60;

/// Maximum per-call timeout the LLM may request.  Values above this are
/// silently clamped — a confused LLM cannot wedge the session indefinitely.
pub const MAX_TIMEOUT_SECS: u64 = 600;

/// Grace window (seconds) after SIGINT before escalating to hard kill.
///
/// Two seconds gives Python time to raise `KeyboardInterrupt`, let
/// `subprocess.run` kill its child, print the traceback, and write the
/// sentinel.  Empirically this takes < 200 ms for a clean REPL; 2 s is
/// generous enough to handle a loaded system.
const SOFT_GRACE_SECS: u64 = 2;

/// I/O drain window (milliseconds) after `kill_group` before giving up on
/// reading any remaining bytes.  Matches `run_command`'s grace period.
const HARD_DRAIN_MS: u64 = 500;

// ---------------------------------------------------------------------------
// Spawn helper
// ---------------------------------------------------------------------------

/// Spawn a python3-compatible process for the REPL.
///
/// The process is spawned in its own process group (`process_group(0)`) so
/// that a hard kill via `kill_group(pgid)` reaches all subprocess descendants.
fn try_spawn_python(python_bin: &str) -> Result<PythonRepl, std::io::Error> {
    let sentinel = gen_sentinel();
    let mut cmd = tokio::process::Command::new(python_bin);
    cmd.arg("-u")
        .arg("-c")
        .arg(PYTHON_WRAPPER)
        .arg(&sentinel)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    #[cfg(unix)]
    cmd.process_group(0);

    let mut child = cmd.spawn()?;

    // The child's PID equals its PGID because we set process_group(0).
    let pgid = child.id();

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| std::io::Error::other("python3 stdin not available"))?;
    let stdout_raw = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::other("python3 stdout not available"))?;
    let _ = child.stderr.take();

    Ok(PythonRepl {
        child,
        stdin,
        stdout: BufReader::new(stdout_raw),
        sentinel,
        pgid,
        dead: false,
    })
}

// ---------------------------------------------------------------------------
// PythonRepl
// ---------------------------------------------------------------------------

/// A long-lived Python 3 interpreter subprocess with a stateful REPL.
///
/// Variables defined in one [`execute`](PythonRepl::execute) call persist
/// into subsequent calls for the lifetime of the `PythonRepl` instance.
///
/// Cleaned up (process group killed) on [`Drop`].
pub struct PythonRepl {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    sentinel: String,
    /// Process group ID — equals the child's PID because we use
    /// `process_group(0)` at spawn time.  `None` only on non-Unix targets
    /// where `child.id()` may be unavailable.
    pgid: Option<u32>,
    /// `true` after a hard kill; the next `execute` call will return an
    /// immediate error.  The dispatch layer in `lib.rs` clears the cached
    /// `Option<PythonRepl>` when it sees `is_dead() == true`, causing a
    /// fresh kernel to be spawned on the next tool call.
    dead: bool,
}

impl fmt::Debug for PythonRepl {
    #[mutants::skip]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PythonRepl")
            .field("sentinel", &self.sentinel)
            .field("dead", &self.dead)
            .finish_non_exhaustive()
    }
}

impl PythonRepl {
    /// `true` after a hard kill — the REPL kernel is gone.
    ///
    /// The dispatch layer checks this after every `execute` call and, when
    /// true, sets the cached `Option<PythonRepl>` to `None` so the next tool
    /// call spawns a fresh kernel.
    #[must_use]
    pub fn is_dead(&self) -> bool {
        self.dead
    }

    /// Spawn the Python wrapper subprocess, bootstrapping python3 via apt-get
    /// when it is absent from `$PATH`.
    ///
    /// `pub(crate)` because only the lazy-init path in `omega_tools::lib`
    /// constructs a `PythonRepl`; external consumers (`omega-agent`) hold
    /// the already-constructed instance behind an `Arc<Mutex<Option<...>>>`.
    ///
    /// # Errors
    ///
    /// Returns `Err(String)` if Python cannot be spawned and bootstrapping also
    /// fails, or if the bootstrap step itself fails.
    pub(crate) fn start() -> Result<(Self, Option<BootstrapInfo>), String> {
        Self::start_inner("python3", cached_bootstrap)
    }

    /// Internal entry point used by tests.
    pub(crate) fn start_inner(
        python_bin: &str,
        bootstrap: impl FnOnce() -> BootstrapOutcome,
    ) -> Result<(Self, Option<BootstrapInfo>), String> {
        match try_spawn_python(python_bin) {
            Ok(repl) => Ok((repl, None)),
            Err(ref e) if is_not_found(e) => {
                let outcome = bootstrap();
                match outcome {
                    BootstrapOutcome::Succeeded {
                        ref duration_ms,
                        ref stderr_excerpt,
                    } => {
                        let info = BootstrapInfo {
                            duration_ms: *duration_ms,
                            stderr_excerpt: stderr_excerpt.clone(),
                        };
                        try_spawn_python(python_bin)
                            .map(|repl| (repl, Some(info)))
                            .map_err(|e2| {
                                format!("python3 not available even after bootstrap: {e2}")
                            })
                    }
                    BootstrapOutcome::AptNotFound => Err(
                        "python3 not found and apt-get is not available for bootstrap".to_owned(),
                    ),
                    BootstrapOutcome::AptFailed { ref stderr } => Err(format!(
                        "python3 not available and bootstrap failed: \
                         apt-get install -y python3 returned: {stderr}"
                    )),
                }
            }
            Err(e) => Err(format!("failed to start python3: {e}")),
        }
    }

    /// Check whether a (newline-stripped) line is the end-of-response sentinel.
    ///
    /// Marked `#[mutants::skip]`: the `== → !=` mutation would cause `execute`
    /// to loop forever reading, making tests time out rather than fail.
    #[mutants::skip]
    fn is_end_sentinel(&self, trimmed: &str) -> bool {
        trimmed == self.sentinel
    }

    /// Execute `code` in the persistent REPL and return the combined
    /// stdout+stderr output.
    ///
    /// # Parameters
    ///
    /// - `code`: Python source to execute.
    /// - `timeout_secs`: Per-call deadline.  Clamped to `[1, MAX_TIMEOUT_SECS]`.
    ///   Default: [`DEFAULT_TIMEOUT_SECS`] (60 s).
    /// - `ctx`: Session context used to derive the tee-log path.  `None` in
    ///   tests (log is written to `$TMPDIR/omega-repl-<pid>/`).
    ///
    /// # Return value
    ///
    /// Always a plain string:
    ///
    /// - Normal output, possibly with a truncation marker.
    /// - Soft-timeout annotation: `[python_repl: call timed out…]` with REPL
    ///   state preserved.
    /// - Hard-timeout annotation: `[python_repl: call timed out…]` with REPL
    ///   state lost; sets `is_dead() = true`.
    #[allow(clippy::too_many_lines)]
    pub async fn execute(
        &mut self,
        code: &str,
        timeout_secs: u64,
        ctx: Option<&ToolCtx>,
    ) -> String {
        if self.dead {
            return "[REPL error: kernel was killed — call should not reach a dead repl]"
                .to_owned();
        }

        let clamped_timeout = timeout_secs.clamp(1, MAX_TIMEOUT_SECS);
        let timeout_dur = tokio::time::Duration::from_secs(clamped_timeout);

        // Open the tee log (best-effort — failure is silently ignored).
        let log_path = make_repl_log_path(ctx);
        let mut log_writer: Option<tokio::io::BufWriter<tokio::fs::File>> =
            open_log_writer(&log_path).await;

        // Write the code snippet followed by the end-of-code sentinel.
        let payload = format!("{code}\n{CODE_END_MARKER}\n");
        if self.stdin.write_all(payload.as_bytes()).await.is_err() {
            return "[REPL error: failed to write code to Python process]".to_owned();
        }
        if self.stdin.flush().await.is_err() {
            return "[REPL error: failed to flush Python stdin]".to_owned();
        }

        // ---------------------------------------------------------------
        // Primary read loop — races the timeout.
        // ---------------------------------------------------------------
        let mut all_lines: Vec<String> = Vec::new();
        let mut sentinel_found = false;
        let mut early_exit_msg: Option<String> = None;

        let timeout_fut = tokio::time::sleep(timeout_dur);
        tokio::pin!(timeout_fut);

        'read_loop: loop {
            let mut line = String::new();
            tokio::select! {
                biased;
                result = self.stdout.read_line(&mut line) => {
                    match result {
                        Ok(0) => {
                            early_exit_msg = Some(
                                "\n[REPL error: Python process exited unexpectedly]\n".to_owned()
                            );
                            break 'read_loop;
                        }
                        Ok(_) => {
                            tee_line(&mut log_writer, &line).await;
                            let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
                            if self.is_end_sentinel(trimmed) {
                                sentinel_found = true;
                                break 'read_loop;
                            }
                            all_lines.push(line);
                        }
                        Err(e) => {
                            early_exit_msg = Some(
                                format!("\n[REPL error: I/O error reading output: {e}]\n")
                            );
                            break 'read_loop;
                        }
                    }
                }
                () = &mut timeout_fut => {
                    break 'read_loop;
                }
            }
        }

        flush_log(&mut log_writer).await;

        // Normal completion (sentinel found or early exit error).
        if sentinel_found || early_exit_msg.is_some() {
            let mut output = truncate_for_llm(&all_lines, false);
            if let Some(msg) = early_exit_msg {
                output.push_str(&msg);
            }
            return output;
        }

        // ---------------------------------------------------------------
        // Soft escalation: SIGINT → wait for grace window.
        // ---------------------------------------------------------------
        if let Some(pid) = self.child.id() {
            kill_soft(pid);
        }

        let mut grace_lines: Vec<String> = Vec::new();
        let mut grace_sentinel = false;

        let grace_fut = tokio::time::sleep(tokio::time::Duration::from_secs(SOFT_GRACE_SECS));
        tokio::pin!(grace_fut);

        'grace_loop: loop {
            let mut line = String::new();
            tokio::select! {
                biased;
                result = self.stdout.read_line(&mut line) => {
                    match result {
                        Ok(0) | Err(_) => break 'grace_loop,
                        Ok(_) => {
                            tee_line(&mut log_writer, &line).await;
                            let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
                            if self.is_end_sentinel(trimmed) {
                                grace_sentinel = true;
                                break 'grace_loop;
                            }
                            grace_lines.push(line);
                        }
                    }
                }
                () = &mut grace_fut => break 'grace_loop,
            }
        }

        flush_log(&mut log_writer).await;

        if grace_sentinel {
            // Soft recovery succeeded — kernel is responsive again.
            // Show tail of the main output (most-recent bytes before hang)
            // followed by the traceback from the grace period.
            let main_output = truncate_for_llm(&all_lines, true);
            let grace_output = truncate_for_llm(&grace_lines, false);
            return format!(
                "{main_output}{grace_output}\
                 \n[python_repl: call timed out after {clamped_timeout}s; \
                 SIGINT sent; kernel responsive — REPL state preserved.]"
            );
        }

        // ---------------------------------------------------------------
        // Hard escalation: kill_group → drain → mark dead.
        // ---------------------------------------------------------------
        if let Some(pgid) = self.pgid {
            kill_group(pgid);
        }
        self.dead = true;

        // Brief I/O drain — the pipe should EOF almost immediately.
        let drain_fut = tokio::time::sleep(tokio::time::Duration::from_millis(HARD_DRAIN_MS));
        tokio::pin!(drain_fut);
        loop {
            let mut line = String::new();
            tokio::select! {
                biased;
                result = self.stdout.read_line(&mut line) => {
                    match result {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {
                            tee_line(&mut log_writer, &line).await;
                        }
                    }
                }
                () = &mut drain_fut => break,
            }
        }
        flush_log(&mut log_writer).await;

        let output = truncate_for_llm(&all_lines, true);
        format!(
            "{output}\
             \n[python_repl: call timed out after {clamped_timeout}s; \
             SIGINT did not recover; kernel killed. \
             All prior REPL state lost; the next call will start a fresh kernel.]"
        )
    }
}

impl Drop for PythonRepl {
    #[mutants::skip]
    fn drop(&mut self) {
        // Kill the whole process group so any subprocess descendants are also
        // reaped, not just the direct child.
        if let Some(pgid) = self.pgid {
            kill_group(pgid);
        }
        let _ = self.child.start_kill();
    }
}

// Tests live in a sibling file to keep this module focused on the protocol.
#[cfg(test)]
mod tests;
