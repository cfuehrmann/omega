//! Session-picker component (Phase 3.2).
//!
//! Renders the picker UI:
//!
//! ```text
//! +------------------------------------------------+
//! | [ + new session ]                              |
//! |------------------------------------------------|
//! | * 2026-05-04T18-37-19-…  active  [rename] [del]|
//! |   2026-05-04T18-32-12-…  beta    [rename] [del]|
//! |   2026-05-04T18-29-04-…  alpha   [rename] [del]|
//! +------------------------------------------------+
//! ```
//!
//! "Active" is `SessionListItem.dir == SessionStore::session_info.dir`.
//! Wire frames sent from this component:
//!
//! - **New** → `ClientFrame::Reset { None, None }` (see Phase 3.2 record
//!   for the Reset-vs-POST decision). Server emits
//!   `session_info → history → reset_done`; an `Effect` watching
//!   `session_info.dir` triggers a refetch so the new dir appears in
//!   the list.
//! - **Rename** → `ClientFrame::RenameSession { dir, name }`. Server
//!   broadcasts `session_renamed`; reducer updates the local list.
//! - **Delete** → `ClientFrame::DeleteSession { dir }` after a
//!   `window.confirm()` prompt. Server broadcasts `session_deleted`;
//!   reducer removes the entry.
//! - **Resume** → `ClientFrame::ResumeSession { dir }` (Phase 3.5).
//!   Server emits `session_info → history → resuming_session →
//!   session_resumed → ready` for the new session derived from the
//!   target's last-message basis. Hits the `Reset`-style path on
//!   the server: the active session is replaced by a fresh one
//!   seeded with a resumption summary.

use leptos::prelude::*;
use leptos::reactive::owner::LocalStorage;
use leptos::task::spawn_local;
use leptos::web_sys;

use crate::http::get_sessions;
use crate::protocol::ClientFrame;
use crate::sessions::{SessionListItem, SessionListStore, is_active};
use crate::store::SessionStore;
use crate::ws::WsClient;

/// Top-level picker view. Looks up `SessionStore`, `SessionListStore`,
/// and `WsClient` from context (provided at the `App` root).
#[component]
pub fn SessionPicker() -> impl IntoView {
    let conv = use_context::<SessionStore>().expect("SessionStore must be provided");
    let list = use_context::<SessionListStore>().expect("SessionListStore must be provided");
    let ws = use_context::<WsClient>().expect("WsClient must be provided");

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

    // Derived "active dir" reader used by item rows.
    let active_dir = Memo::new(move |_| {
        conv.session_info
            .with(|si| si.as_ref().map(|s| s.dir.clone()))
    });

    let on_new_click = move |_| {
        if let Err(err) = ws.send(&ClientFrame::Reset {
            model: None,
            effort: None,
        }) {
            list.set_error(format!("send Reset: {err:?}"));
        }
    };

    view! {
        <section data-testid="leptos-session-picker">
            <header class="picker-header">
                <h2>"Sessions"</h2>
                <button
                    data-testid="leptos-session-new"
                    on:click=on_new_click
                >
                    "+ new session"
                </button>
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
                            <SessionRow item=item active_dir=active_dir />
                        }
                    }
                />
            </ul>
        </section>
    }
}

/// One row in the picker list. Owns the inline-rename edit state.
///
/// `dir` is stored once in a `StoredValue<String>` so every event
/// handler closure captures only `Copy` values and the row cleanly
/// composes inside `<Show>` / `<For>` without `.clone()` ceremony.
#[component]
fn SessionRow(item: SessionListItem, active_dir: Memo<Option<String>>) -> impl IntoView {
    let ws = use_context::<WsClient>().expect("WsClient must be provided");
    let list = use_context::<SessionListStore>().expect("SessionListStore must be provided");

    let dir_sv: StoredValue<String, LocalStorage> = StoredValue::new_local(item.dir.clone());

    // Inline rename: edit-mode flag + draft text.
    let editing = RwSignal::new(false);
    let draft = RwSignal::new(item.name.clone().unwrap_or_else(|| item.dir.clone()));

    let begin_rename = move |_| {
        // Re-seed the draft from the latest server-confirmed name in
        // case the row was renamed by another tab/client since the
        // last open. Reading from `list.sessions` keeps us truthful.
        let dir = dir_sv.get_value();
        let current = list
            .sessions
            .with(|v| v.iter().find(|i| i.dir == dir).cloned());
        if let Some(curr) = current {
            draft.set(curr.name.unwrap_or_else(|| curr.dir.clone()));
        }
        editing.set(true);
    };

    let cancel_rename = move |_| {
        editing.set(false);
    };

    let submit_rename = move |_| {
        let name = draft.get();
        if name.trim().is_empty() {
            // Empty would rename-to-empty, which the server accepts —
            // but unhelpful. Just cancel.
            editing.set(false);
            return;
        }
        let frame = ClientFrame::RenameSession {
            session_dir: dir_sv.get_value(),
            name,
        };
        match ws.send(&frame) {
            Ok(()) => editing.set(false),
            Err(err) => list.set_error(format!("send RenameSession: {err:?}")),
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
    };

    // Resume from this row — Phase 3.5. Sends
    // `ClientFrame::ResumeSession`; the server replaces the active
    // session with a fresh one seeded from this dir's last
    // assistant message + extracted basis. The conversation feed
    // (3.3) renders the resulting `resuming_session` /
    // `session_resumed` events through the status family.
    let on_resume = move |_| {
        let dir = dir_sv.get_value();
        let frame = ClientFrame::ResumeSession { session_dir: dir };
        if let Err(err) = ws.send(&frame) {
            list.set_error(format!("send ResumeSession: {err:?}"));
        }
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
                when=move || !editing.get()
                fallback=move || view! {
                    <span class="session-item-edit">
                        <input
                            data-testid="leptos-session-rename-input"
                            prop:value=move || draft.get()
                            on:input=move |evt| draft.set(event_target_value(&evt))
                        />
                        <button
                            data-testid="leptos-session-rename-submit"
                            on:click=submit_rename
                        >
                            "save"
                        </button>
                        <button
                            data-testid="leptos-session-rename-cancel"
                            on:click=cancel_rename
                        >
                            "cancel"
                        </button>
                    </span>
                }
            >
                <span class="session-item-label" data-testid="leptos-session-label">
                    {label}
                </span>
                <Show when=move || active.get() fallback=|| ().into_any()>
                    <span data-testid="leptos-session-active-marker" class="session-item-active-marker">
                        " (active)"
                    </span>
                </Show>
                <button
                    data-testid="leptos-session-resume"
                    on:click=on_resume
                >
                    "resume"
                </button>
                <button
                    data-testid="leptos-session-rename"
                    on:click=begin_rename
                >
                    "rename"
                </button>
                <button
                    data-testid="leptos-session-delete"
                    on:click=on_delete
                >
                    "delete"
                </button>
            </Show>
        </li>
    }
}

/// Helper to read `<input>` value out of a generic `Event`.
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
async fn refresh_sessions(list: SessionListStore) {
    let token = list.begin_loading();
    match get_sessions().await {
        Ok(items) => list.finish_loading_if_current(token, items),
        Err(message) => list.fail_loading_if_current(token, message),
    }
}
