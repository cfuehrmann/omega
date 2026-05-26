//! Python3 bootstrap via `apt-get`.
//!
//! When the `python3` binary is missing from `$PATH`, the REPL's `start`
//! path retries after invoking [`bootstrap_python3`], which runs
//! `apt-get update` + `apt-get install -y --no-install-recommends python3`.
//! The outcome is cached in a process-static [`OnceLock`] so the apt-get
//! cost is paid at most once per Omega process.
//!
//! Outside an apt-based container (no `apt-get`, e.g. macOS host runs),
//! [`bootstrap_python3`] returns [`BootstrapOutcome::AptNotFound`] and the
//! caller surfaces a clear error.  In Harbor containers `apt-get` is always
//! available, so the warm path is just "try, succeed, never retry".

use std::sync::OnceLock;

/// Timeout for each apt-get command during bootstrap.
#[allow(clippy::duration_suboptimal_units)] // 120s is clearer than 2min for a network timeout
const BOOTSTRAP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Result of a python3 bootstrap attempt.
#[derive(Debug, Clone)]
pub(crate) enum BootstrapOutcome {
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
pub(crate) struct BootstrapInfo {
    /// Total elapsed time of the bootstrap in milliseconds.
    pub duration_ms: u64,
    /// First 500 chars of combined apt-get stderr output.
    pub stderr_excerpt: String,
}

/// Process-static cache of the bootstrap result so we pay the apt-get cost
/// at most once per Omega process.  `None` means bootstrap has not yet been
/// attempted.
static BOOTSTRAP_CACHE: OnceLock<BootstrapOutcome> = OnceLock::new();

/// Return the cached bootstrap outcome, invoking [`bootstrap_python3`] on
/// the first call.  Subsequent calls return the cached clone.
pub(super) fn cached_bootstrap() -> BootstrapOutcome {
    BOOTSTRAP_CACHE.get_or_init(bootstrap_python3).clone()
}

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
pub(crate) fn bootstrap_python3() -> BootstrapOutcome {
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
pub(super) fn is_not_found(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::NotFound
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::expect_used, // test assertions
    clippy::unwrap_used, // test assertions
    clippy::panic,       // test setup helpers use panic for clarity
)]
mod tests {
    use super::*;

    /// Pure unit test of `is_not_found`'s discriminator logic; no I/O.
    #[test]
    fn is_not_found_discriminates_correctly() {
        let not_found = std::io::Error::new(std::io::ErrorKind::NotFound, "no such file");
        assert!(is_not_found(&not_found));

        let permission = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        assert!(!is_not_found(&permission));

        let broken_pipe = std::io::Error::new(std::io::ErrorKind::BrokenPipe, "broken");
        assert!(!is_not_found(&broken_pipe));
    }
}
