//! LLM-facing output truncation for the Python REPL.
//!
//! [`truncate_for_llm`] reduces a `&[String]` of collected output lines to
//! a single `String` bounded by [`MAX_OUTPUT_LINES`] lines or
//! [`MAX_OUTPUT_CHARS`] characters, whichever is hit first.  Two bias modes:
//!
//! - **Head** (default, `Completed`): keep the first N lines/chars; the
//!   truncation marker is appended at the end.
//! - **Tail** (`TimedOut*`): keep the last N lines/chars so the most recent
//!   output — the bytes just before a hang — is always visible; the marker
//!   is prepended.
//!
//! The full untruncated output is always written to the tee log
//! ([`super::tee`]); truncation only affects what the LLM sees.

use std::fmt::Write as _;

/// Maximum number of output lines to include before truncating.
///
/// 200 lines is deliberately low for the benchmark prototype.
pub const MAX_OUTPUT_LINES: usize = 200;

/// Maximum number of output characters to include before truncating.
///
/// 2 000 characters is deliberately low for the benchmark prototype.
pub const MAX_OUTPUT_CHARS: usize = 2_000;

/// Truncate `lines` for LLM consumption, respecting the configured limits.
///
/// - `tail_bias = false` (head): show first N lines/chars; suppressed bytes
///   are at the end; truncation marker appended at the end.
/// - `tail_bias = true`  (tail): show last  N lines/chars; suppressed bytes
///   are at the start; truncation marker prepended at the start.
///
/// The truncation marker uses the new wording encouraging the variable pattern.
pub(super) fn truncate_for_llm(lines: &[String], tail_bias: bool) -> String {
    if tail_bias {
        // Tail bias: find the last window that fits within limits.
        let mut included_chars: usize = 0;
        let mut tail_start = lines.len();

        for (i, line) in lines.iter().enumerate().rev().take(MAX_OUTPUT_LINES) {
            if included_chars + line.len() > MAX_OUTPUT_CHARS {
                break;
            }
            included_chars += line.len();
            tail_start = i;
        }

        let suppressed_count = tail_start;
        let suppressed_chars: usize = lines[..tail_start].iter().map(String::len).sum();

        let mut output = String::new();

        if suppressed_count > 0 {
            let _ = writeln!(
                output,
                "... [output truncated: {suppressed_count} lines / {suppressed_chars} chars suppressed. \
                 Cap: {MAX_OUTPUT_LINES} lines / {MAX_OUTPUT_CHARS} chars. \
                 Capture large values in variables and inspect/slice them \
                 in subsequent calls rather than printing them whole.]"
            );
        }

        for line in &lines[tail_start..] {
            output.push_str(line);
        }
        output
    } else {
        // Head bias: current behaviour.
        let mut output = String::new();
        let mut line_count: usize = 0;
        let mut suppressed_lines: usize = 0;
        let mut suppressed_chars: usize = 0;

        for line in lines {
            if line_count >= MAX_OUTPUT_LINES || output.len() >= MAX_OUTPUT_CHARS {
                suppressed_lines += 1;
                suppressed_chars += line.len();
            } else {
                output.push_str(line);
                line_count += 1;
            }
        }

        if suppressed_lines > 0 {
            let _ = write!(
                output,
                "\n... [output truncated: {suppressed_lines} lines / {suppressed_chars} chars suppressed. \
                 Cap: {MAX_OUTPUT_LINES} lines / {MAX_OUTPUT_CHARS} chars. \
                 Capture large values in variables and inspect/slice them \
                 in subsequent calls rather than printing them whole.]"
            );
        }
        output
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::expect_used, // test assertions
    clippy::unwrap_used, // test assertions
    clippy::panic,       // test setup helpers use panic for clarity
)]
mod tests {
    use super::*;

    // These tests are pure unit tests on `truncate_for_llm` — no subprocess,
    // no I/O.  They pin the boundary conditions that the mutation sweep
    // probes (head/tail bias, char accumulation, marker suppression).

    /// When output is truncated due to a timeout (tail bias), the suppression
    /// marker appears at the start (before the recent output), not at the end.
    #[test]
    fn tail_bias_marker_appears_before_recent_output() {
        // Simulate lines: 210 lines where "recent_line_209" is the last.
        let lines: Vec<String> = (0..=209).map(|i| format!("line_{i}\n")).collect();
        let out = truncate_for_llm(&lines, true);
        // Marker must be present (210 lines > MAX_OUTPUT_LINES=200).
        assert!(
            out.contains("output truncated"),
            "marker missing: {out:.200}"
        );
        // Marker must come BEFORE the tail content.
        let marker_pos = out.find("output truncated").unwrap();
        let recent_pos = out
            .find("line_209")
            .unwrap_or_else(|| panic!("recent line missing: {out:.200}"));
        assert!(
            marker_pos < recent_pos,
            "tail-bias marker must precede recent output (marker={marker_pos} recent={recent_pos})"
        );
    }

    /// Head-bias truncation marker appears at the end (after the early output).
    #[test]
    fn head_bias_marker_appears_after_early_output() {
        let lines: Vec<String> = (0..=209).map(|i| format!("line_{i}\n")).collect();
        let out = truncate_for_llm(&lines, false);
        assert!(out.contains("output truncated"), "marker missing");
        let marker_pos = out.find("output truncated").unwrap();
        let early_pos = out
            .find("line_0")
            .unwrap_or_else(|| panic!("early line missing"));
        assert!(
            early_pos < marker_pos,
            "head-bias early output must precede marker (early={early_pos} marker={marker_pos})"
        );
    }

    /// A line whose length equals exactly `MAX_OUTPUT_CHARS` must be included,
    /// not excluded, in the tail window.
    ///
    /// Catches mutations:
    ///   `included_chars + line.len() > MAX_OUTPUT_CHARS`
    ///     → `== MAX_OUTPUT_CHARS` (would exclude the line)
    ///     → `>= MAX_OUTPUT_CHARS` (would exclude the line)
    #[test]
    fn tail_bias_line_exactly_at_char_limit_is_included() {
        // One suppressed line + one line whose length == MAX_OUTPUT_CHARS.
        // Correct: include the last line (0 + MAX_OUTPUT_CHARS > MAX_OUTPUT_CHARS is false).
        // `==` mutation: 0 + MAX_OUTPUT_CHARS == MAX_OUTPUT_CHARS → break (wrong, excludes it).
        // `>=` mutation: 0 + MAX_OUTPUT_CHARS >= MAX_OUTPUT_CHARS → break (wrong, excludes it).
        // NOTE: check for a long run of 'a's (not just a single 'a'), because the
        // truncation-marker prose ("Capture large values...") also contains 'a'.
        //
        // This test also validates the `.take(MAX_OUTPUT_LINES)` path that
        // replaced the explicit `included_lines` counter.
        let exact_line = "a".repeat(MAX_OUTPUT_CHARS - 1) + "\n"; // len = MAX_OUTPUT_CHARS
        assert_eq!(exact_line.len(), MAX_OUTPUT_CHARS);
        let lines = vec!["suppressed\n".to_owned(), exact_line];
        let out = truncate_for_llm(&lines, true);
        assert!(
            out.contains(&"a".repeat(50)),
            "line at exactly MAX_OUTPUT_CHARS bytes must be included in tail window: {out:.100}"
        );
    }

    /// Multiple lines whose combined length stays within `MAX_OUTPUT_CHARS` must
    /// ALL be included in the tail window, even after the first is accumulated.
    ///
    /// Catches mutation:
    ///   `included_chars + line.len()` → `included_chars * line.len()`
    /// (with `*`, after the first line sets `included_chars=101`, the second
    /// check becomes 101*101=10201 > 2000, so the second line is wrongly excluded).
    #[test]
    fn tail_bias_char_accumulation_is_additive_not_multiplicative() {
        // Three lines: first suppressed, last two each 101 chars.
        // 101 + 101 = 202 << MAX_OUTPUT_CHARS → both must be included.
        // With `*` mutation: after including last line (included=101),
        //   check second-to-last: 101 * 101 = 10 201 > 2 000 → would exclude it.
        // NOTE: use long runs (50+ chars) to distinguish content from the
        // truncation-marker prose (which also contains single 'a' and 'b' chars).
        let medium_line_a = "a".repeat(100) + "\n"; // 101 chars
        let medium_line_b = "b".repeat(100) + "\n"; // 101 chars
        let lines = vec!["suppressed\n".to_owned(), medium_line_a, medium_line_b];
        let out = truncate_for_llm(&lines, true);
        assert!(
            out.contains(&"a".repeat(50)) && out.contains(&"b".repeat(50)),
            "both medium lines must be included (char accumulation must be additive): {out:.200}"
        );
        assert!(
            out.contains("suppressed") || out.contains("output truncated"),
            "first line must be suppressed: {out:.200}"
        );
    }

    /// Char accumulation across many lines must enforce the char limit.
    ///
    /// Catches mutation:
    ///   `included_chars += line.len()` → `included_chars *= line.len()`
    /// (with `*=`, `included_chars` stays 0 forever since 0 * anything = 0,
    /// so ALL lines would pass the char check and none would be suppressed).
    #[test]
    fn tail_bias_accumulated_chars_triggers_truncation() {
        // 12 lines of 201 chars each: total 2 412 > MAX_OUTPUT_CHARS=2 000.
        // With correct `+=`: included_chars grows; ~9 lines fit before limit fires.
        // With `*=` mutation: included_chars stays 0; all 12 lines fit → no suppression.
        let long_line = "z".repeat(200) + "\n"; // 201 chars
        let lines: Vec<String> = (0..12).map(|_| long_line.clone()).collect();
        let out = truncate_for_llm(&lines, true);
        assert!(
            out.contains("output truncated"),
            "12 lines of 201 chars must trigger char-limit truncation: {out:.200}"
        );
    }

    /// When nothing is suppressed in tail bias, no truncation marker must appear.
    ///
    /// Catches mutation:
    ///   `if suppressed_count > 0` → `if suppressed_count >= 0`
    /// (`>= 0` is always true for usize, so the marker would appear even
    /// when `suppressed_count` == 0, i.e. nothing was actually suppressed).
    #[test]
    fn tail_bias_no_marker_when_nothing_suppressed() {
        let lines = vec!["line1\n".to_owned(), "line2\n".to_owned()];
        let out = truncate_for_llm(&lines, true);
        assert!(
            !out.contains("output truncated"),
            "no truncation marker expected when output fits within limits: {out:?}"
        );
    }
    /// The truncation footer for head-bias mode includes the cap values
    /// interpolated from the constants (not hard-coded literals).
    ///
    /// This test will break if the constants change without the marker text
    /// changing — by design.
    #[test]
    fn head_bias_marker_contains_cap_values() {
        let lines: Vec<String> = (0..=209).map(|i| format!("line_{i}\n")).collect();
        let out = truncate_for_llm(&lines, false);
        let expected_lines = MAX_OUTPUT_LINES.to_string();
        let expected_chars = MAX_OUTPUT_CHARS.to_string();
        assert!(
            out.contains(&expected_lines),
            "head-bias marker must contain MAX_OUTPUT_LINES ({expected_lines}): {out:.300}"
        );
        assert!(
            out.contains(&expected_chars),
            "head-bias marker must contain MAX_OUTPUT_CHARS ({expected_chars}): {out:.300}"
        );
    }

    /// The truncation footer for tail-bias mode includes the cap values
    /// interpolated from the constants (not hard-coded literals).
    #[test]
    fn tail_bias_marker_contains_cap_values() {
        let lines: Vec<String> = (0..=209).map(|i| format!("line_{i}\n")).collect();
        let out = truncate_for_llm(&lines, true);
        let expected_lines = MAX_OUTPUT_LINES.to_string();
        let expected_chars = MAX_OUTPUT_CHARS.to_string();
        assert!(
            out.contains(&expected_lines),
            "tail-bias marker must contain MAX_OUTPUT_LINES ({expected_lines}): {out:.300}"
        );
        assert!(
            out.contains(&expected_chars),
            "tail-bias marker must contain MAX_OUTPUT_CHARS ({expected_chars}): {out:.300}"
        );
    }
}
