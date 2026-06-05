//! Monitors panel — badge + modal for the ephemeral live monitor roster.
//!
//! ## Structure
//!
//! ```text
//! App
//!  ├── provide_context::<MonitorsPanelOpen>
//!  └── MonitorsBadge   ← always-visible header entry point to the roster modal
//!       └── MonitorsModal ← full-viewport modal toggled by the badge
//! ```
//!
//! ## Design (§9 of docs/monitors-design.html — server-authoritative push)
//!
//! The badge is **always visible** (never gated on roster size).  Monitors
//! are default-on and the badge is the only entry point to the modal, so
//! hiding it when the roster is empty makes the feature undiscoverable.  The
//! always-visible approach is also simpler and more robust — no reactive
//! guard needed.
//!
//! If the store ever cleanly surfaces whether monitors are disabled in the
//! current session's tool selection, the badge *could* be conditionally
//! hidden — but only when that signal is clean and cheap.  For now,
//! always-visible is the right default.
//!
//! The server pushes a [`WsMessage::MonitorRoster`] snapshot:
//! - once when the client connects/resumes (so the badge is current after a
//!   browser refresh), and
//! - right after every monitor lifecycle event is forwarded
//!   (MonitorStarted / Delivery / Stderr / Stopped) — the seam-aligned
//!   moments at which the roster mutates.
//!
//! The frontend stores the latest snapshot in `SessionStore::roster` and
//! this module reads from that signal.
//!
//! ## Deferred
//!
//! - **Kill button / KillMonitor control frame** — when picked up it needs:
//!   a new `MonitorManager` method doing the Running→Stopped CAS, killing
//!   the process tree, enqueuing `PendingItem::Stopped { reason: StoppedByUser }`
//!   (so the drain loop emits `MonitorStopped(StoppedByUser)` via the
//!   single-writer path), and firing the roster-changed notify.  A
//!   `ClientFrame::KillMonitor { id }` would dispatch to it (mirror
//!   `handle_abort`).  `MonitorStopReason::StoppedByUser` exists in the schema
//!   but is currently unused.
//! - **Live pending-queue visualisation** (PLANNED — wanted, not yet
//!   scheduled; sub-seam state would need per-enqueue streaming).

use leptos::ev;
use leptos::prelude::*;

use crate::protocol::MonitorRosterEntry;
use crate::store::SessionStore;

// ---------------------------------------------------------------------------
// Context: panel open/close toggle
// ---------------------------------------------------------------------------

/// Wraps the open/close boolean for the monitors panel. Provided as context
/// from `App`; consumed by [`MonitorsBadge`] (toggle) and [`MonitorsModal`]
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

/// Label text for the badge count span.
///
/// Returns `"Monitors"` when no monitors are running (idle / discoverable
/// state) so the badge always reads naturally.  Returns `"{n} running"` when
/// at least one monitor is active.
#[must_use]
pub fn badge_label(running: usize) -> String {
    if running == 0 {
        "Monitors".to_owned()
    } else {
        format!("{running} running")
    }
}

// ---------------------------------------------------------------------------
// MonitorsBadge component
// ---------------------------------------------------------------------------

/// Header badge showing live monitor activity — always visible.
///
/// Clicking the badge opens [`MonitorsModal`].  When no monitors are
/// running the badge shows the idle label "Monitors" so the feature remains
/// discoverable even in a session that has never started one.
///
/// Marked `#[mutants::skip]` — the reactive/DOM body is exercised by the
/// snapshot tests; the pure derivation functions (`running_count`,
/// `total_fired`, `badge_label`) carry the mutation-test budget.
#[mutants::skip]
#[component]
pub fn MonitorsBadge() -> impl IntoView {
    let store = use_context::<SessionStore>().expect("SessionStore must be provided");
    let panel_open =
        use_context::<MonitorsPanelOpen>().expect("MonitorsPanelOpen must be provided");

    let running = Memo::new(move |_| store.roster.with(|r| running_count(r)));
    let fired = Memo::new(move |_| store.roster.with(|r| total_fired(r)));

    view! {
        <button
            class="monitors-badge"
            data-testid="monitors-badge"
            title="Live monitors — click to inspect"
            on:click=move |_| panel_open.toggle()
        >
            <span class="monitors-badge-count" data-testid="monitors-badge-count">
                {move || badge_label(running.get())}
            </span>
            <Show when=move || fired.get() != 0 fallback=|| ()>
                <span class="monitors-badge-fired" data-testid="monitors-badge-fired">
                    {move || fired.get()}
                    " fired"
                </span>
            </Show>
        </button>
        <MonitorsModal />
    }
}

// ---------------------------------------------------------------------------
// MonitorsModal component
// ---------------------------------------------------------------------------

/// Full-viewport modal listing all monitors in the current session.
///
/// Visibility is toggled by [`MonitorsBadge`] via [`MonitorsPanelOpen`].
/// The overlay click closes the modal (same pattern as `UsagePanel`'s
/// `TokenLegend`).
///
/// Marked `#[mutants::skip]` — DOM rendering is covered by snapshot tests.
#[mutants::skip]
#[component]
pub fn MonitorsModal() -> impl IntoView {
    let store = use_context::<SessionStore>().expect("SessionStore must be provided");
    let panel_open =
        use_context::<MonitorsPanelOpen>().expect("MonitorsPanelOpen must be provided");

    view! {
        <Show when=move || panel_open.is_open() fallback=|| ()>
            <div
                class="monitors-modal-overlay"
                data-testid="monitors-modal-overlay"
                on:click=move |_| panel_open.close()
            >
                <div
                    class="monitors-modal"
                    data-testid="monitors-modal"
                    on:click=move |e: ev::MouseEvent| e.stop_propagation()
                >
                    <div class="monitors-modal-header">
                        <span class="monitors-modal-title">"Active Monitors"</span>
                        <button
                            class="monitors-modal-close"
                            data-testid="monitors-modal-close"
                            on:click=move |_| panel_open.close()
                        >"✕"</button>
                    </div>

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
                        <table class="monitors-table">
                            <thead>
                                <tr>
                                    <th class="mt-header">"ID"</th>
                                    <th class="mt-header">"Description"</th>
                                    <th class="mt-header">"Command"</th>
                                    <th class="mt-header">"Status"</th>
                                    <th class="mt-header">"Started"</th>
                                    <th class="mt-header">"Events fired"</th>
                                    <th class="mt-header">"Stderr tail"</th>
                                </tr>
                            </thead>
                            <tbody>
                                {move || store.roster.with(|roster| {
                                    roster.iter().map(|m| {
                                        let id = m.id.clone();
                                        let id2 = id.clone();
                                        let description = m.description.clone();
                                        let command = m.command.clone();
                                        let status = m.status.clone();
                                        let started_at = m.started_at.clone();
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
                                                <td class="mt-started">{started_at}</td>
                                                <td class="mt-fired">{fired_count}</td>
                                                <td class="mt-stderr">
                                                    <Show when=move || !stderr.is_empty() fallback=|| ()>
                                                        <pre class="mt-stderr-pre">{stderr2.clone()}</pre>
                                                    </Show>
                                                </td>
                                            </tr>
                                        }
                                    }).collect::<Vec<_>>()
                                })}
                            </tbody>
                        </table>
                    </Show>
                </div>
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
