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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn clean(s: &str) -> String {
        String::from_utf8(clean_output(s.as_bytes())).unwrap()
    }

    // --- fast path ---

    #[test]
    fn no_cr_no_ansi_returns_identical_bytes() {
        let input = b"hello\nworld\n";
        let out = clean_output(input);
        assert_eq!(out, input);
    }

    // --- CRLF normalisation ---

    #[test]
    fn crlf_converted_to_lf() {
        assert_eq!(clean("foo\r\nbar\r\n"), "foo\nbar\n");
    }

    #[test]
    fn mixed_crlf_and_lf_both_normalised() {
        assert_eq!(clean("a\r\nb\nc\r\n"), "a\nb\nc\n");
    }

    // --- apt-get pattern: CRLF real lines mixed with bare-\r progress ---

    #[test]
    fn apt_get_pattern_preserves_package_names() {
        // Real apt-get output structure: package name on a \r\n line,
        // followed by "(Reading database ... \r...5%\r...100%\n".
        let input = "Selecting previously unselected package stockfish.\r\n\
                     (Reading database ... \r\
                     (Reading database ... 50%\r\
                     (Reading database ... 100%\n\
                     Setting up stockfish.\r\n";
        let out = clean(input);
        assert!(
            out.contains("Selecting previously unselected package stockfish."),
            "package name was erased: {out:?}"
        );
        assert!(
            out.contains("Setting up stockfish."),
            "setup line was erased: {out:?}"
        );
        assert!(out.contains("100%"), "last progress frame missing: {out:?}");
        assert!(
            !out.contains("50%"),
            "intermediate progress frame should be gone: {out:?}"
        );
    }

    // --- CR-collapse ---

    #[test]
    fn progress_bar_collapses_to_last_frame() {
        let input = "\rRead 1M words\rRead 2M words\rRead 100M words\n";
        assert_eq!(clean(input), "Read 100M words\n");
    }

    #[test]
    fn multiple_lines_with_cr_each_collapsed_independently() {
        let input = "step1\rSTEP1\nstep2\rSTEP2\n";
        assert_eq!(clean(input), "STEP1\nSTEP2\n");
    }

    #[test]
    fn line_without_cr_untouched() {
        let input = "normal line\nanother\n";
        assert_eq!(clean(input), "normal line\nanother\n");
    }

    #[test]
    fn trailing_newline_preserved() {
        assert!(clean("foo\rbar\n").ends_with('\n'));
    }

    #[test]
    fn no_trailing_newline_preserved() {
        let out = clean("foo\rbar");
        assert!(!out.ends_with('\n'));
        assert_eq!(out, "bar");
    }

    #[test]
    fn cr_at_start_of_line_keeps_rest_of_line() {
        // "\rSomething" on its own line: split("") = ["", "Something"],
        // last part after split is "Something".
        let input = "before\n\rSomething\nafter\n";
        assert_eq!(clean(input), "before\nSomething\nafter\n");
    }

    // --- ANSI stripping ---

    #[test]
    fn sgr_colour_codes_stripped() {
        // \x1b[32m = green, \x1b[0m = reset
        let input = "\x1b[32mok\x1b[0m\n";
        assert_eq!(clean(input), "ok\n");
    }

    #[test]
    fn cursor_movement_stripped() {
        // \x1b[1A = cursor up 1, \x1b[K = erase to end of line
        let input = "line1\x1b[1A\x1b[Kline2\n";
        assert_eq!(clean(input), "line1line2\n");
    }

    #[test]
    fn osc_hyperlink_stripped() {
        // OSC 8 hyperlink: \x1b]8;;url\x1b\\ text \x1b]8;;\x1b\\
        let input = "\x1b]8;;https://example.com\x1b\\click here\x1b]8;;\x1b\\\n";
        assert_eq!(clean(input), "click here\n");
    }

    #[test]
    fn osc_bel_terminated_stripped() {
        let input = "\x1b]0;window title\x07some output\n";
        assert_eq!(clean(input), "some output\n");
    }

    #[test]
    fn single_char_escape_stripped() {
        // \x1bM = reverse-index (scroll up)
        let input = "line\x1bMmore\n";
        assert_eq!(clean(input), "linemore\n");
    }

    // --- combined ---

    #[test]
    fn ffmpeg_style_cr_with_ansi() {
        // ffmpeg writes frame stats with \r and sometimes with ANSI colours.
        let input = "\x1b[33mframe=\x1b[0m    0 fps=0.0\r\
                     \x1b[33mframe=\x1b[0m  100 fps=24.0\n";
        let out = clean(input);
        assert_eq!(out, "frame=  100 fps=24.0\n");
    }

    #[test]
    fn tqdm_progress_collapses_to_final_frame() {
        let input = "Training: \r  0%|          | 0/100\rTraining: \r 50%|█████     | 50/100\rTraining: \r100%|██████████| 100/100\n";
        let out = clean(input);
        assert_eq!(out, "100%|██████████| 100/100\n");
    }

    // --- curl --verbose: bare \r in progress, \n in headers ---

    #[test]
    fn curl_verbose_headers_preserved() {
        // curl --verbose mixes \r progress stats with \n-terminated header lines.
        let input = "  % Total    % Received\n\
                     \r  0     0\r100   256\n\
                     * Connected to example.com\n\
                     > GET / HTTP/1.1\n";
        let out = clean(input);
        assert!(out.contains("* Connected to example.com"), "{out:?}");
        assert!(out.contains("> GET / HTTP/1.1"), "{out:?}");
        assert!(out.contains("100   256"), "{out:?}");
        assert!(!out.contains("\r  0     0"), "{out:?}");
    }

    // --- empty input ---

    #[test]
    fn empty_input_returns_empty() {
        assert_eq!(clean_output(b""), b"");
    }
}
