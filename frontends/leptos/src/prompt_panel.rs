//! Collapsible prompt panel — the operator's composition surface.
//!
//! ## Layout position
//!
//! ```text
//! ┌──────────────────────────────────────────┐
//! │  ConversationFeed  (always fully visible) │
//! ├──────────────────────────────────────────┤ ← panel top edge (open only)
//! │  PromptPanel  (30 vh, flex-shrink:0)      │
//! │   textarea, Send ⏎, ▼ Collapse           │
//! ├──────────────────────────────────────────┤
//! │  Composer bottom bar  (always visible)    │
//! └──────────────────────────────────────────┘
//! ```
//!
//! The panel **pushes** the feed up — it never overlays it.  When
//! collapsed the feed reoccupies the full remaining height.
//!
//! ## Opening the panel
//!
//! The **Prompt** button in [`crate::composer::Composer`] calls
//! `prompt_panel.open.set(true)`.  If the server has an external editor
//! configured (`editor_configured: true` in `SessionInfo`), the button
//! additionally increments `trigger_editor`, which an `Effect` inside
//! this component watches and responds to by calling `do_editor()`.
//!
//! ## Draft persistence
//!
//! The draft is kept in a `RwSignal<String>` that is also mirrored to
//! `localStorage` on every keystroke.  The key is
//! `omega.draft.<origin>` so different Omega instances on different
//! ports/hosts get independent drafts.  On send the key is deleted.
//!
//! ## Closing / collapsing
//!
//! Clicking ▼ or pressing **Esc** (when no completion popup is open)
//! collapses the panel — `open` goes `false` — but the draft is
//! **not** cleared.  The badge dot on the Prompt button remains visible
//! so the operator knows there is unsent text.
//!
//! ## Mutation-test carve-out
//!
//! The component body is DOM/reactive glue (same gap as `composer.rs`,
//! `picker.rs`, `feed.rs`).  Exercised exclusively by the e2e harness.

use leptos::ev;
use leptos::html;
use leptos::prelude::*;
use leptos::reactive::owner::LocalStorage;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;
use web_sys::HtmlTextAreaElement;

use crate::completion::{accept_completion, at_token_at_cursor, next_highlight, selected_item};
use crate::http::{compose_via_editor, get_files};
use crate::protocol::ClientFrame;
use crate::store::SessionStore;
use crate::ws::WsClient;

// ---------------------------------------------------------------------------
// localStorage key
// ---------------------------------------------------------------------------

/// Base key for draft persistence. Suffixed with the window origin at
/// runtime so different Omega instances on different ports are isolated.
#[cfg(target_arch = "wasm32")]
const DRAFT_STORAGE_KEY: &str = "omega.draft";

// ---------------------------------------------------------------------------
// PromptPanelState  (provided as context by App)
// ---------------------------------------------------------------------------

/// Shared reactive state for the collapsible prompt panel.
///
/// Provided as context by [`crate::App`] so that both the
/// [`crate::composer::Composer`] (Prompt button) and
/// [`PromptPanel`] itself can read/write it.
#[derive(Clone, Copy)]
pub struct PromptPanelState {
    /// Whether the panel is currently expanded (visible).
    pub open: RwSignal<bool>,
    /// Current draft text — canonical source of truth.
    pub draft: RwSignal<String>,
    /// `true` while POST /api/compose is in flight (external editor open).
    pub editor_in_flight: RwSignal<bool>,
    /// Monotonic counter: the Prompt button increments this to request the
    /// panel to launch the external editor immediately.  An `Effect` inside
    /// `PromptPanel` watches for increments and fires `do_editor()`.
    pub trigger_editor: RwSignal<u32>,
}

impl PromptPanelState {
    /// Create a new state, pre-loading the draft from `localStorage`.
    /// Call once from [`crate::App`]; provide the result via context.
    #[must_use]
    pub fn new() -> Self {
        Self {
            open: RwSignal::new(false),
            draft: RwSignal::new(load_draft()),
            editor_in_flight: RwSignal::new(false),
            trigger_editor: RwSignal::new(0),
        }
    }
}

// ---------------------------------------------------------------------------
// localStorage helpers
// ---------------------------------------------------------------------------

fn load_draft() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        let key = draft_key();
        web_sys::window()
            .and_then(|w| w.local_storage().ok().flatten())
            .and_then(|s| s.get_item(&key).ok().flatten())
            .unwrap_or_default()
    }
    #[cfg(not(target_arch = "wasm32"))]
    String::new()
}

#[cfg(target_arch = "wasm32")]
fn draft_key() -> String {
    let origin = web_sys::window()
        .and_then(|w| w.location().origin().ok())
        .unwrap_or_default();
    format!("{DRAFT_STORAGE_KEY}.{origin}")
}

fn save_draft(text: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        let key = draft_key();
        if let Some(ls) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
            if text.is_empty() {
                let _ = ls.remove_item(&key);
            } else {
                let _ = ls.set_item(&key, text);
            }
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = text;
    }
}

// ---------------------------------------------------------------------------
// PromptPanel component
// ---------------------------------------------------------------------------

/// Collapsible prompt-input panel.
///
/// When [`PromptPanelState::open`] is `false` the panel renders as nothing
/// (zero height).  When `true` it occupies 30 vh between the feed and the
/// bottom bar.  The textarea is auto-focused on open.
///
/// Skipped from mutation testing: component body is reactive/DOM glue;
/// exercised exclusively by the e2e harness.
#[mutants::skip]
#[component]
pub fn PromptPanel() -> impl IntoView {
    let state = use_context::<PromptPanelState>().expect("PromptPanelState must be provided");
    let store = use_context::<SessionStore>().expect("SessionStore must be provided");
    let ws = use_context::<WsClient>().expect("WsClient must be provided");

    let textarea_ref = NodeRef::<html::Textarea>::new();

    // File-completion popup state.
    let completion_items = RwSignal::new(Vec::<String>::new());
    let completion_highlight = RwSignal::new(-1_i32);
    let completion_open = RwSignal::new(false);
    // Stable counter to drop stale fetch results.
    let completion_seq: StoredValue<u64, LocalStorage> = StoredValue::new_local(0);

    let close_completion = move || {
        completion_open.set(false);
        completion_items.set(Vec::new());
        completion_highlight.set(-1);
    };

    // Fire a /api/files fetch for `prefix`. Stale fetches are discarded by
    // comparing the seq token at completion time.
    let query_completion = move |prefix: String| {
        let next = completion_seq.with_value(|v| v.wrapping_add(1));
        completion_seq.set_value(next);
        spawn_local(async move {
            match get_files(&prefix).await {
                Ok(items) => {
                    if completion_seq.with_value(|v| *v) != next {
                        return; // stale
                    }
                    let any = !items.is_empty();
                    completion_items.set(items);
                    completion_highlight.set(-1);
                    completion_open.set(any);
                }
                Err(_) => {
                    if completion_seq.with_value(|v| *v) != next {
                        return;
                    }
                    close_completion();
                }
            }
        });
    };

    // Read cursor position + value from the live textarea DOM node.
    let read_textarea = move || -> Option<(String, usize)> {
        let el = textarea_ref.get()?;
        let value = el.value();
        let cursor = el
            .selection_start()
            .ok()
            .flatten()
            .map_or_else(|| value.len(), |c| c as usize);
        Some((value, cursor))
    };

    // Apply a textarea state update: value, cursor, and the draft signal.
    let set_textarea_state = move |new_text: String, new_cursor: usize| {
        if let Some(el) = textarea_ref.get() {
            el.set_value(&new_text);
            #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
            let cursor_u32 = new_cursor.min(u32::MAX as usize) as u32;
            let _ = el.set_selection_start(Some(cursor_u32));
            let _ = el.set_selection_end(Some(cursor_u32));
        }
        state.draft.set(new_text);
    };

    // Accept the highlighted completion item (or do nothing if none).
    let accept_highlighted = move || {
        let Some((text, cursor)) = read_textarea() else {
            close_completion();
            return;
        };
        let item_owned = completion_items.with(|items| {
            selected_item(items, completion_highlight.get_untracked()).map(str::to_owned)
        });
        let Some(item) = item_owned else {
            close_completion();
            return;
        };
        let Some(out) = accept_completion(&text, cursor, &item) else {
            close_completion();
            return;
        };
        set_textarea_state(out.new_text, out.new_cursor);
        if out.drill_in {
            query_completion(item);
        } else {
            close_completion();
        }
    };

    // ---- core actions -------------------------------------------------------

    // Send the current draft: enqueue it and clear the textarea, then
    // collapse the panel.  No-op for blank content.
    let do_send = move || {
        let content = state.draft.get();
        if content.trim().is_empty() {
            return;
        }
        if let Err(err) = ws.send(&ClientFrame::UserMessage { content }) {
            leptos::logging::warn!("prompt panel send failed: {err:?}");
            return;
        }
        set_textarea_state(String::new(), 0);
        state.open.set(false);
    };

    // Launch the external editor ($OMEGA_EDITOR / $VISUAL / $EDITOR), seeded
    // with the current draft.  The result is dropped back into the textarea
    // for review — NOT sent automatically.  On failure the draft is left
    // untouched and the error appears in the transport-error banner.
    let do_editor = move || {
        let draft_now = state.draft.get_untracked();
        state.editor_in_flight.set(true);
        spawn_local(async move {
            match compose_via_editor(&draft_now).await {
                Ok(content) => {
                    let cursor = content.len();
                    set_textarea_state(content, cursor);
                    state.editor_in_flight.set(false);
                    if let Some(el) = textarea_ref.get() {
                        let _ = el.focus();
                    }
                }
                Err(err) => {
                    leptos::logging::warn!("prompt panel editor failed: {err}");
                    store
                        .transport_errors
                        .update(|v| v.push(format!("Editor: {err}")));
                    state.editor_in_flight.set(false);
                }
            }
        });
    };

    // ---- reactive effects ---------------------------------------------------

    // Persist draft to localStorage on every change.
    Effect::new(move |_| {
        let text = state.draft.get();
        save_draft(&text);
    });

    // Auto-focus the textarea whenever the panel opens (and editor not busy).
    Effect::new(move |_| {
        if state.open.get() && !state.editor_in_flight.get() {
            spawn_local(async move {
                if let Some(el) = textarea_ref.get_untracked() {
                    let _ = el.focus();
                }
            });
        }
    });

    // Watch trigger_editor: fire the editor when the counter increments.
    // The Prompt button (in Composer) increments this when editor_configured.
    let trigger = state.trigger_editor;
    Effect::new(move |prev: Option<u32>| {
        let count = trigger.get();
        if prev.is_some() && count > prev.unwrap_or(0) {
            do_editor();
        }
        count
    });

    // ---- event handlers ----------------------------------------------------

    let on_send_click = move |_| do_send();
    let on_collapse_click = move |_| state.open.set(false);

    let on_input = move |evt: ev::Event| {
        let Some(el) = evt
            .target()
            .and_then(|t| t.dyn_into::<HtmlTextAreaElement>().ok())
        else {
            return;
        };
        let text = el.value();
        let cursor = el
            .selection_start()
            .ok()
            .flatten()
            .map_or_else(|| text.len(), |c| c as usize);
        state.draft.set(text.clone());
        match at_token_at_cursor(&text, cursor) {
            Some(token) => query_completion(token.prefix),
            None => close_completion(),
        }
    };

    let on_keydown = move |evt: ev::KeyboardEvent| {
        let key = evt.key();
        let shift = evt.shift_key();
        let popup_open = completion_open.get_untracked();

        if popup_open {
            // Popup-scoped keys come first.
            if key == "Escape" {
                evt.prevent_default();
                close_completion();
                return;
            }
            if key == "Enter" {
                evt.prevent_default();
                if completion_highlight.get_untracked() >= 0 {
                    accept_highlighted();
                } else {
                    close_completion();
                }
                return;
            }
            if key == "ArrowDown" || (key == "Tab" && !shift) {
                evt.prevent_default();
                let len = completion_items.with_untracked(Vec::len);
                completion_highlight.update(|h| *h = next_highlight(*h, len, 1));
                return;
            }
            if key == "ArrowUp" || (key == "Tab" && shift) {
                evt.prevent_default();
                let len = completion_items.with_untracked(Vec::len);
                completion_highlight.update(|h| *h = next_highlight(*h, len, -1));
                return;
            }
            // Other keys fall through (typing narrows the prefix; on_input fires).
        }

        // ⏎ (no Shift): send.  ⇧⏎ falls through for a newline.
        if key == "Enter" && !shift {
            evt.prevent_default();
            do_send();
            return;
        }

        // Esc (no popup): collapse the panel.  Draft is preserved.
        if key == "Escape" {
            evt.prevent_default();
            state.open.set(false);
        }
    };

    // ---- view --------------------------------------------------------------

    view! {
        <Show when=move || state.open.get() fallback=|| ()>
            <div class="prompt-panel" data-testid="prompt-panel">
                <div class="prompt-panel-textarea-wrap">
                    <Show when=move || completion_open.get() fallback=|| ().into_any()>
                        <FileCompletionDropdown
                            items=completion_items
                            highlight=completion_highlight
                            on_pick=move |item: String| {
                                let Some((text, cursor)) = read_textarea() else { return };
                                let Some(out) = accept_completion(&text, cursor, &item) else {
                                    return;
                                };
                                set_textarea_state(out.new_text, out.new_cursor);
                                if out.drill_in {
                                    query_completion(item);
                                } else {
                                    close_completion();
                                }
                            }
                        />
                    </Show>
                    <Show
                        when=move || !state.editor_in_flight.get()
                        fallback=|| view! {
                            <div
                                class="prompt-panel-editor-notice"
                                data-testid="prompt-panel-editor-notice"
                            >
                                "Editor open — waiting for it to close…"
                            </div>
                        }.into_any()
                    >
                        <textarea
                            class="prompt-panel-input"
                            data-testid="leptos-prompt-panel-input"
                            node_ref=textarea_ref
                            on:input=on_input
                            on:keydown=on_keydown
                            placeholder="Message Omega… (@ for file path, Enter to send, Shift+Enter for newline, Esc to collapse)"
                        />
                    </Show>
                </div>
                <div class="prompt-panel-actions">
                    <button
                        class="prompt-panel-collapse"
                        data-testid="prompt-panel-collapse"
                        title="Collapse (draft text is kept)"
                        disabled=move || state.editor_in_flight.get()
                        on:click=on_collapse_click
                    >
                        "▼ Collapse"
                    </button>
                    <button
                        class="prompt-panel-send"
                        data-testid="leptos-prompt-panel-send"
                        data-action="send"
                        disabled=move || state.editor_in_flight.get()
                        on:click=on_send_click
                    >
                        "Send ⏎"
                    </button>
                </div>
            </div>
        </Show>
    }
}

// ---------------------------------------------------------------------------
// FileCompletionDropdown  (shared by PromptPanel)
// ---------------------------------------------------------------------------

#[component]
fn FileCompletionDropdown<F>(
    items: RwSignal<Vec<String>>,
    highlight: RwSignal<i32>,
    on_pick: F,
) -> impl IntoView
where
    F: Fn(String) + Copy + Send + Sync + 'static,
{
    let each = move || {
        let v: Vec<(usize, String)> = items.get().into_iter().enumerate().collect();
        v
    };
    let key = |(idx, item): &(usize, String)| (*idx, item.clone());
    let children = move |(idx, item): (usize, String)| {
        let item_for_click = item.clone();
        let item_for_class = item.clone();
        let item_for_attr = item.clone();
        view! {
            <div
                class=move || {
                    let mut s = String::from("leptos-composer-completion-item");
                    #[allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]
                    if highlight.get() == idx as i32 {
                        s.push_str(" leptos-composer-completion-hl");
                    }
                    if item_for_class.ends_with('/') {
                        s.push_str(" leptos-composer-completion-dir");
                    }
                    s
                }
                data-testid="leptos-composer-completion-item"
                data-completion=item_for_attr
                on:mousedown=move |evt: ev::MouseEvent| {
                    evt.prevent_default(); // keep focus in textarea
                    on_pick(item_for_click.clone());
                }
            >
                {item}
            </div>
        }
    };
    view! {
        <div
            class="leptos-composer-completion"
            data-testid="leptos-composer-completion"
        >
            <For each=each key=key children=children />
        </div>
    }
}
