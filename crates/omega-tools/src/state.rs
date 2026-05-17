//! Global background-process registry.
//!
//! Shared by `run_background`, `wait_for_output`, and `write_stdin`.
//! Kept as a module-level static so handles survive across multiple tool
//! calls within the same CLI session.

use std::collections::HashMap;
use std::sync::OnceLock;

use tokio::process::Child;
use tokio::sync::Mutex;

// ---------------------------------------------------------------------------
// Entry
// ---------------------------------------------------------------------------

/// State for a single background process launched by `run_background`.
pub struct BackgroundEntry {
    /// The spawned child process; needed for non-blocking exit-status checks.
    pub child: Child,
    /// Writable stdin pipe; `None` once stdin has been closed via `write_stdin`
    /// with `end_stdin = true`, or if the process was spawned without a piped
    /// stdin.
    pub stdin: Option<tokio::process::ChildStdin>,
    /// `true` once the caller has explicitly closed stdin.
    pub stdin_closed: bool,
}

// ---------------------------------------------------------------------------
// Singleton
// ---------------------------------------------------------------------------

type Registry = Mutex<HashMap<u32, BackgroundEntry>>;

static PROCESSES: OnceLock<Registry> = OnceLock::new();

/// Returns the singleton process registry.
pub fn processes() -> &'static Registry {
    PROCESSES.get_or_init(|| Mutex::new(HashMap::new()))
}

// ---------------------------------------------------------------------------
// Counter for unique log-file names
// ---------------------------------------------------------------------------

use std::sync::atomic::{AtomicU64, Ordering};
static LOG_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Returns a monotonically increasing counter value suitable for use in a
/// temp-file name.
pub fn next_id() -> u64 {
    LOG_COUNTER.fetch_add(1, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::next_id;

    #[test]
    fn next_id_returns_unique_increasing_values() {
        let a = next_id();
        let b = next_id();
        let c = next_id();
        assert_ne!(a, b, "next_id must not return constant 0 or 1");
        assert_ne!(b, c, "next_id must not return constant 0 or 1");
        assert!(b > a && c > b, "next_id must be monotonically increasing");
    }
}
