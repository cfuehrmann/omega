//! Exact-match text replacement for the single- and multi-edit tools.
//!
//! We deliberately do **not** fuzzy-match.  An earlier version ported
//! opencode's nine-strategy cascade (line-trimming, block anchors, Levenshtein
//! similarity, …); analysis of real session logs showed that the cases such a
//! cascade would silently "rescue" are dominated by genuine content mistakes —
//! the model wrote the wrong number, dropped a word, used a glyph where the
//! source has an escape — which a loose matcher would happily edit at the wrong
//! place.  Whitespace can also be load-bearing (Python, YAML, Makefiles,
//! aligned tables, string literals), so silently forgiving it is unsafe.
//!
//! Policy: match the snippet **exactly**.  When that fails, do not guess —
//! return an error, and use the (read-only) diagnostics below to tell the model
//! *why* it failed (whitespace/indentation drift, CRLF line endings, multiple
//! matches) so it can correct itself.

/// A successful replacement: the new file contents and how many occurrences
/// were replaced (always 1 unless `replace_all`).
pub struct Replacement {
    pub content: String,
    pub count: usize,
}

/// Why a replacement could not be applied.
#[derive(Debug, Clone, Copy)]
pub enum ReplaceError {
    /// `old_text` and `new_text` are identical — nothing to do.
    Identical,
    /// `old_text` does not appear in the content.
    NotFound,
    /// `old_text` appears more than once and `replace_all` was not set.
    Ambiguous { count: usize },
}

/// Replace `old` with `new` in `content`, matching exactly.
///
/// * unique match → replaced once;
/// * multiple matches + `replace_all` → all replaced;
/// * multiple matches without `replace_all` → [`ReplaceError::Ambiguous`];
/// * no match → [`ReplaceError::NotFound`].
pub fn replace(
    content: &str,
    old: &str,
    new: &str,
    replace_all: bool,
) -> Result<Replacement, ReplaceError> {
    if old == new {
        return Err(ReplaceError::Identical);
    }
    if old.is_empty() {
        return Err(ReplaceError::NotFound);
    }
    let count = content.matches(old).count();
    match count {
        0 => Err(ReplaceError::NotFound),
        1 => match content.find(old) {
            Some(idx) => {
                let mut s = String::with_capacity(content.len() - old.len() + new.len());
                s.push_str(&content[..idx]);
                s.push_str(new);
                s.push_str(&content[idx + old.len()..]);
                Ok(Replacement {
                    content: s,
                    count: 1,
                })
            }
            None => Err(ReplaceError::NotFound),
        },
        n if replace_all => Ok(Replacement {
            content: content.replace(old, new),
            count: n,
        }),
        n => Err(ReplaceError::Ambiguous { count: n }),
    }
}

/// The 1-based starting line of every (non-overlapping) occurrence of `old`.
/// Used to make an "ambiguous" error point at the matches.
pub fn occurrence_lines(content: &str, old: &str) -> Vec<usize> {
    let mut lines = Vec::new();
    if old.is_empty() {
        return lines;
    }
    let mut start = 0;
    while let Some(rel) = content[start..].find(old) {
        let idx = start + rel;
        lines.push(content[..idx].matches('\n').count() + 1);
        start = idx + old.len();
    }
    lines
}

/// A read-only explanation for a not-found match, used to build a helpful
/// error message.  Diagnosis only — never used to perform an edit.
#[derive(Debug)]
pub enum NotFoundHint {
    /// The snippet is present at this 1-based line, differing only in
    /// leading/trailing whitespace, and that near-match is unique.
    WhitespaceOnly { line: usize },
    /// The snippet matches once CRLF line endings are normalised to LF.
    LineEnding,
    /// No benign explanation found.
    None,
}

/// Explain why `old` was not found in `content` (best-effort, read-only).
pub fn diagnose_not_found(content: &str, old: &str) -> NotFoundHint {
    if old.is_empty() {
        return NotFoundHint::None;
    }
    // Line endings: matches once CR is dropped, but not byte-for-byte.
    let nc = content.replace("\r\n", "\n");
    let no = old.replace("\r\n", "\n");
    if (nc != content || no != old) && nc.contains(&no) && !content.contains(old) {
        return NotFoundHint::LineEnding;
    }
    // Whitespace/indentation drift: a unique run of lines matches after each
    // line is trimmed of leading/trailing whitespace.
    if let Some(line0) = unique_trimmed_block(&nc, &no) {
        return NotFoundHint::WhitespaceOnly { line: line0 + 1 };
    }
    NotFoundHint::None
}

/// The 0-based start line of the unique run of lines in `content` whose
/// per-line-trimmed form equals `old`'s, or `None` if there is no such run or
/// it is not unique.
fn unique_trimmed_block(content: &str, old: &str) -> Option<usize> {
    let clines: Vec<&str> = content.split('\n').collect();
    let mut olines: Vec<&str> = old.split('\n').collect();
    // Drop the single empty trailing element produced when `old` ends in '\n'.
    // (A loop would wrongly eat a genuine trailing blank line, not just the
    // split artefact.)
    if olines.last() == Some(&"") {
        olines.pop();
    }
    if olines.is_empty() || clines.len() < olines.len() {
        return None;
    }
    let n = olines.len();
    let mut found = None;
    for i in 0..=(clines.len() - n) {
        if (0..n).all(|j| clines[i + j].trim() == olines[j].trim()) {
            if found.is_some() {
                return None; // not unique
            }
            found = Some(i);
        }
    }
    found
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    fn ok(r: Result<Replacement, ReplaceError>) -> Replacement {
        match r {
            Ok(v) => v,
            Err(e) => panic!("expected Ok, got {e:?}"),
        }
    }

    #[test]
    fn replace_unique_match() {
        let r = ok(replace("a\nB\nc", "B", "X", false));
        assert_eq!(r.content, "a\nX\nc");
        assert_eq!(r.count, 1);
    }

    #[test]
    fn replace_is_exact_not_fuzzy() {
        // Indentation differs -> NOT found (no silent rescue).
        assert!(matches!(
            replace("    foo()", "foo()", "bar()", false),
            Ok(Replacement { .. })
        )); // substring match is fine
        assert!(matches!(
            replace("\tfoo()", "    foo()", "bar()", false),
            Err(ReplaceError::NotFound)
        ));
    }

    #[test]
    fn replace_not_found() {
        assert!(matches!(
            replace("abc", "xyz", "q", false),
            Err(ReplaceError::NotFound)
        ));
    }

    #[test]
    fn replace_empty_old_is_not_found() {
        assert!(matches!(
            replace("abc", "", "q", false),
            Err(ReplaceError::NotFound)
        ));
    }

    #[test]
    fn replace_identical() {
        assert!(matches!(
            replace("abc", "b", "b", false),
            Err(ReplaceError::Identical)
        ));
    }

    #[test]
    fn replace_ambiguous_reports_count() {
        match replace("x x x", "x", "y", false) {
            Err(ReplaceError::Ambiguous { count }) => assert_eq!(count, 3),
            _ => panic!("expected Ambiguous"),
        }
    }

    #[test]
    fn replace_all_changes_every_occurrence() {
        let r = ok(replace("x x x", "x", "y", true));
        assert_eq!(r.content, "y y y");
        assert_eq!(r.count, 3);
    }

    #[test]
    fn replace_all_single_occurrence_counts_one() {
        let r = ok(replace("only one", "one", "two", true));
        assert_eq!(r.content, "only two");
        assert_eq!(r.count, 1);
    }

    #[test]
    fn occurrence_lines_are_one_based() {
        assert_eq!(
            occurrence_lines("a\nTARGET\nb\nTARGET\n", "TARGET"),
            vec![2, 4]
        );
        assert_eq!(occurrence_lines("none here", "x"), Vec::<usize>::new());
        assert_eq!(occurrence_lines("anything", ""), Vec::<usize>::new());
    }

    #[test]
    fn diagnose_whitespace_only_unique() {
        // File has indentation; snippet does not -> unique near-match at line 2.
        match diagnose_not_found("fn f() {\n        return 1;\n}", "return 1;") {
            NotFoundHint::WhitespaceOnly { line } => assert_eq!(line, 2),
            _ => panic!("expected WhitespaceOnly"),
        }
    }

    #[test]
    fn diagnose_whitespace_only_must_be_unique() {
        // One indented line whose trimmed form appears twice -> ambiguous
        // near-match -> no hint (we don't point at an arbitrary one).
        assert!(matches!(
            diagnose_not_found("  a();\n      a();\n", "    a();"),
            NotFoundHint::None
        ));
    }

    #[test]
    fn diagnose_line_ending() {
        match diagnose_not_found("one\r\ntwo\r\n", "one\ntwo") {
            NotFoundHint::LineEnding => {}
            _ => panic!("expected LineEnding"),
        }
    }

    #[test]
    fn diagnose_none_when_genuinely_absent() {
        assert!(matches!(
            diagnose_not_found("completely\ndifferent\n", "missing line"),
            NotFoundHint::None
        ));
    }

    #[test]
    fn diagnose_crlf_present_but_text_still_absent_is_none() {
        // Content has CRLF, but the snippet isn't there even after normalising.
        // The line-ending hint must require an actual normalised match, not just
        // the presence of CRLF.
        assert!(matches!(
            diagnose_not_found("a\r\nb\r\n", "zzz"),
            NotFoundHint::None
        ));
    }

    #[test]
    fn diagnose_line_ending_when_only_old_has_crlf() {
        // The CRLF difference can be on the snippet side too.
        match diagnose_not_found("a\nb\n", "a\r\nb") {
            NotFoundHint::LineEnding => {}
            other => panic!("expected LineEnding, got {other:?}"),
        }
    }

    #[test]
    fn diagnose_whitespace_only_equal_line_count() {
        // Content and snippet have the same number of lines, differing only in
        // indentation -> unique near-match at line 1.
        match diagnose_not_found("  a\n  b", "a\nb") {
            NotFoundHint::WhitespaceOnly { line } => assert_eq!(line, 1),
            other => panic!("expected WhitespaceOnly, got {other:?}"),
        }
    }

    #[test]
    fn diagnose_none_when_content_shorter_than_snippet() {
        // Fewer lines in the file than in the snippet -> no near-match.
        assert!(matches!(
            diagnose_not_found("a\n", "x\ny\nz"),
            NotFoundHint::None
        ));
    }
}
