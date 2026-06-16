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

/// One requested replacement: replace `old` with `new`, optionally at every
/// (disjoint) occurrence.
#[derive(Debug, Clone, Copy)]
pub struct Edit<'a> {
    pub old: &'a str,
    pub new: &'a str,
    pub replace_all: bool,
}

/// The result of planning and applying a batch of [`Edit`]s.
#[derive(Debug)]
pub struct Planned {
    pub content: String,
    /// Total number of individual replacements made across all edits.
    pub count: usize,
}

/// Why a batch of edits could not be applied.  Every variant names the
/// offending edit by its index into the `edits` slice.
#[derive(Debug, Clone, Copy)]
pub enum PlanError {
    /// This edit's `old` and `new` are identical — nothing to do.
    Identical { edit: usize },
    /// This edit's `old` does not appear in the content.
    NotFound { edit: usize },
    /// This (non-`replace_all`) edit's `old` matches more than once, so the
    /// target is not unique.
    Ambiguous { edit: usize, count: usize },
    /// Two target ranges overlap, so the totality of matches is not disjoint.
    /// When `edit_a == edit_b` the overlap is among a single `replace_all`
    /// edit's own matches (e.g. `aa` within `aaa`); otherwise two different
    /// edits target the same text.
    Overlap {
        edit_a: usize,
        edit_b: usize,
        /// 1-based line of the overlap.
        line: usize,
    },
}

impl PlanError {
    /// The (primary) edit index this error concerns — used to tag the
    /// edit-failure snapshot.
    pub fn edit(&self) -> usize {
        match *self {
            PlanError::Identical { edit }
            | PlanError::NotFound { edit }
            | PlanError::Ambiguous { edit, .. } => edit,
            PlanError::Overlap { edit_a, .. } => edit_a,
        }
    }
}

/// A single target range produced by an edit.
struct Span<'a> {
    start: usize,
    end: usize,
    edit: usize,
    new: &'a str,
}

/// Plan and apply a batch of exact-match edits against `content`.
///
/// The universal rule across every editing scenario (single edit, single edit
/// with `replace_all`, multi-edit) is the same: gather the target range of
/// every match of every edit, and require **the totality of those ranges to be
/// pairwise disjoint**.  On top of that, a non-`replace_all` edit carries the
/// stricter constraint that its `old` must match *exactly once*.
///
/// All edits match against the *original* `content` — they are applied in
/// parallel, not sequentially — so one edit never sees another's output, and a
/// pair of edits targeting the same text is reported as an overlap up front
/// rather than surfacing later as a confusing "not found".
pub fn apply_edits(content: &str, edits: &[Edit]) -> Result<Planned, PlanError> {
    // 1. Gather every target range, enforcing each edit's own constraints.
    let mut spans: Vec<Span> = Vec::new();
    for (i, e) in edits.iter().enumerate() {
        if e.old == e.new {
            return Err(PlanError::Identical { edit: i });
        }
        // `occurrence_starts` returns every (possibly overlapping) match; an
        // empty `old` yields none.
        let starts = occurrence_starts(content, e.old);
        match starts.len() {
            0 => return Err(PlanError::NotFound { edit: i }),
            n if !e.replace_all && n > 1 => {
                return Err(PlanError::Ambiguous { edit: i, count: n });
            }
            _ => spans.extend(starts.into_iter().map(|start| Span {
                start,
                end: start + e.old.len(),
                edit: i,
                new: e.new,
            })),
        }
    }

    // 2. Universal rule: the totality of ranges must be pairwise disjoint.
    // After sorting by start, checking consecutive pairs is sufficient (if
    // every next.start >= prev.end then all pairs are disjoint).
    spans.sort_by_key(|s| s.start);
    for w in spans.windows(2) {
        if w[1].start < w[0].end {
            return Err(PlanError::Overlap {
                edit_a: w[0].edit.min(w[1].edit),
                edit_b: w[0].edit.max(w[1].edit),
                line: content[..w[1].start].matches('\n').count() + 1,
            });
        }
    }

    // 3. Apply from the highest offset down so earlier offsets stay valid.
    let count = spans.len();
    let mut out = content.to_string();
    for s in spans.iter().rev() {
        out.replace_range(s.start..s.end, s.new);
    }
    Ok(Planned {
        content: out,
        count,
    })
}

/// Byte offsets of every (possibly *overlapping*) start position of `old` in
/// `content`.  Overlap matters: `aa` starts at offsets 0 and 1 in `aaa`.
///
/// Implemented as a bounded scan over every char boundary (rather than a
/// `find` loop with manual advancement) so it can never spin: each candidate
/// start is tested independently.
fn occurrence_starts(content: &str, old: &str) -> Vec<usize> {
    if old.is_empty() {
        return Vec::new();
    }
    content
        .char_indices()
        .filter(|&(i, _)| content[i..].starts_with(old))
        .map(|(i, _)| i)
        .collect()
}

/// The 1-based starting line of every (possibly overlapping) occurrence of
/// `old`.  Used to make an "ambiguous" error point at the matches, so it must
/// agree with the overlapping matches gathered by [`apply_edits`].
pub fn occurrence_lines(content: &str, old: &str) -> Vec<usize> {
    occurrence_starts(content, old)
        .into_iter()
        .map(|idx| content[..idx].matches('\n').count() + 1)
        .collect()
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

    fn ok(r: Result<Planned, PlanError>) -> Planned {
        match r {
            Ok(v) => v,
            Err(e) => panic!("expected Ok, got {e:?}"),
        }
    }

    /// Apply a single edit (the `edit_file` shape).
    fn one<'a>(content: &str, old: &'a str, new: &'a str, all: bool) -> Result<Planned, PlanError> {
        apply_edits(
            content,
            &[Edit {
                old,
                new,
                replace_all: all,
            }],
        )
    }

    #[test]
    fn single_unique_match() {
        let r = ok(one("a\nB\nc", "B", "X", false));
        assert_eq!(r.content, "a\nX\nc");
        assert_eq!(r.count, 1);
    }

    #[test]
    fn single_is_exact_not_fuzzy() {
        // `foo()` is a genuine substring of `    foo()` -> matches.
        assert!(one("    foo()", "foo()", "bar()", false).is_ok());
        // Different leading whitespace -> NOT found (no silent rescue).
        assert!(matches!(
            one("\tfoo()", "    foo()", "bar()", false),
            Err(PlanError::NotFound { edit: 0 })
        ));
    }

    #[test]
    fn single_not_found() {
        assert!(matches!(
            one("abc", "xyz", "q", false),
            Err(PlanError::NotFound { edit: 0 })
        ));
    }

    #[test]
    fn single_empty_old_is_not_found() {
        assert!(matches!(
            one("abc", "", "q", false),
            Err(PlanError::NotFound { edit: 0 })
        ));
    }

    #[test]
    fn single_identical() {
        assert!(matches!(
            one("abc", "b", "b", false),
            Err(PlanError::Identical { edit: 0 })
        ));
    }

    #[test]
    fn single_ambiguous_reports_count() {
        match one("x x x", "x", "y", false) {
            Err(PlanError::Ambiguous { edit: 0, count }) => assert_eq!(count, 3),
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn replace_all_changes_every_occurrence() {
        let r = ok(one("x x x", "x", "y", true));
        assert_eq!(r.content, "y y y");
        assert_eq!(r.count, 3);
    }

    #[test]
    fn replace_all_single_occurrence_counts_one() {
        let r = ok(one("only one", "one", "two", true));
        assert_eq!(r.content, "only two");
        assert_eq!(r.count, 1);
    }

    #[test]
    fn single_overlapping_chars_is_ambiguous() {
        // `aa` could start at offset 0 or 1 inside `aaa`; without replace_all
        // the (>1) match count makes it ambiguous.
        match one("aaa", "aa", "X", false) {
            Err(PlanError::Ambiguous { edit: 0, count }) => assert_eq!(count, 2),
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn single_overlapping_whitespace_is_ambiguous() {
        // Three spaces inside a run of four -> two overlapping positions.
        match one("    ", "   ", "X", false) {
            Err(PlanError::Ambiguous { edit: 0, count }) => assert_eq!(count, 2),
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn single_overlapping_lines_is_ambiguous() {
        // The two-line block `X\nX` overlaps itself in three identical lines.
        match one("X\nX\nX\n", "X\nX", "Y", false) {
            Err(PlanError::Ambiguous { edit: 0, count }) => assert_eq!(count, 2),
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn replace_all_overlapping_is_not_disjoint() {
        // Overlapping matches have no unambiguous "replace all" -> the matches
        // are not disjoint, reported as Overlap (within one edit).
        match one("xaaa", "aa", "b", true) {
            Err(PlanError::Overlap {
                edit_a: 0,
                edit_b: 0,
                line,
            }) => assert_eq!(line, 1),
            other => panic!("expected Overlap, got {other:?}"),
        }
    }

    #[test]
    fn replace_all_disjoint_succeeds() {
        let r = ok(one("aa bb aa", "aa", "X", true));
        assert_eq!(r.content, "X bb X");
        assert_eq!(r.count, 2);
    }

    #[test]
    fn replace_all_adjacent_disjoint_succeeds() {
        // Matches exactly `width` apart touch but do not overlap -> allowed.
        // (Pins `<` rather than `<=` in the disjointness check.)
        let r = ok(one("abab", "ab", "X", true));
        assert_eq!(r.content, "XX");
        assert_eq!(r.count, 2);
    }

    #[test]
    fn multi_disjoint_edits_apply_in_parallel() {
        let r = ok(apply_edits(
            "foo and bar",
            &[
                Edit {
                    old: "foo",
                    new: "X",
                    replace_all: false,
                },
                Edit {
                    old: "bar",
                    new: "Y",
                    replace_all: false,
                },
            ],
        ));
        assert_eq!(r.content, "X and Y");
        assert_eq!(r.count, 2);
    }

    #[test]
    fn multi_overlapping_edits_conflict() {
        // Two edits targeting the same `aa` -> overlap naming both edits.
        match apply_edits(
            "aa",
            &[
                Edit {
                    old: "aa",
                    new: "X",
                    replace_all: false,
                },
                Edit {
                    old: "aa",
                    new: "Y",
                    replace_all: false,
                },
            ],
        ) {
            Err(PlanError::Overlap {
                edit_a: 0,
                edit_b: 1,
                line,
            }) => assert_eq!(line, 1),
            other => panic!("expected Overlap, got {other:?}"),
        }
    }

    #[test]
    fn multi_not_found_names_the_edit() {
        match apply_edits(
            "foo",
            &[
                Edit {
                    old: "foo",
                    new: "X",
                    replace_all: false,
                },
                Edit {
                    old: "bar",
                    new: "Y",
                    replace_all: false,
                },
            ],
        ) {
            Err(PlanError::NotFound { edit }) => assert_eq!(edit, 1),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn multi_is_parallel_not_sequential() {
        // Edit 2 targets edit 1's *output* (`b`); against the original `a`
        // there is no `b`, so it is not found rather than chaining.
        match apply_edits(
            "a",
            &[
                Edit {
                    old: "a",
                    new: "b",
                    replace_all: false,
                },
                Edit {
                    old: "b",
                    new: "c",
                    replace_all: false,
                },
            ],
        ) {
            Err(PlanError::NotFound { edit }) => assert_eq!(edit, 1),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn occurrence_lines_counts_overlaps() {
        // Two overlapping matches on the same line -> reported twice.
        assert_eq!(occurrence_lines("aaa", "aa"), vec![1, 1]);
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
