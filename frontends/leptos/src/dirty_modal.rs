//! Dirty-working-tree warning modal.
//!
//! ## Trigger
//!
//! The server enforces a deny-by-default gate (mirroring the CLI's
//! `--allow-dirty` flag): when the operator initiates a `Reset` or
//! `ResumeSession` while the working tree has uncommitted git changes
//! and the frame did **not** set `allowDirty: true`, the server replies
//! with a `pending_changes_warning` frame and *does not* touch the
//! active session.  The reducer in `store.rs` records the warning's
//! `intent` payload on [`SessionStore::pending_changes_warning`].
//!
//! ## Behaviour
//!
//! When that signal is `Some(intent)`, this overlay opens with two
//! buttons:
//! - **Cancel** — clears the signal; nothing changed server-side, so
//!   the operator is back exactly where they were (picker visible if
//!   no prior session, or prior session in the background otherwise).
//! - **Proceed** — re-issues the original frame with
//!   `allow_dirty: true`, then clears the signal.  The server now
//!   tears down the previous active session and creates the new one
//!   normally.

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::protocol::{ClientFrame, PendingChangesIntent};
use crate::store::SessionStore;
use crate::ws::WsClient;

// ---------------------------------------------------------------------------
// Pure helper — translate intent + allow_dirty=true into a ClientFrame
// ---------------------------------------------------------------------------

/// Build the retry frame for a [`PendingChangesIntent`], opting in to
/// `allow_dirty: true` so the server proceeds without re-emitting the
/// warning.  Pure / host-runnable / unit-testable.
#[must_use]
pub fn retry_frame_for(intent: &PendingChangesIntent) -> ClientFrame {
    match intent {
        PendingChangesIntent::Reset { model, effort } => ClientFrame::Reset {
            model: model.clone(),
            effort: effort.clone(),
            allow_dirty: true,
            // TODO(Phase 2.1): plumb the picker's tool selection through
            // `PendingChangesIntent::Reset` so the retry preserves it.
            tool_selection: None,
        },
        PendingChangesIntent::ResumeSession { session_dir } => ClientFrame::ResumeSession {
            session_dir: session_dir.clone(),
            allow_dirty: true,
        },
    }
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

/// Dirty-working-tree warning overlay.
///
/// Renders nothing when [`SessionStore::pending_changes_warning`] is
/// `None`.  When `Some(intent)`, shows the warning with **Cancel** and
/// **Proceed** buttons.
#[component]
pub fn DirtyModal() -> impl IntoView {
    let store = use_context::<SessionStore>().expect("SessionStore must be provided");
    let ws = use_context::<WsClient>().expect("WsClient must be provided");

    let on_cancel = move |_: leptos::ev::MouseEvent| {
        store.pending_changes_warning.set(None);
    };

    // Focusable backdrop: auto-focused on mount so Esc acts as Cancel
    // even before the operator clicks inside.
    let backdrop_ref = NodeRef::<html::Div>::new();
    Effect::new(move |_| {
        if backdrop_ref.get().is_some() {
            spawn_local(async move {
                if let Some(el) = backdrop_ref.get_untracked() {
                    let _ = el.focus();
                }
            });
        }
    });
    let on_keydown = move |evt: leptos::ev::KeyboardEvent| {
        if evt.key() == "Escape" {
            store.pending_changes_warning.set(None);
        }
    };

    let on_proceed = move |_: leptos::ev::MouseEvent| {
        // Take the intent (clear the signal) and re-issue with allow_dirty=true.
        let intent = store.pending_changes_warning.get_untracked();
        store.pending_changes_warning.set(None);
        if let Some(intent) = intent {
            let frame = retry_frame_for(&intent);
            if let Err(err) = ws.send(&frame) {
                // Surface as a transport error; the modal closes either way.
                store.transport_errors.update(|v| {
                    v.push(format!("send dirty-retry: {err:?}"));
                });
            }
        }
    };

    view! {
        <Show
            when=move || store.pending_changes_warning.with(Option::is_some)
            fallback=|| ().into_any()
        >
            <div
                class="leptos-dirty-modal-backdrop"
                data-testid="leptos-dirty-modal-backdrop"
                node_ref=backdrop_ref
                tabindex="-1"
                on:keydown=on_keydown
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
                            on:click=on_proceed
                        >
                            "Proceed anyway"
                        </button>
                        <button
                            class="leptos-dirty-modal-cancel"
                            data-testid="leptos-dirty-modal-cancel"
                            on:click=on_cancel
                        >
                            "Cancel"
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

    // ---- retry_frame_for (pure, host-runnable) ------------------------------

    #[test]
    fn retry_frame_for_reset_preserves_model_effort_and_sets_allow_dirty() {
        let intent = PendingChangesIntent::Reset {
            model: Some("claude-opus-4-8".into()),
            effort: Some("high".into()),
        };
        let frame = retry_frame_for(&intent);
        match frame {
            ClientFrame::Reset {
                model,
                effort,
                allow_dirty,
                tool_selection,
            } => {
                assert_eq!(model.as_deref(), Some("claude-opus-4-8"));
                assert_eq!(effort.as_deref(), Some("high"));
                assert!(allow_dirty);
                assert!(tool_selection.is_none());
            }
            other => panic!("expected Reset, got {other:?}"),
        }
    }

    #[test]
    fn retry_frame_for_reset_with_no_model_or_effort_still_sets_allow_dirty() {
        let intent = PendingChangesIntent::Reset {
            model: None,
            effort: None,
        };
        let frame = retry_frame_for(&intent);
        match frame {
            ClientFrame::Reset {
                model,
                effort,
                allow_dirty,
                tool_selection,
            } => {
                assert!(model.is_none());
                assert!(effort.is_none());
                assert!(allow_dirty);
                assert!(tool_selection.is_none());
            }
            other => panic!("expected Reset, got {other:?}"),
        }
    }

    #[test]
    fn retry_frame_for_resume_session_preserves_dir_and_sets_allow_dirty() {
        let intent = PendingChangesIntent::ResumeSession {
            session_dir: "session-abc".into(),
        };
        let frame = retry_frame_for(&intent);
        match frame {
            ClientFrame::ResumeSession {
                session_dir,
                allow_dirty,
            } => {
                assert_eq!(session_dir, "session-abc");
                assert!(allow_dirty);
            }
            other => panic!("expected ResumeSession, got {other:?}"),
        }
    }

    #[test]
    fn retry_frame_for_reset_serialises_with_allow_dirty_true_on_wire() {
        let intent = PendingChangesIntent::Reset {
            model: None,
            effort: None,
        };
        let frame = retry_frame_for(&intent);
        let json = serde_json::to_string(&frame).unwrap();
        assert_eq!(json, r#"{"type":"reset","allowDirty":true}"#);
    }

    #[test]
    fn retry_frame_for_resume_session_serialises_with_allow_dirty_true_on_wire() {
        let intent = PendingChangesIntent::ResumeSession {
            session_dir: "abc".into(),
        };
        let frame = retry_frame_for(&intent);
        let json = serde_json::to_string(&frame).unwrap();
        assert_eq!(
            json,
            r#"{"type":"resume_session","sessionDir":"abc","allowDirty":true}"#
        );
    }
}
