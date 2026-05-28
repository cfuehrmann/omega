//! Python-side wrapper script and code-block terminator.
//!
//! The wrapper is executed as `python3 -u -c <python_wrapper()> <sentinel>`.
//! It reads code snippets from stdin, runs each one with stdout+stderr
//! merged into a `StringIO` buffer, then writes the combined output
//! followed by the per-instance sentinel line (see [`super::sentinel`]).
//!
//! The Python side and the Rust side agree on the [`CODE_END_MARKER`]
//! string verbatim.

use super::output::{MAX_OUTPUT_CHARS, MAX_OUTPUT_LINES};

/// Code-block terminator written by the Rust side after each snippet.
///
/// Must match the literal that appears in [`python_wrapper`]; both sides
/// of the protocol have to agree on this exact string.
pub(super) const CODE_END_MARKER: &str = "__CODE_END__";

/// Returns the Python bootstrap script executed as
/// `python3 -u -c <wrapper> <sentinel>`.
///
/// Reads code snippets from stdin (terminated by `__CODE_END__` on its own
/// line), executes each with both `sys.stdout` and `sys.stderr` redirected
/// into a `StringIO` buffer, then writes the combined output followed by the
/// sentinel line.
///
/// `BaseException` is caught so that `SystemExit`, `KeyboardInterrupt`, and
/// other non-`Exception` raises produce a traceback in the output rather than
/// killing the wrapper process.
///
/// `OMEGA_OUTPUT_CAP_CHARS` and `OMEGA_OUTPUT_CAP_LINES` are seeded into
/// `_globals` (the execution namespace) so the model can query the cap:
/// `print(OMEGA_OUTPUT_CAP_CHARS)` → 2000.
pub(super) fn python_wrapper() -> String {
    format!(
        "\
import sys, io, traceback, subprocess
def sh(cmd, timeout=None):
    r = subprocess.run([\"bash\", \"-c\", cmd], capture_output=True,
                       text=True, timeout=timeout)
    return r.stdout, r.stderr, r.returncode
OMEGA_OUTPUT_CAP_CHARS = {MAX_OUTPUT_CHARS}
OMEGA_OUTPUT_CAP_LINES = {MAX_OUTPUT_LINES}
_globals = {{\"sh\": sh, \"OMEGA_OUTPUT_CAP_CHARS\": OMEGA_OUTPUT_CAP_CHARS, \
\"OMEGA_OUTPUT_CAP_LINES\": OMEGA_OUTPUT_CAP_LINES}}
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
"
    )
}
