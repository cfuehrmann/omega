#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Integration tests for `session_dir` — real file I/O via temp directories.

use omega_store::{
    SessionMetadata, make_session_dir, read_session_metadata, session_dir_re,
    update_session_metadata, write_session_metadata,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Create a fresh temporary directory for the test.  The returned
/// `TempDir` guard must be kept alive for the duration of the test.
fn temp_root() -> tempfile::TempDir {
    #[allow(clippy::unwrap_used)]
    tempfile::tempdir().unwrap()
}

// ---------------------------------------------------------------------------
// make_session_dir
// ---------------------------------------------------------------------------

#[tokio::test]
async fn make_session_dir_creates_expected_files() {
    let root = temp_root();
    #[allow(clippy::unwrap_used)]
    let paths = make_session_dir(root.path()).await.unwrap();

    // The session directory itself must exist.
    assert!(paths.dir.is_dir(), "session dir not created");

    // All three sibling files must exist.
    assert!(paths.events_file.exists(), "events.jsonl missing");
    assert!(paths.context_file.exists(), "context.jsonl missing");
    assert!(
        paths.dir.join("session.jsonc").exists(),
        "session.jsonc missing"
    );
}

#[tokio::test]
async fn make_session_dir_name_matches_regex() {
    let root = temp_root();
    #[allow(clippy::unwrap_used)]
    let paths = make_session_dir(root.path()).await.unwrap();

    let dir_name = paths
        .dir
        .file_name()
        .and_then(|n| n.to_str())
        .expect("session dir has a name");

    assert!(
        session_dir_re().is_match(dir_name),
        "dir name {dir_name:?} did not match session_dir_re"
    );
}

#[tokio::test]
async fn make_session_dir_creates_empty_jsonl_files() {
    let root = temp_root();
    #[allow(clippy::unwrap_used)]
    let paths = make_session_dir(root.path()).await.unwrap();

    let events_content = std::fs::read_to_string(&paths.events_file).unwrap();
    let context_content = std::fs::read_to_string(&paths.context_file).unwrap();
    assert!(events_content.is_empty(), "events.jsonl should be empty");
    assert!(context_content.is_empty(), "context.jsonl should be empty");
}

// ---------------------------------------------------------------------------
// read_session_metadata
// ---------------------------------------------------------------------------

#[tokio::test]
async fn read_session_metadata_returns_default_when_file_absent() {
    let root = temp_root();
    // Pass the root itself (no session.jsonc there).
    let meta = read_session_metadata(root.path()).await;
    assert_eq!(meta, SessionMetadata::default());
}

#[tokio::test]
async fn read_session_metadata_parses_present_fields() {
    let root = temp_root();
    let jsonc = r#"{"name":"my-session"}"#;
    std::fs::write(root.path().join("session.jsonc"), jsonc).unwrap();

    let meta = read_session_metadata(root.path()).await;
    assert_eq!(meta.name.as_deref(), Some("my-session"));
    assert!(meta.resumed_from.is_none());
}

#[tokio::test]
async fn read_session_metadata_strips_single_line_comments() {
    let root = temp_root();
    let jsonc = "{\n  // this is a comment\n  \"name\": \"commented\"\n}";
    std::fs::write(root.path().join("session.jsonc"), jsonc).unwrap();

    let meta = read_session_metadata(root.path()).await;
    assert_eq!(meta.name.as_deref(), Some("commented"));
}

#[tokio::test]
async fn read_session_metadata_strips_block_comments() {
    let root = temp_root();
    let jsonc = "{ /* block\ncomment */ \"name\": \"block-commented\" }";
    std::fs::write(root.path().join("session.jsonc"), jsonc).unwrap();

    let meta = read_session_metadata(root.path()).await;
    assert_eq!(meta.name.as_deref(), Some("block-commented"));
}

#[tokio::test]
async fn read_session_metadata_accepts_legacy_continuation_of() {
    let root = temp_root();
    let jsonc = r#"{"continuationOf":"2025-07-04T14-32-05-123-a3f7c1b2"}"#;
    std::fs::write(root.path().join("session.jsonc"), jsonc).unwrap();

    let meta = read_session_metadata(root.path()).await;
    assert_eq!(
        meta.resumed_from.as_deref(),
        Some("2025-07-04T14-32-05-123-a3f7c1b2")
    );
}

#[tokio::test]
async fn read_session_metadata_returns_default_on_malformed_json() {
    let root = temp_root();
    std::fs::write(root.path().join("session.jsonc"), "not json").unwrap();

    let meta = read_session_metadata(root.path()).await;
    assert_eq!(meta, SessionMetadata::default());
}

// ---------------------------------------------------------------------------
// write_session_metadata / round-trip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn write_then_read_round_trips() {
    let root = temp_root();
    #[allow(clippy::unwrap_used)]
    let paths = make_session_dir(root.path()).await.unwrap();

    let original = SessionMetadata {
        name: Some("round-trip".to_owned()),
        resumed_from: None,
    };

    write_session_metadata(&paths.dir, &original).await.unwrap();
    let read_back = read_session_metadata(&paths.dir).await;
    assert_eq!(read_back, original);
}

#[tokio::test]
async fn write_omits_none_fields() {
    let root = temp_root();
    #[allow(clippy::unwrap_used)]
    let paths = make_session_dir(root.path()).await.unwrap();

    let meta = SessionMetadata {
        name: Some("only-name".to_owned()),
        resumed_from: None,
    };

    write_session_metadata(&paths.dir, &meta).await.unwrap();

    let raw = std::fs::read_to_string(paths.dir.join("session.jsonc")).unwrap();
    assert!(
        !raw.contains("resumedFrom"),
        "None fields must not be written"
    );
}

// ---------------------------------------------------------------------------
// update_session_metadata
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// strip_jsonc boundary scenarios (via read_session_metadata)
//
// strip_jsonc_comments is private; every edge case is exercised by writing a
// real session.jsonc and calling read_session_metadata.  Each test documents
// which specific mutation it targets.
// ---------------------------------------------------------------------------

/// Lone `/` at the end of the file must pass through unchanged — it is NOT
/// the start of a `//` or `/*` sequence.
///
/// Targets the outer `i + 1 < len` guards (lines ~210 and ~218).  A `< to
/// <=` or `+ to *` mutation makes the guard evaluate `bytes[len]` (OOB panic)
/// when `i` is the last character.
#[tokio::test]
async fn lone_slash_at_end_of_file_does_not_corrupt_output() {
    let root = temp_root();
    // The file ends with a bare `/`; strip_jsonc must copy it through.
    // The result is not valid JSON, so read_session_metadata returns default —
    // but the key assertion is that no OOB panic occurs.
    std::fs::write(root.path().join("session.jsonc"), "{\"name\":\"ok\"}/").unwrap();
    let meta = read_session_metadata(root.path()).await;
    assert_eq!(
        meta,
        SessionMetadata::default(),
        "trailing lone slash makes JSON invalid; default expected"
    );
}

/// `//` at byte position 0 (very first character of the file).
///
/// Targets the `i += 2` instruction for single-line comments.  A `+= to -=`
/// mutation underflows `usize` when `i == 0`.
#[tokio::test]
async fn single_line_comment_at_position_zero_is_stripped() {
    let root = temp_root();
    let content = "// leading comment\n{\"name\":\"at-zero\"}";
    std::fs::write(root.path().join("session.jsonc"), content).unwrap();
    let meta = read_session_metadata(root.path()).await;
    assert_eq!(meta.name.as_deref(), Some("at-zero"));
}

/// `//` comment at the very end of the file with no trailing newline.
///
/// Targets the inner `while i < len && bytes[i] != b'\n'` loop.  A `< to
/// <=` mutation makes the loop body access `bytes[len]` (OOB panic) when
/// the file ends without a newline.
#[tokio::test]
async fn single_line_comment_no_trailing_newline_is_stripped() {
    let root = temp_root();
    // No `\n` after the comment.
    let content = "{\"name\":\"no-nl\"}// trailing comment, no newline";
    std::fs::write(root.path().join("session.jsonc"), content.as_bytes()).unwrap();
    let meta = read_session_metadata(root.path()).await;
    assert_eq!(meta.name.as_deref(), Some("no-nl"));
}

/// `//` comment starting past the midpoint of the file.
///
/// Targets the `i += 2` instruction.  A `+= to *=` mutation sets `i` to
/// `6 * 2 = 12`, which is past the closing newline at position 11, so the
/// inner skip loop consumes `}` instead of stopping at the newline — leaving
/// invalid JSON (parse returns default).
#[tokio::test]
async fn single_line_comment_past_midpoint_is_stripped() {
    let root = temp_root();
    // `//` starts at byte 14 (after `{"name":"pm"`).
    // After stripping: `{"name":"pm"\n}` → name = "pm".
    let content = "{\"name\":\"pm\"}// xx\n}";
    std::fs::write(root.path().join("session.jsonc"), content).unwrap();
    // Wait — the `}` before `//` already closes the object; the second `}`
    // after the newline is extra but tolerated by serde_json's default
    // deserialiser.  We just need to confirm stripping succeeds and name
    // is readable.
    // Use a cleaner pattern instead:
    let content2 = "{\"name\":\"pm-ok\"// side note\n}";
    std::fs::write(root.path().join("session.jsonc"), content2).unwrap();
    let meta = read_session_metadata(root.path()).await;
    assert_eq!(meta.name.as_deref(), Some("pm-ok"));
}

/// `/*` block comment at byte position 0.
///
/// Targets the `i += 2` for block comments.  A `+= to -=` mutation
/// underflows `usize` when `i == 0`.
#[tokio::test]
async fn block_comment_at_position_zero_is_stripped() {
    let root = temp_root();
    let content = "/* leading block comment */ {\"name\":\"blk-zero\"}";
    std::fs::write(root.path().join("session.jsonc"), content).unwrap();
    let meta = read_session_metadata(root.path()).await;
    assert_eq!(meta.name.as_deref(), Some("blk-zero"));
}

/// `/*` block comment starting past the midpoint of the file.
///
/// Targets the `i += 2` for block comments.  A `+= to *=` mutation sets `i`
/// to `2 * p` where `p` is the position of `/*`.  When `2 * p` overshoots
/// the end of the string the inner scan loop never runs, `i += 2` pushes
/// past `len`, and the outer loop exits early — truncating the closing `}`.
///
/// The comment `/* */` is deliberately short so that `2 * 13 = 26` exceeds
/// the total length of 19, making the mutant lose the `}` and produce
/// invalid JSON (returns default instead of name = "bpm").
#[tokio::test]
async fn block_comment_past_midpoint_is_stripped() {
    let root = temp_root();
    // `/*` at position 13; total length 19; short comment so 2*13 > len.
    // Correct: strips comment → `{"name":"bpm"}` → name = "bpm".
    // `+= to *=` mutant: i jumps to 26 > 19, outer loop exits → truncated.
    let content = "{\"name\":\"bpm\"/* */}";
    std::fs::write(root.path().join("session.jsonc"), content).unwrap();
    let meta = read_session_metadata(root.path()).await;
    assert_eq!(meta.name.as_deref(), Some("bpm"));
}

/// Block comment containing a `*` NOT followed by `/`.
///
/// Targets the inner loop's `&&` condition.  A `&&` to `||` mutation makes
/// ANY `*` close the comment — leaking ` b */` into the JSON, which breaks
/// parsing.
#[tokio::test]
async fn block_comment_star_not_followed_by_slash_is_stripped() {
    let root = temp_root();
    // The value is the text AFTER the block comment.  The comment contains
    // a `*` that is NOT immediately followed by `/`, so the correct code
    // keeps scanning.
    let content = "{\"name\":/* a* b */\"star-ok\"}";
    std::fs::write(root.path().join("session.jsonc"), content).unwrap();
    let meta = read_session_metadata(root.path()).await;
    assert_eq!(meta.name.as_deref(), Some("star-ok"));
}

/// Unclosed block comment ending with `*` (no matching `*/`).
///
/// Targets ALL four block-comment inner-loop mutations (`< to <=`,
/// `+ to -`, `+ to *`, `&& to ||`).  The comment ends with `*` so that the
/// last iteration of the inner loop has `bytes[i] == b'*'`, making the second
/// operand `bytes[i + 1]` non-trivially evaluated.  Each mutation widens the
/// loop condition enough to attempt `bytes[len]` (OOB panic).
#[tokio::test]
async fn unclosed_block_comment_at_eof_does_not_panic() {
    let root = temp_root();
    // Everything before `/*` is a valid JSON object.
    // The block comment is never closed; strip_jsonc discards it.
    let content = "{\"name\":\"close-star\"}/* unclosed*";
    std::fs::write(root.path().join("session.jsonc"), content).unwrap();
    let meta = read_session_metadata(root.path()).await;
    assert_eq!(meta.name.as_deref(), Some("close-star"));
}

// ---------------------------------------------------------------------------
// update_session_metadata
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_session_metadata_merges_patch() {
    let root = temp_root();
    #[allow(clippy::unwrap_used)]
    let paths = make_session_dir(root.path()).await.unwrap();

    // Write initial state.
    write_session_metadata(
        &paths.dir,
        &SessionMetadata {
            name: Some("original".to_owned()),
            resumed_from: None,
        },
    )
    .await
    .unwrap();

    // Patch: update name, leave resumed_from unchanged.
    update_session_metadata(
        &paths.dir,
        SessionMetadata {
            name: Some("updated".to_owned()),
            resumed_from: None,
        },
    )
    .await
    .unwrap();

    let result = read_session_metadata(&paths.dir).await;
    assert_eq!(result.name.as_deref(), Some("updated"));
    assert!(result.resumed_from.is_none());
}
