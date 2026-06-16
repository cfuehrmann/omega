//! `edit_file` — replace a single snippet in a file.
//!
//! The model supplies `old_text` (the snippet to find) and `new_text` (its
//! replacement).  Matching is **exact** (see [`crate::tools::text_match`]); the
//! match must resolve to a single region unless `replace_all` is set.  When a
//! match fails we never guess — we return an error that explains why.

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use omega_types::OmegaEvent;
use omega_types::events::EditFailedSnapshotEvent;

use crate::tools::text_match::{self, Edit, NotFoundHint, PlanError};
use crate::{ToolCtx, ToolResult};

pub async fn execute(
    input: Value,
    _cancel: Option<&CancellationToken>,
    ctx: Option<&ToolCtx>,
) -> ToolResult {
    let Some(path) = input["path"].as_str() else {
        return ToolResult::err("edit_file: path is required");
    };
    let Some(old_text) = input["old_text"].as_str() else {
        return ToolResult::err("edit_file: old_text is required");
    };
    let Some(new_text) = input["new_text"].as_str() else {
        return ToolResult::err("edit_file: new_text is required");
    };
    let replace_all = input["replace_all"].as_bool().unwrap_or(false);

    let content = match tokio::fs::read_to_string(path).await {
        Ok(c) => c,
        Err(e) => return ToolResult::err(format!("edit_file: {e}")),
    };

    // A single edit is just the one-element case of the shared planner.
    let edits = [Edit {
        old: old_text,
        new: new_text,
        replace_all,
    }];
    match text_match::apply_edits(&content, &edits) {
        Ok(planned) => {
            if let Err(e) = tokio::fs::write(path, &planned.content).await {
                return ToolResult::err(format!("edit_file: failed to write {path}: {e}"));
            }
            ToolResult::ok(format!(
                "edit_file: {path} — {}",
                summarize(old_text, new_text, planned.count, replace_all)
            ))
        }
        // Match failure: report why, and capture the file as it is on disk
        // for forensics (the single edit_file read *is* the on-disk content).
        Err(e) => {
            let message = format_plan_error("edit_file", e, path, &content, &edits);
            let index = u32::try_from(e.edit() + 1).unwrap_or(u32::MAX);
            match_failure_result(message, ctx, path, &content, index, 1)
        }
    }
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

/// Cap (in bytes) on the file contents stored in an [`EditFailedSnapshotEvent`].
/// The full size and hash are always recorded; only `content` is truncated.
pub(crate) const SNAPSHOT_CAP_BYTES: usize = 512 * 1024;

/// Build a [`ToolResult::err`] for a failed match, attaching an
/// [`OmegaEvent::EditFailedSnapshot`] (when a session context is present) so
/// the file as it was on disk at the attempt is preserved for forensics.
///
/// `disk_content` is the file as freshly read from disk; `failed_edit_index`
/// and `edit_count` are `1`/`1` for `edit_file` and locate the failing edit
/// within a `multi_edit_file` batch.
pub(crate) fn match_failure_result(
    message: String,
    ctx: Option<&ToolCtx>,
    path: &str,
    disk_content: &str,
    failed_edit_index: u32,
    edit_count: u32,
) -> ToolResult {
    let mut res = ToolResult::err(message);
    if let Some(ctx) = ctx {
        res.extra_events.push(snapshot_event(
            ctx,
            path,
            disk_content,
            failed_edit_index,
            edit_count,
        ));
    }
    res
}

fn snapshot_event(
    ctx: &ToolCtx,
    path: &str,
    disk_content: &str,
    failed_edit_index: u32,
    edit_count: u32,
) -> OmegaEvent {
    let byte_len = disk_content.len() as u64;
    let content_sha256 = sha256_hex(disk_content);
    let (content, truncated) = cap_at_char_boundary(disk_content, SNAPSHOT_CAP_BYTES);
    OmegaEvent::EditFailedSnapshot(EditFailedSnapshotEvent {
        time: crate::monitors::now_iso(),
        tool_call_id: ctx.tool_call_id.clone(),
        path: path.to_owned(),
        content,
        truncated,
        byte_len,
        content_sha256,
        failed_edit_index,
        edit_count,
    })
}

/// Lowercase hex SHA-256 of `s`.
fn sha256_hex(s: &str) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    hasher.finalize().iter().fold(String::new(), |mut acc, b| {
        let _ = write!(acc, "{b:02x}");
        acc
    })
}

/// Truncate `s` to at most `cap` bytes without splitting a UTF-8 character.
/// Returns the (possibly shortened) string and whether truncation occurred.
fn cap_at_char_boundary(s: &str, cap: usize) -> (String, bool) {
    if s.len() <= cap {
        return (s.to_owned(), false);
    }
    // Back off to the nearest char boundary at or below `cap`. A UTF-8
    // character is at most 4 bytes, so this needs at most 3 steps; the
    // bounded loop keeps it timeout-proof under mutation testing.
    let mut end = cap;
    for _ in 0..4 {
        if s.is_char_boundary(end) {
            break;
        }
        end -= 1;
    }
    (s[..end].to_owned(), true)
}

/// Map a [`PlanError`] to an actionable message, shared by `edit_file` and
/// `multi_edit_file`.  For a batch (more than one edit) each message names the
/// offending edit as `(edit i/n)`.  `content` and the edits' `old` text are
/// used to diagnose *why* a match failed (read-only).
pub(crate) fn format_plan_error(
    tool: &str,
    err: PlanError,
    path: &str,
    content: &str,
    edits: &[Edit],
) -> String {
    // Suffix locating an edit within a batch; empty for a single edit.
    let at = |i: usize| -> String {
        if edits.len() > 1 {
            format!(" (edit {}/{})", i + 1, edits.len())
        } else {
            String::new()
        }
    };
    match err {
        PlanError::Identical { edit } => format!(
            "{tool}: old_text and new_text are identical in {path}{}; nothing to change.",
            at(edit)
        ),
        PlanError::Ambiguous { edit, count } => {
            let lines = text_match::occurrence_lines(content, edits[edit].old);
            let where_ = lines
                .iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "{tool}: old_text matches {count} locations in {path}{} (lines {where_}). \
                 Add surrounding context to make it unique, or pass \"replace_all\": true to \
                 change every occurrence.",
                at(edit)
            )
        }
        PlanError::NotFound { edit } => {
            let hint = match text_match::diagnose_not_found(content, edits[edit].old) {
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
            format!("{tool}: old_text not found in {path}{}.{hint}", at(edit))
        }
        // The totality of matches is not disjoint.
        PlanError::Overlap {
            edit_a,
            edit_b,
            line,
        } => {
            if edit_a == edit_b {
                // One edit's own matches overlap (only possible with replace_all).
                format!(
                    "{tool}: the matches of old_text in {path}{} overlap at line {line} \
                     (e.g. \"aa\" within \"aaa\"); they are not disjoint, so they cannot all be \
                     replaced. Use a longer snippet that does not overlap itself.",
                    at(edit_a)
                )
            } else {
                // Two different edits target the same text.
                format!(
                    "{tool}: edit {}/{} and edit {}/{} target overlapping text at line {line} in \
                     {path}; edits must apply to disjoint regions. Merge them into one edit, or \
                     target distinct text.",
                    edit_a + 1,
                    edits.len(),
                    edit_b + 1,
                    edits.len()
                )
            }
        }
    }
}

// Carve-out: `cap_at_char_boundary` is a pure helper; a direct unit test is the
// right fit (an `execute_tool` round-trip can't cheaply pin the multi-byte
// back-off, which needs a tiny cap landing mid-character).
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::cap_at_char_boundary;

    #[test]
    fn no_truncation_when_within_cap() {
        assert_eq!(
            cap_at_char_boundary("hello", 100),
            ("hello".to_owned(), false)
        );
        // Exact fit (len == cap) must NOT truncate.
        assert_eq!(
            cap_at_char_boundary("hello", 5),
            ("hello".to_owned(), false)
        );
        assert_eq!(cap_at_char_boundary("hi", 5), ("hi".to_owned(), false));
    }

    #[test]
    fn truncates_ascii_at_exact_cap() {
        assert_eq!(cap_at_char_boundary("hello", 3), ("hel".to_owned(), true));
    }

    #[test]
    fn backs_off_when_cap_splits_a_two_byte_char() {
        // "aé" = [a, 0xC3, 0xA9]; cap=2 lands inside 'é' -> back off to 1.
        assert_eq!(cap_at_char_boundary("aé", 2), ("a".to_owned(), true));
        // cap on an exact boundary keeps the whole char.
        assert_eq!(cap_at_char_boundary("aéb", 3), ("aé".to_owned(), true));
    }

    #[test]
    fn backs_off_when_cap_splits_a_three_byte_char() {
        // "a世" = [a, then 3 bytes]; len 4. cap=2 and cap=3 both split '世'.
        assert_eq!(cap_at_char_boundary("a世", 2), ("a".to_owned(), true));
        assert_eq!(cap_at_char_boundary("a世", 3), ("a".to_owned(), true));
    }
}
