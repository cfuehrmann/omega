//! Monitors panel — full-width in-flow panel for the ephemeral live monitor roster.
//!
//! ## Structure
//!
//! ```text
//! App
//!  ├── provide_context::<MonitorsPanelOpen>
//!  └── MonitorsPanel   ← full-width in-flow panel (inside .bottom-panels-container)
//!       └── <Show when=open>
//!            └── <div class="bottom-panel monitors-panel">
//!                 └── monitors-table (or empty-state message)
//! ```
//!
//! ## Design (Phase 6 — Unified bottom panels)
//!
//! `MonitorsPanel` is modelled on `UsagePanel`: a `<Show when=open>` wrapping
//! a full-width `<div class="bottom-panel monitors-panel">`.  Open/close is
//! toggled via the "Panels" menu in the composer row (not a floating badge).
//! The table now gets the full container width and is styled with visible row
//! *and* column dividers via the shared `.panel-table` CSS class.
//!
//! The server pushes a [`WsMessage::MonitorRoster`] snapshot:
//! - once when the client connects/resumes, and
//! - right after every monitor lifecycle event.
//!
//! The frontend stores the latest snapshot in `SessionStore::roster` and
//! this module reads from that signal.

use leptos::prelude::*;

use crate::event_view::format_time;
use crate::feed::TimestampChip;
use crate::protocol::MonitorRosterEntry;
use crate::store::SessionStore;

// ---------------------------------------------------------------------------
// Context: panel open/close toggle
// ---------------------------------------------------------------------------

/// Wraps the open/close boolean for the monitors panel. Provided as context
/// from `App`; consumed by the "Panels" menu (toggle) and [`MonitorsPanel`]
/// (conditional rendering).
#[derive(Debug, Clone, Copy)]
pub struct MonitorsPanelOpen(pub RwSignal<bool>);

impl MonitorsPanelOpen {
    /// Construct closed state. Must run inside a leptos reactive `Owner`.
    #[must_use]
    pub fn new() -> Self {
        Self(RwSignal::new(false))
    }

    /// Toggle open ↔ closed.
    #[mutants::skip] // reactive signal write; covered by e2e harness.
    pub fn toggle(self) {
        self.0.update(|v| *v = !*v);
    }

    /// Whether the panel is currently open.
    pub fn is_open(self) -> bool {
        self.0.get()
    }

    /// Close the panel.
    #[mutants::skip] // reactive signal write; covered by e2e harness.
    pub fn close(self) {
        self.0.set(false);
    }
}

impl Default for MonitorsPanelOpen {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Pure derivation helpers (mutation-tested)
// ---------------------------------------------------------------------------

/// Count the number of running monitors in `roster`.
#[must_use]
pub fn running_count(roster: &[MonitorRosterEntry]) -> usize {
    roster.iter().filter(|m| m.status == "running").count()
}

/// Sum `fired_count` across all monitors in `roster`.
#[must_use]
pub fn total_fired(roster: &[MonitorRosterEntry]) -> u64 {
    roster.iter().map(|m| m.fired_count).sum()
}

/// Label text for the monitors menu item count summary.
///
/// Returns `"Monitors"` when no monitors are running (idle / discoverable
/// state).  Returns `"{n} running"` when at least one monitor is active.
#[must_use]
pub fn badge_label(running: usize) -> String {
    if running == 0 {
        "Monitors".to_owned()
    } else {
        format!("{running} running")
    }
}

// ---------------------------------------------------------------------------
// MonitorsPanel component
// ---------------------------------------------------------------------------

/// Full-width in-flow panel listing all monitors in the current session.
///
/// Visibility is toggled by the "Panels" menu in the composer row via
/// [`MonitorsPanelOpen`].  When open the panel stacks inside the
/// `.bottom-panels-container` between the conversation feed and the composer.
/// The roster table uses the shared `.panel-table` CSS class for consistent
/// grid lines with the Usage and Queue panels.
///
/// Marked `#[mutants::skip]` — the reactive/DOM body is exercised by the
/// snapshot tests; the pure derivation functions (`running_count`,
/// `total_fired`, `badge_label`) carry the mutation-test budget.
#[mutants::skip]
#[component]
pub fn MonitorsPanel() -> impl IntoView {
    let store = use_context::<SessionStore>().expect("SessionStore must be provided");
    let panel_open =
        use_context::<MonitorsPanelOpen>().expect("MonitorsPanelOpen must be provided");

    view! {
        <Show when=move || panel_open.is_open() fallback=|| ()>
            <div class="bottom-panel monitors-panel" data-testid="monitors-panel">
                <h3 class="panel-heading">"Monitors"</h3>

                // Empty state — shown when no monitors are in the roster.
                <Show
                    when=move || store.roster.with(|r| r.is_empty())
                    fallback=|| ()
                >
                    <p
                        class="monitors-empty-state"
                        data-testid="monitors-empty-state"
                    >
                        "No monitors running — started monitors appear here."
                    </p>
                </Show>

                // Roster table — shown when there is at least one monitor.
                <Show
                    when=move || store.roster.with(|r| !r.is_empty())
                    fallback=|| ()
                >
                    <table class="panel-table monitors-table" data-testid="monitors-table">
                        <thead>
                            <tr>
                                <th>"ID"</th>
                                <th>"Description"</th>
                                <th>"Command"</th>
                                <th>"Status"</th>
                                <th>"Started"</th>
                                <th>"# Events"</th>
                                <th>"Stderr"</th>
                            </tr>
                        </thead>
                        <tbody>
                            {move || {
                                let tz = store.agent_time_zone.get();
                                store.roster.with(|roster| {
                                    roster.iter().map(|m| {
                                        let id = m.id.clone();
                                        let id2 = id.clone();
                                        let description = m.description.clone();
                                        let command = m.command.clone();
                                        let status = m.status.clone();
                                        let iso = m.started_at.clone();
                                        let display = format_time(&iso, &tz);
                                        let fired_count = m.fired_count;
                                        let stderr = m.stderr_tail.join("\n");
                                        let stderr2 = stderr.clone();
                                        let status_class = if status == "running" {
                                            "mt-status-running"
                                        } else {
                                            "mt-status-stopped"
                                        };
                                        view! {
                                            <tr
                                                class="monitors-row"
                                                data-testid="monitors-row"
                                                data-monitor-id=id2
                                            >
                                                <td class="mt-id"><code>{id}</code></td>
                                                <td class="mt-description">{description}</td>
                                                <td class="mt-command"><code>{command}</code></td>
                                                <td class="mt-status">
                                                    <span class=status_class>{status}</span>
                                                </td>
                                                <td class="mt-started">
                                                    <TimestampChip
                                                        iso=iso
                                                        display=display
                                                        pill=true
                                                    />
                                                </td>
                                                <td class="mt-fired">{fired_count}</td>
                                                <td class="mt-stderr">
                                                    <Show when=move || !stderr.is_empty() fallback=|| ()>
                                                        <pre class="mt-stderr-pre">{stderr2.clone()}</pre>
                                                    </Show>
                                                </td>
                                            </tr>
                                        }
                                    }).collect::<Vec<_>>()
                                })
                            }}
                        </tbody>
                    </table>
                </Show>
            </div>
        </Show>
    }
}

// ---------------------------------------------------------------------------
// Tests — pure derivation functions
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]

    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test;

    fn entry(status: &str, fired: u64) -> MonitorRosterEntry {
        MonitorRosterEntry {
            id: "m1".into(),
            description: "desc".into(),
            command: "cmd".into(),
            status: status.into(),
            started_at: "2025-01-01T00:00:00Z".into(),
            fired_count: fired,
            stderr_tail: vec![],
        }
    }

    // ── badge_label ──────────────────────────────────────────────────────

    #[wasm_bindgen_test]
    #[test]
    fn badge_label_zero_returns_monitors() {
        assert_eq!(badge_label(0), "Monitors");
    }

    #[wasm_bindgen_test]
    #[test]
    fn badge_label_one_running() {
        assert_eq!(badge_label(1), "1 running");
    }

    #[wasm_bindgen_test]
    #[test]
    fn badge_label_many_running() {
        assert_eq!(badge_label(5), "5 running");
    }

    // ── running_count ────────────────────────────────────────────────────

    #[wasm_bindgen_test]
    #[test]
    fn running_count_zero_for_empty_roster() {
        assert_eq!(running_count(&[]), 0);
    }

    #[wasm_bindgen_test]
    #[test]
    fn running_count_counts_only_running() {
        let roster = vec![
            entry("running", 0),
            entry("stopped", 0),
            entry("running", 0),
        ];
        assert_eq!(running_count(&roster), 2);
    }

    #[wasm_bindgen_test]
    #[test]
    fn running_count_all_stopped_returns_zero() {
        let roster = vec![entry("stopped", 0), entry("stopped", 5)];
        assert_eq!(running_count(&roster), 0);
    }

    #[wasm_bindgen_test]
    #[test]
    fn running_count_all_running_returns_full_length() {
        let roster = vec![entry("running", 0), entry("running", 0)];
        assert_eq!(running_count(&roster), 2);
    }

    // ── total_fired ──────────────────────────────────────────────────────

    #[wasm_bindgen_test]
    #[test]
    fn total_fired_zero_for_empty_roster() {
        assert_eq!(total_fired(&[]), 0);
    }

    #[wasm_bindgen_test]
    #[test]
    fn total_fired_sums_all_entries() {
        let roster = vec![
            entry("running", 3),
            entry("stopped", 7),
            entry("running", 1),
        ];
        assert_eq!(total_fired(&roster), 11);
    }

    #[wasm_bindgen_test]
    #[test]
    fn total_fired_single_entry() {
        assert_eq!(total_fired(&[entry("running", 42)]), 42);
    }

    #[wasm_bindgen_test]
    #[test]
    fn total_fired_all_zero() {
        let roster = vec![entry("running", 0), entry("stopped", 0)];
        assert_eq!(total_fired(&roster), 0);
    }
}
