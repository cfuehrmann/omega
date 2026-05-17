//! Build script for omega-agent.
//!
//! Captures the current `git rev-parse --short HEAD` hash and exposes it as
//! `OMEGA_GIT_COMMIT` so the agent can embed it in the `session_started`
//! event at runtime.  Falls back to `"unknown"` when git is unavailable or
//! the source tree is not inside a repository.

fn main() {
    // Re-run this script if HEAD or the packed-refs file changes (covers both
    // new commits and branch switches).
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/packed-refs");

    let hash = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        // Run from the workspace root so it works regardless of the crate's
        // location within the repo.
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_owned();
            if s.is_empty() { None } else { Some(s) }
        })
        .unwrap_or_else(|| "unknown".to_owned());

    println!("cargo:rustc-env=OMEGA_GIT_COMMIT={hash}");
}
