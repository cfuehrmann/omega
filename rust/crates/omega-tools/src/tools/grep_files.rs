//! `grep_files` — search for a pattern across files using pure-Rust traversal
//! and regex matching.
//!
//! Uses [`ignore::WalkBuilder`] for directory walking (the same engine that
//! powers ripgrep), [`globset::Glob`] for file-extension filtering, and
//! [`regex::RegexBuilder`] for pattern matching.  No external binary
//! dependencies.
//!
//! Output format matches rg's `--no-heading --with-filename --line-number`
//! style:
//! - match lines:   `path:N:content`
//! - context lines: `path:N-content`
//! - group separator: `--`

use std::fmt::Write as _;
use std::path::Path;

use globset::{Glob, GlobMatcher};
use regex::Regex;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

const DEFAULT_CONTEXT: u64 = 2;
const DEFAULT_MAX_RESULTS: usize = 200;

pub async fn execute(input: Value, _cancel: Option<&CancellationToken>) -> Result<String, String> {
    let pattern = input["pattern"]
        .as_str()
        .ok_or("grep_files: pattern is required")?
        .to_owned();
    let path = input["path"]
        .as_str()
        .ok_or("grep_files: path is required")?
        .to_owned();
    let file_glob = input["file_glob"].as_str().map(ToOwned::to_owned);
    let context_lines = input["context_lines"].as_u64().unwrap_or(DEFAULT_CONTEXT);
    let case_sensitive = input["case_sensitive"].as_bool().unwrap_or(false);
    let max_results = input["max_results"]
        .as_u64()
        .map_or(DEFAULT_MAX_RESULTS, |n| {
            usize::try_from(n).unwrap_or(DEFAULT_MAX_RESULTS)
        });
    // context_lines comes from JSON and is meaningful only as a line count
    // (files don't have u64::MAX lines), so treat the value as a usize
    // directly, saturating at usize::MAX on hypothetical 32-bit targets.
    #[allow(clippy::cast_possible_truncation)]
    let context = context_lines.min(usize::MAX as u64) as usize;

    let (result_lines, truncated) = tokio::task::spawn_blocking(move || {
        search(
            &pattern,
            &path,
            file_glob.as_deref(),
            context,
            case_sensitive,
            max_results,
        )
    })
    .await
    .map_err(|e| format!("grep_files: task panicked: {e}"))??;

    if result_lines.is_empty() {
        return Ok("No matches found.".to_owned());
    }

    let joined = result_lines.join("\n");
    if truncated {
        let mut out = joined;
        // `write!` to a String is infallible.
        let _ = write!(
            out,
            "\n\n[truncated: showing {max_results} of more matches]"
        );
        return Ok(out);
    }

    Ok(joined)
}

// ---------------------------------------------------------------------------
// Pure-Rust search
// ---------------------------------------------------------------------------

fn search(
    pattern: &str,
    path: &str,
    file_glob: Option<&str>,
    context: usize,
    case_sensitive: bool,
    max: usize,
) -> Result<(Vec<String>, bool), String> {
    let re = regex::RegexBuilder::new(pattern)
        .case_insensitive(!case_sensitive)
        .build()
        .map_err(|e| format!("grep_files: invalid regex pattern: {e}"))?;

    let glob_matcher: Option<GlobMatcher> = file_glob
        .map(|g| {
            Glob::new(g)
                .map(|glob| glob.compile_matcher())
                .map_err(|e| format!("grep_files: invalid glob: {e}"))
        })
        .transpose()?;

    let mut results: Vec<String> = Vec::new();
    let mut truncated = false;

    for entry in ignore::WalkBuilder::new(path).build() {
        let Ok(entry) = entry else { continue };

        let Some(ft) = entry.file_type() else {
            continue;
        };
        if !ft.is_file() {
            continue;
        }

        // Apply glob filter against the basename.
        if let Some(ref gm) = glob_matcher
            && !gm.is_match(entry.file_name())
        {
            continue;
        }

        if search_file(entry.path(), &re, context, &mut results, max) {
            truncated = true;
            break;
        }
    }

    Ok((results, truncated))
}

/// Search a single file for `re`, appending formatted match/context lines to
/// `results`.  Returns `true` if `max` was reached (caller should stop).
///
/// Output format per line:
/// - match:   `<path>:<1-based-line-number>:<line-text>`
/// - context: `<path>:<1-based-line-number>-<line-text>`
///
/// A `--` separator is emitted between non-adjacent match groups.
fn search_file(
    path: &Path,
    re: &Regex,
    context: usize,
    results: &mut Vec<String>,
    max: usize,
) -> bool {
    // Non-UTF-8 files are silently skipped, matching rg's default behaviour.
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let lines: Vec<&str> = text.lines().collect();
    // `prev_end` tracks the exclusive end of the last-written context window
    // so we can detect non-adjacent groups and merge overlapping ones.
    let mut prev_end: Option<usize> = None;

    for (i, line) in lines.iter().enumerate() {
        if !re.is_match(line) {
            continue;
        }

        let want_start = i.saturating_sub(context);
        let want_end = (i + context + 1).min(lines.len());

        // Determine where to begin writing for this group.
        let actual_start = if let Some(pe) = prev_end {
            if want_start > pe {
                // Gap between this group and the previous one.
                results.push("--".into());
                want_start
            } else {
                // Adjacent or overlapping: skip lines already written.
                pe
            }
        } else {
            want_start
        };

        for (j, line_text) in lines.iter().enumerate().take(want_end).skip(actual_start) {
            let lnum = j + 1;
            let sep = if j == i { ':' } else { '-' };
            results.push(format!("{}:{lnum}{sep}{line_text}", path.display()));
            if results.len() >= max {
                return true;
            }
        }
        prev_end = Some(want_end);
    }
    false
}
