//! Append-only context record writer.
//!
//! Each session maintains a single `context.jsonl` file.  Every message
//! pushed to the agent's conversation history is written as a
//! [`ContextRecord`] — a [`Message`](omega_types::conversation) augmented with a
//! [`ContextHash`] primary key and an ISO 8601 timestamp.
//!
//! The hash returned by [`ContextStore::append`] is the deterministic
//! [`content_hash`] of `(role, content)`; it is used as a foreign key in
//! `events.jsonl` (`LlmCallEvent.context_hashes`, etc.).
//!
//! Two appends with identical `(role, content)` therefore produce the
//! same hash by design — see HASH-1 in `backlog/hash-1.md`.

use std::path::PathBuf;

use chrono::Utc;
use omega_types::{ContentBlock, Role};
use serde::{Deserialize, Serialize};

use crate::{ContextHash, Result, StoreError, content_hash};

// ---------------------------------------------------------------------------
// ContextRecord
// ---------------------------------------------------------------------------

/// The on-disk shape of a single `context.jsonl` record.
///
/// Extends a conversation message with persistence metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextRecord {
    /// Unique primary key: the deterministic [`content_hash`] of
    /// `(role, content)` — 16 lowercase hex characters (first 8 bytes
    /// of `sha256` over the canonical `(role, content)` encoding).
    ///
    /// Two records with identical `(role, content)` have the same
    /// `hash` by design.  See HASH-1 in `backlog/hash-1.md`.
    ///
    /// [`content_hash`]: crate::content_hash
    pub hash: ContextHash,
    /// ISO 8601 UTC timestamp when this record was appended.
    pub time: String,
    /// Role of the message author.
    pub role: Role,
    /// Content blocks of the message.
    pub content: Vec<ContentBlock>,
}

// ---------------------------------------------------------------------------
// ContextStore
// ---------------------------------------------------------------------------

/// Append-only writer for `context.jsonl`.
///
/// Created once per session from the path returned by
/// [`make_session_dir`](crate::make_session_dir).
pub struct ContextStore {
    path: PathBuf,
}

impl ContextStore {
    /// Create a new [`ContextStore`] that writes to `path`.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Build a [`ContextRecord`] (deterministic content hash, current UTC
    /// time) and append it to `context.jsonl`.
    ///
    /// Returns the [`ContextHash`] computed from `(role, content)` so the
    /// caller can reference it as a foreign key in `events.jsonl` without
    /// re-reading the file.  Identical `(role, content)` produces the
    /// same hash on every call.
    ///
    /// # Errors
    ///
    /// Returns an error if serialisation or the file write fails.
    pub async fn append(&self, role: Role, content: Vec<ContentBlock>) -> Result<ContextHash> {
        let record = Self::build_record(role, content);
        let hash = record.hash.clone();

        let mut line = serde_json::to_string(&record)?;
        line.push('\n');

        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Use spawn_blocking + std::fs to get reliable O_APPEND semantics.
        // tokio::fs::File uses positioned writes (pwrite) internally, which
        // ignores O_APPEND and would overwrite on every call.
        let path = self.path.clone();
        let bytes = line.into_bytes();
        tokio::task::spawn_blocking(move || {
            use std::io::Write as _;
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)?;
            file.write_all(&bytes)
        })
        .await
        .map_err(|e| StoreError::Io(std::io::Error::other(e)))??;

        Ok(hash)
    }

    /// Read every parseable [`ContextRecord`] from `context.jsonl`.
    ///
    /// Each non-blank line is parsed as a [`ContextRecord`]; blank lines
    /// or malformed JSON are silently skipped — mirrors the TS
    /// `lookupContextRecords` reader in `src/web/server.ts`.
    ///
    /// Returns an empty `Vec` when the file does not exist.
    ///
    /// # Errors
    ///
    /// Returns an error only for I/O failures other than "file not found".
    pub async fn read_all(&self) -> Result<Vec<ContextRecord>> {
        let text = match tokio::fs::read_to_string(&self.path).await {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(StoreError::Io(e)),
        };
        let records = text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        Ok(records)
    }

    /// Build a [`ContextRecord`] without writing it — useful for testing the
    /// record shape without I/O.  The returned record's hash is the
    /// deterministic [`content_hash`] of `(role, content)`.
    ///
    /// [`content_hash`]: crate::content_hash
    #[must_use]
    pub fn build_record(role: Role, content: Vec<ContentBlock>) -> ContextRecord {
        let hash = content_hash(&role, &content);
        ContextRecord {
            hash,
            time: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            role,
            content,
        }
    }

    /// Verify that a [`ContextRecord`]'s stored `hash` still matches the
    /// [`content_hash`] of its current `(role, content)`.
    ///
    /// Use this on records freshly read back from `context.jsonl` to
    /// detect tampering or on-disk corruption: an attacker who edits a
    /// message in place — for instance, to retroactively rewrite a
    /// past user instruction — leaves the stored 16-hex hash pointing
    /// at the *original* content, and that mismatch is what this
    /// function surfaces.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::HashMismatch`] when the stored hash does
    /// not match the recomputed [`content_hash`] of `(role, content)`.
    ///
    /// [`content_hash`]: crate::content_hash
    /// [`StoreError::HashMismatch`]: crate::StoreError::HashMismatch
    pub fn verify_record(record: &ContextRecord) -> Result<()> {
        let recomputed = content_hash(&record.role, &record.content);
        if recomputed == record.hash {
            Ok(())
        } else {
            Err(StoreError::HashMismatch {
                stored: record.hash.clone(),
                recomputed,
            })
        }
    }
}
