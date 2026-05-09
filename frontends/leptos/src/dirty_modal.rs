//! Dirty-working-tree warning modal.
//!
//! When the server reports `hasPendingChanges: true` on a fresh session the
//! [`DirtyModal`] overlay is shown, warning the operator that there are
//! uncommitted changes in the working tree.
//!
//! ## State machine
//!
//! [`DirtyModalState`] tracks two things:
//! - `is_open` — whether the modal is currently visible.
//! - `last_triggered_dir` — the session directory for which the modal was
//!   last considered.  Kept so that in-session `session_info` updates (e.g.
//!   model / effort changes) don't re-open a modal the operator already
//!   dismissed.
//!
//! [`DirtyModalState::check`] is called from `App` whenever `session_info`
//! changes.  It delegates the open/no-open decision to the pure
//! [`should_open_dirty_modal`] function, which is directly unit-testable and
//! mutation-tested.

use leptos::prelude::*;

// ---------------------------------------------------------------------------
// Pure trigger predicate
// ---------------------------------------------------------------------------

/// Return `true` when the dirty-warning modal should open.
///
/// Conditions that must both hold:
/// 1. The working tree has uncommitted changes (`has_pending_changes`).
/// 2. This is a **new** session (the `current_dir` differs from
///    `last_triggered_dir`); prevents re-opening the modal for the same
///    session when a model/effort change triggers a reactive re-run.
#[must_use]
pub fn should_open_dirty_modal(
    current_dir: &str,
    last_triggered_dir: &str,
    has_pending_changes: bool,
) -> bool {
    has_pending_changes && current_dir != last_triggered_dir
}

// ---------------------------------------------------------------------------
// Reactive state
// ---------------------------------------------------------------------------

/// App-scoped reactive state for the dirty-working-tree modal.
///
/// Cheaply [`Copy`] — backed by [`RwSignal`] handles.
#[derive(Debug, Clone, Copy)]
pub struct DirtyModalState {
    /// Whether the modal overlay is currently visible.
    pub is_open: RwSignal<bool>,
    /// The session directory for which we last ran the open check.
    /// Stored so in-session reactive updates don't re-trigger the modal.
    last_triggered_dir: RwSignal<String>,
}

impl DirtyModalState {
    /// Create a new, closed state.
    ///
    /// Must be called inside a leptos reactive `Owner` scope.
    #[must_use]
    pub fn new() -> Self {
        Self {
            is_open: RwSignal::new(false),
            last_triggered_dir: RwSignal::new(String::new()),
        }
    }

    /// Evaluate whether the modal should open for the given session
    /// directory and pending-changes flag, then act accordingly.
    ///
    /// Delegates to [`should_open_dirty_modal`].
    pub fn check(self, current_dir: &str, has_pending_changes: bool) {
        let last = self.last_triggered_dir.get_untracked();
        if should_open_dirty_modal(current_dir, &last, has_pending_changes) {
            self.last_triggered_dir.set(current_dir.to_owned());
            self.is_open.set(true);
        } else if current_dir != last.as_str() {
            // New session without pending changes: update the dir so a
            // subsequent in-session change can't incorrectly re-trigger.
            self.last_triggered_dir.set(current_dir.to_owned());
        }
    }

    /// Dismiss the modal.
    pub fn dismiss(self) {
        self.is_open.set(false);
    }
}

impl Default for DirtyModalState {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

/// Dirty-working-tree warning overlay.
///
/// Renders nothing when closed.  Reads open/close state from the
/// app-scoped [`DirtyModalState`] context (provided in `App` via
/// `provide_context`).
#[component]
pub fn DirtyModal() -> impl IntoView {
    let state =
        use_context::<DirtyModalState>().expect("DirtyModalState must be provided");
    let on_dismiss = move |_: leptos::ev::MouseEvent| state.dismiss();

    view! {
        <Show
            when=move || state.is_open.get()
            fallback=|| ().into_any()
        >
            <div
                class="leptos-dirty-modal-backdrop"
                data-testid="leptos-dirty-modal-backdrop"
            >
                <div
                    class="leptos-dirty-modal"
                    role="alertdialog"
                    aria-modal="true"
                    aria-labelledby="dirty-modal-title"
                    data-testid="leptos-dirty-modal"
                >
                    <header class="leptos-dirty-modal-header">
                        <span
                            id="dirty-modal-title"
                            class="leptos-dirty-modal-title"
                        >
                            "⚠ Uncommitted changes"
                        </span>
                    </header>
                    <div class="leptos-dirty-modal-body" data-testid="leptos-dirty-modal-body">
                        <p>
                            "The working tree has uncommitted changes. \
                             Omega may read or write files in this directory \
                             during the session."
                        </p>
                        <p>
                            "Consider committing or stashing your changes first \
                             so you have a clean rollback point."
                        </p>
                    </div>
                    <div class="leptos-dirty-modal-actions">
                        <button
                            class="leptos-dirty-modal-ok"
                            data-testid="leptos-dirty-modal-ok"
                            on:click=on_dismiss
                        >
                            "Proceed anyway"
                        </button>
                    </div>
                </div>
            </div>
        </Show>
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use leptos::reactive::owner::Owner;
    use wasm_bindgen_test::wasm_bindgen_test;

    fn with_owner<F: FnOnce()>(f: F) {
        let owner = Owner::new();
        owner.with(f);
    }

    // ---- should_open_dirty_modal (pure, host-runnable) ----------------------

    #[test]
    fn opens_for_new_dir_with_pending_changes() {
        assert!(should_open_dirty_modal("/new", "/old", true));
    }

    #[test]
    fn does_not_open_for_same_dir_even_with_pending_changes() {
        // In-session model/effort update: dir unchanged → no re-trigger.
        assert!(!should_open_dirty_modal("/same", "/same", true));
    }

    #[test]
    fn does_not_open_when_no_pending_changes() {
        assert!(!should_open_dirty_modal("/new", "/old", false));
    }

    #[test]
    fn does_not_open_for_same_dir_without_pending_changes() {
        assert!(!should_open_dirty_modal("/a", "/a", false));
    }

    #[test]
    fn opens_when_last_triggered_is_empty_and_has_pending() {
        // Fresh state: `last_triggered_dir` is `""`, any real dir triggers.
        assert!(should_open_dirty_modal("/workspace", "", true));
    }

    #[test]
    fn does_not_open_when_last_triggered_is_empty_and_no_pending() {
        assert!(!should_open_dirty_modal("/workspace", "", false));
    }

    // ---- DirtyModalState reactive tests (WASM) ------------------------------

    #[wasm_bindgen_test]
    fn state_starts_closed() {
        with_owner(|| {
            let s = DirtyModalState::new();
            assert!(!s.is_open.get_untracked());
        });
    }

    #[wasm_bindgen_test]
    fn check_opens_modal_for_new_dirty_session() {
        with_owner(|| {
            let s = DirtyModalState::new();
            s.check("/workspace", true);
            assert!(s.is_open.get_untracked());
        });
    }

    #[wasm_bindgen_test]
    fn check_does_not_open_for_clean_session() {
        with_owner(|| {
            let s = DirtyModalState::new();
            s.check("/workspace", false);
            assert!(!s.is_open.get_untracked());
        });
    }

    #[wasm_bindgen_test]
    fn dismiss_closes_modal() {
        with_owner(|| {
            let s = DirtyModalState::new();
            s.check("/workspace", true);
            assert!(s.is_open.get_untracked());
            s.dismiss();
            assert!(!s.is_open.get_untracked());
        });
    }

    #[wasm_bindgen_test]
    fn check_does_not_reopen_for_same_dir_after_dismiss() {
        // Simulates a model/effort change: session_info updates but dir is the same.
        with_owner(|| {
            let s = DirtyModalState::new();
            s.check("/workspace", true);
            s.dismiss();
            // Same dir, pending still true → must NOT reopen.
            s.check("/workspace", true);
            assert!(!s.is_open.get_untracked());
        });
    }

    #[wasm_bindgen_test]
    fn check_reopens_for_new_session_with_pending_changes() {
        with_owner(|| {
            let s = DirtyModalState::new();
            // First session — dirty, opened, dismissed.
            s.check("/session-a", true);
            s.dismiss();
            // Second session — also dirty → must reopen.
            s.check("/session-b", true);
            assert!(s.is_open.get_untracked());
        });
    }

    #[wasm_bindgen_test]
    fn check_does_not_open_for_new_clean_session_after_dirty_one() {
        with_owner(|| {
            let s = DirtyModalState::new();
            s.check("/session-a", true);
            s.dismiss();
            // New session but clean.
            s.check("/session-b", false);
            assert!(!s.is_open.get_untracked());
        });
    }

    #[wasm_bindgen_test]
    fn last_triggered_dir_advances_on_clean_session_too() {
        // If the dir changes but there's no pending change, `last_triggered_dir`
        // must still advance so a subsequent in-session update with the new dir
        // doesn't incorrectly re-trigger.
        with_owner(|| {
            let s = DirtyModalState::new();
            s.check("/session-a", false); // clean, no open
            // Same dir, now with pending — should NOT open (dir already recorded).
            s.check("/session-a", true);
            assert!(!s.is_open.get_untracked());
        });
    }
}
