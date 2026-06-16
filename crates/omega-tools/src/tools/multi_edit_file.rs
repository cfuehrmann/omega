//! `multi_edit_file` — apply an ordered sequence of edits to a single file.
//!
//! Each edit is `{old_text, new_text, replace_all?}` and is applied to the
//! result of the previous one, sharing the same exact matcher as
//! [`crate::tools::edit_file`].  The sequence is atomic: if any edit fails to
//! match, the file is left untouched and the failing edit is reported.

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::ToolResult;
use crate::tools::edit_file::{format_replace_error, match_failure_result, summarize};
use crate::tools::text_match;

pub async fn execute(
    input: Value,
    _cancel: Option<&CancellationToken>,
    ctx: Option<&crate::ToolCtx>,
) -> ToolResult {
    let Some(path) = input["path"].as_str() else {
        return ToolResult::err("multi_edit_file: path is required");
    };

    let Some(edits) = input["edits"].as_array() else {
        return ToolResult::err(
            "multi_edit_file: edits must be a non-empty array of {old_text, new_text}",
        );
    };
    if edits.is_empty() {
        return ToolResult::err(
            "multi_edit_file: edits must be a non-empty array of {old_text, new_text}",
        );
    }

    let mut content = match tokio::fs::read_to_string(path).await {
        Ok(c) => c,
        Err(e) => return ToolResult::err(format!("multi_edit_file: {e}")),
    };

    let total = edits.len();
    let count_u32 = u32::try_from(total).unwrap_or(u32::MAX);
    let mut summaries: Vec<String> = Vec::with_capacity(total);

    for (i, edit) in edits.iter().enumerate() {
        let label = format!(" (edit {}/{total})", i + 1);
        let Some(old_text) = edit["old_text"].as_str() else {
            return ToolResult::err(format!(
                "multi_edit_file: edit {}/{total} missing old_text",
                i + 1
            ));
        };
        let Some(new_text) = edit["new_text"].as_str() else {
            return ToolResult::err(format!(
                "multi_edit_file: edit {}/{total} missing new_text",
                i + 1
            ));
        };
        let replace_all = edit["replace_all"].as_bool().unwrap_or(false);

        match text_match::replace(&content, old_text, new_text, replace_all) {
            Ok(result) => {
                content = result.content;
                summaries.push(summarize(old_text, new_text, result.count, replace_all));
            }
            // Match failure aborts the whole batch (atomic: nothing is
            // written).  The file on disk is therefore the untouched
            // original; re-read it for the snapshot so we record exactly
            // what is on disk at the failed attempt.
            Err(e) => {
                let message =
                    format_replace_error("multi_edit_file", e, path, &label, &content, old_text);
                let on_disk = tokio::fs::read_to_string(path)
                    .await
                    .unwrap_or_else(|_| content.clone());
                let index = u32::try_from(i + 1).unwrap_or(u32::MAX);
                return match_failure_result(message, ctx, path, &on_disk, index, count_u32);
            }
        }
    }

    if let Err(e) = tokio::fs::write(path, &content).await {
        return ToolResult::err(format!("multi_edit_file: failed to write {path}: {e}"));
    }

    let lines_text: String = summaries
        .iter()
        .enumerate()
        .map(|(i, s)| format!("  {}. {s}", i + 1))
        .collect::<Vec<_>>()
        .join("\n");
    ToolResult::ok(format!(
        "multi_edit_file: {path} — {total} edit(s) applied:\n{lines_text}"
    ))
}
