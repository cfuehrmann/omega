//! Append-only context record writer.
//!
//! Each session maintains a single `context.jsonl` file.  Every message
//! pushed to the agent's conversation history is written as a
//! [`ContextRecord`] — a [`Message`](omega_core::Message) augmented with a
//! [`ContextHash`] primary key and an ISO 8601 timestamp.
//!
//! The hash returned by [`ContextStore::append`] is used as a foreign key in
//! `events.jsonl` (`LlmCallEvent.context_hashes`, etc.).

use std::path::PathBuf;

use chrono::Utc;
use omega_core::{ContentBlock, Role};
use serde::{Deserialize, Serialize};

use crate::{ContextHash, Result, StoreError, random_hash};

// ---------------------------------------------------------------------------
// ContextRecord
// ---------------------------------------------------------------------------

/// The on-disk shape of a single `context.jsonl` record.
///
/// Extends a conversation message with persistence metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextRecord {
    /// Unique primary key: 12 lowercase hex characters (6 random bytes).
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

    /// Build a [`ContextRecord`] (random hash, current UTC time) and append
    /// it to `context.jsonl`.
    ///
    /// Returns the generated [`ContextHash`] so the caller can reference it
    /// as a foreign key in `events.jsonl` without re-reading the file.
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

    /// Build a [`ContextRecord`] without writing it — useful for testing the
    /// record shape without I/O.
    #[must_use]
    pub fn build_record(role: Role, content: Vec<ContentBlock>) -> ContextRecord {
        ContextRecord {
            hash: random_hash(),
            time: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            role,
            content,
        }
    }
}
