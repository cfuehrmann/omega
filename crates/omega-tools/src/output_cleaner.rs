//! `output_cleaner` — remove terminal noise from process output before it
//! reaches the LLM.
//!
//! Applied to the combined stdout+stderr buffer of `run_command` and
//! `wait_for_output` **before** the buffer is passed to `cap_and_tee`.
//! The tee log on disk therefore receives the already-cleaned output;
//! the raw bytes are not preserved.  This is intentional: a log full of
//! carriage-return overwrite frames is as useless on disk as it is to the
//! LLM (see `backlog/token-optimizations.md` §2 for the rationale).
//!
//! ## Pipeline
//!
//! ```text
//! raw bytes
//!   │
//!   ▼  Step 1 — CRLF normalise
//!   │  Replace every \r\n with \n.  Without this, lines terminated by the
//!   │  network/POSIX CRLF convention (apt-get, curl --verbose, HTTP) would
//!   │  be erased by step 2.  This step must run first.
//!   │
//!   ▼  Step 2 — CR-collapse
//!   │  For each \n-delimited line, discard everything up to and including
//!   │  the last bare \r.  This collapses progress-bar rewrites
//!   │  (\rRead 1M words\rRead 2M words…) down to their final frame.
//!   │
//!   ▼  Step 3 — ANSI strip
//!      Remove ANSI/VT100 escape sequences: CSI colour codes, cursor moves,
//!      and OSC sequences (hyperlinks, window titles).  The text content is
//!      preserved; only the control bytes are removed.
//! ```
//!
//! ## What is NOT cleaned
//!
//! * `fetch_url` postprocess output — already expected to be structured.
//! * `read_file` / `grep_files` — file content, not live process output.
//! * Binary data — `\r` and `\x1b` inside hex-dump or base64 output are
//!   extremely rare, and if they occur the cleaning is still safe (keeping
//!   the last `\r`-frame of a hex line loses nothing useful).

use std::sync::OnceLock;

/// Run the full three-step cleaning pipeline on raw process output bytes.
///
/// Input and output are byte slices; no UTF-8 validity is assumed or
/// required.  All ANSI escape bytes are ASCII (< 0x80) and therefore cannot
/// coincide with the high bytes of multi-byte UTF-8 sequences.
///
/// Returns the same bytes unmodified if no `\r` or `\x1b` is present
/// (fast path: neither scan produces an allocation).
pub fn clean_output(data: &[u8]) -> Vec<u8> {
    // Fast path: skip all three steps if there is nothing to clean.
    if !data.contains(&b'\r') && !data.contains(&0x1b_u8) {
        return data.to_vec();
    }
    let step1 = crlf_normalize(data);
    let step2 = cr_collapse(&step1);
    ansi_strip(&step2)
}

// ---------------------------------------------------------------------------
// Step 1: CRLF normalisation
// ---------------------------------------------------------------------------

/// Replace every `\r\n` pair with `\n`, leaving lone `\r` bytes intact for
/// step 2.  Must run before [`cr_collapse`].
fn crlf_normalize(data: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        if data[i] == b'\r' && i + 1 < data.len() && data[i + 1] == b'\n' {
            result.push(b'\n');
            i += 2;
        } else {
            result.push(data[i]);
            i += 1;
        }
    }
    result
}

// ---------------------------------------------------------------------------
// Step 2: CR-collapse
// ---------------------------------------------------------------------------

/// For each `\n`-delimited line, keep only the bytes that follow the **last**
/// bare `\r`.  This reduces a stream of carriage-return-overwritten progress
/// frames to the single final frame.
///
/// The output has the same number of `\n` bytes and the same trailing-newline
/// status as the input — only the content between `\n` separators changes.
fn cr_collapse(data: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(data.len());
    let mut first = true;
    for line in data.split(|&b| b == b'\n') {
        // Re-insert the separator before every chunk except the first.
        if !first {
            result.push(b'\n');
        }
        first = false;
        // Keep only the content after the last \r (if any).
        let content = match line.iter().rposition(|&b| b == b'\r') {
            Some(pos) => &line[pos + 1..],
            None => line,
        };
        result.extend_from_slice(content);
    }
    result
}

// ---------------------------------------------------------------------------
// Step 3: ANSI escape stripping
// ---------------------------------------------------------------------------

/// Remove ANSI/VT100 escape sequences from `data`.
///
/// Matches:
/// * CSI sequences — `\x1b[` + parameter bytes + final letter  (colours,
///   cursor movement, erase)
/// * OSC sequences — `\x1b]` + text + `\x07` or `\x1b\\`  (hyperlinks,
///   window titles)
/// * Single-character escapes — `\x1b` + one letter  (e.g. `\x1bM`
///   reverse-index)
fn ansi_strip(data: &[u8]) -> Vec<u8> {
    static RE: OnceLock<regex::bytes::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        // CSI:  \x1b[  param-bytes*  final-byte
        // OSC:  \x1b]  any*  (BEL | ST=\x1b\\)
        // single-char escapes: \x1b + letter  (e.g. \x1bM reverse-index)
        #[allow(clippy::expect_used)]
        regex::bytes::Regex::new(
            r"\x1b(?:\[[0-9;?<=>!]*[A-Za-z@]|\][^\x07\x1b]*(?:\x07|\x1b\\)|[A-Za-z])",
        )
        .expect("ANSI strip regex is valid")
    });
    re.replace_all(data, b"".as_slice()).into_owned()
}
