//! Stateful Python REPL subprocess.
//!
//! [`PythonRepl`] wraps a long-lived `python3 -u` child process and
//! exposes a simple `execute(&mut self, code, timeout_secs, ctx) -> String`
//! method that sends a snippet to the interpreter, collects combined
//! stdout+stderr, applies output truncation, and returns the result.
//!
//! ## Protocol
//!
//! 1. The Rust side writes `<code>\n__CODE_END__\n` to the child's stdin.
//! 2. The Python wrapper executes the snippet with both `sys.stdout` and
//!    `sys.stderr` redirected into a `StringIO` buffer, so the two streams
//!    are combined in execution order.
//! 3. After execution the wrapper writes `<combined output>` followed by
//!    `<sentinel>\n` (e.g. `__REPL_RESPONSE_1a2b3c4d5e6f7890__\n`) to its
//!    real stdout.
//! 4. The Rust side reads lines until it sees the sentinel line.
//!
//! ## Timeout and escalation
//!
//! Each `execute` call accepts a `timeout_secs` parameter (default
//! [`DEFAULT_TIMEOUT_SECS`], max [`MAX_TIMEOUT_SECS`]).  When the timeout
//! fires before the sentinel arrives:
//!
//! 1. **Soft (SIGINT)**: SIGINT is sent to the Python kernel PID.  Python
//!    raises `KeyboardInterrupt`, the wrapper catches it, prints the
//!    traceback, and writes the sentinel.  A grace window
//!    ([`SOFT_GRACE_SECS`]) watches for the sentinel to appear.  If it does,
//!    the result is annotated `[python_repl: timed out — SIGINT sent; kernel
//!    responsive — REPL state preserved.]`; REPL state is intact.
//! 2. **Hard (SIGKILL to process group)**: If the grace window expires without
//!    the sentinel, `kill_group(pgid)` terminates the kernel and all of its
//!    subprocess descendants in one shot.  The REPL is marked dead; the next
//!    call spawns a fresh kernel (all prior variables are lost).
//!
//! ## Tee (forensics)
//!
//! Every byte the kernel writes to stdout is tee'd to a per-call log file at
//! `<session cache>/python_repl/<timestamp>-<call_id>.log`.  This is for
//! human / machine-assisted post-mortem only — the path is **not** surfaced
//! in the LLM-visible result.  Variables are the natural LLM memory model in
//! a stateful REPL; printed bytes are not.
//!
//! ## Truncation
//!
//! Output is truncated at **200 lines** OR **2 000 characters**, whichever
//! is hit first.  Bias:
//!
//! - **Head** (default, `Completed`): show the first N lines/chars.
//! - **Tail** (`TimedOut*`): show the last N lines/chars so the most-recent
//!   output (just before the hang) is always visible.
//!
//! When truncated, a plain-text suffix is appended:
//!
//! ```text
//! ... [output truncated: N lines / M chars suppressed.
//!      Capture large values in variables and inspect/slice them
//!      in subsequent calls rather than printing them whole.]
//! ```
//!
//! ## Process group
//!
//! The Python kernel is spawned in its own process group (`process_group(0)`).
//! On hard kill, `kill_group(pgid)` sends SIGKILL to the entire group, taking
//! any subprocess descendants (stuck 7z, grand-children) with it.
//!
//! ## Lazy startup
//!
//! The subprocess is started on the first [`PythonRepl::execute`] call, not
//! at construction time.  Use [`PythonRepl::start`] to start it eagerly (tests
//! and the lazy-init path in tool dispatch).
//!
//! ## Out of scope (MVP)
//!
//! - Sandboxing / resource limits (Harbor containers handle isolation).
//! - Streaming output line-by-line (blocks until the sentinel arrives or
//!   timeout fires).
//! - Replay on strict resume — see `[oq-repl-replay]` in
//!   `docs/session-design.html`.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use std::fmt;
use std::fmt::Write as _;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};

use crate::process_util::{kill_group, kill_soft};
use crate::tool_ctx::ToolCtx;

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
// Truncation configuration
// ---------------------------------------------------------------------------

/// Maximum number of output lines to include before truncating.
///
/// 200 lines is deliberately low for the benchmark prototype.
pub const MAX_OUTPUT_LINES: usize = 200;

/// Maximum number of output characters to include before truncating.
///
/// 2 000 characters is deliberately low for the benchmark prototype.
pub const MAX_OUTPUT_CHARS: usize = 2_000;

// ---------------------------------------------------------------------------
// Python3 bootstrap
// ---------------------------------------------------------------------------

/// Timeout for each apt-get command during bootstrap.
#[allow(clippy::duration_suboptimal_units)] // 120s is clearer than 2min for a network timeout
pub(crate) const BOOTSTRAP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Result of a python3 bootstrap attempt.
#[derive(Debug, Clone)]
pub enum BootstrapOutcome {
    /// `apt-get install python3` succeeded; python3 should now be available.
    Succeeded {
        /// Total elapsed time in milliseconds (both apt-get steps combined).
        duration_ms: u64,
        /// First 500 chars of combined apt-get stderr output.
        stderr_excerpt: String,
    },
    /// `apt-get` binary was not found — no apt-based bootstrap possible.
    AptNotFound,
    /// `apt-get` ran but exited non-zero, or timed out, or could not be spawned.
    AptFailed {
        /// Captured stderr (or error description when stderr is unavailable).
        stderr: String,
    },
}

/// Info returned when `PythonRepl::start()` triggers a successful bootstrap.
#[derive(Debug, Clone)]
pub struct BootstrapInfo {
    /// Total elapsed time of the bootstrap in milliseconds.
    pub duration_ms: u64,
    /// First 500 chars of combined apt-get stderr output.
    pub stderr_excerpt: String,
}

/// Process-static cache of the bootstrap result so we pay the apt-get cost
/// at most once per Omega process.  `None` means bootstrap has not yet been
/// attempted.
static BOOTSTRAP_CACHE: OnceLock<BootstrapOutcome> = OnceLock::new();

/// Run a single `apt-get` invocation, waiting up to `timeout`.
///
/// Returns `Ok(stderr)` on success or `Err(BootstrapOutcome)` on failure.
/// A non-zero exit code, timeout, or spawn failure all map to
/// [`BootstrapOutcome::AptFailed`]; a missing `apt-get` binary maps to
/// [`BootstrapOutcome::AptNotFound`].
///
/// Marked `#[mutants::skip]`: like `bootstrap_python3`, the branches depend
/// on whether `apt-get` is present and succeeds, which is an OS-level
/// invariant not testable deterministically in unit tests.
#[mutants::skip]
fn run_apt_get(args: &[&str], timeout: std::time::Duration) -> Result<String, BootstrapOutcome> {
    let mut cmd = std::process::Command::new("apt-get");
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(BootstrapOutcome::AptNotFound);
        }
        Err(e) => {
            return Err(BootstrapOutcome::AptFailed {
                stderr: format!("apt-get spawn failed: {e}"),
            });
        }
    };

    // Wait for the child in a background thread so we can apply a timeout.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });

    match rx.recv_timeout(timeout) {
        Err(_) => Err(BootstrapOutcome::AptFailed {
            stderr: "apt-get timed out".to_owned(),
        }),
        Ok(Err(e)) => Err(BootstrapOutcome::AptFailed {
            stderr: format!("apt-get wait failed: {e}"),
        }),
        Ok(Ok(output)) => {
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            if output.status.success() {
                Ok(stderr)
            } else {
                Err(BootstrapOutcome::AptFailed { stderr })
            }
        }
    }
}

/// Attempt to install python3 via `apt-get`.
///
/// Marked `#[mutants::skip]` — see `run_apt_get` for rationale.
#[mutants::skip]
#[must_use]
pub fn bootstrap_python3() -> BootstrapOutcome {
    let overall_start = std::time::Instant::now();

    if let Err(outcome) = run_apt_get(&["update", "-qq"], BOOTSTRAP_TIMEOUT) {
        return outcome;
    }

    match run_apt_get(
        &["install", "-y", "--no-install-recommends", "python3"],
        BOOTSTRAP_TIMEOUT,
    ) {
        Err(outcome) => outcome,
        Ok(stderr) => BootstrapOutcome::Succeeded {
            duration_ms: u64::try_from(overall_start.elapsed().as_millis()).unwrap_or(u64::MAX),
            stderr_excerpt: stderr.chars().take(500).collect(),
        },
    }
}

/// Return `true` when the I/O error indicates the binary was not found.
fn is_not_found(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::NotFound
}

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
// Sentinel generation
// ---------------------------------------------------------------------------

static REPL_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Generate a unique sentinel string for a new `PythonRepl` instance.
///
/// The sentinel marks the end of one call's response — it is printed by the
/// Python wrapper after executing each code snippet.  The name encodes this:
/// `__REPL_RESPONSE_<hex>__` (not `__REPL_END__` which would imply the end
/// of the REPL itself).
fn gen_sentinel() -> String {
    let counter = REPL_COUNTER.fetch_add(1, Ordering::Relaxed);
    let time_ns = u64::from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos(),
    );
    let pid = u64::from(std::process::id());
    let val = mix_sentinel_components(time_ns, pid, counter);
    format!("__REPL_RESPONSE_{val:016x}__")
}

/// Mix three 64-bit inputs into a single sentinel hash value.
#[mutants::skip]
fn mix_sentinel_components(time_ns: u64, pid: u64, counter: u64) -> u64 {
    time_ns.wrapping_mul(0x9e37_79b9_7f4a_7c15)
        ^ pid.wrapping_mul(0x6c62_272e_07bb_0142)
        ^ counter.wrapping_mul(0xd167_4fb4_3ead_e7f3)
}

// ---------------------------------------------------------------------------
// Python wrapper script (inlined)
// ---------------------------------------------------------------------------

/// The Python bootstrap executed as `python3 -u -c <WRAPPER> <sentinel>`.
///
/// Reads code snippets from stdin (terminated by `__CODE_END__` on its own
/// line), executes each with both `sys.stdout` and `sys.stderr` redirected
/// into a `StringIO` buffer, then writes the combined output followed by the
/// sentinel line.
///
/// `BaseException` is caught so that `SystemExit`, `KeyboardInterrupt`, and
/// other non-`Exception` raises produce a traceback in the output rather than
/// killing the wrapper process.
const PYTHON_WRAPPER: &str = "\
import sys, io, traceback
_globals = {}
sentinel = sys.argv[1]
lines = []
for raw_line in sys.stdin:
    if raw_line.rstrip('\\n') == '__CODE_END__':
        code = ''.join(lines)
        lines.clear()
        buf = io.StringIO()
        old_out, old_err = sys.stdout, sys.stderr
        sys.stdout = sys.stderr = buf
        try:
            exec(compile(code, '<repl>', 'exec'), _globals)
        except BaseException:
            traceback.print_exc()
        finally:
            sys.stdout = old_out
            sys.stderr = old_err
        sys.stdout.write(buf.getvalue())
        sys.stdout.write(sentinel + '\\n')
        sys.stdout.flush()
    else:
        lines.append(raw_line)
";

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
    /// # Errors
    ///
    /// Returns `Err(String)` if Python cannot be spawned and bootstrapping also
    /// fails, or if the bootstrap step itself fails.
    pub fn start() -> Result<(Self, Option<BootstrapInfo>), String> {
        Self::start_inner("python3", || {
            BOOTSTRAP_CACHE.get_or_init(bootstrap_python3).clone()
        })
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
        let payload = format!("{code}\n__CODE_END__\n");
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

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Truncate `lines` for LLM consumption, respecting the configured limits.
///
/// - `tail_bias = false` (head): show first N lines/chars; suppressed bytes
///   are at the end; truncation marker appended at the end.
/// - `tail_bias = true`  (tail): show last  N lines/chars; suppressed bytes
///   are at the start; truncation marker prepended at the start.
///
/// The truncation marker uses the new wording encouraging the variable pattern.
fn truncate_for_llm(lines: &[String], tail_bias: bool) -> String {
    if tail_bias {
        // Tail bias: find the last window that fits within limits.
        let mut included_chars: usize = 0;
        let mut tail_start = lines.len();

        for (i, line) in lines.iter().enumerate().rev().take(MAX_OUTPUT_LINES) {
            if included_chars + line.len() > MAX_OUTPUT_CHARS {
                break;
            }
            included_chars += line.len();
            tail_start = i;
        }

        let suppressed_count = tail_start;
        let suppressed_chars: usize = lines[..tail_start].iter().map(String::len).sum();

        let mut output = String::new();

        if suppressed_count > 0 {
            let _ = writeln!(
                output,
                "... [output truncated: {suppressed_count} lines / {suppressed_chars} chars \
                 suppressed. Capture large values in variables and inspect/slice them \
                 in subsequent calls rather than printing them whole.]"
            );
        }

        for line in &lines[tail_start..] {
            output.push_str(line);
        }
        output
    } else {
        // Head bias: current behaviour.
        let mut output = String::new();
        let mut line_count: usize = 0;
        let mut suppressed_lines: usize = 0;
        let mut suppressed_chars: usize = 0;

        for line in lines {
            if line_count >= MAX_OUTPUT_LINES || output.len() >= MAX_OUTPUT_CHARS {
                suppressed_lines += 1;
                suppressed_chars += line.len();
            } else {
                output.push_str(line);
                line_count += 1;
            }
        }

        if suppressed_lines > 0 {
            let _ = write!(
                output,
                "\n... [output truncated: {suppressed_lines} lines / {suppressed_chars} chars \
                 suppressed. Capture large values in variables and inspect/slice them \
                 in subsequent calls rather than printing them whole.]"
            );
        }
        output
    }
}

/// Build the tee-log path for a `python_repl` call.
///
/// With context: `<ctx.cache_dir>/python_repl/<ts-ms>-<call_id>.log`.
/// Without context (tests): `$TMPDIR/omega-repl-<pid>/<ts-ms>.log`.
fn make_repl_log_path(ctx: Option<&ToolCtx>) -> PathBuf {
    let now = chrono::Utc::now();
    let ts = now.format("%Y-%m-%dT%H-%M-%S");
    let ms = now.timestamp_subsec_millis();

    if let Some(c) = ctx {
        let filename = format!("{ts}-{ms:03}-{}.log", c.tool_call_id);
        c.cache_dir.join("python_repl").join(filename)
    } else {
        let filename = format!("{ts}-{ms:03}.log");
        std::env::temp_dir()
            .join(format!("omega-repl-{}", std::process::id()))
            .join(filename)
    }
}

/// Open (or create) the tee-log file for writing.  Parent directories are
/// created as needed.  Returns `None` on any I/O error.
async fn open_log_writer(path: &PathBuf) -> Option<tokio::io::BufWriter<tokio::fs::File>> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.ok()?;
    }
    let file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
        .ok()?;
    Some(tokio::io::BufWriter::new(file))
}

/// Append `line` to the tee log, silently ignoring errors.
async fn tee_line(writer: &mut Option<tokio::io::BufWriter<tokio::fs::File>>, line: &str) {
    if let Some(w) = writer {
        let _ = w.write_all(line.as_bytes()).await;
    }
}

/// Flush the tee log, silently ignoring errors.
async fn flush_log(writer: &mut Option<tokio::io::BufWriter<tokio::fs::File>>) {
    if let Some(w) = writer {
        let _ = w.flush().await;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::expect_used,    // test assertions
    clippy::unwrap_used,    // test assertions
    clippy::panic,          // test setup helpers use panic for clarity
)]
mod tests {
    use super::*;

    // Unit tests for PythonRepl.
    //
    // These tests spawn a real `python3` subprocess.  They are NOT marked
    // `#[ignore]` because `python3` is available on all supported platforms.
    //
    // Most tests call PythonRepl::execute directly rather than going through
    // execute_tool.  Carve-out justification: the timeout / process-kill /
    // process-group tests require precise timing control and inspection of
    // internal state (dead flag, pgid, log file contents) that would require
    // disproportionate setup to test through the full tool-dispatch stack.
    // The end-to-end timeout tests that verify dispatch-layer behaviour (dead
    // repl cleared → fresh kernel on next call) ARE tested through
    // execute_tool in `dispatch_tests` (lib.rs).

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
        let log_content = std::fs::read_to_string(&log_path)
            .unwrap_or_else(|e| panic!("failed to read log: {e}"));

        // Full output: all N lines must be present.
        for i in 0..n {
            assert!(
                log_content.contains(&format!("tee_line_{i}")),
                "tee log missing line {i}: first 200 chars: {log_content:.200}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Tail bias on timeout outcomes
    // -----------------------------------------------------------------------

    /// When output is truncated due to a timeout (tail bias), the suppression
    /// marker appears at the start (before the recent output), not at the end.
    #[test]
    fn tail_bias_marker_appears_before_recent_output() {
        // Simulate lines: 210 lines where "recent_line_209" is the last.
        let lines: Vec<String> = (0..=209).map(|i| format!("line_{i}\n")).collect();
        let out = truncate_for_llm(&lines, true);
        // Marker must be present (210 lines > MAX_OUTPUT_LINES=200).
        assert!(
            out.contains("output truncated"),
            "marker missing: {out:.200}"
        );
        // Marker must come BEFORE the tail content.
        let marker_pos = out.find("output truncated").unwrap();
        let recent_pos = out
            .find("line_209")
            .unwrap_or_else(|| panic!("recent line missing: {out:.200}"));
        assert!(
            marker_pos < recent_pos,
            "tail-bias marker must precede recent output (marker={marker_pos} recent={recent_pos})"
        );
    }

    /// Head-bias truncation marker appears at the end (after the early output).
    #[test]
    fn head_bias_marker_appears_after_early_output() {
        let lines: Vec<String> = (0..=209).map(|i| format!("line_{i}\n")).collect();
        let out = truncate_for_llm(&lines, false);
        assert!(out.contains("output truncated"), "marker missing");
        let marker_pos = out.find("output truncated").unwrap();
        let early_pos = out
            .find("line_0")
            .unwrap_or_else(|| panic!("early line missing"));
        assert!(
            early_pos < marker_pos,
            "head-bias early output must precede marker (early={early_pos} marker={marker_pos})"
        );
    }

    // -----------------------------------------------------------------------
    // Tail-bias truncation — char-limit boundary tests
    // (catch mutations on the inner-loop conditions).
    // These are unit tests on `truncate_for_llm` because constructing
    // precise char-limit scenarios via a live REPL subprocess would be
    // fragile and slow.  The boundary conditions are independent of I/O.
    // -----------------------------------------------------------------------

    /// A line whose length equals exactly `MAX_OUTPUT_CHARS` must be included,
    /// not excluded, in the tail window.
    ///
    /// Catches mutations:
    ///   `included_chars + line.len() > MAX_OUTPUT_CHARS`
    ///     → `== MAX_OUTPUT_CHARS` (would exclude the line)
    ///     → `>= MAX_OUTPUT_CHARS` (would exclude the line)
    #[test]
    fn tail_bias_line_exactly_at_char_limit_is_included() {
        // One suppressed line + one line whose length == MAX_OUTPUT_CHARS.
        // Correct: include the last line (0 + MAX_OUTPUT_CHARS > MAX_OUTPUT_CHARS is false).
        // `==` mutation: 0 + MAX_OUTPUT_CHARS == MAX_OUTPUT_CHARS → break (wrong, excludes it).
        // `>=` mutation: 0 + MAX_OUTPUT_CHARS >= MAX_OUTPUT_CHARS → break (wrong, excludes it).
        // NOTE: check for a long run of 'a's (not just a single 'a'), because the
        // truncation-marker prose ("Capture large values...") also contains 'a'.
        //
        // This test also validates the `.take(MAX_OUTPUT_LINES)` path that
        // replaced the explicit `included_lines` counter.
        let exact_line = "a".repeat(MAX_OUTPUT_CHARS - 1) + "\n"; // len = MAX_OUTPUT_CHARS
        assert_eq!(exact_line.len(), MAX_OUTPUT_CHARS);
        let lines = vec!["suppressed\n".to_owned(), exact_line];
        let out = truncate_for_llm(&lines, true);
        assert!(
            out.contains(&"a".repeat(50)),
            "line at exactly MAX_OUTPUT_CHARS bytes must be included in tail window: {out:.100}"
        );
    }

    /// Multiple lines whose combined length stays within `MAX_OUTPUT_CHARS` must
    /// ALL be included in the tail window, even after the first is accumulated.
    ///
    /// Catches mutation:
    ///   `included_chars + line.len()` → `included_chars * line.len()`
    /// (with `*`, after the first line sets `included_chars=101`, the second
    /// check becomes 101*101=10201 > 2000, so the second line is wrongly excluded).
    #[test]
    fn tail_bias_char_accumulation_is_additive_not_multiplicative() {
        // Three lines: first suppressed, last two each 101 chars.
        // 101 + 101 = 202 << MAX_OUTPUT_CHARS → both must be included.
        // With `*` mutation: after including last line (included=101),
        //   check second-to-last: 101 * 101 = 10 201 > 2 000 → would exclude it.
        // NOTE: use long runs (50+ chars) to distinguish content from the
        // truncation-marker prose (which also contains single 'a' and 'b' chars).
        let medium_line_a = "a".repeat(100) + "\n"; // 101 chars
        let medium_line_b = "b".repeat(100) + "\n"; // 101 chars
        let lines = vec!["suppressed\n".to_owned(), medium_line_a, medium_line_b];
        let out = truncate_for_llm(&lines, true);
        assert!(
            out.contains(&"a".repeat(50)) && out.contains(&"b".repeat(50)),
            "both medium lines must be included (char accumulation must be additive): {out:.200}"
        );
        assert!(
            out.contains("suppressed") || out.contains("output truncated"),
            "first line must be suppressed: {out:.200}"
        );
    }

    /// Char accumulation across many lines must enforce the char limit.
    ///
    /// Catches mutation:
    ///   `included_chars += line.len()` → `included_chars *= line.len()`
    /// (with `*=`, `included_chars` stays 0 forever since 0 * anything = 0,
    /// so ALL lines would pass the char check and none would be suppressed).
    #[test]
    fn tail_bias_accumulated_chars_triggers_truncation() {
        // 12 lines of 201 chars each: total 2 412 > MAX_OUTPUT_CHARS=2 000.
        // With correct `+=`: included_chars grows; ~9 lines fit before limit fires.
        // With `*=` mutation: included_chars stays 0; all 12 lines fit → no suppression.
        let long_line = "z".repeat(200) + "\n"; // 201 chars
        let lines: Vec<String> = (0..12).map(|_| long_line.clone()).collect();
        let out = truncate_for_llm(&lines, true);
        assert!(
            out.contains("output truncated"),
            "12 lines of 201 chars must trigger char-limit truncation: {out:.200}"
        );
    }

    /// When nothing is suppressed in tail bias, no truncation marker must appear.
    ///
    /// Catches mutation:
    ///   `if suppressed_count > 0` → `if suppressed_count >= 0`
    /// (`>= 0` is always true for usize, so the marker would appear even
    /// when `suppressed_count` == 0, i.e. nothing was actually suppressed).
    #[test]
    fn tail_bias_no_marker_when_nothing_suppressed() {
        let lines = vec!["line1\n".to_owned(), "line2\n".to_owned()];
        let out = truncate_for_llm(&lines, true);
        assert!(
            !out.contains("output truncated"),
            "no truncation marker expected when output fits within limits: {out:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Bootstrap logic tests
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

    #[test]
    fn is_not_found_discriminates_correctly() {
        let not_found = std::io::Error::new(std::io::ErrorKind::NotFound, "no such file");
        assert!(is_not_found(&not_found));

        let permission = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        assert!(!is_not_found(&permission));

        let broken_pipe = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "broken");
        assert!(!is_not_found(&broken_pipe));
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
}
