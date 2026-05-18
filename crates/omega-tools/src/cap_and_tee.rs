//! `cap_and_tee` — tee all bytes to a log file and return a capped view
//!
//! Every result carries a footer pointing at the on-disk log so the LLM
//! can `read_file` / `grep_files` the cache for follow-up queries instead
//! of re-running the command:
//!   - non-truncated: `\n[full output: <path>]`
//!   - truncated:     `\n[truncated; showed first 100 KB of 487 KB. Full output: <path>]`
//!
//! The only exception is empty data: we still write the (empty) file, but
//! return an empty body — pointing the LLM at zero bytes is just noise.
//! for the LLM.
//!
//! Every tool that can produce large output calls this helper so that:
//! * the full output is always preserved on disk (in the session cache),
//! * the LLM only receives a bounded window (default 100 KB), and
//! * the window's footer tells the LLM exactly where to find the rest.
//!
//! ## Footer format
//! When truncation fires:
//! ```text
//! [truncated; showed last 100 KB of 487 KB. Full output: .omega/sessions/…/cache/run/….log]
//! ```
//! The word *last*/*first*/*first and last halves* reflects the
//! [`TruncationBias`] so the LLM knows which direction to look.

use std::io;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Controls which portion of a truncated output is returned to the LLM.
///
/// The default bias for each tool:
/// * `run_command` — [`Tail`](TruncationBias::Tail) on non-zero exit (errors
///   are usually at the end), [`Head`](TruncationBias::Head) on exit 0.
/// * `wait_for_output` — [`Tail`](TruncationBias::Tail) (most-recent output
///   is the interesting part).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TruncationBias {
    /// Return the first `cap` bytes.
    Head,
    /// Return the last `cap` bytes.
    Tail,
    /// Return the first `cap/2` bytes and the last `cap/2` bytes with a
    /// gap marker between them.
    Middle,
}

impl TruncationBias {
    /// Parse the LLM-facing string (`"head"`, `"tail"`, `"middle"`).
    /// Unknown values fall back to [`Head`](TruncationBias::Head).
    #[must_use]
    pub fn parse_bias(s: &str) -> Self {
        match s {
            "tail" => TruncationBias::Tail,
            "middle" => TruncationBias::Middle,
            _ => TruncationBias::Head,
        }
    }
}

/// Returned by [`cap_and_tee`].
pub struct CappedOutput {
    /// The portion of the data to return to the LLM, including the footer
    /// when truncation fired.
    pub body: String,
    /// `true` when the data exceeded `cap`.
    pub truncated: bool,
    /// Total bytes written to `log_path` (== `data.len()`).
    pub total_bytes: usize,
    /// Absolute path to the full log.
    pub log_path: PathBuf,
}

// ---------------------------------------------------------------------------
// Core function
// ---------------------------------------------------------------------------

/// Write `data` to `log_path`, then return a capped view for the LLM.
///
/// The parent directory of `log_path` is created if it does not exist.
/// All bytes are always written; only the LLM-facing `body` is capped.
///
/// # Errors
/// Returns `io::Error` if the directory cannot be created or the file
/// cannot be written.
pub async fn cap_and_tee(
    data: &[u8],
    cap: usize,
    bias: TruncationBias,
    log_path: &Path,
) -> io::Result<CappedOutput> {
    // Ensure parent directory exists.
    if let Some(parent) = log_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(log_path, data).await?;

    let total_bytes = data.len();
    let truncated = total_bytes > cap;

    let body = if truncated {
        let log_display = log_path.display();
        let shown = format_size(cap);
        let total = format_size(total_bytes);

        match bias {
            TruncationBias::Head => {
                let end = utf8_boundary_forward(data, cap);
                let window = String::from_utf8_lossy(&data[..end]);
                format!(
                    "{window}\n[truncated; showed first {shown} of {total}. \
                     Full output: {log_display}]"
                )
            }
            TruncationBias::Tail => {
                let start = total_bytes - utf8_boundary_backward(data, cap);
                let window = String::from_utf8_lossy(&data[start..]);
                format!(
                    "{window}\n[truncated; showed last {shown} of {total}. \
                     Full output: {log_display}]"
                )
            }
            TruncationBias::Middle => {
                let half = cap / 2;
                let head_end = utf8_boundary_forward(data, half);
                let tail_len = utf8_boundary_backward(data, half);
                let tail_start = total_bytes - tail_len;
                let omitted = tail_start.saturating_sub(head_end);
                let head = String::from_utf8_lossy(&data[..head_end]);
                let tail = String::from_utf8_lossy(&data[tail_start..]);
                let shown_half = format_size(half);
                format!(
                    "{head}\n[... {omitted} bytes omitted ...]\n{tail}\n\
                     [truncated; showed first and last halves ({shown_half} each) of {total}. \
                     Full output: {log_display}]"
                )
            }
        }
    } else if data.is_empty() {
        String::new()
    } else {
        let log_display = log_path.display();
        let body = String::from_utf8_lossy(data);
        format!("{body}\n[full output: {log_display}]")
    };

    Ok(CappedOutput {
        body,
        truncated,
        total_bytes,
        log_path: log_path.to_owned(),
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Format a byte count as a human-readable SI size string.
///
/// Uses 1 000-byte kilobytes so that `100_000` displays as "100 KB" — matching
/// the doc examples — and `1_000_000` as "1.0 MB".
fn format_size(bytes: usize) -> String {
    #[allow(clippy::cast_precision_loss)]
    if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1_000_000.0)
    } else if bytes >= 1_000 {
        format!("{} KB", bytes / 1_000)
    } else {
        format!("{bytes} B")
    }
}

/// Return the largest byte index ≤ `max` that is a valid UTF-8 code-point
/// boundary in `data`.  Equivalent to snapping a head window to the last
/// complete character.
///
/// Uses `str::from_utf8` to find the exact boundary: if `data[..end]` is
/// already valid UTF-8 the full length is returned; otherwise
/// `Utf8Error::valid_up_to` gives the precise cut point.
fn utf8_boundary_forward(data: &[u8], max: usize) -> usize {
    let end = max.min(data.len());
    match std::str::from_utf8(&data[..end]) {
        Ok(_) => end,
        Err(e) => e.valid_up_to(),
    }
}

/// Return the number of bytes to take from the *end* of `data` such that the
/// window is at most `max` bytes and starts on a valid UTF-8 boundary.
/// The start index is `data.len() - result`.
fn utf8_boundary_backward(data: &[u8], max: usize) -> usize {
    let len = data.len();
    let raw_start = len.saturating_sub(max);
    // Advance past continuation bytes to find the next valid start.
    let mut start = raw_start;
    while start < len && is_utf8_continuation(data[start]) {
        start += 1;
    }
    len - start
}

/// True for UTF-8 continuation bytes (0x80..=0xBF).
#[inline]
fn is_utf8_continuation(b: u8) -> bool {
    (b & 0xC0) == 0x80
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------
// Tests — pure formatting helpers only; I/O scenarios live in process_tools.rs
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // format_size
    // -----------------------------------------------------------------------

    #[test]
    fn format_size_bytes() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(999), "999 B");
    }

    #[test]
    fn format_size_kilobytes() {
        assert_eq!(format_size(1_000), "1 KB");
        assert_eq!(format_size(100_000), "100 KB");
        assert_eq!(format_size(487_000), "487 KB");
        assert_eq!(format_size(999_999), "999 KB");
    }

    #[test]
    fn format_size_megabytes() {
        assert_eq!(format_size(1_000_000), "1.0 MB");
        assert_eq!(format_size(1_100_000), "1.1 MB");
    }

    // -----------------------------------------------------------------------
    // TruncationBias::parse_bias
    // -----------------------------------------------------------------------

    #[test]
    fn from_str_parses_known_values() {
        assert_eq!(TruncationBias::parse_bias("head"), TruncationBias::Head);
        assert_eq!(TruncationBias::parse_bias("tail"), TruncationBias::Tail);
        assert_eq!(TruncationBias::parse_bias("middle"), TruncationBias::Middle);
    }

    #[test]
    fn from_str_unknown_falls_back_to_head() {
        assert_eq!(TruncationBias::parse_bias("unknown"), TruncationBias::Head);
        assert_eq!(TruncationBias::parse_bias(""), TruncationBias::Head);
    }
}
