//! Append-only event log writer.
//!
//! Each session maintains a single `events.jsonl` file.  [`EventStore`]
//! serialises [`OmegaEvent`]s as JSON lines and appends them, one per line.
//!
//! The Rust `OmegaEvent` type has no UI-only fields (that distinction was
//! TS-specific); every field written is meaningful and round-trippable.

use std::path::PathBuf;

use omega_types::OmegaEvent;
use omega_types::ids::{EventId, LoggedEvent};
use uuid::Uuid;

use crate::{Result, StoreError};

/// Append-only writer for `events.jsonl`.
///
/// Created once per session from the path returned by
/// [`make_session_dir`](crate::make_session_dir).
pub struct EventStore {
    path: PathBuf,
}

impl EventStore {
    /// Create a new [`EventStore`] that writes to `path`.
    ///
    /// The file does not need to exist yet; it will be created (and parent
    /// directories made) on the first [`append`](Self::append) call.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Read all parseable event objects from `events.jsonl`.
    ///
    /// Each non-blank line is parsed as a [`serde_json::Value`].  Lines
    /// that are blank or contain malformed JSON are silently skipped —
    /// mirrors the TypeScript `loadReplayEvents` behaviour in
    /// `src/web/server.ts`.
    ///
    /// Returns an empty `Vec` if the file does not exist.
    ///
    /// # Errors
    ///
    /// Returns an error only for I/O failures other than "file not found".
    pub async fn read_all(&self) -> Result<Vec<serde_json::Value>> {
        let text = match tokio::fs::read_to_string(&self.path).await {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(StoreError::Io(e)),
        };
        let events = text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        Ok(events)
    }

    /// Serialise `event` as a JSON line and append it to `events.jsonl`.
    ///
    /// The event is wrapped in a [`LoggedEvent`] envelope with a fresh UUID v7
    /// assigned as the `eventId`.  This ID is the stable, durable identity
    /// for the event within the session's `events.jsonl`.
    ///
    /// Creates the file and any missing parent directories if they do not
    /// exist.
    ///
    /// # Errors
    ///
    /// Returns an error if serialisation or the file write fails.
    pub async fn append(&self, event: &OmegaEvent) -> Result<()> {
        let envelope = LoggedEvent {
            event_id: Some(EventId(Uuid::now_v7())),
            event: event.clone(),
        };
        let mut line = serde_json::to_string(&envelope)?;
        line.push('\n');

        // Create parent directories defensively — make_session_dir already
        // does this, but guard against unexpected paths.
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

        Ok(())
    }
}
