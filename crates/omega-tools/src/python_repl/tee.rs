//! Tee-log side channel: every byte the Python kernel writes to stdout is
//! also appended to a per-call log file, untouched by the LLM-facing
//! truncation in [`super::output`].
//!
//! The path is **never** surfaced in the LLM-visible result — variables are
//! the natural memory model in a stateful REPL.  The log is for human or
//! machine-assisted post-mortem only.
//!
//! Layout:
//!
//! - With a [`ToolCtx`]: `<ctx.cache_dir>/python_repl/<ts-ms>-<call_id>.log`.
//! - Without context (unit tests): `$TMPDIR/omega-repl-<pid>/<ts-ms>.log`.

use std::path::PathBuf;

use tokio::io::AsyncWriteExt as _;

use crate::tool_ctx::ToolCtx;

/// Build the tee-log path for a `python_repl` call.
///
/// With context: `<ctx.cache_dir>/python_repl/<ts-ms>-<call_id>.log`.
/// Without context (tests): `$TMPDIR/omega-repl-<pid>/<ts-ms>.log`.
pub(super) fn make_repl_log_path(ctx: Option<&ToolCtx>) -> PathBuf {
    let now = chrono::Utc::now();
    let ts = now.format("%Y-%m-%dT%H-%M-%S");
    let ms = now.timestamp_subsec_millis();

    if let Some(c) = ctx {
        let filename = format!("{ts}-{ms:03}-{}.log", c.tool_call_id);
        c.cache_dir.join("python_repl").join(filename)
    } else {
        let filename = format!("{ts}-{ms:03}.log");
        std::env::temp_dir()
            .join(format!("omega-repl-{}", std::process::id()))
            .join(filename)
    }
}

/// Open (or create) the tee-log file for writing.  Parent directories are
/// created as needed.  Returns `None` on any I/O error.
pub(super) async fn open_log_writer(
    path: &PathBuf,
) -> Option<tokio::io::BufWriter<tokio::fs::File>> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.ok()?;
    }
    let file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
        .ok()?;
    Some(tokio::io::BufWriter::new(file))
}

/// Append `line` to the tee log, silently ignoring errors.
pub(super) async fn tee_line(
    writer: &mut Option<tokio::io::BufWriter<tokio::fs::File>>,
    line: &str,
) {
    if let Some(w) = writer {
        let _ = w.write_all(line.as_bytes()).await;
    }
}

/// Flush the tee log, silently ignoring errors.
pub(super) async fn flush_log(writer: &mut Option<tokio::io::BufWriter<tokio::fs::File>>) {
    if let Some(w) = writer {
        let _ = w.flush().await;
    }
}
