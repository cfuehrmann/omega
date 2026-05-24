//! Runtime feature flags for the Omega agent.
//!
//! [`FeatureFlags`] is recorded in every [`SessionStartedEvent`] so forensic
//! analysis can identify which features were active in a given session,
//! making the four-cell matrix (vanilla / REPL-only / subagents-only / RLM)
//! visible without analysing behaviour.
//!
//! See: docs/repl-and-subagents-research.html §8 and "Next steps" step 1.
//!
//! [`SessionStartedEvent`]: crate::events::SessionStartedEvent

use serde::{Deserialize, Serialize};

/// Runtime feature flags controlling optional agent capabilities.
///
/// All flags default to `false` — the "vanilla" cell in the four-cell matrix
/// (vanilla / REPL-only / subagents-only / RLM).
///
/// Loaded from environment variables at agent startup via
/// [`FeatureFlags::from_env`]. Recorded in every
/// [`SessionStartedEvent`](crate::events::SessionStartedEvent) for forensic
/// traceability.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct FeatureFlags {
    /// Enable the REPL tool.
    ///
    /// Set `OMEGA_FEATURE_REPL=1` or `OMEGA_FEATURE_REPL=true`.
    pub repl: bool,
    /// Enable subagent spawning.
    ///
    /// Set `OMEGA_FEATURE_SUBAGENTS=1` or `OMEGA_FEATURE_SUBAGENTS=true`.
    pub subagents: bool,
    /// Experimental.  When true, removes six file-op tools
    /// (`read_file`, `write_file`, `edit_file`, `find_files`, `grep_files`,
    /// `list_files`) from the tool list so that `python_repl` is the
    /// primary alternative for file work.  Requires `repl=true`.
    ///
    /// Set `OMEGA_FEATURE_REPL_REPLACES_FILEOPS=1` or
    /// `OMEGA_FEATURE_REPL_REPLACES_FILEOPS=true`.
    ///
    /// Validation: setting this flag without `repl=true` is a configuration
    /// error.  [`FeatureFlags::validate`] returns
    /// [`FeatureFlagsConfigError::ReplLimitWithoutRepl`] in that case.
    pub repl_replaces_fileops: bool,
}

/// Configuration errors detected during [`FeatureFlags::validate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeatureFlagsConfigError {
    /// `OMEGA_FEATURE_REPL_REPLACES_FILEOPS=1` was set without
    /// `OMEGA_FEATURE_REPL=1`.  The limit-mode flag has no effect without a
    /// REPL to replace the removed tools.
    ReplLimitWithoutRepl,
}

impl std::fmt::Display for FeatureFlagsConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ReplLimitWithoutRepl => write!(
                f,
                "OMEGA_FEATURE_REPL_REPLACES_FILEOPS=1 requires OMEGA_FEATURE_REPL=1"
            ),
        }
    }
}

impl std::error::Error for FeatureFlagsConfigError {}

impl FeatureFlags {
    /// Read feature flags from environment variables.
    ///
    /// | Variable                                | Flag                                               |
    /// |-----------------------------------------|----------------------------------------------------|
    /// | `OMEGA_FEATURE_REPL`                    | [`FeatureFlags::repl`]                             |
    /// | `OMEGA_FEATURE_SUBAGENTS`               | [`FeatureFlags::subagents`]                        |
    /// | `OMEGA_FEATURE_REPL_REPLACES_FILEOPS`   | [`FeatureFlags::repl_replaces_fileops`]            |
    ///
    /// Truthy values: `"1"` or `"true"` (case-insensitive).\
    /// Falsy values: `"0"`, `"false"`, `""`, or the variable being unset —
    /// all map to `false` silently.\
    /// Any other value maps to `false` and emits a warning to stderr; the
    /// agent continues to start normally.
    ///
    /// **Does not validate** cross-flag constraints.  Call
    /// [`FeatureFlags::validate`] separately (done inside
    /// [`Agent::init`](crate::Agent::init)) to catch invalid combinations.
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
        let repl_val = std::env::var("OMEGA_FEATURE_REPL").ok();
        let subagents_val = std::env::var("OMEGA_FEATURE_SUBAGENTS").ok();
        let repl_replaces_fileops_val = std::env::var("OMEGA_FEATURE_REPL_REPLACES_FILEOPS").ok();
        Self::from_values(
            repl_val.as_deref(),
            subagents_val.as_deref(),
            repl_replaces_fileops_val.as_deref(),
        )
    }

    /// Parse feature flags from raw string values (as if read from env vars).
    ///
    /// `None` means the variable was not set.\
    /// `Some(s)` means the variable was set to `s`.
    ///
    /// This is the testable core; [`FeatureFlags::from_env`] is a thin
    /// wrapper around it.
    ///
    /// **Does not validate** cross-flag constraints — call
    /// [`FeatureFlags::validate`] for that.
    #[must_use]
    pub(crate) fn from_values(
        repl_val: Option<&str>,
        subagents_val: Option<&str>,
        repl_replaces_fileops_val: Option<&str>,
    ) -> Self {
        Self {
            repl: parse_flag_value("OMEGA_FEATURE_REPL", repl_val),
            subagents: parse_flag_value("OMEGA_FEATURE_SUBAGENTS", subagents_val),
            repl_replaces_fileops: parse_flag_value(
                "OMEGA_FEATURE_REPL_REPLACES_FILEOPS",
                repl_replaces_fileops_val,
            ),
        }
    }

    /// Validate cross-flag constraints.
    ///
    /// Currently checks:
    /// - [`FeatureFlagsConfigError::ReplLimitWithoutRepl`][]: `repl_replaces_fileops`
    ///   requires `repl`.
    ///
    /// Called by [`Agent::init`](crate::Agent::init) at startup; the agent
    /// exits with a clear error message if validation fails.
    ///
    /// # Errors
    ///
    /// Returns `Err(FeatureFlagsConfigError::ReplLimitWithoutRepl)` when
    /// `repl_replaces_fileops` is `true` but `repl` is `false`.
    pub fn validate(self) -> Result<(), FeatureFlagsConfigError> {
        if self.repl_replaces_fileops && !self.repl {
            return Err(FeatureFlagsConfigError::ReplLimitWithoutRepl);
        }
        Ok(())
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

    // -----------------------------------------------------------------------
    // FeatureFlags::default

    #[test]
    fn default_all_off() {
        let flags = FeatureFlags::default();
        assert!(!flags.repl, "default repl must be false");
        assert!(!flags.subagents, "default subagents must be false");
        assert!(
            !flags.repl_replaces_fileops,
            "default repl_replaces_fileops must be false"
        );
    }

    // -----------------------------------------------------------------------
    // parse_flag_value (via from_values)

    #[test]
    fn unset_repl_is_false() {
        let f = FeatureFlags::from_values(None, None, None);
        assert!(!f.repl);
    }

    #[test]
    fn unset_subagents_is_false() {
        let f = FeatureFlags::from_values(None, None, None);
        assert!(!f.subagents);
    }

    #[test]
    fn unset_repl_replaces_fileops_is_false() {
        let f = FeatureFlags::from_values(None, None, None);
        assert!(!f.repl_replaces_fileops);
    }

    #[test]
    fn truthy_one_enables_repl() {
        let f = FeatureFlags::from_values(Some("1"), None, None);
        assert!(f.repl);
        assert!(!f.subagents);
        assert!(!f.repl_replaces_fileops);
    }

    #[test]
    fn truthy_true_lowercase_enables_subagents() {
        let f = FeatureFlags::from_values(None, Some("true"), None);
        assert!(!f.repl);
        assert!(f.subagents);
    }

    #[test]
    fn truthy_true_uppercase_enables_repl() {
        let f = FeatureFlags::from_values(Some("TRUE"), None, None);
        assert!(f.repl);
    }

    #[test]
    fn truthy_true_mixed_case_enables_repl() {
        let f = FeatureFlags::from_values(Some("True"), None, None);
        assert!(f.repl);
    }

    #[test]
    fn falsy_zero_disables_repl() {
        let f = FeatureFlags::from_values(Some("0"), None, None);
        assert!(!f.repl);
    }

    #[test]
    fn falsy_false_disables_subagents() {
        let f = FeatureFlags::from_values(None, Some("false"), None);
        assert!(!f.subagents);
    }

    #[test]
    fn falsy_empty_string_disables_repl() {
        let f = FeatureFlags::from_values(Some(""), None, None);
        assert!(!f.repl);
    }

    #[test]
    fn garbage_value_defaults_to_false_repl() {
        // "yes" is not a recognised value; must default to false without panic.
        let f = FeatureFlags::from_values(Some("yes"), None, None);
        assert!(!f.repl, "garbage repl value must default to false");
    }

    #[test]
    fn garbage_value_defaults_to_false_subagents() {
        let f = FeatureFlags::from_values(None, Some("on"), None);
        assert!(
            !f.subagents,
            "garbage subagents value must default to false"
        );
    }

    #[test]
    fn both_flags_on() {
        let f = FeatureFlags::from_values(Some("1"), Some("1"), None);
        assert!(f.repl);
        assert!(f.subagents);
    }

    #[test]
    fn repl_on_subagents_off() {
        let f = FeatureFlags::from_values(Some("true"), Some("0"), None);
        assert!(f.repl);
        assert!(!f.subagents);
    }

    #[test]
    fn repl_off_subagents_on() {
        let f = FeatureFlags::from_values(Some("false"), Some("1"), None);
        assert!(!f.repl);
        assert!(f.subagents);
    }

    // -----------------------------------------------------------------------
    // repl_replaces_fileops parsing

    #[test]
    fn truthy_one_enables_repl_replaces_fileops() {
        let f = FeatureFlags::from_values(Some("1"), None, Some("1"));
        assert!(f.repl_replaces_fileops);
    }

    #[test]
    fn truthy_true_enables_repl_replaces_fileops() {
        let f = FeatureFlags::from_values(Some("1"), None, Some("true"));
        assert!(f.repl_replaces_fileops);
    }

    #[test]
    fn truthy_true_uppercase_enables_repl_replaces_fileops() {
        let f = FeatureFlags::from_values(Some("1"), None, Some("TRUE"));
        assert!(f.repl_replaces_fileops);
    }

    #[test]
    fn falsy_zero_disables_repl_replaces_fileops() {
        let f = FeatureFlags::from_values(Some("1"), None, Some("0"));
        assert!(!f.repl_replaces_fileops);
    }

    #[test]
    fn falsy_false_disables_repl_replaces_fileops() {
        let f = FeatureFlags::from_values(Some("1"), None, Some("false"));
        assert!(!f.repl_replaces_fileops);
    }

    #[test]
    fn garbage_value_defaults_to_false_repl_replaces_fileops() {
        let f = FeatureFlags::from_values(Some("1"), None, Some("yes"));
        assert!(!f.repl_replaces_fileops);
    }

    #[test]
    fn all_three_flags_on() {
        let f = FeatureFlags::from_values(Some("1"), Some("1"), Some("1"));
        assert!(f.repl);
        assert!(f.subagents);
        assert!(f.repl_replaces_fileops);
    }

    // -----------------------------------------------------------------------
    // validate

    #[test]
    fn validate_all_off_is_ok() {
        let f = FeatureFlags {
            repl: false,
            subagents: false,
            repl_replaces_fileops: false,
        };
        assert!(f.validate().is_ok());
    }

    #[test]
    fn validate_repl_on_only_is_ok() {
        let f = FeatureFlags {
            repl: true,
            subagents: false,
            repl_replaces_fileops: false,
        };
        assert!(f.validate().is_ok());
    }

    #[test]
    fn validate_repl_and_limit_mode_is_ok() {
        let f = FeatureFlags {
            repl: true,
            subagents: false,
            repl_replaces_fileops: true,
        };
        assert!(f.validate().is_ok());
    }

    #[test]
    fn validate_limit_mode_without_repl_is_error() {
        let f = FeatureFlags {
            repl: false,
            subagents: false,
            repl_replaces_fileops: true,
        };
        let err = f.validate().unwrap_err();
        assert_eq!(err, FeatureFlagsConfigError::ReplLimitWithoutRepl);
    }

    #[test]
    fn validate_error_message_contains_env_var_names() {
        let err = FeatureFlagsConfigError::ReplLimitWithoutRepl;
        let msg = err.to_string();
        assert!(
            msg.contains("OMEGA_FEATURE_REPL_REPLACES_FILEOPS"),
            "message must name the offending var: {msg}"
        );
        assert!(
            msg.contains("OMEGA_FEATURE_REPL"),
            "message must name the required var: {msg}"
        );
    }

    #[test]
    fn validate_subagents_on_only_is_ok() {
        let f = FeatureFlags {
            repl: false,
            subagents: true,
            repl_replaces_fileops: false,
        };
        assert!(f.validate().is_ok());
    }

    // -----------------------------------------------------------------------
    // Serde round-trips

    #[test]
    fn serde_round_trip_default() {
        let flags = FeatureFlags::default();
        let json = serde_json::to_string(&flags).unwrap();
        let back: FeatureFlags = serde_json::from_str(&json).unwrap();
        assert_eq!(flags, back);
    }

    #[test]
    fn serde_round_trip_repl_on() {
        let flags = FeatureFlags {
            repl: true,
            subagents: false,
            repl_replaces_fileops: false,
        };
        let json = serde_json::to_string(&flags).unwrap();
        let back: FeatureFlags = serde_json::from_str(&json).unwrap();
        assert_eq!(flags, back);
    }

    #[test]
    fn serde_round_trip_both_on() {
        let flags = FeatureFlags {
            repl: true,
            subagents: true,
            repl_replaces_fileops: false,
        };
        let json = serde_json::to_string(&flags).unwrap();
        let back: FeatureFlags = serde_json::from_str(&json).unwrap();
        assert_eq!(flags, back);
    }

    #[test]
    fn serde_round_trip_limit_mode() {
        let flags = FeatureFlags {
            repl: true,
            subagents: false,
            repl_replaces_fileops: true,
        };
        let json = serde_json::to_string(&flags).unwrap();
        let back: FeatureFlags = serde_json::from_str(&json).unwrap();
        assert_eq!(flags, back);
    }

    #[test]
    fn serde_json_field_names() {
        let flags = FeatureFlags {
            repl: true,
            subagents: false,
            repl_replaces_fileops: true,
        };
        let v: serde_json::Value = serde_json::to_value(flags).unwrap();
        assert_eq!(v["repl"], true);
        assert_eq!(v["subagents"], false);
        assert_eq!(v["repl_replaces_fileops"], true);
    }
}
