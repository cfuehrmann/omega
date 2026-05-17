//! `wait_for_output` — poll a background-process log file until a condition is
//! met.
//!
//! Returns a JSON object: `{ output, matched, minBytesReached, timedOut,
//! processExited?, exitCode? }`.
//!
//! The `output` field is capped at 200 KB with tail bias (most-recent output
//! is the relevant part).  The full log content is tee'd to a snapshot in the
//! session cache (`cache_dir/wait/<ts>-pid<N>.log`).

use std::path::PathBuf;

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::cap_and_tee::{TruncationBias, cap_and_tee};
use crate::output_cleaner::clean_output;
use crate::state::processes;
use crate::tool_ctx::ToolCtx;

const POLL_INTERVAL_MS: u64 = 200;

/// LLM-facing cap on the `output` field.
const OUTPUT_CAP: usize = 200_000;

pub async fn execute(
    input: Value,
    cancel: Option<&CancellationToken>,
    ctx: Option<&ToolCtx>,
) -> Result<String, String> {
    let log_file = input["logFile"]
        .as_str()
        .ok_or("wait_for_output: logFile is required")?
        .to_owned();
    let pid = {
        let raw = input["pid"]
            .as_u64()
            .ok_or("wait_for_output: pid is required")?;
        u32::try_from(raw).map_err(|_| format!("wait_for_output: pid {raw} out of range"))?
    };
    let timeout_ms = input["timeoutMs"]
        .as_u64()
        .ok_or("wait_for_output: timeoutMs is required")?;
    let pattern_str = input["pattern"].as_str().map(str::to_owned);
    let min_bytes = input["minBytes"]
        .as_u64()
        .map(|n| usize::try_from(n).unwrap_or(usize::MAX));

    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);

    let pattern_re: Option<regex::Regex> = pattern_str.as_deref().map(|p| {
        regex::Regex::new(p).unwrap_or_else(|_| {
            // SAFETY: regex::escape always produces a valid pattern.
            #[allow(clippy::expect_used)]
            regex::Regex::new(&regex::escape(p)).expect("escaped regex is always valid")
        })
    });

    let has_pattern = pattern_re.is_some();
    let has_min_bytes = min_bytes.is_some();

    let effective_min_bytes: Option<usize> = if has_min_bytes {
        min_bytes
    } else if !has_pattern {
        Some(1)
    } else {
        None
    };

    loop {
        if cancel.is_some_and(CancellationToken::is_cancelled) {
            let output = read_log(&log_file).await;
            return done(output, false, false, false, None, pid, ctx).await;
        }

        let content = read_log(&log_file).await;
        let (matched, min_bytes_reached) =
            evaluate(&content, pattern_re.as_ref(), effective_min_bytes);

        if matched {
            return done(content, true, false, false, None, pid, ctx).await;
        }

        if min_bytes_reached {
            return done(content, false, true, false, None, pid, ctx).await;
        }

        if let Some(exit_code) = check_exit(pid).await {
            let final_content = read_log(&log_file).await;
            let (matched, min_bytes_reached) =
                evaluate(&final_content, pattern_re.as_ref(), effective_min_bytes);
            return done(
                final_content,
                matched,
                min_bytes_reached,
                false,
                Some(exit_code),
                pid,
                ctx,
            )
            .await;
        }

        let now = std::time::Instant::now();
        if now >= deadline {
            return done(content, false, false, true, None, pid, ctx).await;
        }

        let remaining = deadline.saturating_duration_since(now);
        let sleep_dur = std::time::Duration::from_millis(POLL_INTERVAL_MS).min(remaining);

        if let Some(ct) = cancel {
            tokio::select! {
                    () = tokio::time::sleep(sleep_dur) => {}
                    () = ct.cancelled() => {}
            }
        } else {
            tokio::time::sleep(sleep_dur).await;
        }
    }
}

/// Build the tee-log path for a `wait_for_output` snapshot.
///
/// With a session context: `<ctx.cache_dir>/wait/<ts-ms>-<tool_call_id>-pid<N>.log`.
/// Without context (test fallback): a per-process temp directory.
fn make_wait_log_path(ctx: Option<&ToolCtx>, pid: u32) -> PathBuf {
    let now = chrono::Utc::now();
    let ts = now.format("%Y-%m-%dT%H-%M-%S");
    let ms = now.timestamp_subsec_millis();

    if let Some(c) = ctx {
        let filename = format!("{ts}-{ms:03}-{}-pid{pid}.log", c.tool_call_id);
        c.cache_dir.join("wait").join(filename)
    } else {
        let filename = format!("{ts}-{ms:03}-pid{pid}.log");
        std::env::temp_dir()
            .join(format!("omega-wait-{}", std::process::id()))
            .join(filename)
    }
}

/// Cap the output string, tee it to a snapshot file, and build the JSON result.
#[allow(clippy::too_many_arguments)]
async fn done(
    output: String,
    matched: bool,
    min_bytes_reached: bool,
    timed_out: bool,
    exit_code: Option<i32>,
    pid: u32,
    ctx: Option<&ToolCtx>,
) -> Result<String, String> {
    let log_path = make_wait_log_path(ctx, pid);

    let cleaned = clean_output(output.as_bytes());
    let capped = cap_and_tee(&cleaned, OUTPUT_CAP, TruncationBias::Tail, &log_path)
        .await
        .map_err(|e| format!("wait_for_output: failed to write snapshot: {e}"))?;

    let mut v = serde_json::json!({
        "output":          capped.body,
        "matched":         matched,
        "minBytesReached": min_bytes_reached,
        "timedOut":        timed_out,
    });
    if let Some(code) = exit_code {
        v["processExited"] = true.into();
        v["exitCode"] = code.into();
    }
    Ok(v.to_string())
}

async fn read_log(path: &str) -> String {
    tokio::fs::read_to_string(path).await.unwrap_or_default()
}

async fn check_exit(pid: u32) -> Option<i32> {
    let mut procs = processes().lock().await;
    let entry = procs.get_mut(&pid)?;
    match entry.child.try_wait() {
        Ok(Some(status)) => Some(status.code().unwrap_or(-1)),
        _ => None,
    }
}

/// Evaluate the two completion conditions against a snapshot of the log.
///
/// Hoisted out of [`execute`] so the same expression is shared by the
/// in-loop check and the post-exit recomputation. With both call sites
/// going through this single function, the `len() >= min` predicate is
/// covered by one direct unit test instead of needing two integration
/// tests that race against process exit.
fn evaluate(
    content: &str,
    pattern: Option<&regex::Regex>,
    min_bytes: Option<usize>,
) -> (bool, bool) {
    let matched = pattern.is_some_and(|re| re.is_match(content));
    let min_bytes_reached = min_bytes.is_some_and(|min| content.len() >= min);
    (matched, min_bytes_reached)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::evaluate;

    #[test]
    fn evaluate_no_pattern_no_min_bytes_returns_false_false() {
        assert_eq!(evaluate("anything", None, None), (false, false));
    }

    #[test]
    fn evaluate_pattern_matches_when_present() {
        let re = regex::Regex::new("READY").unwrap();
        assert_eq!(evaluate("server READY now", Some(&re), None), (true, false));
    }

    #[test]
    fn evaluate_pattern_does_not_match_when_absent() {
        let re = regex::Regex::new("READY").unwrap();
        assert_eq!(
            evaluate("server starting up", Some(&re), None),
            (false, false)
        );
    }

    #[test]
    fn evaluate_min_bytes_fires_at_exact_threshold() {
        // Pins the `len() >= min` boundary: 5 bytes vs. min=5 must fire.
        // Kills the `>= → <` mutation directly at the helper.
        assert_eq!(evaluate("abcde", None, Some(5)), (false, true));
    }

    #[test]
    fn evaluate_min_bytes_fires_when_above_threshold() {
        assert_eq!(evaluate("abcdefghij", None, Some(5)), (false, true));
    }

    #[test]
    fn evaluate_min_bytes_does_not_fire_below_threshold() {
        // Pins the strict-less branch: 4 bytes vs. min=5 must NOT fire.
        // Kills the `>= → >` mutation (4 > 5 is false, same as prod —
        // but 5 > 5 differs, which is covered by the previous test).
        assert_eq!(evaluate("abcd", None, Some(5)), (false, false));
    }

    #[test]
    fn evaluate_both_conditions_can_be_true_simultaneously() {
        let re = regex::Regex::new("ok").unwrap();
        assert_eq!(evaluate("all ok now", Some(&re), Some(3)), (true, true));
    }
}
