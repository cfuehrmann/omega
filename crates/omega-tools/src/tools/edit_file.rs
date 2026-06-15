//! `edit_file` — replace a single snippet in a file.
//!
//! The model supplies `old_text` (the snippet to find) and `new_text` (its
//! replacement).  Matching goes through the shared fuzzy cascade in
//! [`crate::tools::text_match`], which tolerates whitespace, indentation and
//! escaping drift while still requiring the match to resolve to a single
//! region (unless `replace_all` is set).

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::tools::text_match::{self, ReplaceError};

pub async fn execute(input: Value, _cancel: Option<&CancellationToken>) -> Result<String, String> {
    let path = input["path"]
        .as_str()
        .ok_or("edit_file: path is required")?;
    let old_text = input["old_text"]
        .as_str()
        .ok_or("edit_file: old_text is required")?;
    let new_text = input["new_text"]
        .as_str()
        .ok_or("edit_file: new_text is required")?;
    let replace_all = input["replace_all"].as_bool().unwrap_or(false);

    let content = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| format!("edit_file: {e}"))?;

    let result = text_match::replace(&content, old_text, new_text, replace_all)
        .map_err(|e| format_replace_error("edit_file", e, path, ""))?;

    tokio::fs::write(path, &result.content)
        .await
        .map_err(|e| format!("edit_file: failed to write {path}: {e}"))?;

    Ok(format!(
        "edit_file: {path} — {}",
        summarize(old_text, new_text, result.count, replace_all)
    ))
}

/// Human-readable summary of one applied replacement.
pub(crate) fn summarize(old_text: &str, new_text: &str, count: usize, replace_all: bool) -> String {
    let old_lines = old_text.split('\n').count();
    let new_lines = new_text.split('\n').count();
    if replace_all && count != 1 {
        format!("replaced {old_lines} line(s) with {new_lines} line(s) at {count} occurrences")
    } else {
        format!("replaced {old_lines} line(s) with {new_lines} line(s)")
    }
}

/// Map a [`ReplaceError`] to an actionable message.  `label` is an optional
/// suffix such as `" (edit 2/3)"` used by `multi_edit_file`.
pub(crate) fn format_replace_error(
    tool: &str,
    err: ReplaceError,
    path: &str,
    label: &str,
) -> String {
    match err {
        ReplaceError::Identical => format!(
            "{tool}: old_text and new_text are identical in {path}{label}; nothing to change."
        ),
        ReplaceError::NotFound => format!(
            "{tool}: old_text not found in {path}{label}. Whitespace and indentation drift is \
             tolerated, but the snippet must be present — copy it from the current file contents."
        ),
        ReplaceError::Ambiguous => format!(
            "{tool}: old_text matches multiple locations in {path}{label}. Provide a larger, \
             unique snippet, or pass \"replace_all\": true to change every occurrence."
        ),
    }
}
