//! `ContextHash` newtype — content-derived 16-character lowercase hex string.
//!
//! Used as the primary key of `context.jsonl` records and as a foreign key
//! in `events.jsonl` (`LlmCallEvent.context_hashes`, `ToolCallEvent.context_hash`,
//! etc.).
//!
//! ## Hash ABI
//!
//! A `ContextHash` is the first 8 bytes of `sha256` over the canonical
//! UTF-8 bytes produced by
//!
//! ```text
//! serde_json::to_vec(&(role, content))
//! ```
//!
//! where `role: &Role` and `content: &[ContentBlock]`.  This is rendered
//! as 16 lowercase hex characters.
//!
//! Any change to `Role` or `ContentBlock` that affects their `serde`
//! output — field order, variant order, `#[serde(rename)]`,
//! `#[serde(skip_serializing_if)]`, addition of a new variant or field —
//! is a **breaking change to the hash ABI**.  Such a change invalidates
//! every previously-saved session's hashes.  See `backlog/hash-1.md`.

use std::fmt;

use omega_types::{ContentBlock, Role};
use serde::{Deserialize, Serialize};
use sha2::Digest;

use crate::{Result, StoreError};

/// A 16-character lowercase hex string: the first 8 bytes of `sha256`
/// over the canonical encoding of `(role, content)`.
///
/// Construct via [`content_hash`] from a `(role, content)` pair, or
/// validate an existing string with [`hash_from_str`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContextHash(String);

impl ContextHash {
    /// Returns `true` iff `s` is exactly 16 lowercase hex characters.
    fn is_valid(s: &str) -> bool {
        s.len() == 16 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    }

    /// Construct a [`ContextHash`] from a string our hashing path
    /// produced.  Private — bypasses validation, so callers must already
    /// know `s` is 16 lowercase hex chars.
    fn from_validated(s: String) -> Self {
        debug_assert!(
            s.len() == 16 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')),
            "from_validated called with non-hash string: {s:?}",
        );
        Self(s)
    }
}

/// Compute the deterministic content hash of a `(role, content)` pair.
///
/// See the module-level docs for the canonical-form contract.
///
/// # Panics
///
/// Does not panic in practice: `Role` and `ContentBlock` consist of
/// strings, enums, and JSON `Value`s, all of which serialise infallibly.
/// The internal `expect` is therefore unreachable.
#[must_use]
#[allow(clippy::expect_used)]
pub fn content_hash(role: &Role, content: &[ContentBlock]) -> ContextHash {
    let canonical = serde_json::to_vec(&(role, content))
        .expect("Role and ContentBlock are infallible to serialise");
    let digest = sha2::Sha256::digest(&canonical);
    let prefix = &digest[..8];
    ContextHash::from_validated(hex::encode(prefix))
}

/// Parse a [`ContextHash`] from an existing string, validating it matches
/// `[0-9a-f]{16}`.
///
/// # Errors
///
/// Returns [`StoreError::InvalidHash`] if `s` is not exactly 16 lowercase
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
    use omega_types::{ContentBlock, Role};
    use serde_json::json;

    // T-LEN — length-12 hashes are now unambiguously rejected.
    #[test]
    fn hash_from_str_rejects_12_char() {
        assert!(hash_from_str("0123456789ab").is_err());
    }

    #[test]
    fn hash_from_str_accepts_valid_16() {
        let h = hash_from_str("0123456789abcdef").unwrap();
        assert_eq!(h.as_ref(), "0123456789abcdef");
    }

    #[test]
    fn hash_from_str_rejects_uppercase() {
        assert!(hash_from_str("0123456789ABCDEF").is_err());
    }

    #[test]
    fn hash_from_str_rejects_short() {
        assert!(hash_from_str("0123456789abcde").is_err());
    }

    #[test]
    fn hash_from_str_rejects_long() {
        assert!(hash_from_str("0123456789abcdef0").is_err());
    }

    #[test]
    fn hash_from_str_rejects_non_hex() {
        assert!(hash_from_str("0123456789abcdez").is_err());
    }

    #[test]
    fn into_string_works() {
        let h = hash_from_str("aabbccddeeff0011").unwrap();
        let s: String = h.into();
        assert_eq!(s, "aabbccddeeff0011");
    }

    // -----------------------------------------------------------------
    // T-SHAPE — output shape
    // -----------------------------------------------------------------

    #[test]
    fn content_hash_is_16_lower_hex() {
        let h = content_hash(&Role::User, &[]);
        assert_eq!(h.as_ref().len(), 16);
        assert!(
            h.as_ref()
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        );
    }

    // -----------------------------------------------------------------
    // T-DET — determinism
    // -----------------------------------------------------------------

    #[test]
    fn content_hash_is_deterministic() {
        let role = Role::Assistant;
        let content = vec![ContentBlock::Text {
            text: "hello".into(),
        }];
        assert_eq!(content_hash(&role, &content), content_hash(&role, &content),);
    }

    // -----------------------------------------------------------------
    // T-DIST — distinctness across meaningful changes
    // -----------------------------------------------------------------

    fn text(s: &str) -> ContentBlock {
        ContentBlock::Text { text: s.into() }
    }

    #[test]
    fn dist_different_text() {
        let a = content_hash(&Role::User, &[text("hello")]);
        let b = content_hash(&Role::User, &[text("world")]);
        assert_ne!(a, b);
    }

    #[test]
    fn dist_different_role() {
        let c = vec![text("hello")];
        assert_ne!(
            content_hash(&Role::User, &c),
            content_hash(&Role::Assistant, &c),
        );
    }

    #[test]
    fn dist_different_block_kind_same_text() {
        let a = content_hash(&Role::Assistant, &[text("x")]);
        let b = content_hash(
            &Role::Assistant,
            &[ContentBlock::Thinking {
                thinking: "x".into(),
                signature: None,
            }],
        );
        assert_ne!(a, b);
    }

    #[test]
    fn dist_different_block_order() {
        let a = content_hash(&Role::Assistant, &[text("a"), text("b")]);
        let b = content_hash(&Role::Assistant, &[text("b"), text("a")]);
        assert_ne!(a, b);
    }

    #[test]
    fn dist_different_thinking_signature() {
        let a = content_hash(
            &Role::Assistant,
            &[ContentBlock::Thinking {
                thinking: "t".into(),
                signature: Some("sig-1".into()),
            }],
        );
        let b = content_hash(
            &Role::Assistant,
            &[ContentBlock::Thinking {
                thinking: "t".into(),
                signature: Some("sig-2".into()),
            }],
        );
        assert_ne!(a, b);
    }

    #[test]
    fn dist_thinking_signature_some_vs_none() {
        let a = content_hash(
            &Role::Assistant,
            &[ContentBlock::Thinking {
                thinking: "t".into(),
                signature: Some("sig".into()),
            }],
        );
        let b = content_hash(
            &Role::Assistant,
            &[ContentBlock::Thinking {
                thinking: "t".into(),
                signature: None,
            }],
        );
        assert_ne!(a, b);
    }

    #[test]
    fn dist_same_signature_different_thinking_text() {
        let a = content_hash(
            &Role::Assistant,
            &[ContentBlock::Thinking {
                thinking: "alpha".into(),
                signature: Some("sig".into()),
            }],
        );
        let b = content_hash(
            &Role::Assistant,
            &[ContentBlock::Thinking {
                thinking: "beta".into(),
                signature: Some("sig".into()),
            }],
        );
        assert_ne!(a, b);
    }

    #[test]
    fn dist_different_tool_use_id() {
        let a = content_hash(
            &Role::Assistant,
            &[ContentBlock::ToolUse {
                id: "tu_1".into(),
                name: "read_file".into(),
                input: json!({"path": "x"}),
            }],
        );
        let b = content_hash(
            &Role::Assistant,
            &[ContentBlock::ToolUse {
                id: "tu_2".into(),
                name: "read_file".into(),
                input: json!({"path": "x"}),
            }],
        );
        assert_ne!(a, b);
    }

    #[test]
    fn dist_different_tool_use_name() {
        let a = content_hash(
            &Role::Assistant,
            &[ContentBlock::ToolUse {
                id: "tu_1".into(),
                name: "read_file".into(),
                input: json!({}),
            }],
        );
        let b = content_hash(
            &Role::Assistant,
            &[ContentBlock::ToolUse {
                id: "tu_1".into(),
                name: "write_file".into(),
                input: json!({}),
            }],
        );
        assert_ne!(a, b);
    }

    #[test]
    fn dist_different_tool_use_input() {
        let a = content_hash(
            &Role::Assistant,
            &[ContentBlock::ToolUse {
                id: "tu_1".into(),
                name: "read_file".into(),
                input: json!({"path": "a"}),
            }],
        );
        let b = content_hash(
            &Role::Assistant,
            &[ContentBlock::ToolUse {
                id: "tu_1".into(),
                name: "read_file".into(),
                input: json!({"path": "b"}),
            }],
        );
        assert_ne!(a, b);
    }

    #[test]
    fn dist_different_tool_result_is_error() {
        let a = content_hash(
            &Role::User,
            &[ContentBlock::ToolResult {
                tool_use_id: "tu_1".into(),
                content: "x".into(),
                is_error: false,
            }],
        );
        let b = content_hash(
            &Role::User,
            &[ContentBlock::ToolResult {
                tool_use_id: "tu_1".into(),
                content: "x".into(),
                is_error: true,
            }],
        );
        assert_ne!(a, b);
    }

    #[test]
    fn dist_empty_content_different_role() {
        assert_ne!(
            content_hash(&Role::User, &[]),
            content_hash(&Role::Assistant, &[]),
        );
    }

    // -----------------------------------------------------------------
    // T-LOCK — locked-in fixtures (canary for canonical-form drift)
    //
    // If any of these tests fails, the canonical hash form has drifted.
    // Every previously-saved session's hashes will be invalidated.
    // STOP and discuss before updating these strings.  Bumping a
    // lockdown value is a session-invalidation event.
    //
    // Each fixture's `(role, content)` is documented above its
    // assertion in plain English so a reviewer can spot-check the
    // fixture without running the code.
    // -----------------------------------------------------------------

    #[test]
    fn lock1_user_hello() {
        // Role: User
        // Content: one Text block, text = "hello"
        let role = Role::User;
        let content = vec![ContentBlock::Text {
            text: "hello".into(),
        }];
        assert_eq!(content_hash(&role, &content).as_ref(), "9f2e4309a794fdf6");
    }

    #[test]
    fn lock2_assistant_ok() {
        // Role: Assistant
        // Content: one Text block, text = "ok"
        let role = Role::Assistant;
        let content = vec![ContentBlock::Text { text: "ok".into() }];
        assert_eq!(content_hash(&role, &content).as_ref(), "2e510e919b5b5cbe");
    }

    #[test]
    fn lock3_assistant_thinking_signed() {
        // Role: Assistant
        // Content: one Thinking block, thinking = "let me see",
        //          signature = Some("sig-abc")
        let role = Role::Assistant;
        let content = vec![ContentBlock::Thinking {
            thinking: "let me see".into(),
            signature: Some("sig-abc".into()),
        }];
        assert_eq!(content_hash(&role, &content).as_ref(), "8f76c965d0fafe2f");
    }

    #[test]
    fn lock4_assistant_thinking_unsigned() {
        // Role: Assistant
        // Content: one Thinking block, thinking = "let me see",
        //          signature = None
        let role = Role::Assistant;
        let content = vec![ContentBlock::Thinking {
            thinking: "let me see".into(),
            signature: None,
        }];
        assert_eq!(content_hash(&role, &content).as_ref(), "ca3a241139199ad7");
    }

    #[test]
    fn lock5_assistant_multiblock() {
        // Role: Assistant
        // Content: three blocks in this order:
        //   1. Thinking { thinking = "planning", signature = Some("sig-xyz") }
        //   2. Text { text = "I'll read the file." }
        //   3. ToolUse { id = "tu_1", name = "read_file",
        //                input = {"path": "src/lib.rs"} }
        let role = Role::Assistant;
        let content = vec![
            ContentBlock::Thinking {
                thinking: "planning".into(),
                signature: Some("sig-xyz".into()),
            },
            ContentBlock::Text {
                text: "I'll read the file.".into(),
            },
            ContentBlock::ToolUse {
                id: "tu_1".into(),
                name: "read_file".into(),
                input: json!({"path": "src/lib.rs"}),
            },
        ];
        assert_eq!(content_hash(&role, &content).as_ref(), "c5752c92fd82da20");
    }

    #[test]
    fn lock6_user_tool_result() {
        // Role: User
        // Content: one ToolResult { tool_use_id = "tu_1",
        //                           content = "result body",
        //                           is_error = false }
        let role = Role::User;
        let content = vec![ContentBlock::ToolResult {
            tool_use_id: "tu_1".into(),
            content: "result body".into(),
            is_error: false,
        }];
        assert_eq!(content_hash(&role, &content).as_ref(), "ed7b06022b2da70f");
    }

    #[test]
    fn lock7_user_empty() {
        // Role: User
        // Content: empty vector
        let role = Role::User;
        let content: Vec<ContentBlock> = vec![];
        assert_eq!(content_hash(&role, &content).as_ref(), "3084ab9589db83c1");
    }

    // -----------------------------------------------------------------
    // T-PAIRWISE — pairwise distinctness over generated samples
    // -----------------------------------------------------------------

    #[test]
    fn content_hashes_distinct_across_synthetic_samples() {
        let mut seen = std::collections::HashSet::new();
        let mut count = 0usize;
        for role in [Role::User, Role::Assistant] {
            // text variations
            for i in 0..200 {
                let c = vec![text(&format!("msg-{i}"))];
                let h = content_hash(&role, &c);
                assert!(seen.insert(h), "collision at text {i} role {role:?}");
                count += 1;
            }
            // thinking variations (signed and unsigned)
            for i in 0..150 {
                let c = vec![ContentBlock::Thinking {
                    thinking: format!("th-{i}"),
                    signature: if i % 2 == 0 {
                        Some(format!("sig-{i}"))
                    } else {
                        None
                    },
                }];
                let h = content_hash(&role, &c);
                assert!(seen.insert(h), "collision at thinking {i}");
                count += 1;
            }
            // tool_use variations
            for i in 0..150 {
                let c = vec![ContentBlock::ToolUse {
                    id: format!("tu_{i}"),
                    name: "tool".into(),
                    input: json!({"k": i}),
                }];
                let h = content_hash(&role, &c);
                assert!(seen.insert(h), "collision at tool_use {i}");
                count += 1;
            }
            // tool_result variations
            for i in 0..150 {
                let c = vec![ContentBlock::ToolResult {
                    tool_use_id: format!("tu_{i}"),
                    content: format!("body-{i}"),
                    is_error: i % 3 == 0,
                }];
                let h = content_hash(&role, &c);
                assert!(seen.insert(h), "collision at tool_result {i}");
                count += 1;
            }
        }
        assert!(count >= 1000, "expected ≥1000 samples, got {count}");
    }

    // -----------------------------------------------------------------
    // Fixture-print helper — run with
    //   cargo test -p omega-store print_lockdown_fixtures -- --nocapture --ignored
    // to print the canonical bytes and computed hash for each LOCK-N
    // fixture.  Used once during HASH-1 implementation to lock the
    // values into the lockN_* tests above.
    // -----------------------------------------------------------------

    #[test]
    #[ignore = "fixture-print helper; run with --ignored --nocapture"]
    fn print_lockdown_fixtures() {
        fn show(label: &str, role: Role, content: &[ContentBlock]) {
            let canonical = serde_json::to_vec(&(&role, content)).unwrap();
            let canonical_str = std::str::from_utf8(&canonical).unwrap();
            let h = content_hash(&role, content);
            println!("{label}");
            println!("  canonical: {canonical_str}");
            println!("  hash:      {}", h.as_ref());
        }
        show(
            "LOCK-1 (User, [Text { \"hello\" }])",
            Role::User,
            &[ContentBlock::Text {
                text: "hello".into(),
            }],
        );
        show(
            "LOCK-2 (Assistant, [Text { \"ok\" }])",
            Role::Assistant,
            &[ContentBlock::Text { text: "ok".into() }],
        );
        show(
            "LOCK-3 (Assistant, [Thinking { \"let me see\", Some(\"sig-abc\") }])",
            Role::Assistant,
            &[ContentBlock::Thinking {
                thinking: "let me see".into(),
                signature: Some("sig-abc".into()),
            }],
        );
        show(
            "LOCK-4 (Assistant, [Thinking { \"let me see\", None }])",
            Role::Assistant,
            &[ContentBlock::Thinking {
                thinking: "let me see".into(),
                signature: None,
            }],
        );
        show(
            "LOCK-5 (Assistant, [Thinking, Text, ToolUse])",
            Role::Assistant,
            &[
                ContentBlock::Thinking {
                    thinking: "planning".into(),
                    signature: Some("sig-xyz".into()),
                },
                ContentBlock::Text {
                    text: "I'll read the file.".into(),
                },
                ContentBlock::ToolUse {
                    id: "tu_1".into(),
                    name: "read_file".into(),
                    input: json!({"path": "src/lib.rs"}),
                },
            ],
        );
        show(
            "LOCK-6 (User, [ToolResult { \"tu_1\", \"result body\", false }])",
            Role::User,
            &[ContentBlock::ToolResult {
                tool_use_id: "tu_1".into(),
                content: "result body".into(),
                is_error: false,
            }],
        );
        show("LOCK-7 (User, [])", Role::User, &[]);
    }
}
