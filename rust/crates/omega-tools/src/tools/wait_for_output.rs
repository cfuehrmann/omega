//! `wait_for_output` — poll a background-process log file until a condition is
//! met.
//!
//! Returns a JSON object: `{ output, matched, minBytesReached, timedOut,
//! processExited?, exitCode? }`.

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::state::processes;

const POLL_INTERVAL_MS: u64 = 200;

pub async fn execute(input: Value, cancel: Option<&CancellationToken>) -> Result<String, String> {
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
            return Ok(done(output, false, false, false, None));
        }

        let content = read_log(&log_file).await;

        if pattern_re.as_ref().is_some_and(|re| re.is_match(&content)) {
            return Ok(done(content, true, false, false, None));
        }

        if effective_min_bytes.is_some_and(|min| content.len() >= min) {
            return Ok(done(content, false, true, false, None));
        }

        if let Some(exit_code) = check_exit(pid).await {
            let final_content = read_log(&log_file).await;
            let matched = pattern_re
                .as_ref()
                .is_some_and(|re| re.is_match(&final_content));
            let min_bytes_reached =
                effective_min_bytes.is_some_and(|min| final_content.len() >= min);
            return Ok(serde_json::json!({
                "output":          final_content,
                "matched":         matched,
                "minBytesReached": min_bytes_reached,
                "timedOut":        false,
                "processExited":   true,
                "exitCode":        exit_code,
            })
            .to_string());
        }

        let now = std::time::Instant::now();
        if now >= deadline {
            return Ok(done(content, false, false, true, None));
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

#[allow(clippy::needless_pass_by_value)]
fn done(
    output: String,
    matched: bool,
    min_bytes_reached: bool,
    timed_out: bool,
    exit_code: Option<i32>,
) -> String {
    let mut v = serde_json::json!({
        "output":          output,
        "matched":         matched,
        "minBytesReached": min_bytes_reached,
        "timedOut":        timed_out,
    });
    if let Some(code) = exit_code {
        v["processExited"] = true.into();
        v["exitCode"] = code.into();
    }
    v.to_string()
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
