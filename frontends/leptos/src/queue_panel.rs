//! Queue panel — full-width in-flow panel for the ephemeral input-queue snapshot.
//!
//! ## Structure
//!
//! ```text
//! App
//!  ├── provide_context::<QueuePanelOpen>
//!  └── QueuePanel  ← full-width in-flow panel (inside .bottom-panels-container)
//!       └── <Show when=open>
//!            └── <div class="bottom-panel queue-panel">
//!                 └── queue-table (or empty-state message)
//! ```
//!
//! ## Design (Phase 6 — Unified bottom panels)
//!
//! `QueuePanel` is modelled on `UsagePanel`: a `<Show when=open>` wrapping
//! a full-width `<div class="bottom-panel queue-panel">`.  Open/close is
//! toggled via the "Panels ▾" menu in the composer row.
//! The table uses the shared `.panel-table` CSS class for consistent grid
//! lines with the Usage and Monitors panels.
//!
//! ## Server push points
//!
//! The server emits [`WsMessage::InputQueue`] frames:
//! 1. Immediately after `handle_user_message` pushes a human item.
//! 2. On ANY `InputQueue` push — human OR monitor — via the queue's
//!    `on_change` callback.
//! 3. After the agent drains an item.
//! 4. On connect / reset / resume — initial state.
//!
//! The frontend stores the latest snapshot in [`SessionStore::input_queue`]
//! and this module reads from that signal.

use leptos::prelude::*;

use crate::protocol::InputQueueItem;
use crate::store::SessionStore;

// ---------------------------------------------------------------------------
// Context: panel open/close toggle
// ---------------------------------------------------------------------------

/// Wraps the open/close boolean for the queue panel.  Provided as context
/// from `App`; consumed by the "Panels ▾" menu (toggle) and [`QueuePanel`]
/// (conditional rendering).
#[derive(Debug, Clone, Copy)]
pub struct QueuePanelOpen(pub RwSignal<bool>);

impl QueuePanelOpen {
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

impl Default for QueuePanelOpen {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Pure derivation helpers (mutation-tested)
// ---------------------------------------------------------------------------

/// Count the number of pending items in `items`.
///
/// Currently equivalent to `items.len()`, but extracted as a named
/// function so the mutation-test suite can cover it explicitly.
#[must_use]
pub fn pending_count(items: &[InputQueueItem]) -> usize {
    items.len()
}

/// Label text for the queue menu item.
///
/// Returns `"Queue"` when the queue is empty (idle / discoverable state)
/// so the label always reads naturally.  Returns `"1 pending"` /
/// `"N pending"` when items are waiting for delivery.
#[must_use]
pub fn badge_label(count: usize) -> String {
    match count {
        0 => "Queue".to_owned(),
        1 => "1 pending".to_owned(),
        n => format!("{n} pending"),
    }
}

/// Human-readable source label from the raw `source` field.
///
/// `"human"` → `"Human"` (capitalised).
/// `"monitor:<id>"` → `"Monitor <id>"` (U2 — monitors deliver through the
/// same inbox as human input; their queued items carry a `monitor:`-prefixed
/// source).  Any other value is returned as-is.
#[must_use]
pub fn format_source(source: &str) -> String {
    match source {
        "human" => "Human".to_owned(),
        other => match other.strip_prefix("monitor:") {
            Some(id) => format!("Monitor {id}"),
            None => other.to_owned(),
        },
    }
}

// ---------------------------------------------------------------------------
// QueuePanel component
// ---------------------------------------------------------------------------

/// Full-width in-flow panel listing all items currently pending in the queue.
///
/// Visibility is toggled by the "Panels ▾" menu in the composer row via
/// [`QueuePanelOpen`].  When open the panel stacks inside the
/// `.bottom-panels-container` between the conversation feed and the composer.
/// The queue is displayed as a table using the shared `.panel-table` CSS class
/// for consistent grid lines with the Usage and Monitors panels.
///
/// Marked `#[mutants::skip]` — DOM rendering is covered by snapshot tests;
/// the pure derivation functions (`pending_count`, `badge_label`,
/// `format_source`) carry the mutation-test budget.
#[mutants::skip]
#[component]
pub fn QueuePanel() -> impl IntoView {
    let store = use_context::<SessionStore>().expect("SessionStore must be provided");
    let panel_open = use_context::<QueuePanelOpen>().expect("QueuePanelOpen must be provided");

    view! {
        <Show when=move || panel_open.is_open() fallback=|| ()>
            <div class="bottom-panel queue-panel" data-testid="queue-panel">

                // Empty state
                <Show
                    when=move || store.input_queue.with(|q| q.is_empty())
                    fallback=|| ()
                >
                    <p
                        class="queue-empty-state"
                        data-testid="queue-empty-state"
                    >
                        "No items pending — the queue is empty."
                    </p>
                </Show>

                // Queue table — shown when there is at least one item.
                <Show
                    when=move || store.input_queue.with(|q| !q.is_empty())
                    fallback=|| ()
                >
                    <table class="panel-table queue-table" data-testid="queue-table">
                        <thead>
                            <tr>
                                <th>"Source"</th>
                                <th>"Content"</th>
                                <th>"Queued At"</th>
                            </tr>
                        </thead>
                        <tbody>
                            {move || store.input_queue.with(|items| {
                                items.iter().map(|item| {
                                    let source = format_source(&item.source);
                                    let preview = item.content_preview.clone();
                                    let enqueued_at = item.enqueued_at.clone();
                                    view! {
                                        <tr
                                            class="queue-item"
                                            data-testid="queue-item"
                                        >
                                            <td
                                                class="queue-item-source"
                                                data-testid="queue-item-source"
                                            >
                                                {source}
                                            </td>
                                            <td
                                                class="queue-item-preview"
                                                data-testid="queue-item-preview"
                                            >
                                                {preview}
                                            </td>
                                            <td class="queue-item-time">
                                                {enqueued_at}
                                            </td>
                                        </tr>
                                    }
                                }).collect::<Vec<_>>()
                            })}
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

    fn item(source: &str, preview: &str) -> InputQueueItem {
        InputQueueItem {
            source: source.into(),
            content_preview: preview.into(),
            enqueued_at: "2025-01-01T00:00:00.000Z".into(),
        }
    }

    // ── pending_count ────────────────────────────────────────────────────

    #[wasm_bindgen_test]
    #[test]
    fn pending_count_zero_for_empty_queue() {
        assert_eq!(pending_count(&[]), 0);
    }

    #[wasm_bindgen_test]
    #[test]
    fn pending_count_one_for_single_item() {
        assert_eq!(pending_count(&[item("human", "hello")]), 1);
    }

    #[wasm_bindgen_test]
    #[test]
    fn pending_count_two_for_two_items() {
        assert_eq!(pending_count(&[item("human", "a"), item("human", "b")]), 2);
    }

    // ── badge_label ──────────────────────────────────────────────────────

    #[wasm_bindgen_test]
    #[test]
    fn badge_label_zero_returns_idle_label() {
        assert_eq!(badge_label(0), "Queue");
    }

    #[wasm_bindgen_test]
    #[test]
    fn badge_label_one_returns_singular() {
        assert_eq!(badge_label(1), "1 pending");
    }

    #[wasm_bindgen_test]
    #[test]
    fn badge_label_many_returns_plural() {
        assert_eq!(badge_label(3), "3 pending");
    }

    #[wasm_bindgen_test]
    #[test]
    fn badge_label_large_number() {
        assert_eq!(badge_label(99), "99 pending");
    }

    // ── format_source ────────────────────────────────────────────────────

    #[wasm_bindgen_test]
    #[test]
    fn format_source_human_capitalises() {
        assert_eq!(format_source("human"), "Human");
    }

    #[wasm_bindgen_test]
    #[test]
    fn format_source_monitor_renders_label_with_id() {
        // U2: monitor sources are `monitor:<id>` and render as `Monitor <id>`.
        assert_eq!(format_source("monitor:abc"), "Monitor abc");
    }

    #[wasm_bindgen_test]
    #[test]
    fn format_source_monitor_empty_id() {
        assert_eq!(format_source("monitor:"), "Monitor ");
    }

    #[wasm_bindgen_test]
    #[test]
    fn format_source_unknown_passes_through() {
        assert_eq!(format_source("system"), "system");
    }

    #[wasm_bindgen_test]
    #[test]
    fn format_source_empty_string_passes_through() {
        assert_eq!(format_source(""), "");
    }
}
