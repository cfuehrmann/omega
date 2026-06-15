//! `multi_edit_file` — apply an ordered sequence of edits to a single file.
//!
//! Each edit is `{old_text, new_text, replace_all?}` and is applied to the
//! result of the previous one, sharing the same fuzzy matcher as
//! [`crate::tools::edit_file`].  The sequence is atomic: if any edit fails to
//! match, the file is left untouched and the failing edit is reported.

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::tools::edit_file::{format_replace_error, summarize};
use crate::tools::text_match;

pub async fn execute(input: Value, _cancel: Option<&CancellationToken>) -> Result<String, String> {
    let path = input["path"]
        .as_str()
        .ok_or("multi_edit_file: path is required")?;

    let edits = input["edits"]
        .as_array()
        .ok_or("multi_edit_file: edits must be a non-empty array of {old_text, new_text}")?;
    if edits.is_empty() {
        return Err(
            "multi_edit_file: edits must be a non-empty array of {old_text, new_text}".into(),
        );
    }

    let mut content = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| format!("multi_edit_file: {e}"))?;

    let total = edits.len();
    let mut summaries: Vec<String> = Vec::with_capacity(total);

    for (i, edit) in edits.iter().enumerate() {
        let label = format!(" (edit {}/{total})", i + 1);
        let old_text = edit["old_text"]
            .as_str()
            .ok_or_else(|| format!("multi_edit_file: edit {}/{total} missing old_text", i + 1))?;
        let new_text = edit["new_text"]
            .as_str()
            .ok_or_else(|| format!("multi_edit_file: edit {}/{total} missing new_text", i + 1))?;
        let replace_all = edit["replace_all"].as_bool().unwrap_or(false);

        let result = text_match::replace(&content, old_text, new_text, replace_all)
            .map_err(|e| format_replace_error("multi_edit_file", e, path, &label))?;
        content = result.content;
        summaries.push(summarize(old_text, new_text, result.count, replace_all));
    }

    tokio::fs::write(path, &content)
        .await
        .map_err(|e| format!("multi_edit_file: failed to write {path}: {e}"))?;

    let lines_text: String = summaries
        .iter()
        .enumerate()
        .map(|(i, s)| format!("  {}. {s}", i + 1))
        .collect::<Vec<_>>()
        .join("\n");
    Ok(format!(
        "multi_edit_file: {path} — {total} edit(s) applied:\n{lines_text}"
    ))
}
