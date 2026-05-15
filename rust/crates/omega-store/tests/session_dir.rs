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
    let jsonc = r#"{"name":"my-session","description":"test run"}"#;
    std::fs::write(root.path().join("session.jsonc"), jsonc).unwrap();

    let meta = read_session_metadata(root.path()).await;
    assert_eq!(meta.name.as_deref(), Some("my-session"));
    assert_eq!(meta.description.as_deref(), Some("test run"));
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
        description: Some("desc".to_owned()),
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
        description: None,
        resumed_from: None,
    };

    write_session_metadata(&paths.dir, &meta).await.unwrap();

    let raw = std::fs::read_to_string(paths.dir.join("session.jsonc")).unwrap();
    assert!(
        !raw.contains("description"),
        "None fields must not be written"
    );
    assert!(
        !raw.contains("resumedFrom"),
        "None fields must not be written"
    );
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
            description: Some("keep me".to_owned()),
            resumed_from: None,
        },
    )
    .await
    .unwrap();

    // Patch: update name, leave description and resumed_from unchanged.
    update_session_metadata(
        &paths.dir,
        SessionMetadata {
            name: Some("updated".to_owned()),
            description: None,
            resumed_from: None,
        },
    )
    .await
    .unwrap();

    let result = read_session_metadata(&paths.dir).await;
    assert_eq!(result.name.as_deref(), Some("updated"));
    assert_eq!(result.description.as_deref(), Some("keep me"));
    assert!(result.resumed_from.is_none());
}
