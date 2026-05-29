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
//!    (`SOFT_GRACE_SECS` in [`repl`]) watches for the sentinel to appear.
//!    If it does, the result is annotated `[python_repl: timed out — SIGINT
//!    sent; kernel responsive — REPL state preserved.]`; REPL state is intact.
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
//! ## Module layout
//!
//! The implementation is split into one submodule per concern:
//!
//! | Module | Responsibility |
//! |--------|----------------|
//! | [`bootstrap`] | apt-get install retry when python3 is absent |
//! | [`sentinel`] | per-instance sentinel hash generation |
//! | [`wrapper`] | the Python-side wrapper script + `__CODE_END__` marker |
//! | [`output`] | head/tail truncation for the LLM-visible result |
//! | [`tee`] | full-fidelity tee log (forensics only) |
//! | [`repl`] | `PythonRepl` itself: spawn, execute, escalate, Drop |
//!
//! ## Out of scope (MVP)
//!
//! - Sandboxing / resource limits (Harbor containers handle isolation).
//! - Streaming output line-by-line (blocks until the sentinel arrives or
//!   timeout fires).
//! - Replay on strict resume — see `[oq-repl-replay]` in
//!   `docs/session-design.html`.

mod bootstrap;
mod output;
mod repl;
mod sentinel;
mod tee;
mod wrapper;

pub use repl::PythonRepl;

// Internal re-exports for other modules in the omega-tools crate
// (dispatch layer, ToolCtx-using code).  None of these are part of the
// public API; the only `pub use` above is `PythonRepl`.
pub(crate) use bootstrap::BootstrapInfo;
pub use output::{MAX_OUTPUT_CHARS, MAX_OUTPUT_LINES};
pub use repl::{DEFAULT_TIMEOUT_SECS, MAX_TIMEOUT_SECS};
