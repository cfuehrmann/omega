//! `find_files` — find files/directories by name/glob using pure-Rust traversal.
//!
//! Uses [`ignore::WalkBuilder`] for directory walking (the same engine that
//! powers ripgrep) and [`globset::Glob`] for pattern matching.  No external
//! binary dependencies.

use std::fmt::Write as _;

use globset::Glob;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

const DEFAULT_MAX_RESULTS: usize = 200;

pub async fn execute(input: Value, _cancel: Option<&CancellationToken>) -> Result<String, String> {
    let pattern = input["pattern"]
        .as_str()
        .ok_or("find_files: pattern is required")?
        .to_owned();
    let path = input["path"]
        .as_str()
        .ok_or("find_files: path is required")?
        .to_owned();
    let type_filter = input["type"].as_str().map(ToOwned::to_owned);
    let hidden = input["hidden"].as_bool().unwrap_or(false);
    let max_results = input["max_results"]
        .as_u64()
        .map_or(DEFAULT_MAX_RESULTS, |n| {
            usize::try_from(n).unwrap_or(DEFAULT_MAX_RESULTS)
        });

    let mut lines =
        tokio::task::spawn_blocking(move || walk(&pattern, &path, type_filter.as_deref(), hidden))
            .await
            .map_err(|e| format!("find_files: task panicked: {e}"))??;

    if lines.is_empty() {
        return Ok("No files found.".to_owned());
    }

    let truncated = lines.len() > max_results;
    if truncated {
        lines.truncate(max_results);
    }

    let mut result = lines.join("\n");
    if truncated {
        // `write!` to a String is infallible; the `Err` variant is unreachable.
        let _ = write!(result, "\n\n[Truncated at {max_results} results]");
    }
    Ok(result)
}

fn walk(
    pattern: &str,
    path: &str,
    type_filter: Option<&str>,
    hidden: bool,
) -> Result<Vec<String>, String> {
    let matcher = Glob::new(pattern)
        .map_err(|e| format!("find_files: invalid glob pattern '{pattern}': {e}"))?
        .compile_matcher();

    let mut builder = ignore::WalkBuilder::new(path);
    if hidden {
        // Include dotfiles and disable .gitignore / .ignore rules (mirrors
        // fd's `--hidden --no-ignore` flags).
        builder.hidden(false).git_ignore(false).ignore(false);
    } else {
        // Default: skip dotfiles, respect .gitignore (the WalkBuilder default,
        // made explicit here to match fd's default behaviour).
        builder.hidden(true);
    }

    let mut results: Vec<String> = Vec::new();

    for entry in builder.build() {
        let Ok(entry) = entry else { continue };

        // Depth 0 is the root path itself — skip it.
        if entry.depth() == 0 {
            continue;
        }

        let Some(ft) = entry.file_type() else {
            continue;
        };

        // Apply the optional type filter.
        match type_filter {
            Some("f") if !ft.is_file() => continue,
            Some("d") if !ft.is_dir() => continue,
            Some("l") if !ft.is_symlink() => continue,
            _ => {}
        }

        // Match the glob against the basename only, following fd semantics.
        if !matcher.is_match(entry.file_name()) {
            continue;
        }

        results.push(entry.path().display().to_string());
    }

    Ok(results)
}
