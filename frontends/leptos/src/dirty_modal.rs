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
// sessionStorage persistence
// ---------------------------------------------------------------------------

/// `sessionStorage` key used to remember which session dir the operator has
/// already been warned about.  `sessionStorage` survives browser refresh
/// within the same tab but resets when the tab is closed, which is exactly
/// the scope we want.
const STORAGE_KEY: &str = "omega.dirty_modal.last_dir";

/// Read the last-acknowledged session dir from `sessionStorage`.
///
/// Returns an empty string when storage is unavailable (non-WASM build,
/// private-browsing mode with blocked storage, etc.).
#[cfg(target_arch = "wasm32")]
#[mutants::skip] // browser API; covered by e2e tests
fn load_last_triggered_dir() -> String {
    web_sys::window()
        .and_then(|w| w.session_storage().ok().flatten())
        .and_then(|s| s.get_item(STORAGE_KEY).ok().flatten())
        .unwrap_or_default()
}

#[cfg(not(target_arch = "wasm32"))]
fn load_last_triggered_dir() -> String {
    String::new()
}

/// Persist `dir` to `sessionStorage` so that a browser refresh in the same
/// tab does not re-open the modal for a session the operator has already
/// acknowledged.
#[cfg(target_arch = "wasm32")]
#[mutants::skip] // browser API; covered by e2e tests
fn save_last_triggered_dir(dir: &str) {
    if let Some(storage) = web_sys::window()
        .and_then(|w| w.session_storage().ok().flatten())
    {
        let _ = storage.set_item(STORAGE_KEY, dir);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn save_last_triggered_dir(_dir: &str) {}

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
    /// `last_triggered_dir` is seeded from `sessionStorage` so that a
    /// browser refresh in the same tab does not re-open the modal for a
    /// session the operator already acknowledged.
    ///
    /// Must be called inside a leptos reactive `Owner` scope.
    #[must_use]
    pub fn new() -> Self {
        Self {
            is_open: RwSignal::new(false),
            last_triggered_dir: RwSignal::new(load_last_triggered_dir()),
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
            save_last_triggered_dir(current_dir);
            self.is_open.set(true);
        } else if current_dir != last.as_str() {
            // New session without pending changes: update the dir so a
            // subsequent in-session change can't incorrectly re-trigger.
            self.last_triggered_dir.set(current_dir.to_owned());
            save_last_triggered_dir(current_dir);
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

    /// Clear the `sessionStorage` key before each test so tests are isolated
    /// from one another.  No-op on non-WASM builds.
    fn clear_dirty_modal_storage() {
        #[cfg(target_arch = "wasm32")]
        {
            if let Some(storage) =
                web_sys::window().and_then(|w| w.session_storage().ok().flatten())
            {
                let _ = storage.remove_item(STORAGE_KEY);
            }
        }
    }

    /// Standard test wrapper: clears sessionStorage then runs `f` inside a
    /// fresh reactive owner.  Use [`with_owner_keep_storage`] when the test
    /// intentionally spans two owner scopes to simulate a page refresh.
    fn with_owner<F: FnOnce()>(f: F) {
        clear_dirty_modal_storage();
        let owner = Owner::new();
        owner.with(f);
    }

    /// Like `with_owner` but does **not** clear sessionStorage first —
    /// used by the refresh regression test to carry storage across two
    /// simulated "page loads".
    fn with_owner_keep_storage<F: FnOnce()>(f: F) {
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

    // ---- Refresh regression (Bug 1) -----------------------------------------

    #[wasm_bindgen_test]
    fn refresh_does_not_reopen_for_previously_acknowledged_dirty_session() {
        // Regression for: modal re-fires on browser refresh because
        // `last_triggered_dir` was not persisted across page loads.
        //
        // This test requires a real browser sessionStorage.  The
        // wasm-bindgen-test runner uses Node.js where sessionStorage is
        // absent; in that environment the test is a no-op (the feature is
        // still verified by the e2e suite which runs against Chromium).
        let Some(storage) =
            web_sys::window().and_then(|w| w.session_storage().ok().flatten())
        else {
            return; // Node.js runner — skip gracefully
        };
        let _ = storage.remove_item(STORAGE_KEY); // explicit clean slate

        // --- First page load ---
        with_owner_keep_storage(|| {
            let s = DirtyModalState::new();
            s.check("/session-abc", true);
            assert!(s.is_open.get_untracked(), "modal should open on first load");
            s.dismiss();
        });

        // --- Simulated browser refresh (new owner = new page context) ---
        with_owner_keep_storage(|| {
            let s = DirtyModalState::new(); // reads "/session-abc" from sessionStorage
            s.check("/session-abc", true); // same dir → must NOT reopen
            assert!(
                !s.is_open.get_untracked(),
                "modal must not reopen after browser refresh for same session"
            );
        });
    }
}
