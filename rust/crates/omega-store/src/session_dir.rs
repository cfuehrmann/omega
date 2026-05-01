//! Session directory management.
//!
//! Each Omega session gets its own timestamped folder under a configurable
//! root (default `.omega/sessions/`) in the current working directory:
//!
//! ```text
//! .omega/sessions/2025-07-04T14-32-05-123-a3f7c1b2/
//!   context.jsonl
//!   events.jsonl
//!   session.jsonc
//! ```
//!
//! The folder name is an ISO 8601 datetime at millisecond precision (colons
//! and the decimal point replaced with hyphens for filesystem safety),
//! followed by a hyphen and an 8-char random hex suffix.  `ls`-by-name
//! ordering matches chronological ordering.

use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};

use chrono::{DateTime, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::Result;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Root directory for all session folders (relative to cwd).
pub const SESSIONS_ROOT: &str = ".omega/sessions";

/// Name of the metadata file inside every session folder.
const SESSION_METADATA_FILE: &str = "session.jsonc";

// ---------------------------------------------------------------------------
// Session directory name
// ---------------------------------------------------------------------------

static SESSION_DIR_RE: OnceLock<Regex> = OnceLock::new();

/// Return a compiled `Regex` that matches all three historical session-dir
/// name formats:
///
/// - `YYYY-MM-DDTHH-MM-SS`              — legacy, second precision
/// - `YYYY-MM-DDTHH-MM-SS-<hex8>`       — v2, second + suffix
/// - `YYYY-MM-DDTHH-MM-SS-mmm-<hex8>`   — current, millisecond + suffix
///
/// # Panics
///
/// Never panics — the regex pattern is a validated compile-time constant.
#[must_use]
pub fn session_dir_re() -> &'static Regex {
    SESSION_DIR_RE.get_or_init(|| {
        // Infallible: pattern is a validated compile-time constant.
        #[allow(clippy::unwrap_used)]
        Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}(-\d{3})?(-[0-9a-f]{8})?$").unwrap()
    })
}

/// Generate a session folder name from a UTC timestamp at millisecond
/// precision, plus a random 8-char hex suffix.
///
/// Format: `YYYY-MM-DDTHH-MM-SS-mmm-<hex8>`
#[must_use]
pub fn make_session_dir_name(now: DateTime<Utc>) -> String {
    let ts = now.format("%Y-%m-%dT%H-%M-%S").to_string();
    let ms = now.timestamp_subsec_millis();
    let suffix: [u8; 4] = rand::random();
    let hex = suffix.iter().fold(String::with_capacity(8), |mut s, b| {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
        s
    });
    format!("{ts}-{ms:03}-{hex}")
}

// ---------------------------------------------------------------------------
// Session paths
// ---------------------------------------------------------------------------

/// Paths to the key files inside a session directory.
pub struct SessionPaths {
    /// Path to the session directory itself.
    pub dir: PathBuf,
    /// Path to `context.jsonl` inside the session dir.
    pub context_file: PathBuf,
    /// Path to `events.jsonl` inside the session dir.
    pub events_file: PathBuf,
}

/// Create a new session directory under `root` and return its paths.
///
/// Creates `<root>/<name>/` plus empty `context.jsonl`, `events.jsonl`, and
/// `session.jsonc` (containing `{}`).
///
/// # Errors
///
/// Returns an error if any filesystem operation fails.
pub async fn make_session_dir(root: &Path) -> Result<SessionPaths> {
    let name = make_session_dir_name(Utc::now());
    let dir = root.join(name);
    tokio::fs::create_dir_all(&dir).await?;

    let context_file = dir.join("context.jsonl");
    let events_file = dir.join("events.jsonl");
    let metadata_file = dir.join(SESSION_METADATA_FILE);

    // Create all three files eagerly so the session directory is complete
    // from birth.  write() truncates, but the dir is brand-new so no data
    // is lost.
    tokio::fs::write(&context_file, b"").await?;
    tokio::fs::write(&events_file, b"").await?;
    tokio::fs::write(&metadata_file, b"{}\n").await?;

    Ok(SessionPaths {
        dir,
        context_file,
        events_file,
    })
}

// ---------------------------------------------------------------------------
// Session metadata
// ---------------------------------------------------------------------------

/// Metadata for a session.  All fields are optional.
///
/// Written as JSONC so humans can add comments when editing manually.
/// Programmatic writes use plain JSON (a valid subset of JSONC).
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionMetadata {
    /// Short human-readable label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Free-text description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Relative folder name of the session this one resumes.
    /// `continuationOf` is accepted as a legacy alias.
    #[serde(alias = "continuationOf", skip_serializing_if = "Option::is_none")]
    pub resumed_from: Option<String>,
}

/// Read session metadata from `session.jsonc` inside `dir`.
///
/// Returns [`SessionMetadata::default`] if the file is absent or unparseable.
pub async fn read_session_metadata(dir: &Path) -> SessionMetadata {
    let path = dir.join(SESSION_METADATA_FILE);
    let Ok(raw) = tokio::fs::read_to_string(&path).await else {
        return SessionMetadata::default();
    };
    let stripped = strip_jsonc_comments(&raw);
    serde_json::from_str(&stripped).unwrap_or_default()
}

/// Write (overwrite) session metadata to `session.jsonc` inside `dir`.
///
/// Only fields that are `Some` are written; `None` fields are omitted.
///
/// # Errors
///
/// Returns an error if serialisation or the file write fails.
pub async fn write_session_metadata(dir: &Path, meta: &SessionMetadata) -> Result<()> {
    let json = serde_json::to_string_pretty(meta)?;
    tokio::fs::write(dir.join(SESSION_METADATA_FILE), format!("{json}\n")).await?;
    Ok(())
}

/// Merge `patch` into the existing metadata for `dir`.
///
/// `None` patch fields leave the existing value unchanged; `Some` patch
/// fields overwrite it.
///
/// # Errors
///
/// Returns an error if the write fails.
pub async fn update_session_metadata(dir: &Path, patch: SessionMetadata) -> Result<()> {
    let mut existing = read_session_metadata(dir).await;
    if patch.name.is_some() {
        existing.name = patch.name;
    }
    if patch.description.is_some() {
        existing.description = patch.description;
    }
    if patch.resumed_from.is_some() {
        existing.resumed_from = patch.resumed_from;
    }
    write_session_metadata(dir, &existing).await
}

// ---------------------------------------------------------------------------
// JSONC comment stripping
// ---------------------------------------------------------------------------

/// Strip `// …` single-line comments and `/* … */` block comments from
/// `text`, returning valid JSON.
fn strip_jsonc_comments(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;

    while i < len {
        // Invariant: i < len.  The debug_assert makes mutation `< → <=`
        // observable: the extra iteration (i == len) would panic in debug mode.
        debug_assert!(i < len, "outer loop invariant violated");
        // Single-line comment: skip to end of line (keep the newline).
        if i + 1 < len && bytes[i] == b'/' && bytes[i + 1] == b'/' {
            i += 2;
            while i < len && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        // Block comment: skip to closing `*/`.
        if i + 1 < len && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < len && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i += 2; // consume `*/`
            continue;
        }
        // Safety: we index into a &str by byte position.  We verify that
        // each byte we push via `as_bytes()[i]` is valid UTF-8 by checking
        // only ASCII delimiters (/, *, \n) above; non-ASCII multi-byte
        // sequences pass through the else branch unchanged via `chars`.
        //
        // Re-encode character-by-character to handle multi-byte UTF-8
        // correctly without unsafe indexing.
        if let Some(ch) = text[i..].chars().next() {
            result.push(ch);
            i += ch.len_utf8();
        } else {
            break;
        }
    }

    result
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn session_dir_name_matches_re() {
        let now = Utc::now();
        let name = make_session_dir_name(now);
        assert!(
            session_dir_re().is_match(&name),
            "name={name:?} did not match regex"
        );
    }

    #[test]
    fn session_dir_re_matches_legacy_formats() {
        let re = session_dir_re();
        // Legacy (second precision, no suffix)
        assert!(re.is_match("2025-07-04T14-32-05"));
        // v2 (second + suffix)
        assert!(re.is_match("2025-07-04T14-32-05-a3f7c1b2"));
        // Current (millisecond + suffix)
        assert!(re.is_match("2025-07-04T14-32-05-123-a3f7c1b2"));
        // Should not match random strings
        assert!(!re.is_match("not-a-session-dir"));
    }

    #[test]
    fn strip_jsonc_single_line_comment() {
        let input = "{ // comment\n\"key\": 1 }";
        let output = strip_jsonc_comments(input);
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["key"], 1);
    }

    #[test]
    fn strip_jsonc_block_comment() {
        let input = "{ /* block\ncomment */ \"key\": 2 }";
        let output = strip_jsonc_comments(input);
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["key"], 2);
    }

    // -----------------------------------------------------------------------
    // Boundary / mutation-killing tests for strip_jsonc_comments.
    // Each comment explains which specific mutant(s) it targets.
    // -----------------------------------------------------------------------

    #[test]
    fn strip_jsonc_lone_slash_at_end_passes_through() {
        // Targets lines 210 and 218: `< to <=` and `+ to *` in the `i + 1 < len`
        // guard.  With either mutation the last `/` causes bytes[len] (OOB).
        let output = strip_jsonc_comments("42/");
        assert_eq!(
            output, "42/",
            "trailing lone slash must pass through unchanged"
        );
    }

    #[test]
    fn strip_jsonc_single_line_comment_no_trailing_newline() {
        // Targets line 212: `< to <=` in the inner `while i < len` loop.
        // Without a trailing newline the loop reaches i == len; the mutation
        // would access bytes[len] (OOB panic).
        let output = strip_jsonc_comments("{}//trailing comment, no newline");
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed, serde_json::json!({}));
    }

    #[test]
    fn strip_jsonc_single_line_comment_at_position_zero() {
        // Targets line 211: `+= to -=`.  `//` at position 0 makes i = 0,
        // so `i -= 2` underflows (usize arithmetic panic in debug mode).
        let output = strip_jsonc_comments("// leading comment\n{}");
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed, serde_json::json!({}));
    }

    #[test]
    fn strip_jsonc_single_line_comment_past_midpoint() {
        // Targets line 211: `+= to *=`.  `//` at position 6; 6*2 = 12 > 11
        // (the newline).  The mutation causes the inner loop to start past
        // the newline, consuming `}` and leaving invalid JSON.
        let output = strip_jsonc_comments("{\"a\":1// xx\n}");
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["a"], 1);
    }

    #[test]
    fn strip_jsonc_block_comment_at_position_zero() {
        // Targets line 219: `+= to -=`.  `/*` at position 0 makes i = 0,
        // so `i -= 2` underflows (usize arithmetic panic in debug mode).
        let output = strip_jsonc_comments("/* leading */ {}");
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed, serde_json::json!({}));
    }

    #[test]
    fn strip_jsonc_block_comment_past_midpoint() {
        // Targets line 219: `+= to *=`.  `/*` at position 6; 6*2 = 12 > 10
        // (the `*` of `*/`).  The mutation skips past `*/`, consuming `}` and
        // leaving invalid JSON.
        let output = strip_jsonc_comments("{\"a\":1/* x*/}");
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["a"], 1);
    }

    #[test]
    fn strip_jsonc_block_comment_star_not_followed_by_slash() {
        // Targets line 223 col 53: inner `&&` to `||` mutation.
        // With `||`, ANY `*` (not just `*/`) would close the block comment,
        // leaking ` b */2` into the output and producing invalid JSON.
        let output = strip_jsonc_comments("{\"k\":/* a* b */2}");
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["k"], 2);
    }

    #[test]
    fn strip_jsonc_unclosed_block_comment_kills_boundary_mutants() {
        // Targets all four block-comment inner-loop mutations
        // (< to <=, + to -, + to *, && to ||).
        //
        // The comment ends with `*` so that the last inner-loop iteration
        // (i = len - 1) has bytes[i] == b'*', making the second operand
        // `bytes[i + 1]` non-trivially evaluated.  Each mutation widens the
        // loop condition just enough to attempt bytes[len] (OOB panic).
        let output = strip_jsonc_comments("{\"k\":2}/* close*");
        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["k"], 2);
    }
}
