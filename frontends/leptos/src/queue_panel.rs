//! Queue panel — badge + modal for the ephemeral input-queue snapshot.
//!
//! ## Structure
//!
//! ```text
//! App
//!  ├── provide_context::<QueuePanelOpen>
//!  └── QueueBadge  ← visible-when-non-empty header entry point
//!       └── QueueModal ← full-viewport modal toggled by the badge
//! ```
//!
//! ## Discoverability design (§15 queue visualisation)
//!
//! **Decision: badge ALWAYS VISIBLE (never hidden when empty).**
//!
//! Rationale: the queue panel is a *review instrument*. Hiding its
//! entry point when the queue is empty makes the feature
//! undiscoverable and untestable — the empty state is exactly when a
//! reviewer needs to be able to find the control and confirm it works.
//! This is the same lesson learned from the monitor badge (which was
//! also initially gated and then made always-visible in Phase 5).
//!
//! The idle label is `"Queue"` so the badge reads naturally when
//! nothing is pending, and `"N pending"` when items are waiting for
//! delivery at the next Gather seam. Clicking on the always-visible
//! badge opens the modal, which shows an explicit empty-state message
//! when the queue is empty.
//!
//! U2 (§15): monitor sources now join the queue — monitor stdout/stop are
//! delivered through the *same* inbox as human input and carry a
//! `"monitor:<id>"` source (rendered as `"Monitor <id>"` by
//! [`format_source`]).
//!
//! ## Server push points
//!
//! The server emits [`WsMessage::InputQueue`] frames:
//! 1. Immediately after `handle_user_message` pushes a human item.
//! 2. On ANY `InputQueue` push — human OR monitor — via the queue's
//!    `on_change` callback registered in `spawn_run_task` (U2: this is how
//!    a monitor enqueue reaches the WS layer, even though it originates on a
//!    background reader task).
//! 3. After the agent drains an item (triggered by `UserMessage` /
//!    `MonitorDelivery` / `MonitorStopped` events flowing through
//!    `spawn_run_task`); snapshot reflects the post-drain queue.
//! 4. On connect / reset / resume — initial state.
//!
//! The frontend stores the latest snapshot in
//! [`SessionStore::input_queue`] and this module reads from that signal.

use leptos::ev;
use leptos::prelude::*;

use crate::protocol::InputQueueItem;
use crate::store::SessionStore;

// ---------------------------------------------------------------------------
// Context: panel open/close toggle
// ---------------------------------------------------------------------------

/// Wraps the open/close boolean for the queue panel.  Provided as context
/// from `App`; consumed by [`QueueBadge`] (toggle) and [`QueueModal`]
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

/// Label text for the queue badge.
///
/// Returns `"Queue"` when the queue is empty (idle / discoverable state)
/// so the badge always reads naturally.  Returns `"1 pending"` /
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
// QueueBadge component
// ---------------------------------------------------------------------------

/// Header badge showing the number of items pending in the input queue —
/// **always visible** (never gated on queue size).
///
/// Shows `"Queue"` when idle and `"N pending"` when items are waiting.
/// Clicking the badge opens [`QueueModal`], which shows an explicit
/// empty-state message when nothing is pending.
///
/// Marked `#[mutants::skip]` — reactive / DOM body is exercised by the
/// snapshot tests; the pure derivation functions (`pending_count`,
/// `badge_label`, `format_source`) carry the mutation-test budget.
#[mutants::skip]
#[component]
pub fn QueueBadge() -> impl IntoView {
    let store = use_context::<SessionStore>().expect("SessionStore must be provided");
    let panel_open = use_context::<QueuePanelOpen>().expect("QueuePanelOpen must be provided");

    let count = Memo::new(move |_| store.input_queue.with(|q| pending_count(q)));

    view! {
        <button
            class="queue-badge"
            data-testid="queue-badge"
            title="Items pending delivery to the agent — click to review"
            on:click=move |_| panel_open.toggle()
        >
            <span class="queue-badge-count" data-testid="queue-badge-count">
                {move || badge_label(count.get())}
            </span>
        </button>
        <QueueModal />
    }
}

// ---------------------------------------------------------------------------
// QueueModal component
// ---------------------------------------------------------------------------

/// Full-viewport modal listing all items currently pending in the queue.
///
/// Visibility is toggled by [`QueueBadge`] via [`QueuePanelOpen`].
/// The overlay click closes the modal (same pattern as `MonitorsModal`).
///
/// Marked `#[mutants::skip]` — DOM rendering is covered by snapshot tests.
#[mutants::skip]
#[component]
pub fn QueueModal() -> impl IntoView {
    let store = use_context::<SessionStore>().expect("SessionStore must be provided");
    let panel_open = use_context::<QueuePanelOpen>().expect("QueuePanelOpen must be provided");

    view! {
        <Show when=move || panel_open.is_open() fallback=|| ()>
            <div
                class="queue-modal-overlay"
                data-testid="queue-modal-overlay"
                on:click=move |_| panel_open.close()
            >
                <div
                    class="queue-modal"
                    data-testid="queue-modal"
                    on:click=move |e: ev::MouseEvent| e.stop_propagation()
                >
                    <div class="queue-modal-header">
                        <span class="queue-modal-title">"Input Queue"</span>
                        <button
                            class="queue-modal-close"
                            data-testid="queue-modal-close"
                            on:click=move |_| panel_open.close()
                        >"✕"</button>
                    </div>
                    <p class="queue-modal-subtitle">
                        "Items below are queued for delivery to the agent at the next Gather seam."
                    </p>

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

                    // Item list
                    <Show
                        when=move || store.input_queue.with(|q| !q.is_empty())
                        fallback=|| ()
                    >
                        <ul
                            class="queue-item-list"
                            data-testid="queue-item-list"
                        >
                            {move || store.input_queue.with(|items| {
                                items.iter().map(|item| {
                                    let source = format_source(&item.source);
                                    let preview = item.content_preview.clone();
                                    let enqueued_at = item.enqueued_at.clone();
                                    view! {
                                        <li
                                            class="queue-item"
                                            data-testid="queue-item"
                                        >
                                            <div class="queue-item-meta">
                                                <span
                                                    class="queue-item-source"
                                                    data-testid="queue-item-source"
                                                >
                                                    {source}
                                                </span>
                                                <span class="queue-item-time">
                                                    {enqueued_at}
                                                </span>
                                            </div>
                                            <p
                                                class="queue-item-preview"
                                                data-testid="queue-item-preview"
                                            >
                                                {preview}
                                            </p>
                                            <p class="queue-item-note">
                                                "⏳ pending delivery at the next seam"
                                            </p>
                                        </li>
                                    }
                                }).collect::<Vec<_>>()
                            })}
                        </ul>
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
