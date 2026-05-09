//! Session-picker component (Phase 3.2 + Phase 3.9).
//!
//! Renders the picker UI as a modal overlay (Phase 3.9 open/close):
//!
//! ```text
//! +-- backdrop (fixed, click-outside closes) ----+
//! | +-- picker panel (.leptos-session-picker) --+|
//! | | [ Sessions ]               [ + new ] [✕] ||
//! | |-------------------------------------------||
//! | | * 2026-05-04T18-37-19-…  (active) [↩] [✎]||
//! | |   2026-05-04T18-32-12-…          [↩] [✎] ||
//! | +-------------------------------------------+|
//! +-----------------------------------------------+
//! ```
//!
//! "Active" is `SessionListItem.dir == SessionStore::session_info.dir`.
//! Wire frames sent from this component:
//!
//! - **New** → `ClientFrame::Reset { None, None }` (see Phase 3.2 record
//!   for the Reset-vs-POST decision). Server emits
//!   `session_info → history → reset_done`; an `Effect` watching
//!   `session_info.dir` triggers a refetch so the new dir appears in
//!   the list. Picker auto-closes on success (Phase 3.9 TODO-2).
//! - **Rename** → `ClientFrame::RenameSession { dir, name }`. Server
//!   broadcasts `session_renamed`; reducer updates the local list.
//! - **Delete** → `ClientFrame::DeleteSession { dir }` after a
//!   `window.confirm()` prompt. Server broadcasts `session_deleted`;
//!   reducer removes the entry.
//! - **Resume** → `ClientFrame::ResumeSession { dir }` (Phase 3.5).
//!   Server emits `session_info → history → resuming_session →
//!   session_resumed → ready` for the new session derived from the
//!   target's last-message basis. Picker auto-closes on success
//!   (Phase 3.9 TODO-2).
//!
//! ## Open/close state (Phase 3.9 TODO-1)
//!
//! [`PickerOpen`] is provided at the App root and consumed by both
//! `SessionPicker` (to render/hide the backdrop + panel) and
//! `Composer` (to add the "Sessions" open button). Default is `true`
//! so every existing spec that doesn't click "open" first continues
//! to pass; the picker is open on first mount.
//!
//! Dismissal vectors:
//! - `✕` button in picker header
//! - Click on the dark backdrop outside the panel
//! - Esc key while the backdrop is focused / active
//! - Creating a new session (Reset) — auto-closes on send
//! - Resuming a session — auto-closes on send

use leptos::html;
use leptos::prelude::*;
use leptos::reactive::owner::LocalStorage;
use leptos::task::spawn_local;
use leptos::web_sys;

use crate::http::get_sessions;
use crate::composer::ComposerInsert;
use crate::protocol::ClientFrame;
use crate::sessions::{SessionListItem, SessionListStore, is_active};
use crate::store::SessionStore;
use crate::ws::WsClient;

// ---------------------------------------------------------------------------
// PickerOpen context handle (Phase 3.9 TODO-1)
// ---------------------------------------------------------------------------

/// Wraps the picker's open/close `RwSignal<bool>`.
///
/// Provided at the `App` root so both `SessionPicker` (renders the
/// panel) and `Composer` (renders the "Sessions" open button) can read
/// and write the same signal without prop-drilling.
///
/// Default is `false` — picker closed on first mount (Phase 3.10
/// TODO-F). The `App` opens the picker via an `Effect` once the WS
/// connection lands **and** there is no active session yet (typical
/// fresh-server case). Browser refresh of an active session lands
/// directly in the conversation feed.
///
/// Wrapped in a newtype so `provide_context` / `use_context` find a
/// unique type — leptos's context lookup is type-keyed.
#[derive(Debug, Clone, Copy)]
pub struct PickerOpen(pub RwSignal<bool>);

impl PickerOpen {
    /// Construct fresh state (picker hidden). Must run inside a leptos
    /// reactive `Owner` scope.
    #[must_use]
    pub fn new() -> Self {
        Self(RwSignal::new(false))
    }

    /// Open the picker.
    #[mutants::skip] // reactive signal write; covered by e2e harness picker tests.
    pub fn open(self) {
        self.0.set(true);
    }

    /// Close the picker.
    #[mutants::skip] // reactive signal write; covered by e2e harness picker tests.
    pub fn close(self) {
        self.0.set(false);
    }
}

impl Default for PickerOpen {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// SessionPicker component
// ---------------------------------------------------------------------------

/// Top-level picker view. Looks up `SessionStore`, `SessionListStore`,
/// `WsClient`, and `PickerOpen` from context (provided at the `App` root).
///
/// Renders as a fixed-position dark backdrop with the picker panel
/// centred inside. Backdrop click + Esc key + `✕` button all dismiss
/// the picker. `+ new session` and `[resume]` buttons auto-close the
/// picker after the send succeeds (Phase 3.9 TODO-2).
/// Mutations skipped: session-watch Effect conditions and key handlers
/// require live reactive/DOM context; all behaviour verified by e2e harness.
#[mutants::skip]
#[component]
pub fn SessionPicker() -> impl IntoView {
    let conv = use_context::<SessionStore>().expect("SessionStore must be provided");
    let list = use_context::<SessionListStore>().expect("SessionListStore must be provided");
    let ws = use_context::<WsClient>().expect("WsClient must be provided");
    let picker_open = use_context::<PickerOpen>().expect("PickerOpen must be provided");

    // Initial fetch on mount.
    Effect::new(move |_| {
        spawn_local(async move {
            refresh_sessions(list).await;
        });
    });

    // Refetch whenever the active session's dir changes (covers the
    // post-Reset session_info broadcast). Tracking returns the prior
    // value so a no-op transition (same dir) skips the fetch.
    Effect::new(move |prev: Option<Option<String>>| {
        let dir = conv
            .session_info
            .with(|si| si.as_ref().map(|s| s.dir.clone()));
        let prev = prev.flatten();
        if prev != dir && dir.is_some() {
            spawn_local(async move {
                refresh_sessions(list).await;
            });
        }
        dir
    });

    // True when a session is active — used to lock down all close vectors
    // when no session exists (the operator must pick or create one).
    let has_session = Memo::new(move |_| conv.session_info.with(Option::is_some));

    // Which row is currently being renamed (None = none). Shared across all
    // rows so at most one can be in edit mode at a time.
    let editing_dir: RwSignal<Option<String>> = RwSignal::new(None);

    // NodeRef used to focus the backdrop when the picker opens so that
    // keydown events reach it even before the operator interacts with the
    // panel. Mirrors the same spawn_local autofocus pattern used by
    // SessionRow for the rename input.
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

    // When a rename finishes (editing_dir transitions Some → None), return
    // focus to the backdrop so Esc-to-close keeps working. Without this the
    // focus stays on the now-unmounted <input> and keydown never reaches the
    // backdrop's on:keydown handler.
    Effect::new(move |prev_editing: Option<Option<String>>| {
        let current = editing_dir.get();
        if matches!(prev_editing, Some(Some(_))) && current.is_none() {
            spawn_local(async move {
                if let Some(el) = backdrop_ref.get_untracked() {
                    let _ = el.focus();
                }
            });
        }
        current
    });

    // Derived "active dir" reader used by item rows.
    let active_dir = Memo::new(move |_| {
        conv.session_info
            .with(|si| si.as_ref().map(|s| s.dir.clone()))
    });

    // TODO-2: Reset auto-closes the picker.
    let on_new_click = move |_| {
        if let Err(err) = ws.send(&ClientFrame::Reset {
            model: None,
            effort: None,
        }) {
            list.set_error(format!("send Reset: {err:?}"));
            return;
        }
        picker_open.close();
    };

    // TODO-1: close handler shared by `✕` button and backdrop click.
    // Guard: no-op when there is no active session — the operator must
    // pick or create one before the picker can be dismissed.
    let on_close = move |_: leptos::ev::MouseEvent| {
        if has_session.get_untracked() {
            picker_open.close();
        }
    };

    // Esc-key dismissal on the backdrop div.
    // Guards:
    //  1. No active session → Esc is ignored (operator must choose a session).
    //  2. A rename is in progress → Esc is ignored here (the rename input's
    //     own keydown already handles it and calls stop_propagation, but this
    //     check is a safety-net for any other path that might bubble).
    let on_keydown = move |evt: leptos::ev::KeyboardEvent| {
        if evt.key() == "Escape"
            && has_session.get_untracked()
            && editing_dir.with_untracked(Option::is_none)
        {
            picker_open.close();
        }
    };

    // Stop propagation so clicking inside the panel doesn't bubble
    // up to the backdrop and close the picker.
    let stop_propagation =
        move |evt: leptos::ev::MouseEvent| evt.stop_propagation();

    view! {
        <Show when=move || picker_open.0.get() fallback=|| ()>
            <div
                class="picker-backdrop"
                data-testid="leptos-picker-backdrop"
                node_ref=backdrop_ref
                on:click=on_close
                on:keydown=on_keydown
                // tabindex makes the div focusable so keydown fires.
                // Auto-focused on mount (see Effect above) so Esc works
                // immediately even before the operator clicks inside the panel.
                tabindex="-1"
            >
                <section
                    data-testid="leptos-session-picker"
                    on:click=stop_propagation
                >
                    <header class="picker-header">
                        <h2>"Sessions"</h2>
                        <div class="picker-header-btns">
                            <button
                                data-testid="leptos-session-new"
                                on:click=on_new_click
                            >
                                "+ new session"
                            </button>
                            <Show when=move || has_session.get() fallback=|| ()>
                                <button
                                    class="picker-close"
                                    data-testid="leptos-picker-close"
                                    on:click=on_close
                                >
                                    "✕"
                                </button>
                            </Show>
                        </div>
                    </header>
                    <Show
                        when=move || list.last_error.with(Option::is_some)
                        fallback=|| ().into_any()
                    >
                        <p
                            data-testid="leptos-session-error"
                            class="picker-error"
                        >
                            {move || list.last_error.with(|e| e.clone().unwrap_or_default())}
                        </p>
                    </Show>
                    <ul data-testid="leptos-session-list">
                        <For
                            each=move || list.sessions.get()
                            key=|item: &SessionListItem| item.dir.clone()
                            children=move |item: SessionListItem| {
                                view! {
                                    <SessionRow
                                        item=item
                                        active_dir=active_dir
                                        editing_dir=editing_dir
                                        backdrop_ref=backdrop_ref
                                    />
                                }
                            }
                        />
                    </ul>
                </section>
            </div>
        </Show>
    }
}

// ---------------------------------------------------------------------------
// SessionRow component
// ---------------------------------------------------------------------------

/// One row in the picker list. Participates in shared inline-rename state.
///
/// `dir` is stored once in a `StoredValue<String>` so every event
/// handler closure captures only `Copy` values and the row cleanly
/// composes inside `<Show>` / `<For>` without `.clone()` ceremony.
///
/// `editing_dir` is the picker-level `RwSignal<Option<String>>` that holds
/// the `dir` of the row currently being renamed, or `None`. Because it is
/// shared across all rows only one row can be in edit mode at a time:
/// clicking a label calls `editing_dir.set(Some(own_dir))`, which
/// automatically collapses any other row that was open.
///
/// TODO-2: `on_resume` sets `PickerOpen` to `false` after the send
/// succeeds so the picker auto-closes when the operator resumes a
/// session. Rename and delete do NOT close the picker (the operator
/// is mid-task on the list).
/// Mutations skipped: all row-condition checks and state mutations require
/// live reactive context; all behaviour verified by e2e harness.
#[mutants::skip]
#[component]
fn SessionRow(
    item: SessionListItem,
    active_dir: Memo<Option<String>>,
    editing_dir: RwSignal<Option<String>>,
    /// Backdrop NodeRef from the parent `SessionPicker`. Used by
    /// `on_rename_keydown` to synchronously return focus to the picker
    /// before tearing down the rename input — otherwise focus escapes to
    /// `<body>` and the picker becomes a focus-less ghost (especially
    /// problematic when there is no active session and the modal can't be
    /// dismissed). The post-rename Effect on `SessionPicker` also focuses
    /// the backdrop, but it does so via `spawn_local`, which runs as a
    /// microtask — too late: the browser has already moved focus to <body>
    /// when the focused input was unmounted.
    backdrop_ref: NodeRef<html::Div>,
) -> impl IntoView {
    let ws = use_context::<WsClient>().expect("WsClient must be provided");
    let list = use_context::<SessionListStore>().expect("SessionListStore must be provided");
    let picker_open = use_context::<PickerOpen>().expect("PickerOpen must be provided");
    let composer_insert =
        use_context::<ComposerInsert>().expect("ComposerInsert must be provided");

    let dir_sv: StoredValue<String, LocalStorage> = StoredValue::new_local(item.dir.clone());

    // Draft text for the rename input.
    let draft = RwSignal::new(item.name.clone().unwrap_or_else(|| item.dir.clone()));

    // This row is in edit mode when the shared editing_dir matches our dir.
    let editing = move || {
        let dir = dir_sv.get_value();
        editing_dir.with(|opt| opt.as_deref() == Some(dir.as_str()))
    };

    // NodeRef for the rename <input>. Used to focus + select-all when edit
    // mode activates so the user can type straight away.
    let input_ref = NodeRef::<html::Input>::new();

    // Runs whenever the input element is mounted into the DOM — i.e. the
    // moment <Show>'s fallback renders it (editing just became true).
    // spawn_local defers to a microtask so the DOM node is fully live
    // before focus/select are called.
    Effect::new(move |_| {
        if input_ref.get().is_some() {
            spawn_local(async move {
                if let Some(el) = input_ref.get_untracked() {
                    let _ = el.focus();
                    el.select();
                }
            });
        }
    });

    // Clicking the session label opens rename mode. Re-seeds the draft
    // from the latest server-confirmed name so a concurrent rename by
    // another tab is picked up correctly. Setting editing_dir to our dir
    // automatically closes any other row that was open.
    let begin_rename = move |_: leptos::ev::MouseEvent| {
        let dir = dir_sv.get_value();
        let current = list
            .sessions
            .with(|v| v.iter().find(|i| i.dir == dir).cloned());
        if let Some(curr) = current {
            draft.set(curr.name.unwrap_or_else(|| curr.dir.clone()));
        }
        editing_dir.set(Some(dir));
    };

    // Enter submits the rename; Escape cancels. No separate save button.
    //
    // Both branches stop propagation: keystrokes that finish a rename are
    // "consumed" by the input and must not surface as picker-level shortcuts.
    // Both branches also synchronously move focus back to the picker backdrop
    // *before* tearing down the input. Otherwise the browser reassigns focus
    // to <body> when the focused input is unmounted, and the picker becomes
    // a focus-less ghost — particularly bad when there is no active session
    // and the modal can't be dismissed: the operator has to click into the
    // panel again before the next keypress is heard. The post-rename Effect
    // also focuses the backdrop, but it does so via `spawn_local` (a
    // microtask), which runs *after* the browser has already moved focus to
    // <body>; doing it inline here closes that gap.
    let refocus_backdrop = move || {
        if let Some(el) = backdrop_ref.get_untracked() {
            let _ = el.focus();
        }
    };
    let on_rename_keydown = move |evt: leptos::ev::KeyboardEvent| {
        match evt.key().as_str() {
            "Enter" => {
                evt.prevent_default();
                evt.stop_propagation();
                let name = draft.get();
                if name.trim().is_empty() {
                    // Empty name — just cancel, same as before.
                    refocus_backdrop();
                    editing_dir.set(None);
                    return;
                }
                let frame = ClientFrame::RenameSession {
                    session_dir: dir_sv.get_value(),
                    name,
                };
                match ws.send(&frame) {
                    Ok(()) => {
                        refocus_backdrop();
                        editing_dir.set(None);
                    }
                    Err(err) => list.set_error(format!("send RenameSession: {err:?}")),
                }
            }
            "Escape" => {
                // Stop propagation so the Escape doesn't bubble up to
                // the picker backdrop and close the whole picker.
                evt.stop_propagation();
                refocus_backdrop();
                editing_dir.set(None);
            }
            _ => {}
        }
    };

    let on_delete = move |_| {
        let dir = dir_sv.get_value();
        let confirmed = web_sys::window()
            .and_then(|w| w.confirm_with_message(&format!("Delete session {dir}?")).ok())
            .unwrap_or(false);
        if !confirmed {
            return;
        }
        let frame = ClientFrame::DeleteSession { session_dir: dir };
        if let Err(err) = ws.send(&frame) {
            list.set_error(format!("send DeleteSession: {err:?}"));
        }
        // NOTE: delete does NOT close the picker — the operator is
        // mid-task on the list.
    };

    // Resume from this row — Phase 3.5 + Phase 3.9 TODO-2.
    // Sends `ClientFrame::ResumeSession`; the server replaces the
    // active session with a fresh one seeded from this dir's last
    // assistant message + extracted basis. The conversation feed
    // (3.3) renders the resulting `resuming_session` /
    // `session_resumed` events through the status family.
    // Auto-closes picker on success.
    let on_resume = move |_| {
        let dir = dir_sv.get_value();
        let frame = ClientFrame::ResumeSession { session_dir: dir };
        if let Err(err) = ws.send(&frame) {
            list.set_error(format!("send ResumeSession: {err:?}"));
            return;
        }
        picker_open.close();
    };

    let on_insert_at = move |_| {
        let dir = dir_sv.get_value();
        // Build the full relative path so the agent can locate the
        // session on disk: `.omega/sessions/<dir>/`.
        let path = format!(".omega/sessions/{}/", dir);
        composer_insert.insert(path);
        picker_open.close();
    };

    let active = Memo::new(move |_| {
        let dir = dir_sv.get_value();
        active_dir.with(|d| {
            // The pure helper takes the full struct; only `dir` is
            // read. The default-padding makes the call clean.
            let it = SessionListItem { dir, ..Default::default() };
            is_active(&it, d.as_deref())
        })
    });

    // Reactive label: re-reads from the list signal so a server-confirmed
    // rename updates the visible text without a row re-mount.
    let label = move || {
        let dir = dir_sv.get_value();
        list.sessions.with(|v| {
            v.iter()
                .find(|i| i.dir == dir)
                .and_then(|i| i.name.clone())
                .unwrap_or(dir)
        })
    };

    view! {
        <li
            data-testid="leptos-session-item"
            data-session-dir=move || dir_sv.get_value()
            data-active=move || if active.get() { "true" } else { "false" }
            class=move || if active.get() { "session-item session-item-active" } else { "session-item" }
        >
            <Show
                when=move || !editing()
                fallback=move || view! {
                    <span class="session-item-edit">
                        <input
                            data-testid="leptos-session-rename-input"
                            node_ref=input_ref
                            prop:value=move || draft.get()
                            on:input=move |evt| draft.set(event_target_value(&evt))
                            on:keydown=on_rename_keydown
                        />
                    </span>
                }
            >
                <span
                    class="session-item-label"
                    data-testid="leptos-session-label"
                    on:click=begin_rename
                    title="Click to rename"
                >
                    {label}
                </span>
                <div class="session-item-actions">
                    <Show when=move || !active.get() fallback=|| ().into_any()>
                        <button
                            data-testid="leptos-session-resume"
                            on:click=on_resume
                        >
                            "resume"
                        </button>
                        <button
                            data-testid="leptos-session-delete"
                            on:click=on_delete
                        >
                            "delete"
                        </button>
                    </Show>
                    <button
                        data-testid="leptos-session-insert-at"
                        title="Insert session path as @ reference in prompt"
                        on:click=on_insert_at
                    >
                        "@ path"
                    </button>
                </div>
            </Show>
        </li>
    }
}

/// Helper to read `<input>` value out of a generic `Event`.
#[mutants::skip] // web-sys DOM cast; covered by e2e harness compose/rename tests.
fn event_target_value(evt: &leptos::ev::Event) -> String {
    use wasm_bindgen::JsCast;
    evt.target()
        .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
        .map(|el| el.value())
        .unwrap_or_default()
}

/// Fetch `/api/sessions` and dispatch the result into the store.
///
/// Captures the [`SessionListStore::begin_loading`] token before the
/// fetch starts, then routes the result through
/// [`finish_loading_if_current`] / [`fail_loading_if_current`] so a
/// concurrent `SessionDeleted` / `SessionRenamed` broadcast (which
/// bumps the generation) drops this stale result. See `sessions.rs`
/// struct-level docs for the race scenario.
#[mutants::skip] // async fetch side-effect; covered by e2e harness session-list tests.
async fn refresh_sessions(list: SessionListStore) {
    let token = list.begin_loading();
    match get_sessions().await {
        Ok(items) => list.finish_loading_if_current(token, items),
        Err(message) => list.fail_loading_if_current(token, message),
    }
}
