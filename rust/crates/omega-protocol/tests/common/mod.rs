//! Shared helpers for `omega-protocol` integration tests.
//!
//! Each `tests/*.rs` file is a separate test binary; include this module
//! via `mod common;`.  Unused items are silenced with `#[allow(dead_code)]`.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// id_redactor — stateful placeholder redaction
// ---------------------------------------------------------------------------

/// Stateful redactor that assigns stable `[id_N]` placeholders to unique
/// string values across multiple JSON paths.
///
/// Construct one per test; state is not shared across tests.  Multiple
/// [`redaction`](IdRedactor::redaction) calls on the same `IdRedactor`
/// share the same numbering space, so the same id value receives the same
/// placeholder even when it appears under different JSON keys.
///
/// # Example
///
/// ```ignore
/// let r = id_redactor();
/// insta::assert_json_snapshot!(value, {
///     "[].id"          => r.redaction(),
///     "[].tool_use_id" => r.redaction(),
/// });
/// ```
pub struct IdRedactor(Arc<Mutex<HashMap<String, usize>>>);

impl IdRedactor {
    #[must_use]
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(HashMap::new())))
    }

    /// Returns an [`insta::Redaction`] that maps string values to `[id_N]`
    /// placeholders, sharing the numbering space with every other redaction
    /// produced from this `IdRedactor`.
    #[must_use]
    pub fn redaction(&self) -> insta::internals::Redaction {
        let map = Arc::clone(&self.0);
        insta::dynamic_redaction(move |value, _path| {
            let insta::internals::Content::String(s) = value else {
                return value;
            };
            let mut m = map.lock().expect("id_redactor mutex poisoned");
            let next_n = m.len() + 1;
            let idx = *m.entry(s).or_insert(next_n);
            insta::internals::Content::String(format!("[id_{idx}]"))
        })
    }
}

/// Create a fresh [`IdRedactor`] scoped to one test.
#[must_use]
pub fn id_redactor() -> IdRedactor {
    IdRedactor::new()
}
