//! omega-store — filesystem persistence for Omega sessions.
//!
//! This crate owns all on-disk I/O for a session:
//!
//! - [`context_hash`] — the [`ContextHash`] newtype and generation helpers.
//! - [`session_dir`]  — session folder creation, naming, and metadata I/O.
//! - [`event_store`]  — [`EventStore`]: append [`OmegaEvent`]s to `events.jsonl`.
//! - [`context_store`]— [`ContextStore`]: append context records to `context.jsonl`.
//!
//! [`OmegaEvent`]: omega_types::OmegaEvent

pub mod context_hash;
pub mod context_store;
pub mod event_store;
pub mod session_dir;

pub use context_hash::{ContextHash, hash_from_str, random_hash};
pub use context_store::{ContextRecord, ContextStore};
pub use event_store::EventStore;
pub use session_dir::{
    SESSIONS_ROOT, SessionMetadata, SessionPaths, make_session_dir, make_session_dir_name,
    read_session_metadata, session_dir_re, update_session_metadata, write_session_metadata,
};

use thiserror::Error;

/// Errors returned by `omega-store` operations.
#[derive(Debug, Error)]
pub enum StoreError {
    /// Filesystem or I/O failure.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON serialisation / deserialisation failure.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// A string passed to [`hash_from_str`] did not match `[0-9a-f]{12}`.
    #[error("invalid context hash: {0:?}")]
    InvalidHash(String),
}

/// Convenience `Result` alias for `omega-store` operations.
pub type Result<T> = std::result::Result<T, StoreError>;
