//! Runtime feature flags for the Omega agent.
//!
//! [`FeatureFlags`] currently carries a single capability switch
//! ([`FeatureFlags::subagents`]).  It is recorded on the agent's runtime
//! state (not on `SessionStartedEvent` — the toolset is captured separately
//! via `tool_selection`).
//!
//! See: docs/repl-and-substrates.html.

use serde::{Deserialize, Serialize};

/// Runtime feature flags controlling optional agent capabilities.
///
/// `subagents` defaults to `false`.  Loaded from environment variables at
/// agent startup via [`FeatureFlags::from_env`].
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct FeatureFlags {
    /// Enable subagent spawning.
    ///
    /// Set `OMEGA_FEATURE_SUBAGENTS=1` or `OMEGA_FEATURE_SUBAGENTS=true`.
    pub subagents: bool,
}

impl FeatureFlags {
    /// Read feature flags from environment variables.
    ///
    /// | Variable                                | Flag                                               |
    /// |-----------------------------------------|----------------------------------------------------|
    /// | `OMEGA_FEATURE_SUBAGENTS`               | [`FeatureFlags::subagents`]                        |
    ///
    /// Truthy values: `"1"` or `"true"` (case-insensitive).\
    /// Falsy values: `"0"`, `"false"`, `""`, or the variable being unset —
    /// all map to `false` silently.\
    /// Any other value maps to `false` and emits a warning to stderr; the
    /// agent continues to start normally.
    ///
    /// # Env-var thread-safety note
    ///
    /// This function reads from the process environment and is therefore not
    /// safe to call concurrently with [`std::env::set_var`] in the same
    /// process.  In production the call happens once during agent startup
    /// before any threads are spawned that could race on env vars.
    /// # Mutation-testing note
    ///
    /// `#[mutants::skip]` is needed because the mutation
    /// `replace from_env -> Self with Default::default()` cannot be caught
    /// without calling `std::env::set_var` / `remove_var`, which are `unsafe`
    /// in Rust 2024 edition.  The workspace forbids `unsafe_code`, so a
    /// direct env-var test is not possible.  The parsing logic itself is
    /// fully covered by the `from_values` tests.
    #[mutants::skip]
    #[must_use]
    pub fn from_env() -> Self {
        let subagents_val = std::env::var("OMEGA_FEATURE_SUBAGENTS").ok();
        Self::from_values(subagents_val.as_deref())
    }

    /// Parse feature flags from raw string values (as if read from env vars).
    ///
    /// `None` means the variable was not set.\
    /// `Some(s)` means the variable was set to `s`.
    ///
    /// This is the testable core; [`FeatureFlags::from_env`] is a thin
    /// wrapper around it.
    #[must_use]
    pub(crate) fn from_values(subagents_val: Option<&str>) -> Self {
        Self {
            subagents: parse_flag_value("OMEGA_FEATURE_SUBAGENTS", subagents_val),
        }
    }
}

/// Parse a single feature-flag value that came from an environment variable.
///
/// - `None`  (variable not set) → `false`, no warning.
/// - `"1"` or `"true"` (case-insensitive) → `true`.
/// - `"0"`, `"false"`, or `""` → `false`, no warning.
/// - Any other non-empty string → `false`, emits a warning to stderr.
fn parse_flag_value(name: &str, raw: Option<&str>) -> bool {
    let Some(val) = raw else {
        return false;
    };
    match val.to_ascii_lowercase().as_str() {
        "1" | "true" => true,
        "0" | "false" | "" => false,
        other => {
            eprintln!(
                "warning: {name}={other:?} is not a recognised feature-flag value; \
                 expected \"1\", \"true\", \"0\", or \"false\" — defaulting to off"
            );
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    // Unit tests: these test the pure parsing logic (`parse_flag_value` /
    // `from_values`) rather than `from_env` itself.  `from_env` reads from
    // process environment variables.  In Rust 2024 edition, `set_var` and
    // `remove_var` are `unsafe`, and the workspace forbids `unsafe_code`,
    // so direct env-var tests are not possible here.  The `from_env` body
    // is therefore marked `#[mutants::skip]`; the parsing logic under test
    // is fully covered by `from_values` tests.

    #[test]
    fn default_subagents_off() {
        let flags = FeatureFlags::default();
        assert!(!flags.subagents, "default subagents must be false");
    }

    #[test]
    fn unset_subagents_is_false() {
        let f = FeatureFlags::from_values(None);
        assert!(!f.subagents);
    }

    #[test]
    fn truthy_one_enables_subagents() {
        let f = FeatureFlags::from_values(Some("1"));
        assert!(f.subagents);
    }

    #[test]
    fn truthy_true_lowercase_enables_subagents() {
        let f = FeatureFlags::from_values(Some("true"));
        assert!(f.subagents);
    }

    #[test]
    fn truthy_true_uppercase_enables_subagents() {
        let f = FeatureFlags::from_values(Some("TRUE"));
        assert!(f.subagents);
    }

    #[test]
    fn truthy_true_mixed_case_enables_subagents() {
        let f = FeatureFlags::from_values(Some("True"));
        assert!(f.subagents);
    }

    #[test]
    fn falsy_zero_disables_subagents() {
        let f = FeatureFlags::from_values(Some("0"));
        assert!(!f.subagents);
    }

    #[test]
    fn falsy_false_disables_subagents() {
        let f = FeatureFlags::from_values(Some("false"));
        assert!(!f.subagents);
    }

    #[test]
    fn falsy_empty_string_disables_subagents() {
        let f = FeatureFlags::from_values(Some(""));
        assert!(!f.subagents);
    }

    #[test]
    fn garbage_value_defaults_to_false_subagents() {
        let f = FeatureFlags::from_values(Some("yes"));
        assert!(
            !f.subagents,
            "garbage subagents value must default to false"
        );
    }

    #[test]
    fn serde_round_trip_default() {
        let flags = FeatureFlags::default();
        let json = serde_json::to_string(&flags).unwrap();
        let back: FeatureFlags = serde_json::from_str(&json).unwrap();
        assert_eq!(flags, back);
    }

    #[test]
    fn serde_round_trip_subagents_on() {
        let flags = FeatureFlags { subagents: true };
        let json = serde_json::to_string(&flags).unwrap();
        let back: FeatureFlags = serde_json::from_str(&json).unwrap();
        assert_eq!(flags, back);
    }

    #[test]
    fn serde_json_field_names() {
        let flags = FeatureFlags { subagents: true };
        let v: serde_json::Value = serde_json::to_value(flags).unwrap();
        assert_eq!(v["subagents"], true);
    }
}
