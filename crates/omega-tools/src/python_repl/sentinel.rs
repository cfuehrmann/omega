//! Per-call sentinel generation.
//!
//! Each `PythonRepl` instance gets a unique sentinel string of the form
//! `__REPL_RESPONSE_<hex>__`.  The Python wrapper writes this sentinel on
//! its own line after each code snippet's output; the Rust side reads
//! stdout line by line until it sees the sentinel.
//!
//! The hash mixes time (`subsec_nanos`), the host PID, and a process-static
//! counter — enough entropy that no two REPL instances in the same process
//! share a sentinel even if started in the same nanosecond.

use std::sync::atomic::{AtomicU64, Ordering};

/// Process-wide counter folded into the sentinel hash so sentinels are
/// unique even when two REPLs are started in the same nanosecond.
static REPL_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Generate a unique sentinel string for a new `PythonRepl` instance.
///
/// The sentinel marks the end of one call's response — it is printed by the
/// Python wrapper after executing each code snippet.  The name encodes this:
/// `__REPL_RESPONSE_<hex>__` (not `__REPL_END__` which would imply the end
/// of the REPL itself).
pub(super) fn gen_sentinel() -> String {
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
