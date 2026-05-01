//! omega-protocol — shared type definitions for the Omega agent protocol.
//!
//! This crate owns the canonical Rust representation of every type that
//! crosses a persistence or network boundary:
//!
//! - [`OmegaEvent`] — the unified discriminated union written to
//!   `events.jsonl` and streamed over WebSocket.
//! - [`StreamSignal`] — ephemeral streaming primitives (text/thinking
//!   token fragments) that are never persisted.
//!
//! All types implement `serde::Serialize` + `serde::Deserialize`.
//! The JSON representation is intentionally close to the TypeScript
//! representation that preceded it; field names are camelCase to match
//! existing `events.jsonl` files.

pub mod events;
pub mod stream_signal;

pub use events::{
    ContinueMode, InterruptReason, LlmResponseUsage, LlmRetryReason, OmegaEvent, ServerStopOutcome,
    TurnMetrics,
};
pub use stream_signal::StreamSignal;

// ---------------------------------------------------------------------------
// Primitive aliases
// ---------------------------------------------------------------------------

/// ISO 8601 datetime string (e.g. `"2024-01-15T12:00:00.000Z"`).
///
/// Stored as a plain `String` at the protocol layer.  Validation (format
/// checking, ordering) is the responsibility of `omega-core`.
pub type ISOTimestamp = String;

/// 12-character lowercase hex string encoding 6 random bytes.
///
/// Used as the primary key of `context.jsonl` records and as a foreign key
/// in `events.jsonl`.  Format validation lives in `omega-core`.
pub type ContextHash = String;
