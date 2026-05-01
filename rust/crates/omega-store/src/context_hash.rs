//! `ContextHash` newtype — 12 lowercase hex characters (6 random bytes).
//!
//! Used as the primary key of `context.jsonl` records and as a foreign key
//! in `events.jsonl` (`LlmCallEvent.context_hashes`, `ToolCallEvent.context_hash`,
//! etc.).

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{Result, StoreError};

/// A 12-character lowercase hex string encoding 6 random bytes.
///
/// Serves as the primary key of `context.jsonl` records and as a foreign key
/// in `events.jsonl`.  Construct with [`random_hash`] or validate an existing
/// string with [`hash_from_str`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContextHash(String);

impl ContextHash {
    /// Returns `true` iff `s` is exactly 12 lowercase hex characters.
    fn is_valid(s: &str) -> bool {
        s.len() == 12 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    }
}

/// Generate a fresh [`ContextHash`] from 6 cryptographically random bytes.
///
/// Uses the thread-local random number generator from the `rand` crate.
#[must_use]
pub fn random_hash() -> ContextHash {
    let bytes: [u8; 6] = rand::random();
    let hex = bytes.iter().fold(String::with_capacity(12), |mut s, b| {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
        s
    });
    ContextHash(hex)
}

/// Parse a [`ContextHash`] from an existing string, validating it matches
/// `[0-9a-f]{12}`.
///
/// # Errors
///
/// Returns [`StoreError::InvalidHash`] if `s` is not exactly 12 lowercase
/// hex characters.
pub fn hash_from_str(s: &str) -> Result<ContextHash> {
    if ContextHash::is_valid(s) {
        Ok(ContextHash(s.to_owned()))
    } else {
        Err(StoreError::InvalidHash(s.to_owned()))
    }
}

impl fmt::Display for ContextHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl AsRef<str> for ContextHash {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<ContextHash> for String {
    fn from(h: ContextHash) -> Self {
        h.0
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn random_hash_is_12_hex_chars() {
        let h = random_hash();
        assert_eq!(h.as_ref().len(), 12);
        assert!(
            h.as_ref()
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        );
    }

    #[test]
    fn random_hash_display_equals_inner() {
        let h = random_hash();
        assert_eq!(h.to_string(), h.as_ref());
    }

    #[test]
    fn hash_from_str_accepts_valid() {
        let h = hash_from_str("0123456789ab").unwrap();
        assert_eq!(h.as_ref(), "0123456789ab");
    }

    #[test]
    fn hash_from_str_rejects_uppercase() {
        assert!(hash_from_str("0123456789AB").is_err());
    }

    #[test]
    fn hash_from_str_rejects_short() {
        assert!(hash_from_str("0123456789a").is_err());
    }

    #[test]
    fn hash_from_str_rejects_long() {
        assert!(hash_from_str("0123456789abc").is_err());
    }

    #[test]
    fn hash_from_str_rejects_non_hex() {
        assert!(hash_from_str("0123456789xz").is_err());
    }

    #[test]
    fn into_string_works() {
        let h = hash_from_str("aabbccddeeff").unwrap();
        let s: String = h.into();
        assert_eq!(s, "aabbccddeeff");
    }
}
