//! `write_stdin` — write text to the stdin of a tracked background process.

use serde_json::Value;
use tokio::io::AsyncWriteExt as _;
use tokio_util::sync::CancellationToken;

use crate::state::processes;

pub async fn execute(
    input: Value,
    _cancel: Option<&CancellationToken>,
) -> Result<String, String> {
    let raw_pid = input["pid"]
        .as_u64()
        .ok_or("write_stdin: pid is required")?;
    let pid = u32::try_from(raw_pid)
        .map_err(|_| format!("write_stdin: pid {raw_pid} out of range"))?;
    let text = input["text"]
        .as_str()
        .ok_or("write_stdin: text is required")?;
    let end_stdin = input["end_stdin"].as_bool().unwrap_or(false);

    let char_count = text.chars().count();
    let text_bytes = text.as_bytes().to_vec();

    let mut procs = processes().lock().await;
    let entry = procs.get_mut(&pid).ok_or_else(|| {
        format!(
            "No tracked process with pid {pid}. \
             Only processes started with run_background can receive stdin."
        )
    })?;

    if entry.stdin_closed {
        return Err(format!("stdin for pid {pid} is already closed."));
    }

    {
        let stdin = entry.stdin.as_mut().ok_or_else(|| {
            format!("Process {pid} has no writable stdin handle.")
        })?;
        stdin
            .write_all(&text_bytes)
            .await
            .map_err(|e| format!("write_stdin: write failed for pid {pid}: {e}"))?;
    }

    if end_stdin {
        entry.stdin_closed = true;
        let _ = entry.stdin.take();
        return Ok(format!(
            "Wrote {char_count} chars to stdin of pid {pid} and closed stdin (EOF)"
        ));
    }

    Ok(format!("Wrote {char_count} chars to stdin of pid {pid}"))
}
