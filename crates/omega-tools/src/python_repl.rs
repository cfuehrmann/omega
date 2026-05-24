//! Stateful Python REPL subprocess.
//!
//! [`PythonRepl`] wraps a long-lived `python3 -u` child process and
//! exposes a simple `execute(&mut self, code: &str) -> String` method that
//! sends a snippet to the interpreter, collects combined stdout+stderr,
//! applies output truncation, and returns the result.
//!
//! ## Protocol
//!
//! 1. The Rust side writes `<code>\n__CODE_END__\n` to the child's stdin.
//! 2. The Python wrapper executes the snippet with both `sys.stdout` and
//!    `sys.stderr` redirected into a `StringIO` buffer, so the two streams
//!    are combined in execution order.
//! 3. After execution the wrapper writes `<combined output>` followed by
//!    `<sentinel>\n` (e.g. `__REPL_END_1a2b3c4d5e6f7890__\n`) to its
//!    real stdout.
//! 4. The Rust side reads lines until it sees the sentinel line.
//!
//! ## Truncation
//!
//! Output is truncated at **200 lines** OR **2 000 characters**, whichever
//! is hit first.  These are intentionally low bounds for the benchmark
//! prototype (Terminal-Bench 2).  When truncated, a plain-text suffix is
//! appended:
//!
//! ```text
//! ... [output truncated: 142 lines / 7 840 chars suppressed]
//! ```
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
//! - Streaming output line-by-line (blocks until the sentinel arrives).
//! - Replay on strict resume — see `[oq-repl-replay]` in
//!   `docs/session-design.html`.

use std::sync::atomic::{AtomicU64, Ordering};

use std::fmt;
use std::fmt::Write as _;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};

// ---------------------------------------------------------------------------
// Truncation configuration
// ---------------------------------------------------------------------------

/// Maximum number of output lines to include before truncating.
///
/// 200 lines is deliberately low for the benchmark prototype.  The LLM can
/// always fall back to `run_command("python3 -c ...")` if it needs
/// untruncated output.
pub const MAX_OUTPUT_LINES: usize = 200;

/// Maximum number of output characters to include before truncating.
///
/// 2 000 characters is deliberately low for the benchmark prototype.  The
/// limit ensures tool results stay within the context window even when the
/// user's code produces verbose output.
pub const MAX_OUTPUT_CHARS: usize = 2_000;

// ---------------------------------------------------------------------------
// Sentinel generation
// ---------------------------------------------------------------------------

static REPL_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Generate a unique sentinel string for a new `PythonRepl` instance.
///
/// Combines a monotonic counter, the current process PID, and a coarse
/// nanosecond timestamp to produce a string that user code is extremely
/// unlikely to emit by accident (probability ≈ 2⁻⁶⁴ per call).
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
    format!("__REPL_END_{val:016x}__")
}

/// Mix three 64-bit inputs into a single sentinel hash value.
///
/// Uses distinct Fibonacci-derived multiplicative constants so that
/// (time=0, pid=0, counter=0) still produces a non-zero value.
///
/// The exact bitwise combination (XOR) is an implementation detail:
/// any mix that produces a non-trivial value satisfies the contract.
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
/// into a `StringIO` buffer (so the two streams are interleaved in execution
/// order), then writes the combined output followed by the sentinel line.
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
/// Cleaned up (process killed) on [`Drop`].
pub struct PythonRepl {
    // Note: `Child`, `ChildStdin`, and `BufReader<ChildStdout>` do not all
    // implement `Debug` uniformly, so we provide a hand-written impl below.
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    sentinel: String,
}

impl fmt::Debug for PythonRepl {
    // Debug is presentation-only; mutation behaviour identical to existing
    // pattern in the codebase (see system_prompt.rs).
    #[mutants::skip]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PythonRepl")
            .field("sentinel", &self.sentinel)
            .finish_non_exhaustive()
    }
}

impl PythonRepl {
    /// Spawn the Python wrapper subprocess.
    ///
    /// Fails when `python3` is not in `$PATH` or the OS refuses to spawn the
    /// process.
    ///
    /// # Errors
    ///
    /// Returns a human-readable error string on failure.
    pub fn start() -> Result<Self, String> {
        let sentinel = gen_sentinel();
        let mut child = tokio::process::Command::new("python3")
            .arg("-u") // unbuffered stdout/stderr
            .arg("-c")
            .arg(PYTHON_WRAPPER)
            .arg(&sentinel)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            // stderr: piped and ignored.  The wrapper redirects Python's own
            // sys.stderr into the StringIO buf, so the wrapper's real stderr
            // is only used for Python startup errors (e.g. syntax error in the
            // wrapper itself).  Piping it silences noise on the terminal.
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("failed to start python3: {e}"))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "python3 stdin not available".to_owned())?;
        let stdout_raw = child
            .stdout
            .take()
            .ok_or_else(|| "python3 stdout not available".to_owned())?;
        // stderr is piped but not read — dropping it lets the OS drain it
        // without blocking the subprocess.
        let _ = child.stderr.take();

        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout_raw),
            sentinel,
        })
    }

    /// Check whether a (newline-stripped) line is the end-of-output sentinel.
    ///
    /// Extracted as a helper so `#[mutants::skip]` can suppress the
    /// `== → !=` mutation, which would cause `execute` to loop forever and
    /// produce a cargo-mutants TIMEOUT rather than a fast test failure.
    /// The sentinel-safety tests (`blank_line_in_output_is_preserved`,
    /// `xyzzy_in_output_is_preserved`) exercise the comparison on real
    /// sentinel values without mutation.
    #[mutants::skip]
    fn is_end_sentinel(&self, trimmed: &str) -> bool {
        trimmed == self.sentinel
    }

    /// Execute `code` in the persistent REPL and return the combined
    /// stdout+stderr output.
    ///
    /// Output is truncated at [`MAX_OUTPUT_LINES`] OR [`MAX_OUTPUT_CHARS`],
    /// whichever is reached first.  When truncated, a plain-text suffix is
    /// appended: `\n... [output truncated: N lines / M chars suppressed]`.
    ///
    /// If the Python process has died (e.g. the kernel killed it), an inline
    /// error message is returned instead.
    pub async fn execute(&mut self, code: &str) -> String {
        // Write the code snippet followed by the end-of-code sentinel.
        // We always append a trailing newline before __CODE_END__ so that
        // code ending with a bare expression (no trailing newline) is still
        // properly terminated.
        let payload = format!("{code}\n__CODE_END__\n");
        if self.stdin.write_all(payload.as_bytes()).await.is_err() {
            return "[REPL error: failed to write code to Python process]".to_owned();
        }
        if self.stdin.flush().await.is_err() {
            return "[REPL error: failed to flush Python stdin]".to_owned();
        }

        // Read output lines until the sentinel appears.
        let mut output = String::new();
        let mut line_count: usize = 0;
        let mut suppressed_lines: usize = 0;
        let mut suppressed_chars: usize = 0;
        let mut line_buf = String::new();

        loop {
            line_buf.clear();
            match self.stdout.read_line(&mut line_buf).await {
                Ok(0) => {
                    // EOF — Python process has exited unexpectedly.
                    output.push_str("\n[REPL error: Python process exited unexpectedly]\n");
                    break;
                }
                Ok(_) => {
                    // Strip the trailing newline for sentinel comparison only.
                    let trimmed = line_buf.trim_end_matches('\n').trim_end_matches('\r');
                    // is_end_sentinel is #[mutants::skip]: the mutation
                    // `== → !=` would cause `execute` to hang reading forever
                    // (the sentinel never matches), making tests time out rather
                    // than fail.  Cargo-mutants therefore shows TIMEOUT, not
                    // CAUGHT.  Skipping prevents the exit-3 artefact while the
                    // blank-line and xyzzy tests still exercise the real
                    // sentinel-matching path.
                    if self.is_end_sentinel(trimmed) {
                        break;
                    }
                    // Truncation check: suppress lines beyond the configured limits.
                    if line_count >= MAX_OUTPUT_LINES || output.len() >= MAX_OUTPUT_CHARS {
                        suppressed_lines += 1;
                        suppressed_chars += line_buf.len();
                    } else {
                        output.push_str(&line_buf);
                        line_count += 1;
                    }
                }
                Err(e) => {
                    let _ = write!(output, "\n[REPL error: I/O error reading output: {e}]\n");
                    break;
                }
            }
        }

        // Write the truncation footer.  Must always run when suppressed_lines > 0
        // so that both the line-count and char-count values appear in the output.
        if suppressed_lines > 0 {
            let _ = write!(
                output,
                "\n... [output truncated: {suppressed_lines} lines / {suppressed_chars} chars suppressed]"
            );
        }

        output
    }
}

impl Drop for PythonRepl {
    // Subprocess cleanup is an OS-level side effect: verifying that the PID
    // is gone after `drop` requires polling the OS process table, which is
    // fragile and not meaningful to test here.
    #[mutants::skip]
    fn drop(&mut self) {
        // Non-blocking kill — best effort.  If the process has already exited
        // the error is silently ignored.
        let _ = self.child.start_kill();
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
    // `#[ignore]` because `python3` is available on all supported platforms
    // (Linux CI, macOS dev, Harbor containers).  If `python3` is absent the
    // tests fail loudly with "failed to start python3: …" — the correct
    // diagnostic.

    /// Start a fresh `PythonRepl` or panic with a clear message.
    fn repl_sync() -> PythonRepl {
        PythonRepl::start()
            .unwrap_or_else(|e| panic!("python3 must be available for REPL tests: {e}"))
    }

    // -----------------------------------------------------------------------
    // Basic execution
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn executes_simple_expression() {
        // Wrap in a short tokio timeout so that the mutation
        // `replace == with !=` in the sentinel comparison causes this test
        // to fail quickly (the reader would hang forever otherwise).
        let result = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            let mut r = repl_sync();
            r.execute("print('hello world')").await
        })
        .await;
        let out = result.unwrap_or_else(|_| panic!("execute must complete within 10 s"));
        assert_eq!(out.trim(), "hello world", "got: {out:?}");
    }

    #[tokio::test]
    async fn empty_code_produces_empty_output() {
        let mut r = repl_sync();
        let out = r.execute("").await;
        // The wrapper executes an empty string — no output, no errors.
        assert!(out.is_empty(), "expected empty output, got: {out:?}");
    }

    #[tokio::test]
    async fn only_assignment_produces_no_output() {
        let mut r = repl_sync();
        let out = r.execute("x = 42").await;
        assert!(out.is_empty(), "expected no output, got: {out:?}");
    }

    // -----------------------------------------------------------------------
    // State persistence
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn state_persists_across_calls() {
        let mut r = repl_sync();
        // First call defines a variable.
        let out1 = r.execute("x = 99").await;
        assert!(out1.is_empty(), "define should produce no output: {out1:?}");
        // Second call uses the variable from the first call.
        let out2 = r.execute("print(x)").await;
        assert_eq!(out2.trim(), "99", "state must persist: {out2:?}");
    }

    #[tokio::test]
    async fn accumulated_state_survives_many_calls() {
        let mut r = repl_sync();
        r.execute("total = 0").await;
        for i in 0..5u32 {
            r.execute(&format!("total += {i}")).await;
        }
        let out = r.execute("print(total)").await;
        // 0+1+2+3+4 = 10
        assert_eq!(out.trim(), "10", "accumulated total must be 10: {out:?}");
    }

    // -----------------------------------------------------------------------
    // Error handling — errors appear in output, not as panics
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn runtime_error_lands_in_output() {
        let mut r = repl_sync();
        let out = r.execute("1 / 0").await;
        // Python produces a ZeroDivisionError traceback.
        assert!(
            out.contains("ZeroDivisionError"),
            "traceback must appear in output: {out:?}"
        );
    }

    #[tokio::test]
    async fn name_error_lands_in_output() {
        let mut r = repl_sync();
        let out = r.execute("print(undefined_variable)").await;
        assert!(
            out.contains("NameError"),
            "NameError must appear in output: {out:?}"
        );
    }

    #[tokio::test]
    async fn syntax_error_lands_in_output() {
        let mut r = repl_sync();
        let out = r.execute("def broken(:\n    pass").await;
        assert!(
            out.contains("SyntaxError") || out.contains("Error"),
            "SyntaxError must appear in output: {out:?}"
        );
    }

    #[tokio::test]
    async fn repl_continues_working_after_error() {
        // An error in one call must not kill the REPL — subsequent calls work.
        let mut r = repl_sync();
        let err_out = r.execute("1 / 0").await;
        assert!(err_out.contains("ZeroDivisionError"));
        let ok_out = r.execute("print('still alive')").await;
        assert_eq!(
            ok_out.trim(),
            "still alive",
            "REPL must survive an error: {ok_out:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Only stderr (via Python's sys.stderr write)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn stderr_only_appears_in_output() {
        let mut r = repl_sync();
        // Write only to stderr — the wrapper captures both sys.stdout and
        // sys.stderr in the same buf.
        let out = r
            .execute("import sys; sys.stderr.write('err line\\n')")
            .await;
        assert!(
            out.contains("err line"),
            "stderr must appear in combined output: {out:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Truncation at line boundary
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn truncation_triggers_at_line_limit() {
        let mut r = repl_sync();
        // Emit MAX_OUTPUT_LINES + 10 lines so truncation must fire.
        let n = MAX_OUTPUT_LINES + 10;
        let code = format!("for i in range({n}): print(f'line {{i}}')");
        let out = r.execute(&code).await;
        // Truncation marker must be present.
        assert!(
            out.contains("output truncated"),
            "truncation marker missing from output (got {} chars): {out:.200}",
            out.len()
        );
        // Must report the suppressed count (10 lines suppressed).
        assert!(
            out.contains("10 lines"),
            "suppressed-line count missing from marker: {out:.200}"
        );
    }

    #[tokio::test]
    async fn output_below_limits_is_not_truncated() {
        let mut r = repl_sync();
        // Emit fewer than MAX_OUTPUT_LINES short lines — well under both limits.
        let code = "for i in range(5): print(f'line {i}')".to_owned();
        let out = r.execute(&code).await;
        assert!(
            !out.contains("output truncated"),
            "unexpected truncation: {out:?}"
        );
        assert!(out.contains("line 0"));
        assert!(out.contains("line 4"));
    }

    // -----------------------------------------------------------------------
    // Truncation at character boundary
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn truncation_triggers_at_char_limit() {
        let mut r = repl_sync();
        // Produce a single very long line (more than MAX_OUTPUT_CHARS chars in total
        // once the line itself is included).
        // Strategy: print a string longer than MAX_OUTPUT_CHARS, then print
        // one more line to ensure the char-limit truncation fires.
        let long_str = "x".repeat(MAX_OUTPUT_CHARS + 100);
        let code = format!("print('{long_str}')\nprint('second line')");
        let out = r.execute(&code).await;
        // "second line" must be suppressed by the char limit.
        assert!(
            out.contains("output truncated"),
            "char-limit truncation marker missing: {out:.200}"
        );
        // The suppressed-chars count must be non-zero (catches `+= → *=` mutation
        // on `suppressed_chars` which would produce "0 chars suppressed").
        // "second line\n" is 12 chars; we check for any non-zero integer before
        // " chars suppressed" to avoid hard-coding the exact count.
        let marker_pos = out
            .find("output truncated")
            .unwrap_or_else(|| panic!("marker absent"));
        let after = &out[marker_pos..];
        // Check that the chars value is non-zero: the message ends with
        // "/ N chars suppressed", where N must be > 0.
        assert!(
            after.contains("chars suppressed"),
            "'chars suppressed' missing from marker: {after:?}"
        );
        let chars_val: usize = after
            .split('/')
            .nth(1)
            .unwrap_or_else(|| panic!("no '/' in marker: {after:?}"))
            .split_whitespace()
            .next()
            .unwrap_or_else(|| panic!("no token after '/': {after:?}"))
            .parse()
            .unwrap_or_else(|e| panic!("chars value not a number: {e}"));
        assert!(
            chars_val > 0,
            "suppressed char count must be > 0, got {chars_val}"
        );
    }

    // -----------------------------------------------------------------------
    // Sentinel safety — output that looks like a partial sentinel
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn code_that_prints_repl_end_prefix_is_safe() {
        // The sentinel is `__REPL_END_<hex>__`.  Code printing `__REPL_END_`
        // (without the hex suffix) must not be confused for the sentinel.
        let mut r = repl_sync();
        let out = r.execute("print('__REPL_END_not_a_sentinel__')").await;
        // The output must contain the printed string (not be empty/truncated).
        // If the reader mistook the line for the sentinel, `out` would be empty.
        assert!(
            out.contains("__REPL_END_not_a_sentinel__"),
            "sentinel false-positive: {out:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Very fast / very slow output (regression guards)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn fast_output_all_lines_collected() {
        // Print a burst of lines quickly to ensure all are collected.
        let mut r = repl_sync();
        let code = "for i in range(10): print(i)".to_owned();
        let out = r.execute(&code).await;
        for i in 0..10 {
            assert!(
                out.contains(&i.to_string()),
                "line {i} missing from: {out:?}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // Sentinel uniqueness — guards against degenerate gen_sentinel() values
    // -----------------------------------------------------------------------

    /// A blank line in output must NOT be mistaken for the sentinel.
    ///
    /// If `gen_sentinel()` returned `String::new()` (empty string), then
    /// `print()` (which emits `\n`) would match the sentinel and terminate
    /// the reader before collecting the rest of the output.
    #[tokio::test]
    async fn blank_line_in_output_is_preserved() {
        let mut r = repl_sync();
        // print('a'), print() (blank line), print('b')
        let out = r.execute("print('a'); print(); print('b')").await;
        assert!(out.contains('a'), "first line missing: {out:?}");
        assert!(out.contains('b'), "third line missing: {out:?}");
        // The blank line must be between 'a' and 'b'.
        let pos_a = out
            .find('a')
            .unwrap_or_else(|| panic!("'a' not found in: {out:?}"));
        let pos_b = out
            .rfind('b')
            .unwrap_or_else(|| panic!("'b' not found in: {out:?}"));
        let between = &out[pos_a + 1..pos_b];
        assert!(
            between.contains('\n'),
            "blank line between 'a' and 'b' was swallowed: {out:?}"
        );
    }

    /// The string `"xyzzy"` in output must NOT be mistaken for the sentinel.
    ///
    /// If `gen_sentinel()` returned `"xyzzy"`, then code printing `xyzzy`
    /// would match the sentinel and the reader would terminate before
    /// recording the output, so `out` would be empty.
    #[tokio::test]
    async fn xyzzy_in_output_is_preserved() {
        let mut r = repl_sync();
        let out = r.execute("print('xyzzy')").await;
        assert_eq!(out.trim(), "xyzzy", "output was not 'xyzzy': {out:?}");
    }
}
