//! Integration tests for `ContextStore` and `ContextHash` — real file I/O
//! via temp directories.

use omega_core::{ContentBlock, Role};
use omega_store::{ContextRecord, ContextStore, hash_from_str};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn temp_context_file() -> (tempfile::TempDir, std::path::PathBuf) {
    #[allow(clippy::unwrap_used)]
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("context.jsonl");
    (dir, path)
}

fn text_block(text: &str) -> ContentBlock {
    ContentBlock::Text {
        text: text.to_owned(),
    }
}

// ---------------------------------------------------------------------------
// hash_from_str validation
// ---------------------------------------------------------------------------

#[test]
fn hash_from_str_accepts_valid_12_hex() {
    #[allow(clippy::unwrap_used)]
    let h = hash_from_str("0123456789ab").unwrap();
    assert_eq!(h.as_ref(), "0123456789ab");
}

#[test]
fn hash_from_str_rejects_uppercase_letters() {
    assert!(hash_from_str("0123456789AB").is_err());
}

#[test]
fn hash_from_str_rejects_too_short() {
    assert!(hash_from_str("0123456789a").is_err());
}

#[test]
fn hash_from_str_rejects_too_long() {
    assert!(hash_from_str("0123456789abc").is_err());
}

#[test]
fn hash_from_str_rejects_non_hex_chars() {
    assert!(hash_from_str("0123456789xz").is_err());
}

#[test]
fn hash_from_str_rejects_empty_string() {
    assert!(hash_from_str("").is_err());
}

// ---------------------------------------------------------------------------
// ContextStore::build_record (no I/O)
// ---------------------------------------------------------------------------

#[test]
fn build_record_has_valid_hash() {
    let record = ContextStore::build_record(Role::User, vec![text_block("hi")]);
    // hash must be 12 lowercase hex chars
    assert!(hash_from_str(record.hash.as_ref()).is_ok());
}

#[test]
fn build_record_preserves_role_and_content() {
    let record = ContextStore::build_record(Role::Assistant, vec![text_block("response")]);
    assert_eq!(record.role, Role::Assistant);
    assert_eq!(record.content.len(), 1);
    assert!(matches!(&record.content[0], ContentBlock::Text { text } if text == "response"));
}

#[test]
fn build_record_time_is_iso8601() {
    let record = ContextStore::build_record(Role::User, vec![]);
    // Must parse as an RFC 3339 datetime.
    #[allow(clippy::unwrap_used)]
    chrono::DateTime::parse_from_rfc3339(&record.time)
        .expect("time field must be a valid RFC 3339 timestamp");
}

// ---------------------------------------------------------------------------
// ContextStore::append (real I/O)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn append_returns_valid_hash() {
    let (_guard, path) = temp_context_file();
    let store = ContextStore::new(path);

    #[allow(clippy::unwrap_used)]
    let hash = store
        .append(Role::User, vec![text_block("hello")])
        .await
        .unwrap();

    assert!(
        hash_from_str(hash.as_ref()).is_ok(),
        "returned hash must be valid"
    );
}

#[tokio::test]
async fn append_writes_valid_jsonl_that_round_trips() {
    let (_guard, path) = temp_context_file();
    let store = ContextStore::new(path.clone());

    #[allow(clippy::unwrap_used)]
    let hash = store
        .append(Role::User, vec![text_block("round-trip")])
        .await
        .unwrap();

    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.ends_with('\n'), "JSONL line must end with newline");

    let record: ContextRecord = serde_json::from_str(content.trim_end()).unwrap();
    assert_eq!(record.hash, hash);
    assert_eq!(record.role, Role::User);
    assert!(matches!(&record.content[0], ContentBlock::Text { text } if text == "round-trip"));
}

#[tokio::test]
async fn two_appends_produce_different_hashes() {
    let (_guard, path) = temp_context_file();
    let store = ContextStore::new(path);

    #[allow(clippy::unwrap_used)]
    let h1 = store
        .append(Role::User, vec![text_block("first")])
        .await
        .unwrap();
    #[allow(clippy::unwrap_used)]
    let h2 = store
        .append(Role::Assistant, vec![text_block("second")])
        .await
        .unwrap();

    assert_ne!(h1, h2, "consecutive hashes must be different");
}

#[tokio::test]
async fn append_creates_multiple_jsonl_lines() {
    let (_guard, path) = temp_context_file();
    let store = ContextStore::new(path.clone());

    #[allow(clippy::unwrap_used)]
    {
        store
            .append(Role::User, vec![text_block("msg1")])
            .await
            .unwrap();
        store
            .append(Role::Assistant, vec![text_block("msg2")])
            .await
            .unwrap();
    }

    let content = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines.len(), 2, "expected exactly 2 context records");

    let r1: ContextRecord = serde_json::from_str(lines[0]).unwrap();
    let r2: ContextRecord = serde_json::from_str(lines[1]).unwrap();
    assert_eq!(r1.role, Role::User);
    assert_eq!(r2.role, Role::Assistant);
}

#[tokio::test]
async fn append_creates_parent_dirs_if_needed() {
    let root = tempfile::tempdir().unwrap();
    let path = root
        .path()
        .join("nested")
        .join("deep")
        .join("context.jsonl");
    let store = ContextStore::new(path.clone());

    #[allow(clippy::unwrap_used)]
    store.append(Role::User, vec![]).await.unwrap();

    assert!(
        path.exists(),
        "context.jsonl should be created inside nested dirs"
    );
}
