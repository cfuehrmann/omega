//! `edit_file` — apply an ordered list of exact-match replacements.
//!
//! Each replacement must match exactly once in the file (at the point where
//! it is applied); ambiguous or missing matches are rejected with a helpful
//! error message.

use serde_json::Value;
use tokio_util::sync::CancellationToken;

pub async fn execute(input: Value, _cancel: Option<&CancellationToken>) -> Result<String, String> {
    let path = input["path"]
        .as_str()
        .ok_or("edit_file: path is required")?;

    let replacements = input["replacements"]
        .as_array()
        .ok_or("edit_file requires a non-empty replacements array.")?;

    if replacements.is_empty() {
        return Err("edit_file requires a non-empty replacements array.".into());
    }

    let mut content = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| format!("edit_file: {e}"))?;

    let total = replacements.len();
    let mut summaries: Vec<String> = Vec::with_capacity(total);

    for (i, rep) in replacements.iter().enumerate() {
        let label = if total > 1 {
            format!(" (replacement {}/{total})", i + 1)
        } else {
            String::new()
        };

        let old_text = rep["old_text"]
            .as_str()
            .ok_or_else(|| format!("edit_file: replacement {}/{total} missing old_text", i + 1))?;
        let new_text = rep["new_text"]
            .as_str()
            .ok_or_else(|| format!("edit_file: replacement {}/{total} missing new_text", i + 1))?;

        // Count byte-level occurrences matching the TypeScript indexOf+1 step.
        let count = count_occurrences(content.as_bytes(), old_text.as_bytes());

        match count {
            0 => {
                return Err(format!(
                    "old_text not found in {path}{label}. Make sure it matches exactly \
                     (including whitespace)."
                ));
            }
            1 => {} // proceed
            n => {
                return Err(format!(
                    "old_text found {n} times in {path}{label}. It must appear exactly once. \
                     Use a larger/more unique snippet."
                ));
            }
        }

        // Perform the first (and only) occurrence replacement.
        let pos = content.find(old_text).ok_or_else(|| {
            format!("edit_file: internal error – old_text disappeared in {path}{label}")
        })?;
        content.replace_range(pos..pos + old_text.len(), new_text);

        let old_lines = old_text.split('\n').count();
        let new_lines = new_text.split('\n').count();
        summaries.push(format!(
            "replaced {old_lines} line(s) with {new_lines} line(s)"
        ));
    }

    tokio::fs::write(path, &content)
        .await
        .map_err(|e| format!("edit_file: failed to write {path}: {e}"))?;

    if summaries.len() == 1 {
        Ok(format!("edit_file: {path} — {}", summaries[0]))
    } else {
        let lines_text: String = summaries
            .iter()
            .enumerate()
            .map(|(i, s)| format!("  {}. {s}", i + 1))
            .collect::<Vec<_>>()
            .join("\n");
        Ok(format!(
            "edit_file: {path} — {} replacements applied:\n{lines_text}",
            summaries.len()
        ))
    }
}

/// Count non-overlapping-from-start occurrences of `needle` in `haystack`,
/// advancing by 1 byte after each found position (matching TypeScript's
/// `indexOf(needle, pos + 1)` behaviour).  Returns early once count exceeds 1.
fn count_occurrences(haystack: &[u8], needle: &[u8]) -> usize {
    if needle.is_empty() {
        return 0;
    }
    let mut count = 0usize;
    let mut i = 0usize;
    while i + needle.len() <= haystack.len() {
        if haystack[i..].starts_with(needle) {
            count += 1;
            if count > 1 {
                break;
            }
            i += 1;
        } else {
            i += 1;
        }
    }
    count
}
