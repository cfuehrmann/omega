//! Fuzzy text-replacement cascade shared by `edit_file` and `multi_edit_file`.
//!
//! This is a faithful Rust port of opencode's `replace()` strategy cascade
//! (`packages/opencode/src/tool/edit.ts`).  The model supplies an `old_text`
//! snippet that *should* be a byte-exact slice of the file, but in practice it
//! is frequently off by whitespace, indentation, or escaping.  Rather than
//! reject every near-miss (the dominant `edit_file` failure mode — roughly
//! three-quarters of observed errors were "`old_text` not found"), we try a series of
//! progressively looser matchers and accept the first that resolves to a
//! single unambiguous region.
//!
//! Each *replacer* yields candidate substrings of `content` that it considers
//! equivalent to the search snippet.  The driver ([`replace`]) walks the
//! replacers in order; for the first yielded candidate that occurs in
//! `content` it either replaces all occurrences (`replace_all`) or, when the
//! candidate occurs exactly once, performs the single replacement.  A
//! candidate that is found but is ambiguous (more than one occurrence, with
//! `replace_all` off) is skipped in favour of the next candidate; if every
//! candidate is either absent or ambiguous the driver reports [`NotFound`] or
//! [`Ambiguous`] respectively.
//!
//! [`NotFound`]: ReplaceError::NotFound
//! [`Ambiguous`]: ReplaceError::Ambiguous

/// Why a replacement could not be applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplaceError {
    /// `old_text` and `new_text` are identical — nothing to do.
    Identical,
    /// `old_text` did not match any region under any strategy.
    NotFound,
    /// `old_text` matched more than one region and `replace_all` was off.
    Ambiguous,
}

/// A successful replacement: the rewritten file content and how many
/// occurrences were replaced (1 unless `replace_all` matched several).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replacement {
    /// The full file content after the replacement.
    pub content: String,
    /// Number of occurrences replaced.
    pub count: usize,
}

/// The replacer cascade, in the exact order opencode applies them.
type Replacer = fn(&str, &str) -> Vec<String>;

const REPLACERS: &[Replacer] = &[
    simple,
    line_trimmed,
    block_anchor,
    whitespace_normalized,
    indentation_flexible,
    escape_normalized,
    trimmed_boundary,
    context_aware,
    multi_occurrence,
];

/// Replace `old` with `new` in `content` using the fuzzy cascade.
///
/// With `replace_all` off the matched region must be unique; otherwise every
/// occurrence of the resolved candidate is replaced.
///
/// # Errors
/// Returns [`ReplaceError`] when `old == new`, when no candidate matches, or
/// when the match is ambiguous and `replace_all` is off.
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

    let mut found = false;
    for replacer in REPLACERS {
        for search in replacer(content, old) {
            if search.is_empty() {
                continue;
            }
            let Some(index) = content.find(&search) else {
                continue;
            };
            found = true;
            if replace_all {
                let count = content.matches(&search).count();
                return Ok(Replacement {
                    content: content.replace(&search, new),
                    count,
                });
            }
            // Unique only if first and last occurrence coincide.
            if content.rfind(&search) != Some(index) {
                continue;
            }
            let mut out = String::with_capacity(content.len() - search.len() + new.len());
            out.push_str(&content[..index]);
            out.push_str(new);
            out.push_str(&content[index + search.len()..]);
            return Ok(Replacement {
                content: out,
                count: 1,
            });
        }
    }

    if found {
        Err(ReplaceError::Ambiguous)
    } else {
        Err(ReplaceError::NotFound)
    }
}

// ---------------------------------------------------------------------------
// Replacers
// ---------------------------------------------------------------------------

/// Exact match: yield the snippet unchanged.
fn simple(_content: &str, find: &str) -> Vec<String> {
    vec![find.to_string()]
}

/// Split `find` into lines, dropping a single trailing empty line (the common
/// artefact of a trailing newline).
fn search_lines(find: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = find.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    lines
}

/// Byte offset of the start of line `i` (0-indexed) within `lines`, assuming
/// the lines were produced by splitting on a single-byte `'\n'`.
fn line_start_offset(lines: &[&str], i: usize) -> usize {
    lines[..i].iter().map(|l| l.len() + 1).sum()
}

/// The substring of `content` spanning `lines[start..=end]`, reconstructed
/// from byte lengths (each interior line is followed by one `'\n'`).
fn block_substring(content: &str, lines: &[&str], start: usize, end: usize) -> String {
    let from = line_start_offset(lines, start);
    let mut to = from;
    for (k, line) in lines.iter().enumerate().take(end + 1).skip(start) {
        to += line.len();
        if k < end {
            to += 1; // newline between lines, not after the last
        }
    }
    content[from..to].to_string()
}

/// Match line-by-line ignoring leading/trailing whitespace on each line.
fn line_trimmed(content: &str, find: &str) -> Vec<String> {
    let original: Vec<&str> = content.split('\n').collect();
    let search = search_lines(find);
    if search.is_empty() || original.len() < search.len() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for i in 0..=(original.len() - search.len()) {
        let matches = (0..search.len()).all(|j| original[i + j].trim() == search[j].trim());
        if matches {
            out.push(block_substring(content, &original, i, i + search.len() - 1));
        }
    }
    out
}

/// Levenshtein edit distance between two strings (character-based).
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    // No empty-input fast path: the matrix initialisation below already yields
    // the correct distance when either side is empty (`prev` starts as the
    // identity row, and an empty `a` skips the outer loop).
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Trimmed-similarity of the interior lines of two equal-anchored blocks.
/// Returns 1.0 when there are no interior lines to compare.
#[allow(clippy::cast_precision_loss)] // ratios over small line counts; exactness irrelevant
fn middle_similarity(original: &[&str], start: usize, search: &[&str], block_len: usize) -> f64 {
    let lines_to_check = (search.len().saturating_sub(2)).min(block_len.saturating_sub(2));
    if lines_to_check == 0 {
        return 1.0;
    }
    let mut similarity = 0.0;
    // Compare the interior lines (skip the matched first/last anchors). A `for`
    // range (rather than a manual `while … { j += 1 }`) keeps the increment
    // out of reach of mutation (a `*=` there would loop forever).
    let upper = (search.len() - 1).min(block_len - 1);
    for j in 1..upper {
        let orig = original[start + j].trim();
        let srch = search[j].trim();
        let max_len = orig.chars().count().max(srch.chars().count());
        if max_len != 0 {
            let distance = levenshtein(orig, srch);
            similarity += 1.0 - (distance as f64 / max_len as f64);
        }
    }
    similarity / lines_to_check as f64
}

/// Anchor on the first and last lines of a (>=3 line) block, tolerating drift
/// in the interior. With a single anchored candidate the interior is trusted;
/// with several, the most similar interior wins if it clears 0.3.
fn block_anchor(content: &str, find: &str) -> Vec<String> {
    let original: Vec<&str> = content.split('\n').collect();
    let raw: Vec<&str> = find.split('\n').collect();
    if raw.len() < 3 {
        return Vec::new();
    }
    let search = search_lines(find);
    let first = search[0].trim();
    let last = search[search.len() - 1].trim();

    let mut candidates: Vec<(usize, usize)> = Vec::new();
    for (i, line) in original.iter().enumerate() {
        if line.trim() != first {
            continue;
        }
        for (j, cand) in original.iter().enumerate().skip(i + 2) {
            if cand.trim() == last {
                candidates.push((i, j));
                break;
            }
        }
    }
    if candidates.is_empty() {
        return Vec::new();
    }

    if candidates.len() == 1 {
        // Threshold is 0.0 upstream: a matched pair of anchors always wins.
        let (start, end) = candidates[0];
        return vec![block_substring(content, &original, start, end)];
    }

    let mut best: Option<(usize, usize)> = None;
    let mut max_similarity = -1.0_f64;
    for &(start, end) in &candidates {
        let similarity = middle_similarity(&original, start, &search, end - start + 1);
        if similarity > max_similarity {
            max_similarity = similarity;
            best = Some((start, end));
        }
    }
    match best {
        Some((start, end)) if max_similarity >= 0.3 => {
            vec![block_substring(content, &original, start, end)]
        }
        _ => Vec::new(),
    }
}

/// Collapse every run of whitespace to a single space and trim the ends.
fn normalize_ws(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Match ignoring differences in whitespace runs (per line and across blocks).
fn whitespace_normalized(content: &str, find: &str) -> Vec<String> {
    let normalized_find = normalize_ws(find);
    let mut out = Vec::new();
    let lines: Vec<&str> = content.split('\n').collect();

    for line in &lines {
        let normalized_line = normalize_ws(line);
        if normalized_line == normalized_find {
            out.push((*line).to_string());
        } else if normalized_line.contains(&normalized_find) {
            // Reconstruct the matching substring inside the line via a
            // whitespace-flexible regex built from the snippet's words.
            let words: Vec<&str> = find.split_whitespace().collect();
            if !words.is_empty() {
                let pattern = words
                    .iter()
                    .map(|w| regex::escape(w))
                    .collect::<Vec<_>>()
                    .join(r"\s+");
                if let Ok(re) = regex::Regex::new(&pattern)
                    && let Some(m) = re.find(line)
                {
                    out.push(m.as_str().to_string());
                }
            }
        }
    }

    let find_lines: Vec<&str> = find.split('\n').collect();
    if find_lines.len() > 1 && lines.len() >= find_lines.len() {
        for i in 0..=(lines.len() - find_lines.len()) {
            let block = lines[i..i + find_lines.len()].join("\n");
            if normalize_ws(&block) == normalized_find {
                out.push(block);
            }
        }
    }
    out
}

/// Strip the common minimum indentation from non-empty lines.
fn remove_indentation(text: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let min_indent = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min();
    let Some(min_indent) = min_indent else {
        return text.to_string();
    };
    lines
        .iter()
        .map(|l| {
            if l.trim().is_empty() {
                (*l).to_string()
            } else {
                l[min_indent..].to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Match a block ignoring a uniform shift in indentation.
fn indentation_flexible(content: &str, find: &str) -> Vec<String> {
    let normalized_find = remove_indentation(find);
    let content_lines: Vec<&str> = content.split('\n').collect();
    let find_lines: Vec<&str> = find.split('\n').collect();
    if content_lines.len() < find_lines.len() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for i in 0..=(content_lines.len() - find_lines.len()) {
        let block = content_lines[i..i + find_lines.len()].join("\n");
        if remove_indentation(&block) == normalized_find {
            out.push(block);
        }
    }
    out
}

/// Interpret common backslash escapes (`\n`, `\t`, …) in the snippet.
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.peek() {
                Some('n') => {
                    out.push('\n');
                    chars.next();
                }
                Some('t') => {
                    out.push('\t');
                    chars.next();
                }
                Some('r') => {
                    out.push('\r');
                    chars.next();
                }
                Some(&q @ ('\'' | '"' | '`' | '\\' | '$' | '\n')) => {
                    out.push(q);
                    chars.next();
                }
                _ => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Match after interpreting escape sequences in the snippet.
fn escape_normalized(content: &str, find: &str) -> Vec<String> {
    let unescaped = unescape(find);
    let mut out = Vec::new();
    if content.contains(&unescaped) {
        out.push(unescaped.clone());
    }
    let lines: Vec<&str> = content.split('\n').collect();
    let find_lines: Vec<&str> = unescaped.split('\n').collect();
    if lines.len() >= find_lines.len() {
        for i in 0..=(lines.len() - find_lines.len()) {
            let block = lines[i..i + find_lines.len()].join("\n");
            if unescape(&block) == unescaped {
                out.push(block);
            }
        }
    }
    out
}

/// Match after trimming leading/trailing whitespace from the whole snippet.
fn trimmed_boundary(content: &str, find: &str) -> Vec<String> {
    let trimmed = find.trim();
    if trimmed == find {
        return Vec::new();
    }
    let mut out = Vec::new();
    if content.contains(trimmed) {
        out.push(trimmed.to_string());
    }
    let lines: Vec<&str> = content.split('\n').collect();
    let find_lines: Vec<&str> = find.split('\n').collect();
    if lines.len() >= find_lines.len() {
        for i in 0..=(lines.len() - find_lines.len()) {
            let block = lines[i..i + find_lines.len()].join("\n");
            if block.trim() == trimmed {
                out.push(block);
            }
        }
    }
    out
}

/// Anchor on first/last lines and require >=50% of interior lines to match
/// (trimmed), for blocks of identical line count.
#[allow(clippy::cast_precision_loss)] // ratio over small line counts; exactness irrelevant
fn context_aware(content: &str, find: &str) -> Vec<String> {
    let find_lines = search_lines(find);
    if find_lines.len() < 3 {
        return Vec::new();
    }
    let content_lines: Vec<&str> = content.split('\n').collect();
    let first = find_lines[0].trim();
    let last = find_lines[find_lines.len() - 1].trim();

    for (i, line) in content_lines.iter().enumerate() {
        if line.trim() != first {
            continue;
        }
        for (j, cand) in content_lines.iter().enumerate().skip(i + 2) {
            if cand.trim() != last {
                continue;
            }
            let block_lines = &content_lines[i..=j];
            if block_lines.len() == find_lines.len() {
                let mut matching = 0usize;
                let mut total = 0usize;
                for k in 1..block_lines.len() - 1 {
                    let bl = block_lines[k].trim();
                    let fl = find_lines[k].trim();
                    if !bl.is_empty() || !fl.is_empty() {
                        total += 1;
                        if bl == fl {
                            matching += 1;
                        }
                    }
                }
                if total == 0 || (matching as f64 / total as f64) >= 0.5 {
                    return vec![block_substring(content, &content_lines, i, j)];
                }
            }
            break;
        }
    }
    Vec::new()
}

/// Last resort: yield the exact snippet if present (lets `replace_all` handle
/// multiple occurrences and otherwise surfaces an ambiguity).
fn multi_occurrence(content: &str, find: &str) -> Vec<String> {
    if !find.is_empty() && content.contains(find) {
        vec![find.to_string()]
    } else {
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
//
// Carve-out (per AGENTS.md): `replace` is a pure function with intricate
// internal strategy logic.  Driving every cascade branch through the
// `execute_tool` boundary would require constructing dozens of files and
// asserting on rendered output; unit-testing the pure matcher directly is the
// proportionate way to pin the strategy semantics.  End-to-end coverage of the
// tool wiring lives in `tests/file_tools.rs`.
// ---------------------------------------------------------------------------
#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)] // test assertions
mod tests {
    use super::*;

    fn ok(content: &str, old: &str, new: &str, all: bool) -> Replacement {
        replace(content, old, new, all).expect("expected a successful replacement")
    }

    #[test]
    fn identical_old_and_new_is_rejected() {
        assert_eq!(
            replace("abc", "x", "x", false),
            Err(ReplaceError::Identical)
        );
    }

    #[test]
    fn empty_old_is_not_found() {
        assert_eq!(replace("abc", "", "y", false), Err(ReplaceError::NotFound));
    }

    #[test]
    fn simple_exact_match() {
        let r = ok("Hello, world!", "world", "Rust", false);
        assert_eq!(r.content, "Hello, Rust!");
        assert_eq!(r.count, 1);
    }

    #[test]
    fn missing_snippet_is_not_found() {
        assert_eq!(
            replace("hello world", "MISSING", "x", false),
            Err(ReplaceError::NotFound)
        );
    }

    #[test]
    fn duplicate_exact_is_ambiguous_without_replace_all() {
        assert_eq!(
            replace("aa bb aa", "aa", "zz", false),
            Err(ReplaceError::Ambiguous)
        );
    }

    #[test]
    fn replace_all_replaces_every_occurrence() {
        let r = ok("aa bb aa", "aa", "zz", true);
        assert_eq!(r.content, "zz bb zz");
        assert_eq!(r.count, 2);
    }

    #[test]
    fn line_trimmed_tolerates_indentation_drift() {
        // Snippet lacks the leading indentation present in the file.
        let content = "fn main() {\n    let x = 1;\n}\n";
        let r = ok(content, "let x = 1;", "let x = 2;", false);
        assert_eq!(r.content, "fn main() {\n    let x = 2;\n}\n");
    }

    #[test]
    fn whitespace_normalized_collapses_runs() {
        let content = "let   a    =     1;";
        let r = ok(content, "let a = 1;", "let a = 2;", false);
        assert_eq!(r.content, "let a = 2;");
    }

    #[test]
    fn escape_normalized_interprets_backslash_n() {
        let content = "first\nsecond";
        let r = ok(content, "first\\nsecond", "merged", false);
        assert_eq!(r.content, "merged");
    }

    #[test]
    fn block_anchor_tolerates_interior_drift() {
        // line_trimmed misses (interior differs); block_anchor catches via anchors.
        let content = "begin\n  middle line here\nend\n";
        let snippet = "begin\n  MIDDLE\nend";
        let r = ok(content, snippet, "REPLACED", false);
        assert_eq!(r.content, "REPLACED\n");
    }

    #[test]
    fn replace_all_with_fuzzy_candidate_counts_occurrences() {
        let content = "x = 1;\nx = 1;\n";
        let r = ok(content, "x = 1;", "y = 2;", true);
        assert_eq!(r.content, "y = 2;\ny = 2;\n");
        assert_eq!(r.count, 2);
    }

    #[test]
    fn first_strategy_wins_over_later_ambiguous_one() {
        // Exact unique match resolves immediately even though looser strategies
        // would also (ambiguously) match — proves cascade order + uniqueness.
        let content = "A\n  a\nB\n  a\n";
        // Unique exact line "B" — simple replacer handles it.
        let r = ok(content, "B", "C", false);
        assert_eq!(r.content, "A\n  a\nC\n  a\n");
    }

    // --- direct per-replacer unit tests (cascade-order shadowing makes some
    // replacers unreachable through `replace`, so pin them directly) ---

    #[test]
    fn simple_yields_find_unchanged() {
        assert_eq!(simple("irrelevant", "abc"), vec!["abc".to_string()]);
    }

    #[test]
    fn search_lines_drops_single_trailing_blank() {
        assert_eq!(search_lines("a\nb\n"), vec!["a", "b"]);
        assert_eq!(search_lines("a\nb"), vec!["a", "b"]);
        // Only ONE trailing blank is dropped.
        assert_eq!(search_lines("a\n\n"), vec!["a", ""]);
    }

    #[test]
    fn line_offset_and_block_substring_are_byte_accurate() {
        let content = "aa\nbbb\ncccc";
        let lines: Vec<&str> = content.split('\n').collect();
        assert_eq!(line_start_offset(&lines, 0), 0);
        assert_eq!(line_start_offset(&lines, 1), 3); // "aa\n"
        assert_eq!(line_start_offset(&lines, 2), 7); // "aa\nbbb\n"
        assert_eq!(block_substring(content, &lines, 1, 2), "bbb\ncccc");
        assert_eq!(block_substring(content, &lines, 0, 0), "aa");
    }

    #[test]
    fn line_trimmed_yields_original_spans() {
        let content = "x\n   y  \nz";
        assert_eq!(line_trimmed(content, "y"), vec!["   y  ".to_string()]);
        // Multi-line span returns the original (untrimmed) text.
        assert_eq!(line_trimmed(content, "y\nz"), vec!["   y  \nz".to_string()]);
        // Too-long snippet yields nothing.
        assert!(line_trimmed("a", "a\nb\nc").is_empty());
        // Snippet exactly as long as the file still matches (pins the
        // `original.len() < search.len()` boundary: must be `<`, not `<=`).
        assert_eq!(line_trimmed("a\nb", "a\nb"), vec!["a\nb".to_string()]);
    }

    #[test]
    fn levenshtein_basic() {
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("abc", ""), 3);
        assert_eq!(levenshtein("", ""), 0);
        assert_eq!(levenshtein("abc", "abc"), 0);
        // One of each edit kind — pins the three `+` recurrence terms
        // (substitution / insertion / deletion) independently.
        assert_eq!(levenshtein("abc", "abd"), 1); // substitution
        assert_eq!(levenshtein("a", "ab"), 1); // insertion
        assert_eq!(levenshtein("ab", "a"), 1); // deletion
    }

    #[test]
    fn middle_similarity_scores_interior_lines() {
        let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
        let original = vec!["begin", "x", "end"];
        let search = vec!["begin", "x", "end"];
        assert!(close(middle_similarity(&original, 0, &search, 3), 1.0));
        let search2 = vec!["begin", "y", "end"];
        assert!(close(middle_similarity(&original, 0, &search2, 3), 0.0));
        // No interior lines -> trusted (1.0).
        let two = vec!["begin", "end"];
        assert!(close(middle_similarity(&two, 0, &two, 2), 1.0));
        // Multi-char interior with a single-char diff -> 1 - 1/4 = 0.75.
        // Pins the `distance / max_len` ratio (a `*` there gives a negative).
        let o4 = vec!["A", "abcd", "B"];
        let s4 = vec!["A", "abxd", "B"];
        assert!(close(middle_similarity(&o4, 0, &s4, 3), 0.75));
        // Two interior lines, one perfect + one total mismatch -> 1.0/2 = 0.5.
        // Pins the final `similarity / lines_to_check` divide.
        let o5 = vec!["A", "xx", "zz", "B"];
        let s5 = vec!["A", "xx", "yy", "B"];
        assert!(close(middle_similarity(&o5, 0, &s5, 4), 0.5));
        // Asymmetric bounds: when `block_len` < search length the block side
        // limits the loop, and vice-versa. Both must score 1.0 (only the
        // shared first interior line is compared); pins the `.min(..)` bound.
        let long_search = vec!["A", "xx", "yy", "zz", "B"];
        assert!(close(
            middle_similarity(&["A", "xx", "B"], 0, &long_search, 3),
            1.0
        ));
        let long_block = vec!["A", "xx", "yy", "zz", "B"];
        assert!(close(
            middle_similarity(&long_block, 0, &["A", "xx", "B"], 5),
            1.0
        ));
        // The `-1` on each side of the `.min` bound matters: it stops the loop
        // BEFORE the trailing anchor line. If the search-side `- 1` is dropped
        // the extra (matching) anchor line is scored and the average doubles;
        // likewise for the block-side `- 1`.
        assert!(close(
            middle_similarity(&["A", "p", "Z"], 0, &["A", "p", "Z"], 10),
            1.0
        ));
        assert!(close(
            middle_similarity(&["A", "p", "q"], 0, &["A", "p", "q", "r", "Z"], 3),
            1.0
        ));
    }

    #[test]
    fn block_anchor_requires_three_lines() {
        assert!(block_anchor("a\nb\n", "a\nb").is_empty());
        // A four-line block DOES match — pins the `raw.len() < 3` guard
        // (a `>` there would reject everything longer than three lines).
        assert_eq!(
            block_anchor("A\nb\nc\nZ\n", "A\nb\nc\nZ"),
            vec!["A\nb\nc\nZ".to_string()]
        );
    }

    #[test]
    fn block_anchor_tie_keeps_first_candidate() {
        // Two anchored candidates with identical interior similarity (both
        // interiors trim to "X"). The first must win (`>` not `>=`), so the
        // returned span is the earlier one — including its raw " X " interior.
        let content = "begin\n X \nend\nbegin\nX\nend\n";
        assert_eq!(
            block_anchor(content, "begin\nX\nend"),
            vec!["begin\n X \nend".to_string()]
        );
    }

    #[test]
    fn block_anchor_scores_each_candidate_with_its_own_length() {
        // Two candidates of different lengths (3-line and 4-line blocks) for a
        // 4-line snippet. With the correct per-candidate `end - start + 1`
        // block length both score 1.0 and the first wins; corrupting that
        // length arithmetic re-scores the candidates and flips the winner.
        let content = "pre\nA\nX\nZ\npad\nA\nX\nY\nZ\n";
        assert_eq!(
            block_anchor(content, "A\nX\nY\nZ"),
            vec!["A\nX\nZ".to_string()]
        );
    }

    #[test]
    fn block_anchor_empty_when_anchors_absent() {
        assert!(block_anchor("p\nq\nr\n", "X\nq\nZ").is_empty());
    }

    #[test]
    fn block_anchor_single_candidate_trusts_anchors() {
        let content = "begin\n  middle line here\nend\n";
        assert_eq!(
            block_anchor(content, "begin\n  MIDDLE\nend"),
            vec!["begin\n  middle line here\nend".to_string()]
        );
    }

    #[test]
    fn block_anchor_multi_candidate_picks_most_similar() {
        let content = "begin\n  alpha\nend\nbegin\n  beta\nend\n";
        assert_eq!(
            block_anchor(content, "begin\n  beta\nend"),
            vec!["begin\n  beta\nend".to_string()]
        );
    }

    #[test]
    fn block_anchor_multi_candidate_below_threshold_rejected() {
        // Two candidates, both interiors wildly different from snippet's;
        // best average similarity < 0.3 -> no match.
        let content = "begin\n  zzzzzzzz\nend\nbegin\n  wwwwwwww\nend\n";
        assert!(block_anchor(content, "begin\n  aaaaaaaa\nend").is_empty());
    }

    #[test]
    fn normalize_ws_collapses_and_trims() {
        assert_eq!(normalize_ws("  a   b\t c "), "a b c");
    }

    #[test]
    fn whitespace_normalized_full_line_and_substring() {
        // Full single-line match yields the original line.
        assert_eq!(
            whitespace_normalized("let   a  =  1;", "let a = 1;"),
            vec!["let   a  =  1;".to_string()]
        );
        // Substring within a longer line yields just the matching span.
        let got = whitespace_normalized("x; let   a = 1; y", "let a = 1");
        assert!(got.contains(&"let   a = 1".to_string()), "got: {got:?}");
    }

    #[test]
    fn whitespace_normalized_multiline_block() {
        // Match at the LAST possible window of a 5-line file (block of 2),
        // so the `lines.len() - find_lines.len()` upper bound must be exact
        // (a `/` there would stop scanning before reaching index 3).
        let content = "a\nb\nc\nfoo\n   bar    baz";
        let got = whitespace_normalized(content, "foo\nbar baz");
        assert!(
            got.contains(&"foo\n   bar    baz".to_string()),
            "got: {got:?}"
        );
    }

    #[test]
    fn remove_indentation_strips_common_minimum() {
        assert_eq!(remove_indentation("    a\n      b"), "a\n  b");
        // Blank lines are preserved untouched.
        assert_eq!(remove_indentation("  a\n\n  b"), "a\n\nb");
        // No indentation -> unchanged.
        assert_eq!(remove_indentation("a\nb"), "a\nb");
    }

    #[test]
    fn indentation_flexible_matches_uniform_shift() {
        // Three-line file, two-line snippet: content is longer than the
        // snippet, so the `content_lines.len() < find_lines.len()` guard must
        // be `<` (a `>` there would reject this and only accept shorter files).
        let content = "        a();\n        b();\n        c();";
        assert_eq!(
            indentation_flexible(content, "a();\nb();"),
            vec!["        a();\n        b();".to_string()]
        );
        // Equal line counts must still match (pins `<`: not `==` / `<=`).
        assert_eq!(
            indentation_flexible("    a\n    b", "a\nb"),
            vec!["    a\n    b".to_string()]
        );
    }

    #[test]
    fn unescape_handles_known_escapes() {
        assert_eq!(unescape("a\\nb\\tc"), "a\nb\tc");
        // Carriage return arm (pins the `Some('r')` match arm).
        assert_eq!(unescape("a\\rb"), "a\rb");
        assert_eq!(unescape("q\\\"q"), "q\"q");
        // Unknown escape is left as a literal backslash.
        assert_eq!(unescape("a\\zb"), "a\\zb");
    }

    #[test]
    fn escape_normalized_direct_block() {
        let content = "first\nsecond";
        assert!(
            escape_normalized(content, "first\\nsecond").contains(&"first\nsecond".to_string())
        );
        // Content that LITERALLY contains a backslash escape: the
        // `content.contains(unescaped)` path fails (no real tab present), so
        // only the per-line block loop can match. This pins that loop's
        // `lines.len() >= find_lines.len()` guard and range bound.
        let literal = "x\\ty"; // the four chars  x \ t y
        assert_eq!(
            escape_normalized(literal, "x\\ty"),
            vec!["x\\ty".to_string()]
        );
    }

    #[test]
    fn trimmed_boundary_noop_when_already_trimmed() {
        assert!(trimmed_boundary("value", "value").is_empty());
    }

    #[test]
    fn trimmed_boundary_matches_trimmed_form() {
        // Yields BOTH the bare trimmed form (the `contains` path) and the
        // original whitespace-padded line (the per-line block-loop fallback).
        // Asserting the exact pair pins the block loop's `>=` guard, its
        // `i + find_lines.len()` window bound, and its `==` equality check.
        assert_eq!(
            trimmed_boundary(" ab ", " ab "),
            vec!["ab".to_string(), " ab ".to_string()]
        );
    }

    #[test]
    fn context_aware_requires_half_interior_match() {
        let content = "head\n a\n b\ntail\n";
        // interior: " a" matches, " X" doesn't -> 1/2 = 0.5 >= 0.5 -> match.
        assert_eq!(
            context_aware(content, "head\n a\n X\ntail"),
            vec!["head\n a\n b\ntail".to_string()]
        );
    }

    #[test]
    fn context_aware_rejects_below_half() {
        let content = "head\n a\n b\n c\ntail\n";
        // interior 3 lines, 1 matches -> 1/3 < 0.5 -> no match.
        assert!(context_aware(content, "head\n a\n X\n Y\ntail").is_empty());
    }

    #[test]
    fn context_aware_requires_three_lines() {
        assert!(context_aware("a\nb\n", "a\nb").is_empty());
        // A three-line block (one interior line) DOES match — pins the
        // `find_lines.len() < 3` guard against `==` / `<=`.
        assert_eq!(
            context_aware("head\n a\ntail\n", "head\n a\ntail"),
            vec!["head\n a\ntail".to_string()]
        );
    }

    #[test]
    fn context_aware_counts_lines_where_either_side_nonempty() {
        // Interior pair (content "x", snippet "") is a genuine mismatch that
        // must be counted -> ratio 0/1 -> no match. Dropping a `!` or turning
        // `||` into `&&` makes `total` 0, which spuriously short-circuits to a
        // match. Both orderings pin both `!`s.
        assert!(context_aware("head\nx\ntail\n", "head\n\ntail").is_empty());
        assert!(context_aware("head\n\ntail\n", "head\nb\ntail").is_empty());
    }

    #[test]
    fn context_aware_block_must_span_at_least_three_lines() {
        // A spurious `last` anchor only two lines below `first` forms a 2-line
        // block that must be skipped (`skip(i + 2)`); the real 3-line block one
        // line further down is the match. With `i * 2` the early 2-line block
        // is considered first and breaks the search, yielding nothing.
        assert_eq!(
            context_aware("x\nH\nT\nT\n", "H\nT\nT"),
            vec!["H\nT\nT".to_string()]
        );
    }

    #[test]
    fn multi_occurrence_yields_present_snippet_only() {
        assert_eq!(multi_occurrence("abcabc", "bc"), vec!["bc".to_string()]);
        assert!(multi_occurrence("abc", "zz").is_empty());
        assert!(multi_occurrence("abc", "").is_empty());
    }
}
