#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::clone_on_copy,
    clippy::single_char_add_str,
    clippy::doc_markdown
)]

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

// ---------------------------------------------------------------------------
// Multi-record round-trip (workspace-level integration)
// ---------------------------------------------------------------------------

/// Drives a realistic four-message conversation through `ContextStore`
/// and checks every record at the integration level:
///
///   1. User asks a question.
///   2. Assistant emits a `Thinking` block, a `Text` block, and a
///      `ToolUse` block (read_file).
///   3. User returns the corresponding `ToolResult` block.
///   4. Assistant emits the final answer as `Text`.
///
/// For every record we assert:
///
/// - `append()` returned the same hash that `read_all()` reports.
/// - `verify_record()` accepts the record as untampered.
/// - The hash is exactly what `content_hash(&role, &content)` would
///   produce for the same `(role, content)` — i.e. the writer and
///   the hashing free function agree end-to-end.
///
/// Then we tamper with the second record's content and prove that
/// `verify_record` rejects exactly that record while the other three
/// still verify cleanly — a positive demonstration that the
/// integrity check is record-local rather than session-global.
#[tokio::test]
async fn multi_record_session_round_trips_and_verifies_per_record() {
    let (_guard, path) = temp_context_file();
    let store = ContextStore::new(path);

    // Build the four messages.
    let m1_role = Role::User;
    let m1_content = vec![text_block("What does foo.txt contain?")];

    let m2_role = Role::Assistant;
    let m2_content = vec![
        ContentBlock::Thinking {
            thinking: "I should read the file.".to_owned(),
            signature: Some("sig-1".to_owned()),
        },
        ContentBlock::Text {
            text: "Let me check.".to_owned(),
        },
        ContentBlock::ToolUse {
            id: "tu_1".to_owned(),
            name: "read_file".to_owned(),
            input: serde_json::json!({ "path": "foo.txt" }),
        },
    ];

    let m3_role = Role::User;
    let m3_content = vec![ContentBlock::ToolResult {
        tool_use_id: "tu_1".to_owned(),
        content: "hello world\n".to_owned(),
        is_error: false,
    }];

    let m4_role = Role::Assistant;
    let m4_content = vec![text_block("The file contains: hello world")];

    // Append all four; record the hashes append() returned.
    #[allow(clippy::unwrap_used)]
    let h1 = store.append(m1_role, m1_content.clone()).await.unwrap();
    #[allow(clippy::unwrap_used)]
    let h2 = store.append(m2_role, m2_content.clone()).await.unwrap();
    #[allow(clippy::unwrap_used)]
    let h3 = store.append(m3_role, m3_content.clone()).await.unwrap();
    #[allow(clippy::unwrap_used)]
    let h4 = store.append(m4_role, m4_content.clone()).await.unwrap();
    let append_hashes = [h1, h2, h3, h4];

    // Read everything back.
    #[allow(clippy::unwrap_used)]
    let records = store.read_all().await.unwrap();
    assert_eq!(
        records.len(),
        4,
        "expected 4 records, got {}",
        records.len()
    );

    // (role, content) tuples in the same order as the messages above,
    // for cross-checking against content_hash directly.
    let expected = [
        (Role::User, m1_content),
        (Role::Assistant, m2_content),
        (Role::User, m3_content),
        (Role::Assistant, m4_content),
    ];

    // Per-record assertions: hash agreement, free-function agreement,
    // and verify_record acceptance.
    for (i, record) in records.iter().enumerate() {
        let (ref role, ref content) = expected[i];
        assert_eq!(
            record.hash, append_hashes[i],
            "record {i}: append() hash differs from read_all() hash",
        );
        assert_eq!(
            record.hash,
            content_hash(role, content),
            "record {i}: stored hash diverges from content_hash(&role, &content)",
        );
        assert_eq!(record.role, *role, "record {i}: role round-trip diverged");
        assert_eq!(
            record.content, *content,
            "record {i}: content round-trip diverged",
        );
        assert!(
            ContextStore::verify_record(record).is_ok(),
            "record {i}: verify_record rejected an untampered record",
        );
    }

    // All four hashes must be distinct — the four messages have
    // distinct (role, content), so a collision would point at a real
    // hashing bug rather than at chance.
    let mut seen = std::collections::HashSet::new();
    for r in &records {
        assert!(
            seen.insert(r.hash.clone()),
            "unexpected hash collision in multi-record session: {:?}",
            r.hash,
        );
    }

    // Tamper with record 1 (the assistant turn): replace its content
    // with something different.  Only that record should fail
    // verify_record; the other three remain valid — proves integrity
    // checks are record-local.
    let mut tampered = records;
    tampered[1].content = vec![text_block("injected text")];
    assert!(ContextStore::verify_record(&tampered[0]).is_ok());
    assert!(matches!(
        ContextStore::verify_record(&tampered[1]),
        Err(StoreError::HashMismatch { .. })
    ));
    assert!(ContextStore::verify_record(&tampered[2]).is_ok());
    assert!(ContextStore::verify_record(&tampered[3]).is_ok());
}
