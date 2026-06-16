//! `edit_file` — replace a single snippet in a file.
//!
//! The model supplies `old_text` (the snippet to find) and `new_text` (its
//! replacement).  Matching is **exact** (see [`crate::tools::text_match`]); the
//! match must resolve to a single region unless `replace_all` is set.  When a
//! match fails we never guess — we return an error that explains why.

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::tools::text_match::{self, NotFoundHint, ReplaceError};

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
        .map_err(|e| format_replace_error("edit_file", e, path, "", &content, old_text))?;

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
/// suffix such as `" (edit 2/3)"` used by `multi_edit_file`.  `content` and
/// `old_text` are used to diagnose *why* a match failed (read-only).
pub(crate) fn format_replace_error(
    tool: &str,
    err: ReplaceError,
    path: &str,
    label: &str,
    content: &str,
    old_text: &str,
) -> String {
    match err {
        ReplaceError::Identical => format!(
            "{tool}: old_text and new_text are identical in {path}{label}; nothing to change."
        ),
        ReplaceError::Ambiguous { count } => {
            let lines = text_match::occurrence_lines(content, old_text);
            let where_ = lines
                .iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "{tool}: old_text matches {count} locations in {path}{label} (lines {where_}). \
                 Add surrounding context to make it unique, or pass \"replace_all\": true to \
                 change every occurrence."
            )
        }
        ReplaceError::NotFound => {
            let hint = match text_match::diagnose_not_found(content, old_text) {
                NotFoundHint::WhitespaceOnly { line } => format!(
                    " The text appears at line {line} but with different whitespace or \
                     indentation — copy it exactly as it appears in the file (whitespace is not \
                     forgiven)."
                ),
                NotFoundHint::LineEnding => " The file uses CRLF (Windows) line endings; \
                     re-read it and copy old_text with matching line endings."
                    .to_string(),
                NotFoundHint::None => " Re-read the file and copy the snippet exactly, \
                     including whitespace and indentation."
                    .to_string(),
            };
            format!("{tool}: old_text not found in {path}{label}.{hint}")
        }
    }
}
