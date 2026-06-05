//! Canonical tool-name lists, preset registry, and pure selection helpers.
//!
//! This module is the **single source of truth** for:
//!
//! - [`DEFAULT_TOOL_NAMES`] / [`ALL_TOOL_NAMES`] / [`REPL_CENTRIC_TOOLS`]
//! - [`Preset`] and the [`PRESETS`] registry
//! - Pure selection helpers: [`default_tool_selection`], [`resolve_preset`],
//!   [`serialize_selection`], [`parse_stored_selection`]
//!
//! Both the native/CLI side (`omega-tools`) and the wasm-only frontend
//! (`omega-web`) depend on `omega-types`, so placing the canonical data
//! here ensures structural agreement — there is no mirror to drift.
//!
//! ## Why omega-types?
//!
//! `omega-tools` is native-only (tokio, reqwest, …) and cannot be linked
//! into the wasm32 frontend.  `omega-types` has only `serde`/`serde_json`/
//! `uuid` as dependencies — all wasm-safe.  The frontend's `protocol.rs`
//! re-exports from here; `omega-tools/schemas.rs` also re-exports from here
//! so existing call sites (`omega_tools::DEFAULT_TOOL_NAMES`, `::PRESETS`,
//! `::preset_by_id`, …) keep compiling without change.

use serde_json;

// ---------------------------------------------------------------------------
// Tool-name constants
// ---------------------------------------------------------------------------

/// The default toolset — 14 tools (file ops + shell + web + monitors).
///
/// Used when [`AgentConfig::tool_selection`] is `None`.  Order is canonical
/// and matches the order `tool_definitions` emits.  `python_repl` is opt-in
/// (only in the `all` preset); everything else is on by default, including
/// the two monitor tools.
///
/// [`AgentConfig::tool_selection`]: ../../omega_agent/struct.AgentConfig.html#structfield.tool_selection
pub const DEFAULT_TOOL_NAMES: &[&str] = &[
    "read_file",
    "write_file",
    "run_command",
    "edit_file",
    "list_files",
    "web_search",
    "fetch_url",
    "grep_files",
    "find_files",
    "run_background",
    "wait_for_output",
    "write_stdin",
    "monitor",
    "stop_monitor",
];

/// Every tool Omega knows how to expose, in canonical order.
///
/// `python_repl` is in `ALL_TOOL_NAMES` but not in [`DEFAULT_TOOL_NAMES`] —
/// it must be requested explicitly via `AgentConfig::tool_selection` (the
/// `all` preset includes it).  All other tools — including `monitor` and
/// `stop_monitor` — are in both.
///
/// Names not present in this list are rejected by the agent at session
/// creation time.
pub const ALL_TOOL_NAMES: &[&str] = &[
    "read_file",
    "write_file",
    "run_command",
    "edit_file",
    "list_files",
    "web_search",
    "fetch_url",
    "grep_files",
    "find_files",
    "run_background",
    "wait_for_output",
    "write_stdin",
    "monitor",
    "stop_monitor",
    "python_repl",
];

/// Tools in the REPL-centric preset: Python REPL plus web tools plus
/// monitors.  File I/O and subprocess work live inside the REPL.
pub(crate) const REPL_CENTRIC_TOOLS: &[&str] = &[
    "python_repl",
    "web_search",
    "fetch_url",
    "monitor",
    "stop_monitor",
];

// ---------------------------------------------------------------------------
// Preset registry
// ---------------------------------------------------------------------------

/// A named tool-selection preset.
///
/// Single source of truth for the CLI (`omega run --preset <id>`) and the UI
/// tool-picker chips (label + description).  Adding a new preset requires
/// extending only [`PRESETS`] — both surfaces pick it up automatically.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Preset {
    /// CLI identifier (kebab-case, lowercase).  Stable wire value.
    pub id: &'static str,
    /// UI chip label.  May contain Unicode and punctuation.
    pub label: &'static str,
    /// Tools enabled by this preset, in canonical order.
    pub tools: &'static [&'static str],
    /// Short description — used in `--help` and (later) UI tooltips.
    pub description: &'static str,
}

/// All known presets, in display order.  See [`Preset`].
///
/// Order is the order the UI shows preset chips and the CLI lists
/// `--preset` choices in `--help`.
pub const PRESETS: &[Preset] = &[
    Preset {
        id: "standard",
        label: "Standard",
        tools: DEFAULT_TOOL_NAMES,
        description: "14 tools — file ops, shell, web, monitors (no Python REPL)",
    },
    Preset {
        id: "all",
        label: "+ Python REPL & monitors",
        tools: ALL_TOOL_NAMES,
        description: "All 15 tools — standard plus python_repl",
    },
    Preset {
        id: "repl-centric",
        label: "REPL-centric",
        tools: REPL_CENTRIC_TOOLS,
        description: "Python REPL plus web tools and monitors; file/shell work inside the REPL",
    },
];

/// Look up a preset by its CLI id.  Returns `None` for unknown ids.
#[must_use]
pub fn preset_by_id(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.id == id)
}

// ---------------------------------------------------------------------------
// Pure selection helpers
// ---------------------------------------------------------------------------

/// Standard preset materialised as a fresh `Vec<String>` — the fallback
/// when localStorage is empty / corrupt, and the initial state of a new
/// tool picker.
#[must_use]
pub fn default_tool_selection() -> Vec<String> {
    PRESETS[0].tools.iter().map(|s| (*s).to_owned()).collect()
}

/// Resolve a checkbox selection back to a preset id by **set equality**
/// (order doesn't matter).  Returns `None` when the selection matches no
/// named preset; the UI surfaces this as the *Custom* chip.
#[must_use]
pub fn resolve_preset(selection: &[String]) -> Option<&'static str> {
    PRESETS
        .iter()
        .find(|p| {
            p.tools.len() == selection.len()
                && p.tools.iter().all(|t| selection.iter().any(|s| s == t))
        })
        .map(|p| p.id)
}

/// Parse a localStorage payload (JSON array of strings) into a tool
/// selection.  Returns the Standard preset on any of:
///
/// * `raw == None`               — storage empty
/// * not valid JSON              — corrupt
/// * not a JSON array of strings — corrupt
/// * empty array                 — the UI requires ≥1 tool
///
/// Pure function — testable without a browser.
#[must_use]
pub fn parse_stored_selection(raw: Option<&str>) -> Vec<String> {
    raw.and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(default_tool_selection)
}

/// Serialise a tool selection for localStorage.  Returns a JSON array
/// string.  Cannot fail for `Vec<String>`.
///
/// # Panics
///
/// Never panics in practice — `Vec<String>` always serialises to valid JSON.
#[must_use]
#[allow(clippy::expect_used)] // Vec<String> is always valid JSON
pub fn serialize_selection(selection: &[String]) -> String {
    serde_json::to_string(selection).expect("Vec<String> always serialises")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    fn sel(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| (*s).to_owned()).collect()
    }

    // -----------------------------------------------------------------------
    // Structural consistency invariants
    // (these guard against drift between the three lists and the preset array)
    // -----------------------------------------------------------------------

    /// `ALL_TOOL_NAMES` == `DEFAULT_TOOL_NAMES` ∪ {`python_repl`} (as sets).
    ///
    /// Catches: adding a tool to ALL without adding it to DEFAULT (or vice
    /// versa), or forgetting to keep `python_repl` as the only non-default tool.
    #[test]
    fn all_tool_names_is_default_plus_python_repl() {
        // Exactly one extra tool in ALL vs DEFAULT.
        assert_eq!(
            ALL_TOOL_NAMES.len(),
            DEFAULT_TOOL_NAMES.len() + 1,
            "ALL must have exactly one more tool than DEFAULT (python_repl)"
        );
        // python_repl is in ALL but NOT in DEFAULT.
        assert!(
            ALL_TOOL_NAMES.contains(&"python_repl"),
            "python_repl must be in ALL_TOOL_NAMES"
        );
        assert!(
            !DEFAULT_TOOL_NAMES.contains(&"python_repl"),
            "python_repl must NOT be in DEFAULT_TOOL_NAMES"
        );
        // Every tool in ALL except python_repl is in DEFAULT.
        for t in ALL_TOOL_NAMES {
            if *t != "python_repl" {
                assert!(
                    DEFAULT_TOOL_NAMES.contains(t),
                    "{t} is in ALL_TOOL_NAMES but not in DEFAULT_TOOL_NAMES"
                );
            }
        }
        // Every tool in DEFAULT is in ALL.
        for t in DEFAULT_TOOL_NAMES {
            assert!(
                ALL_TOOL_NAMES.contains(t),
                "{t} is in DEFAULT_TOOL_NAMES but not in ALL_TOOL_NAMES"
            );
        }
    }

    /// `monitor` + `stop_monitor` are in ALL THREE presets (default, all,
    /// repl-centric).
    ///
    /// This is the guard that prevents Phase-2-style drift: Phase 2 added
    /// monitors to the backend `ALL_TOOL_NAMES` but the frontend mirror was
    /// never updated, making monitors unreachable from the UI.  With a
    /// single source of truth, this invariant structurally prevents that.
    #[test]
    fn monitor_tools_are_in_all_presets() {
        for p in PRESETS {
            assert!(
                p.tools.contains(&"monitor"),
                "preset '{}' is missing 'monitor'",
                p.id
            );
            assert!(
                p.tools.contains(&"stop_monitor"),
                "preset '{}' is missing 'stop_monitor'",
                p.id
            );
        }
    }

    /// Every tool referenced by any preset must appear in `ALL_TOOL_NAMES`.
    ///
    /// Prevents a preset from referencing a tool that the agent doesn't
    /// know about — that would produce an unknown-tool validation error at
    /// session creation time.
    #[test]
    fn every_preset_tool_is_in_all_tool_names() {
        for p in PRESETS {
            for t in p.tools {
                assert!(
                    ALL_TOOL_NAMES.contains(t),
                    "preset '{}' references unknown tool '{}'",
                    p.id,
                    t
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // DEFAULT_TOOL_NAMES
    // -----------------------------------------------------------------------

    #[test]
    fn default_tool_names_are_fourteen_in_canonical_order() {
        assert_eq!(DEFAULT_TOOL_NAMES.len(), 14);
        assert_eq!(
            DEFAULT_TOOL_NAMES,
            &[
                "read_file",
                "write_file",
                "run_command",
                "edit_file",
                "list_files",
                "web_search",
                "fetch_url",
                "grep_files",
                "find_files",
                "run_background",
                "wait_for_output",
                "write_stdin",
                "monitor",
                "stop_monitor",
            ]
        );
    }

    #[test]
    fn default_tool_names_includes_monitor_tools() {
        assert!(
            DEFAULT_TOOL_NAMES.contains(&"monitor"),
            "monitor must be in DEFAULT_TOOL_NAMES"
        );
        assert!(
            DEFAULT_TOOL_NAMES.contains(&"stop_monitor"),
            "stop_monitor must be in DEFAULT_TOOL_NAMES"
        );
    }

    // -----------------------------------------------------------------------
    // ALL_TOOL_NAMES
    // -----------------------------------------------------------------------

    #[test]
    fn all_tool_names_are_fifteen() {
        assert_eq!(ALL_TOOL_NAMES.len(), 15);
    }

    #[test]
    fn all_tool_names_ends_with_python_repl() {
        assert_eq!(
            *ALL_TOOL_NAMES.last().unwrap(),
            "python_repl",
            "python_repl must be the last entry (comes after all default tools)"
        );
    }

    // -----------------------------------------------------------------------
    // Presets
    // -----------------------------------------------------------------------

    /// Kills: `replace PRESETS with &[]` (every lookup goes away).
    #[test]
    fn presets_has_three_entries_in_display_order() {
        let ids: Vec<&str> = PRESETS.iter().map(|p| p.id).collect();
        assert_eq!(ids, vec!["standard", "all", "repl-centric"]);
    }

    /// standard preset MUST be verbatim `DEFAULT_TOOL_NAMES` so that
    /// `--preset standard` and omitting `--preset` mean the same thing.
    #[test]
    fn standard_preset_matches_default_tool_names() {
        let p = preset_by_id("standard").expect("standard exists");
        assert_eq!(p.tools, DEFAULT_TOOL_NAMES);
        assert_eq!(p.label, "Standard");
        assert_eq!(p.tools.len(), 14);
        assert!(!p.tools.contains(&"python_repl"));
        assert!(p.tools.contains(&"monitor"));
        assert!(p.tools.contains(&"stop_monitor"));
    }

    #[test]
    fn all_preset_matches_all_tool_names() {
        let p = preset_by_id("all").expect("all exists");
        assert_eq!(p.tools, ALL_TOOL_NAMES);
        assert_eq!(p.tools.len(), 15);
        assert!(p.tools.contains(&"python_repl"));
        assert!(p.tools.contains(&"monitor"));
    }

    #[test]
    fn repl_centric_preset_contains_python_repl_web_and_monitors() {
        let p = preset_by_id("repl-centric").expect("repl-centric exists");
        assert_eq!(p.tools.len(), 5);
        assert!(p.tools.contains(&"python_repl"));
        assert!(p.tools.contains(&"web_search"));
        assert!(p.tools.contains(&"fetch_url"));
        assert!(p.tools.contains(&"monitor"));
        assert!(p.tools.contains(&"stop_monitor"));
    }

    /// Kills: `replace preset_by_id -> Option<&Preset> with Some(&PRESETS[0])`
    /// or similar always-Some mutants.
    #[test]
    fn preset_by_id_returns_none_for_unknown() {
        assert!(preset_by_id("nope").is_none());
        assert!(preset_by_id("").is_none());
        assert!(
            preset_by_id("Standard").is_none(),
            "id lookup is case-sensitive"
        );
    }

    // -----------------------------------------------------------------------
    // default_tool_selection
    // -----------------------------------------------------------------------

    /// Kills: body-replacement mutants on `default_tool_selection`.
    #[test]
    fn default_tool_selection_matches_standard_preset_by_set_equality() {
        let sel = default_tool_selection();
        assert_eq!(resolve_preset(&sel), Some("standard"));
    }

    #[test]
    fn default_tool_selection_has_fourteen_tools() {
        assert_eq!(default_tool_selection().len(), 14);
    }

    // -----------------------------------------------------------------------
    // resolve_preset
    // -----------------------------------------------------------------------

    #[test]
    fn resolve_preset_finds_standard_in_canonical_order() {
        let sel: Vec<String> = PRESETS[0].tools.iter().map(|s| (*s).to_owned()).collect();
        assert_eq!(resolve_preset(&sel), Some("standard"));
    }

    #[test]
    fn resolve_preset_finds_standard_ignoring_order() {
        // Set equality — reversed input still matches.
        let mut sel: Vec<String> = PRESETS[0].tools.iter().map(|s| (*s).to_owned()).collect();
        sel.reverse();
        assert_eq!(resolve_preset(&sel), Some("standard"));
    }

    #[test]
    fn resolve_preset_finds_all_fifteen() {
        let sel: Vec<String> = PRESETS[1].tools.iter().map(|s| (*s).to_owned()).collect();
        assert_eq!(resolve_preset(&sel), Some("all"));
    }

    #[test]
    fn resolve_preset_finds_repl_centric() {
        let sel: Vec<String> = PRESETS[2].tools.iter().map(|s| (*s).to_owned()).collect();
        assert_eq!(resolve_preset(&sel), Some("repl-centric"));
    }

    #[test]
    fn resolve_preset_returns_none_for_unchecking_one_tool_from_standard() {
        // Standard minus run_command — diverges from every preset.
        let sel: Vec<String> = PRESETS[0]
            .tools
            .iter()
            .filter(|t| **t != "run_command")
            .map(|s| (*s).to_owned())
            .collect();
        assert_eq!(resolve_preset(&sel), None);
    }

    #[test]
    fn resolve_preset_returns_none_for_empty_selection() {
        assert_eq!(resolve_preset(&[]), None);
    }

    #[test]
    fn resolve_preset_returns_none_for_superset_of_a_preset() {
        // REPL-centric plus one extra — superset, not equal, so Custom.
        let mut sel: Vec<String> = PRESETS[2].tools.iter().map(|s| (*s).to_owned()).collect();
        sel.push("run_command".into());
        assert_eq!(resolve_preset(&sel), None);
    }

    /// Kills the `replace == with !=` mutant in `resolve_preset`.
    ///
    /// A selection with the SAME SIZE as the standard preset but one different
    /// tool (monitor replaced by `python_repl`) must return `None`, not
    /// `Some("standard")`.  The mutant (`s != t` instead of `s == t`) makes
    /// `any()` fire on the first mismatching element, which is always true
    /// for a multi-element selection — so it falsely identifies any same-size
    /// selection as a preset match.  This test guards that path.
    #[test]
    fn resolve_preset_requires_exact_tool_membership() {
        // Build a 14-tool selection that is the right *size* for standard
        // but has python_repl in place of monitor.
        let sel: Vec<String> = DEFAULT_TOOL_NAMES
            .iter()
            .filter(|t| **t != "monitor")
            .map(|t| (*t).to_owned())
            .chain(std::iter::once("python_repl".to_owned()))
            .collect();
        assert_eq!(
            sel.len(),
            14,
            "selection must be same size as standard preset"
        );
        assert_eq!(
            resolve_preset(&sel),
            None,
            "same-size selection with wrong tools must not match any preset"
        );
    }

    // -----------------------------------------------------------------------
    // parse_stored_selection
    // -----------------------------------------------------------------------

    #[test]
    fn parse_stored_selection_returns_standard_on_none() {
        let sel = parse_stored_selection(None);
        assert_eq!(resolve_preset(&sel), Some("standard"));
    }

    #[test]
    fn parse_stored_selection_returns_standard_on_invalid_json() {
        let sel = parse_stored_selection(Some("not-json"));
        assert_eq!(resolve_preset(&sel), Some("standard"));
    }

    #[test]
    fn parse_stored_selection_returns_standard_on_wrong_shape() {
        // JSON object, not an array of strings.
        let sel = parse_stored_selection(Some(r#"{"foo":"bar"}"#));
        assert_eq!(resolve_preset(&sel), Some("standard"));
    }

    #[test]
    fn parse_stored_selection_returns_standard_on_empty_array() {
        // Empty selection isn't a valid UI state (≥1 tool required) — fall back.
        let sel = parse_stored_selection(Some("[]"));
        assert_eq!(resolve_preset(&sel), Some("standard"));
    }

    #[test]
    fn parse_stored_selection_round_trips_repl_centric() {
        let stored = serialize_selection(
            PRESETS[2]
                .tools
                .iter()
                .map(|s| (*s).to_owned())
                .collect::<Vec<_>>()
                .as_slice(),
        );
        let sel = parse_stored_selection(Some(&stored));
        assert_eq!(resolve_preset(&sel), Some("repl-centric"));
    }

    // -----------------------------------------------------------------------
    // serialize_selection
    // -----------------------------------------------------------------------

    #[test]
    fn serialize_selection_emits_json_array_of_strings() {
        let s = serialize_selection(&sel(&["a", "b"]));
        assert_eq!(s, r#"["a","b"]"#);
    }

    #[test]
    fn serialize_and_parse_round_trip_standard() {
        let original = sel(DEFAULT_TOOL_NAMES);
        let stored = serialize_selection(&original);
        let recovered = parse_stored_selection(Some(&stored));
        // Same set (parse may produce same order since we serialised in order).
        assert_eq!(resolve_preset(&recovered), Some("standard"));
        assert_eq!(recovered.len(), DEFAULT_TOOL_NAMES.len());
    }
}
