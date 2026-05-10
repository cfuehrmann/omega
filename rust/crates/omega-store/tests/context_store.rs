//! Integration tests for `ContextStore` and `ContextHash` — real file I/O
//! via temp directories.

use omega_store::{ContextRecord, ContextStore, StoreError, content_hash, hash_from_str};
use omega_types::{ContentBlock, Role};

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
fn hash_from_str_accepts_valid_16_hex() {
    #[allow(clippy::unwrap_used)]
    let h = hash_from_str("0123456789abcdef").unwrap();
    assert_eq!(h.as_ref(), "0123456789abcdef");
}

// T-LEN — the legacy 12-char length is unambiguously rejected.
#[test]
fn hash_from_str_rejects_legacy_12_char() {
    assert!(hash_from_str("0123456789ab").is_err());
}

#[test]
fn hash_from_str_rejects_uppercase_letters() {
    assert!(hash_from_str("0123456789ABCDEF").is_err());
}

#[test]
fn hash_from_str_rejects_too_short() {
    assert!(hash_from_str("0123456789abcde").is_err());
}

#[test]
fn hash_from_str_rejects_too_long() {
    assert!(hash_from_str("0123456789abcdef0").is_err());
}

#[test]
fn hash_from_str_rejects_non_hex_chars() {
    assert!(hash_from_str("0123456789abcdez").is_err());
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
    // hash must be 16 lowercase hex chars
    assert!(hash_from_str(record.hash.as_ref()).is_ok());
}

#[test]
fn build_record_hash_is_deterministic_content_hash() {
    let role = Role::User;
    let content = vec![text_block("hi")];
    let record = ContextStore::build_record(role.clone(), content.clone());
    assert_eq!(record.hash, content_hash(&role, &content));
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

    assert_ne!(
        h1, h2,
        "distinct (role, content) must produce distinct hashes"
    );
}

// T-RT — the hash returned by `append` equals `content_hash(role, content)`,
// and the same hash recomputed from the on-disk record's deserialised
// fields matches the on-disk hash.  Catches any serialisation asymmetry
// (e.g., a `skip_serializing_if` that omits a field on write but defaults
// it on read).
#[tokio::test]
async fn append_then_verify_roundtrip() {
    let (_guard, path) = temp_context_file();
    let store = ContextStore::new(path.clone());

    let role = Role::Assistant;
    let content = vec![text_block("ok")];

    #[allow(clippy::unwrap_used)]
    let returned = store.append(role.clone(), content.clone()).await.unwrap();
    let recomputed = content_hash(&role, &content);
    assert_eq!(returned, recomputed);

    #[allow(clippy::unwrap_used)]
    let line = std::fs::read_to_string(&path).unwrap();
    #[allow(clippy::unwrap_used)]
    let record: ContextRecord = serde_json::from_str(line.trim_end()).unwrap();
    assert_eq!(record.hash, recomputed);
    assert_eq!(content_hash(&record.role, &record.content), record.hash);
}

// T-CONT — identical (role, content) on two separate appends produces
// identical hashes.  Documents the intentional collision-by-design.
#[tokio::test]
async fn duplicate_content_yields_same_hash() {
    let (_guard, path) = temp_context_file();
    let store = ContextStore::new(path);

    let role = Role::User;
    let content = vec![text_block("hi")];

    #[allow(clippy::unwrap_used)]
    let h1 = store.append(role.clone(), content.clone()).await.unwrap();
    #[allow(clippy::unwrap_used)]
    let h2 = store.append(role.clone(), content.clone()).await.unwrap();

    assert_eq!(
        h1, h2,
        "identical (role, content) must yield identical content hash",
    );
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

// ---------------------------------------------------------------------------
// ContextStore::read_all (real I/O)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn read_all_propagates_non_notfound_io_error() {
    let dir = tempfile::tempdir().unwrap();
    // Placing a *directory* at the path produces an IsADirectory I/O error
    // (not NotFound) when read_to_string is called — must propagate as Err.
    let path = dir.path().join("context.jsonl");
    std::fs::create_dir(&path).unwrap();
    let store = ContextStore::new(path);
    let result = store.read_all().await;
    assert!(
        result.is_err(),
        "non-NotFound I/O error must propagate as Err, not return empty Vec",
    );
}

#[tokio::test]
async fn read_all_returns_empty_vec_when_file_missing() {
    let (_guard, path) = temp_context_file();
    let store = ContextStore::new(path);

    #[allow(clippy::unwrap_used)]
    let records = store.read_all().await.unwrap();
    assert!(records.is_empty());
}

#[tokio::test]
async fn read_all_returns_empty_vec_for_empty_file() {
    let (_guard, path) = temp_context_file();
    std::fs::write(&path, "").unwrap();
    let store = ContextStore::new(path);

    #[allow(clippy::unwrap_used)]
    let records = store.read_all().await.unwrap();
    assert!(records.is_empty());
}

#[tokio::test]
async fn read_all_round_trips_appended_records() {
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

    #[allow(clippy::unwrap_used)]
    let records = store.read_all().await.unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].hash, h1);
    assert_eq!(records[0].role, Role::User);
    assert_eq!(records[1].hash, h2);
    assert_eq!(records[1].role, Role::Assistant);
}

#[tokio::test]
async fn read_all_skips_blank_and_malformed_lines() {
    let (_guard, path) = temp_context_file();
    let store = ContextStore::new(path.clone());
    #[allow(clippy::unwrap_used)]
    store
        .append(Role::User, vec![text_block("ok")])
        .await
        .unwrap();
    // Inject a malformed line + a blank line.
    let mut text = std::fs::read_to_string(&path).unwrap();
    text.push_str("\n");
    text.push_str("not json\n");
    text.push_str("\n");
    std::fs::write(&path, text).unwrap();

    #[allow(clippy::unwrap_used)]
    let records = store.read_all().await.unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].role, Role::User);
}

// ---------------------------------------------------------------------------
// verify_record — T-INT (tamper detection)
// ---------------------------------------------------------------------------

/// Happy path: an unmodified record verifies successfully.
#[tokio::test]
async fn verify_record_accepts_unmodified_round_tripped_record() {
    let (_guard, path) = temp_context_file();
    let store = ContextStore::new(path);
    #[allow(clippy::unwrap_used)]
    store
        .append(Role::User, vec![text_block("original")])
        .await
        .unwrap();
    #[allow(clippy::unwrap_used)]
    let records = store.read_all().await.unwrap();
    assert_eq!(records.len(), 1);
    assert!(ContextStore::verify_record(&records[0]).is_ok());
}

/// T-INT — tampering with `content` after the record is written must be
/// surfaced by `verify_record`.  The stored hash points at the original
/// content; an attacker who rewrites the message body cannot also fix
/// up the hash without recomputing it, and that is exactly what this
/// check catches.
#[tokio::test]
async fn verify_record_rejects_tampered_content() {
    let (_guard, path) = temp_context_file();
    let store = ContextStore::new(path);
    #[allow(clippy::unwrap_used)]
    let original_hash = store
        .append(Role::User, vec![text_block("original instruction")])
        .await
        .unwrap();

    #[allow(clippy::unwrap_used)]
    let mut records = store.read_all().await.unwrap();
    // Rewrite the message body in memory — simulating an attacker
    // editing context.jsonl while leaving the hash field intact.
    records[0].content = vec![text_block("rewritten instruction")];

    match ContextStore::verify_record(&records[0]) {
        Err(StoreError::HashMismatch { stored, recomputed }) => {
            assert_eq!(stored, original_hash);
            assert_ne!(recomputed, original_hash);
            // Recomputed hash must match a fresh content_hash over the
            // tampered content — proves the mismatch is reported
            // against the *current* (role, content), not a stale value.
            assert_eq!(
                recomputed,
                content_hash(&records[0].role, &records[0].content)
            );
        }
        other => panic!("expected HashMismatch, got {other:?}"),
    }
}

/// Tampering with the role alone is just as detectable as tampering
/// with the content — role is part of the canonical hash input.
#[tokio::test]
async fn verify_record_rejects_tampered_role() {
    let (_guard, path) = temp_context_file();
    let store = ContextStore::new(path);
    #[allow(clippy::unwrap_used)]
    store
        .append(Role::User, vec![text_block("hello")])
        .await
        .unwrap();
    #[allow(clippy::unwrap_used)]
    let mut records = store.read_all().await.unwrap();
    records[0].role = Role::Assistant;
    assert!(matches!(
        ContextStore::verify_record(&records[0]),
        Err(StoreError::HashMismatch { .. })
    ));
}
