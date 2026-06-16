//! `multi_edit_file` — apply several edits to a single file in one call.
//!
//! Each edit is `{old_text, new_text, replace_all?}`.  All edits are matched
//! against the **original** file contents (in parallel, not sequentially) and
//! their target regions must be pairwise disjoint, so one edit never sees
//! another's output and two edits aimed at the same text are reported as a
//! conflict up front.  The batch is atomic: if any edit fails, the file is
//! left untouched.  Matching and the disjointness rule are shared with
//! [`crate::tools::edit_file`] via [`text_match::apply_edits`].

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::ToolResult;
use crate::tools::edit_file::{format_plan_error, match_failure_result};
use crate::tools::text_match::{self, Edit};

pub async fn execute(
    input: Value,
    _cancel: Option<&CancellationToken>,
    ctx: Option<&crate::ToolCtx>,
) -> ToolResult {
    let Some(path) = input["path"].as_str() else {
        return ToolResult::err("multi_edit_file: path is required");
    };

    let Some(edits_json) = input["edits"].as_array() else {
        return ToolResult::err(
            "multi_edit_file: edits must be a non-empty array of {old_text, new_text}",
        );
    };
    if edits_json.is_empty() {
        return ToolResult::err(
            "multi_edit_file: edits must be a non-empty array of {old_text, new_text}",
        );
    }

    // Parse the JSON edits into the shared `Edit` shape (borrowing from `input`).
    let total = edits_json.len();
    let mut edits: Vec<Edit> = Vec::with_capacity(total);
    for (i, e) in edits_json.iter().enumerate() {
        let Some(old) = e["old_text"].as_str() else {
            return ToolResult::err(format!(
                "multi_edit_file: edit {}/{total} missing old_text",
                i + 1
            ));
        };
        let Some(new) = e["new_text"].as_str() else {
            return ToolResult::err(format!(
                "multi_edit_file: edit {}/{total} missing new_text",
                i + 1
            ));
        };
        edits.push(Edit {
            old,
            new,
            replace_all: e["replace_all"].as_bool().unwrap_or(false),
        });
    }

    let content = match tokio::fs::read_to_string(path).await {
        Ok(c) => c,
        Err(e) => return ToolResult::err(format!("multi_edit_file: {e}")),
    };

    match text_match::apply_edits(&content, &edits) {
        Ok(planned) => {
            if let Err(e) = tokio::fs::write(path, &planned.content).await {
                return ToolResult::err(format!("multi_edit_file: failed to write {path}: {e}"));
            }
            let listing = edits
                .iter()
                .enumerate()
                .map(|(i, e)| {
                    let ol = e.old.split('\n').count();
                    let nl = e.new.split('\n').count();
                    let all = if e.replace_all {
                        " (all occurrences)"
                    } else {
                        ""
                    };
                    format!("  {}. replaced {ol} line(s) with {nl} line(s){all}", i + 1)
                })
                .collect::<Vec<_>>()
                .join("\n");
            ToolResult::ok(format!(
                "multi_edit_file: {path} — {total} edit(s) applied ({} replacement(s)):\n{listing}",
                planned.count
            ))
        }
        // Nothing was written (atomic), so `content` is exactly the on-disk
        // file at the failed attempt — capture it for forensics.
        Err(err) => {
            let message = format_plan_error("multi_edit_file", err, path, &content, &edits);
            let index = u32::try_from(err.edit() + 1).unwrap_or(u32::MAX);
            let total_u32 = u32::try_from(total).unwrap_or(u32::MAX);
            match_failure_result(message, ctx, path, &content, index, total_u32)
        }
    }
}
