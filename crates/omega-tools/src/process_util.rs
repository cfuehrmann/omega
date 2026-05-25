//! Process-group kill helpers shared between `run_command` and `python_repl`.
//!
//! Both tools spawn subprocesses in their own process group (`process_group(0)`)
//! so that hard kills reach all descendants in one shot.  This module provides
//! the two helpers they share:
//!
//! * [`kill_group`] — SIGKILL the entire process group (hard kill).
//! * [`kill_soft`]  — SIGINT one PID (soft interrupt; lets Python raise
//!   `KeyboardInterrupt` and clean up its own children before we escalate).

/// Send SIGKILL to the entire process group identified by `pgid`.
///
/// We go through the **shell's built-in** `kill` rather than the external
/// `util-linux kill` binary.  On systems where `/usr/bin/kill` is the
/// util-linux build, passing a negative PID (`-pgid`) causes it to do a
/// process-name search instead of a process-group signal, silently failing.
/// The POSIX shell built-in calls `kill(-pgid, SIGKILL)` correctly.
pub fn kill_group(pgid: u32) {
    let _ = std::process::Command::new("sh")
        .args(["-c", &format!("kill -9 -{pgid}")])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .stdin(std::process::Stdio::null())
        .status();
}

/// Send SIGINT to a single process by PID.
///
/// Used for the soft-escalation step in `python_repl`: we send SIGINT to the
/// Python kernel PID so it raises `KeyboardInterrupt` inside its currently
/// executing snippet, giving `subprocess.run` (and similar) a chance to kill
/// its own children and let the REPL wrapper print the traceback + sentinel.
///
/// Goes through the shell built-in for the same reason as [`kill_group`].
pub fn kill_soft(pid: u32) {
    let _ = std::process::Command::new("sh")
        .args(["-c", &format!("kill -INT {pid}")])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .stdin(std::process::Stdio::null())
        .status();
}
