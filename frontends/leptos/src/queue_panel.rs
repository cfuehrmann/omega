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
//! ## Discoverability design (§15 first-cut queue visualisation)
//!
//! **Decision: badge visible only when the queue is non-empty.**
//!
//! Rationale: the input queue is almost always empty — items are
//! processed at the very next Gather seam, typically within
//! milliseconds of enqueue.  An always-visible "0 pending" badge would
//! add permanent visual noise for a state that is never interesting.
//! When something *is* pending (e.g. a message sent while the agent is
//! mid-turn), the badge appears immediately and draws the operator's
//! attention — exactly when a review instrument is needed.
//!
//! The "persistent entry point" is therefore the *session* itself: the
//! badge appears as soon as there is anything to review, remains
//! visible and clickable throughout, and disappears once the queue
//! drains.  No further UI decoration is needed for the empty state.
//!
//! Monitor sources join the queue in U2; the `source` field is already
//! structured to carry them (currently always `"human"`).
//!
//! ## Server push points
//!
//! The server emits [`WsMessage::InputQueue`] frames:
//! 1. Immediately after `handle_user_message` pushes the item (snapshot
//!    atomically includes the new item).
//! 2. After the agent drains the item (triggered by the `UserMessage`
//!    event flowing through `spawn_run_task`); snapshot is now empty.
//! 3. On connect / reset / resume — initial state.
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
/// Returns `"1 pending"` / `"N pending"` for non-zero counts.
/// (The badge is hidden entirely when count is zero, so a label for
/// that case is not needed; an empty string is returned for safety.)
#[must_use]
pub fn badge_label(count: usize) -> String {
    match count {
        0 => String::new(),
        1 => "1 pending".to_owned(),
        n => format!("{n} pending"),
    }
}

/// Human-readable source label from the raw `source` field.
///
/// `"human"` → `"Human"` (capitalised).
/// Unknown values are returned as-is so future U2 monitor sources
/// render without a code change.
#[must_use]
pub fn format_source(source: &str) -> String {
    match source {
        "human" => "Human".to_owned(),
        other => other.to_owned(),
    }
}

// ---------------------------------------------------------------------------
// QueueBadge component
// ---------------------------------------------------------------------------

/// Header badge showing the number of items pending in the input queue.
///
/// **Visible only when non-empty** (see module docs for the discoverability
/// rationale).  When the queue is non-empty, clicking the badge opens
/// [`QueueModal`].
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
        <Show when=move || count.get() != 0 fallback=|| ()>
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
        </Show>
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
    fn badge_label_zero_returns_empty_string() {
        assert_eq!(badge_label(0), "");
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
    fn format_source_unknown_passes_through() {
        // Future U2 monitor sources should render as-is.
        assert_eq!(format_source("monitor:abc"), "monitor:abc");
    }

    #[wasm_bindgen_test]
    #[test]
    fn format_source_empty_string_passes_through() {
        assert_eq!(format_source(""), "");
    }
}
