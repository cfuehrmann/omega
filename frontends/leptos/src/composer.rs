//! Composer component (Phase 3.4).
//!
//! ```text
//!  <Composer>
//!   ├─ ModelSelect    (hard-coded 3 models — sends `set_model`)
//!   ├─ EffortSelect   (hard-coded 4 efforts — sends `set_effort`)
//!   ├─ <div .textarea-wrap>
//!   │   ├─ <Show completion_open>
//!   │   │   └─ <FileCompletionDropdown items=… highlight=… />
//!   │   └─ <textarea .composer-input on:input on:keydown />
//!   └─ Button matrix keyed on `turn_state` × `pre_committed`:
//!
//!        Idle              → "Send ⏎"        ⇒ ClientFrame::UserMessage
//!        Running           → "Pause ⎋"       ⇒ ClientFrame::Pause
//!        PauseRequested    → "Abort ⎋"  ⇒ ClientFrame::Abort
//!                               + "Continue ⏎"    ⇒ pre_committed = true
//!        PauseRequested    → "Abort ⎋"  ⇒ ClientFrame::Abort
//!        (pre_committed)        + "Take it back"  ⇒ pre_committed = false
//!        Paused            → "Abort ⎋"  ⇒ ClientFrame::Abort
//!                               + "Continue ⏎"    ⇒ ClientFrame::Continue { content }
//!
//! ## Pre-commit / auto-drain flow
//!
//! Clicking "Continue ⏎" during `PauseRequested` does **not** send a WS
//! frame immediately — the agent is still mid-stream. Instead it sets
//! `store.pre_committed = true`, switching the status chip to
//! "Pausing, will continue". An `Effect` watches the
//! `PauseRequested → Paused` transition: when it fires while
//! `pre_committed` is set, it auto-fires `ClientFrame::Continue` (with
//! any interjection typed meanwhile) and clears the flag.
//!
//! "Take it back" is available during `PauseRequested` + pre_committed
//! and reverts the promise: `pre_committed = false`. The pause will
//! still land, but the UI will not auto-fire a continue.
//!
//! ## Keyboard shortcuts
//!
//! | Key   | State                          | Action                        |
//! |-------|--------------------------------|-------------------------------|
//! | `⎋`   | Running                        | Pause                         |
//! | `⎋`   | PauseRequested \| Paused       | Abort                         |
//! | `⏎`   | Idle                           | Send                          |
//! | `⏎`   | PauseRequested (not committed) | Continue (pre-commit)         |
//! | `⏎`   | Paused                         | Continue (send WS frame)      |
//! | `⏎`   | Running \| PauseRequested+pc  | newline (compose interjection) |
//!
//! ## Pure projection
//!
//! [`composer_action`] is the only place the button-matrix mapping lives.
//! [`status_str`] / [`status_label`] are the only places the status chip
//! label mapping lives. Both are pure, mutation-tested, no DOM reads.
//!
//! ## Mutation-test carve-out
//!
//! Component glue (textarea events, dropdown reactivity, completion popup
//! positioning, focus management, NodeRef DOM reads, reactive Effects) is
//! the JS-interop edge — same gap pattern as 3.1's `ws.rs` / 3.2's
//! `picker.rs` / 3.3's `feed.rs`.

use leptos::ev;
use leptos::html;
use leptos::prelude::*;
use leptos::reactive::owner::LocalStorage;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;
use web_sys::HtmlTextAreaElement;

use crate::completion::{
    accept_completion, at_token_at_cursor, insert_item_text, next_highlight, selected_item,
};
use crate::event_view::current_status_label;
use crate::http::get_files;
use crate::picker::PickerOpen;
use crate::protocol::{ClientFrame, TurnState};
use crate::store::SessionStore;
use crate::usage_panel::UsagePanelOpen;
use crate::ws::WsClient;

// ---------------------------------------------------------------------------
// Hard-coded model / effort menus
// ---------------------------------------------------------------------------

/// Models the operator may choose between in the composer's model
/// dropdown. Hard-coded — see Phase 3.4 design notes for the
/// "discovery endpoint" rationale.
pub const MODELS: &[(&str, &str)] = &[
    ("claude-sonnet-4-6", "Sonnet"),
    ("claude-opus-4-7", "Opus 4.7"),
];

/// Effort levels for Sonnet 4.6 (and Opus 4.6 if ever re-added).
/// These models support `low` / `medium` / `high` / `max`.
/// `xhigh` is not available on Sonnet.
pub const EFFORTS: &[(&str, &str)] = &[
    ("low", "Low"),
    ("medium", "Medium"),
    ("high", "High"),
    ("max", "Max"),
];

/// Effort levels for Claude Opus 4.7, which additionally exposes
/// `xhigh` — a tier between `high` and `max` recommended as the
/// starting point for long-horizon coding and agentic tasks.
pub const EFFORTS_OPUS47: &[(&str, &str)] = &[
    ("low", "Low"),
    ("medium", "Medium"),
    ("high", "High"),
    ("xhigh", "XHigh"),
    ("max", "Max"),
];

/// Return the appropriate effort slice for `model`.
/// `claude-opus-4-7` gets the extended list (including `xhigh`);
/// all other models fall back to the standard four-level list.
#[must_use]
pub fn efforts_for_model(model: &str) -> &'static [(&'static str, &'static str)] {
    if model == "claude-opus-4-7" {
        EFFORTS_OPUS47
    } else {
        EFFORTS
    }
}

/// Project the (value, label) pairs to the label whose value matches
/// `current`. Falls back to `current` itself (verbatim) when no value
/// matches — this is what the SolidJS UI also does, and it lets
/// out-of-band server states (e.g. `xhigh` from a CLI) still render
/// readably even if the dropdown doesn't include them.
///
/// Currently unused in production: the composer renders native
/// `<select>` elements, so the browser displays the selected option's
/// label automatically. Kept (and mutation-tested) because Phase 3.6
/// may switch to a custom-trigger dropdown.
#[allow(dead_code)]
#[must_use]
pub fn selected_label_for<'a>(options: &'a [(&'a str, &'a str)], current: &'a str) -> &'a str {
    for (value, label) in options {
        if *value == current {
            return label;
        }
    }
    current
}

// ---------------------------------------------------------------------------
// Pure state-machine projection
// ---------------------------------------------------------------------------

/// Primary action the composer should offer for the given server-reported
/// turn state and client-local `pre_committed` flag.
///
/// - `Idle`                              → `Send`
/// - `Running`                           → `Pause`
/// - `PauseRequested` (not committed)    → `Continue` (pre-commit: sets flag, no WS)
/// - `PauseRequested` (committed)        → `TakeItBack` (clears flag)
/// - `Paused`                            → `Continue` (sends WS `continue` frame)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposerAction {
    Send,
    Pause,
    Continue,
    TakeItBack,
}

/// Pure projection from server turn state + pre_committed flag to the
/// primary composer action.
#[must_use]
pub fn composer_action(turn_state: TurnState, pre_committed: bool) -> ComposerAction {
    match turn_state {
        TurnState::Idle => ComposerAction::Send,
        TurnState::Running => ComposerAction::Pause,
        TurnState::PauseRequested => {
            if pre_committed {
                ComposerAction::TakeItBack
            } else {
                ComposerAction::Continue
            }
        }
        TurnState::Paused => ComposerAction::Continue,
    }
}

/// Stable visible label for each action (key icon shown to the right
/// of the text so the operator can see which key drives the button).
#[must_use]
pub fn action_label(action: ComposerAction) -> &'static str {
    match action {
        ComposerAction::Send => "Send ⏎",
        ComposerAction::Pause => "Pause ⎋",
        ComposerAction::Continue => "Continue ⏎",
        ComposerAction::TakeItBack => "Take it back",
    }
}

/// Stable `data-action` attribute string for each action — Playwright
/// spec selector.
#[must_use]
pub fn action_tag(action: ComposerAction) -> &'static str {
    match action {
        ComposerAction::Send => "send",
        ComposerAction::Pause => "pause",
        ComposerAction::Continue => "continue",
        ComposerAction::TakeItBack => "takeitback",
    }
}

/// Whether a secondary `Abort ⎋` button should render alongside the
/// primary one. Shown whenever the turn is paused or pause-requested:
/// both states allow the operator to escalate to an immediate abort.
/// (`Running` requires pausing first; `Idle` has nothing to abort.)
#[must_use]
pub fn show_secondary_abort(turn_state: TurnState) -> bool {
    matches!(turn_state, TurnState::Paused | TurnState::PauseRequested)
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

/// Shared signal for injecting text into the composer textarea from
/// outside (e.g. the session picker's "@ path" button).
///
/// When set to `Some(item)` the Composer's Effect inserts `@item` at
/// the current cursor position — using `accept_completion` if the
/// cursor is already inside an `@`-token, otherwise inserting at the
/// cursor with a leading space if needed. The signal is immediately
/// reset to `None` after consumption.
///
/// Provided by the `Composer` component at mount; consumed by any
/// component that holds a reference to the textarea (currently the
/// session picker's `SessionRow`).
#[derive(Debug, Clone, Copy)]
pub struct ComposerInsert(pub RwSignal<Option<String>>);

impl ComposerInsert {
    /// Construct with no pending insert. Must run inside a leptos
    /// reactive `Owner` scope.
    #[must_use]
    pub fn new() -> Self {
        Self(RwSignal::new(None))
    }

    /// Queue `item` for insertion as `@item` at the composer cursor.
    #[mutants::skip]
    pub fn insert(self, item: String) {
        self.0.set(Some(item));
    }
}

impl Default for ComposerInsert {
    fn default() -> Self {
        Self::new()
    }
}

/// Top-level composer surface. Reads from `SessionStore` (turn_state,
/// session_info) and `WsClient` (send) via context.
/// Skipped from mutation testing: all mutations are in reactive signal
/// callbacks and DOM event handlers; exercised exclusively by the e2e harness.
#[mutants::skip]
#[component]
pub fn Composer() -> impl IntoView {
    let store = use_context::<SessionStore>().expect("SessionStore must be provided");
    let ws = use_context::<WsClient>().expect("WsClient must be provided");

    let textarea_ref = NodeRef::<html::Textarea>::new();

    // The inject-text context is now provided by `App` so that
    // `SessionRow` (rendered inside `SessionPicker`, a sibling of
    // `Composer`) can always access it — even when `Composer` itself
    // is not rendered (no active session).
    let composer_insert =
        use_context::<ComposerInsert>().expect("ComposerInsert must be provided by App");

    // Draft text. The textarea is the canonical source of truth for
    // visible text via `prop:value`; we mirror it into `draft` for
    // the send/continue handlers.
    let draft = RwSignal::new(String::new());

    // File-completion popup state.
    let completion_items = RwSignal::new(Vec::<String>::new());
    let completion_highlight = RwSignal::new(-1_i32);
    let completion_open = RwSignal::new(false);
    // Stable counter to drop stale fetch results — same pattern as
    // SessionListStore::fetch_generation in 3.2.
    let completion_seq: StoredValue<u64, LocalStorage> = StoredValue::new_local(0);

    // Active model + effort, derived from session_info.
    let active_model = Memo::new(move |_| {
        store
            .session_info
            .with(|si| si.as_ref().map_or_else(String::new, |s| s.model.clone()))
    });
    let active_effort = Memo::new(move |_| {
        store
            .session_info
            .with(|si| si.as_ref().map_or_else(String::new, |s| s.effort.clone()))
    });

    // Primary action derived from turn state + pre_committed — mutation-testable
    // projections live in `composer_action`.
    let action =
        Memo::new(move |_| composer_action(store.turn_state.get(), store.pre_committed.get()));

    #[allow(unused_variables)]
    let close_completion = move || {
        completion_open.set(false);
        completion_items.set(Vec::new());
        completion_highlight.set(-1);
    };

    // Fire a /api/files fetch for `prefix`. Stale fetches are
    // discarded by comparing the seq token at completion time.
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

    // Read cursor + value from the live textarea. JS-interop edge.
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

    // Apply a textarea state update. Sets the value + cursor + draft
    // signal in one shot. JS-interop edge.
    let set_textarea_state = move |new_text: String, new_cursor: usize| {
        if let Some(el) = textarea_ref.get() {
            el.set_value(&new_text);
            #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
            let cursor_u32 = new_cursor.min(u32::MAX as usize) as u32;
            let _ = el.set_selection_start(Some(cursor_u32));
            let _ = el.set_selection_end(Some(cursor_u32));
        }
        draft.set(new_text);
    };

    // Accept the highlighted completion (or do nothing if none).
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
            // Drill into the directory: fetch its children and keep
            // popup open.
            query_completion(item);
        } else {
            close_completion();
        }
    };

    // ---- primary-action click ----------------------------------------------

    let do_send = move || {
        let content = draft.get();
        if content.trim().is_empty() {
            return;
        }
        if let Err(err) = ws.send(&ClientFrame::UserMessage { content }) {
            leptos::logging::warn!("composer send failed: {err:?}");
            return;
        }
        set_textarea_state(String::new(), 0);
    };

    let do_pause = move || {
        if let Err(err) = ws.send(&ClientFrame::Pause) {
            leptos::logging::warn!("composer pause failed: {err:?}");
        }
    };

    let do_abort = move || {
        if let Err(err) = ws.send(&ClientFrame::Abort) {
            leptos::logging::warn!("composer abort failed: {err:?}");
        }
    };

    // Continue from Paused: send the WS frame (with optional interjection draft).
    let do_continue_from_paused = move || {
        let typed = draft.get();
        let content = if typed.trim().is_empty() {
            None
        } else {
            Some(typed)
        };
        let had_content = content.is_some();
        if let Err(err) = ws.send(&ClientFrame::Continue { content }) {
            leptos::logging::warn!("composer continue failed: {err:?}");
            return;
        }
        if had_content {
            set_textarea_state(String::new(), 0);
        }
        store.pre_committed.set(false);
    };

    // Continue from PauseRequested: the pause seam hasn’t landed yet.
    // Don’t send anything over the wire — just set the pre_committed flag.
    // When turn_state transitions PauseRequested → Paused, the auto-drain
    // Effect below will fire `do_continue_from_paused`.
    let do_continue_from_pause_requested = move || {
        store.pre_committed.set(true);
    };

    // Take it back: revert a pre-committed Continue while still in PauseRequested.
    let do_take_it_back = move || {
        store.pre_committed.set(false);
    };

    let on_primary_click = move |_| match action.get() {
        ComposerAction::Send => do_send(),
        ComposerAction::Pause => do_pause(),
        ComposerAction::Continue => {
            // `Continue` covers two states:
            //   PauseRequested → pre-commit (no WS yet)
            //   Paused         → send WS continue frame
            if store.turn_state.get_untracked() == TurnState::PauseRequested {
                do_continue_from_pause_requested();
            } else {
                do_continue_from_paused();
            }
        }
        ComposerAction::TakeItBack => do_take_it_back(),
    };

    let on_secondary_abort = move |_| do_abort();

    // Auto-drain effect: client-only (Effects need a WASM executor; SSR has none).
    // When turn_state transitions PauseRequested → Paused while pre_committed is
    // set, auto-fire the continue WS frame.  Belt-and-suspenders: also clears
    // pre_committed if we leave PauseRequested/Paused without a normal drain
    // (e.g. disconnect → reconnect into Idle, or server reset).  The disconnect
    // path is also covered by ws.rs’s on_close handler.
    #[cfg(not(feature = "ssr"))]
    {
        let auto_drain_prev: StoredValue<TurnState, LocalStorage> =
            StoredValue::new_local(TurnState::Idle);
        Effect::new(move |_| {
            let current = store.turn_state.get();
            let prev = auto_drain_prev.get_value();
            let pc = store.pre_committed.get_untracked();
            if prev == TurnState::PauseRequested && current == TurnState::Paused && pc {
                let typed = draft.get_untracked();
                let content = if typed.trim().is_empty() {
                    None
                } else {
                    Some(typed)
                };
                let had_content = content.is_some();
                if ws.send(&ClientFrame::Continue { content }).is_ok() && had_content {
                    draft.set(String::new());
                    if let Some(el) = textarea_ref.get_untracked() {
                        el.set_value("");
                        let _ = el.set_selection_start(Some(0));
                        let _ = el.set_selection_end(Some(0));
                    }
                }
                store.pre_committed.set(false);
            }
            if !matches!(current, TurnState::PauseRequested | TurnState::Paused) && pc {
                store.pre_committed.set(false);
            }
            auto_drain_prev.set_value(current);
        });
    }

    // ---- model + effort dropdowns ------------------------------------------

    let on_model_change = move |value: String| {
        if let Err(err) = ws.send(&ClientFrame::SetModel { model: value }) {
            leptos::logging::warn!("composer set_model failed: {err:?}");
        }
    };
    let on_effort_change = move |value: String| {
        if let Err(err) = ws.send(&ClientFrame::SetEffort { effort: value }) {
            leptos::logging::warn!("composer set_effort failed: {err:?}");
        }
    };

    // ---- textarea event handlers -------------------------------------------

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
        draft.set(text.clone());
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
            // Other keys fall through to the textarea (typing narrows
            // the prefix; on_input fires next).
        }

        // ⎋: pause from Running; abort from PauseRequested / Paused.
        // (Popup Escape is already handled above and returns early.)
        if key == "Escape" {
            match store.turn_state.get_untracked() {
                TurnState::Running => {
                    evt.prevent_default();
                    do_pause();
                }
                TurnState::PauseRequested | TurnState::Paused => {
                    evt.prevent_default();
                    do_abort();
                }
                TurnState::Idle => {} // browser default
            }
            return;
        }

        // ⏎ (no Shift): fire the primary action for the current state.
        // In Running, or in PauseRequested with a pre-committed Continue,
        // there is no Enter action — fall through so the textarea can
        // receive a newline for composing a multi-line interjection.
        if key == "Enter" && !shift {
            let ts = store.turn_state.get_untracked();
            let pc = store.pre_committed.get_untracked();
            match ts {
                TurnState::Idle => {
                    evt.prevent_default();
                    do_send();
                }
                TurnState::PauseRequested if !pc => {
                    evt.prevent_default();
                    do_continue_from_pause_requested();
                }
                TurnState::Paused => {
                    evt.prevent_default();
                    do_continue_from_paused();
                }
                // Running or PauseRequested+preCommitted: fall through to newline.
                _ => {}
            }
        }
    };

    // ---- composer-insert Effect (from session picker "@ path" button) ------
    // When `composer_insert` is set to `Some(item)`, insert `@item` at
    // the current textarea cursor. If the cursor is inside an existing
    // `@`-token, `accept_completion` replaces the token; otherwise the
    // text is inserted raw at the cursor (with a leading space when
    // the preceding character is not whitespace). The signal is cleared
    // immediately so the Effect doesn't re-trigger.
    Effect::new(move |_| {
        let Some(item) = composer_insert.0.get() else {
            return;
        };
        // Consume immediately to avoid re-triggering.
        composer_insert.0.set(None);

        let Some((text, _)) = read_textarea() else {
            return;
        };

        // `insert_item_text` always appends at the end of the existing
        // text, avoiding the stale / zero cursor that browsers report
        // after the textarea loses focus when the picker button is clicked.
        let (new_text, new_cursor) = insert_item_text(&text, &item);

        set_textarea_state(new_text, new_cursor);

        // Refocus the textarea so the user can continue typing.
        spawn_local(async move {
            if let Some(el) = textarea_ref.get_untracked() {
                let _ = el.focus();
            }
        });
    });

    // ---- view --------------------------------------------------------------

    // "Sessions" button toggles the picker (Phase 3.9 TODO-1).
    let picker_open = use_context::<PickerOpen>().expect("PickerOpen must be provided");
    let on_sessions_click = move |_| {
        if picker_open.open.get_untracked() {
            picker_open.close();
        } else {
            picker_open.open();
        }
    };

    // "Usage" button toggles the usage panel.
    let usage_panel_open =
        use_context::<UsagePanelOpen>().expect("UsagePanelOpen must be provided");

    view! {
        <section
            class="leptos-composer"
            data-testid="leptos-composer"
            data-turn-state=move || turn_state_tag(store.turn_state.get())
        >
            <button
                class="leptos-composer-sessions"
                data-testid="leptos-composer-sessions"
                data-panel-open=move || picker_open.open.get().to_string()
                on:click=on_sessions_click
            >
                "Sessions"
            </button>
            <button
                class="leptos-composer-usage"
                data-testid="leptos-composer-usage"
                data-panel-open=move || usage_panel_open.is_open().to_string()
                on:click=move |_| usage_panel_open.toggle()
            >
                {move || if usage_panel_open.is_open() { "▲ Usage" } else { "▼ Usage" }}
            </button>
            <ModelSelect active=active_model on_change=on_model_change />
            <EffortSelect active=active_effort active_model=active_model on_change=on_effort_change />
            <div class="leptos-composer-textarea-wrap">
                <Show
                    when=move || completion_open.get()
                    fallback=|| ().into_any()
                >
                    <FileCompletionDropdown
                        items=completion_items
                        highlight=completion_highlight
                        on_pick=move |item: String| {
                            let Some((text, cursor)) = read_textarea() else { return };
                            let Some(out) = accept_completion(&text, cursor, &item) else { return };
                            set_textarea_state(out.new_text, out.new_cursor);
                            if out.drill_in {
                                query_completion(item);
                            } else {
                                close_completion();
                            }
                        }
                    />
                </Show>
                <textarea
                    class="leptos-composer-input"
                    data-testid="leptos-composer-input"
                    node_ref=textarea_ref
                    on:input=on_input
                    on:keydown=on_keydown
                    placeholder="Message Omega… (@ for file, Enter to send, Shift+Enter for newline)"
                />
            </div>
            <Show when=move || show_secondary_abort(store.turn_state.get()) fallback=|| ().into_any()>
                <button
                    class="leptos-composer-abort"
                    data-testid="leptos-composer-abort"
                    on:click=on_secondary_abort
                >
                    "Abort ⎋"
                </button>
            </Show>
            <button
                class="leptos-composer-primary"
                data-testid="leptos-composer-primary"
                data-action=move || action_tag(action.get())
                on:click=on_primary_click
            >
                {move || action_label(action.get())}
            </button>
            <StatusChip />
        </section>
    }
}

/// Stable wire string for `data-turn-state` (Playwright selector).
fn turn_state_tag(turn_state: TurnState) -> &'static str {
    match turn_state {
        TurnState::Idle => "idle",
        TurnState::Running => "running",
        TurnState::PauseRequested => "pause_requested",
        TurnState::Paused => "paused",
    }
}

// ---------------------------------------------------------------------------
// Sub-components
// ---------------------------------------------------------------------------

#[component]
fn ModelSelect<F>(active: Memo<String>, on_change: F) -> impl IntoView
where
    F: Fn(String) + Copy + 'static,
{
    view! {
        <select
            class="leptos-composer-model"
            data-testid="leptos-composer-model"
            // Bind the *property* (not the attribute) to the active
            // model. Setting `select.value` imperatively makes the
            // matching `<option>` selected, and — critically — won't
            // fight a user's `change` event the way per-option
            // `selected=` would (the browser flushes prop writes
            // before re-rendering after a user interaction).
            prop:value=move || active.get()
            on:change=move |evt| {
                if let Some(el) = evt
                    .target()
                    .and_then(|t| t.dyn_into::<web_sys::HtmlSelectElement>().ok())
                {
                    on_change(el.value());
                }
            }
        >
            {MODELS.iter().map(|(value, label)| view! {
                <option value=*value>{*label}</option>
            }).collect_view()}
        </select>
    }
}

#[component]
fn EffortSelect<F>(active: Memo<String>, active_model: Memo<String>, on_change: F) -> impl IntoView
where
    F: Fn(String) + Copy + 'static,
{
    view! {
        <select
            class="leptos-composer-effort"
            data-testid="leptos-composer-effort"
            prop:value=move || active.get()
            on:change=move |evt| {
                if let Some(el) = evt
                    .target()
                    .and_then(|t| t.dyn_into::<web_sys::HtmlSelectElement>().ok())
                {
                    on_change(el.value());
                }
            }
        >
            {move || {
                efforts_for_model(&active_model.get())
                    .iter()
                    .map(|(value, label)| view! {
                        <option value=*value>{*label}</option>
                    })
                    .collect_view()
            }}
        </select>
    }
}

// ---------------------------------------------------------------------------
// Status chip (inline in the composer row)
// ---------------------------------------------------------------------------

/// Maps the connected/turn-state/pre_committed triple to a CSS
/// `data-status` string.
///
/// | `data-status`          | when                                   |
/// |------------------------|----------------------------------------|
/// | `offline`              | not connected                          |
/// | `streaming`            | `Running`                              |
/// | `pausing`              | `PauseRequested`, not pre-committed    |
/// | `pausing-will-continue`| `PauseRequested`, pre-committed        |
/// | `paused`               | `Paused`                               |
/// | `ready`                | `Idle`                                 |
pub(crate) fn status_str(
    connected: bool,
    turn_state: TurnState,
    pre_committed: bool,
) -> &'static str {
    if !connected {
        "offline"
    } else {
        match turn_state {
            TurnState::Running => "streaming",
            TurnState::PauseRequested => {
                if pre_committed {
                    "pausing-will-continue"
                } else {
                    "pausing"
                }
            }
            TurnState::Paused => "paused",
            TurnState::Idle => "ready",
        }
    }
}

/// Maps a `data-status` string to its human-readable chip label.
///
/// This is the **fallback** label for the streaming case — used only
/// when `current_status_label` returns `None` (no events yet, no live
/// streaming buffer).  In practice the chip prefers the running
/// event's own label so the operator can see exactly which phase the
/// turn is in (LLM call / thinking / tool call / …) rather than a
/// generic "Streaming…".
pub(crate) fn status_label(status: &str) -> &'static str {
    match status {
        "offline" => "Offline",
        "streaming" => "Streaming…",
        "pausing" => "Pausing…",
        "pausing-will-continue" => "Pausing, will continue",
        "paused" => "Paused",
        _ => "Ready",
    }
}

/// Inline status badge rendered at the right end of the composer row.
///
/// Six base states driven by `store.connected`, `store.turn_state`, and
/// `store.pre_committed`:
///
/// | `data-status`           | colour | text                    |
/// |-------------------------|--------|-------------------------|
/// | `ready`                 | teal   | `Ready`                 |
/// | `streaming`             | llm†  | event label (dynamic)    |
/// | `pausing`               | yellow | `Pausing…`              |
/// | `pausing-will-continue` | green  | `Pausing, will continue`|
/// | `paused`                | yellow | `Paused`                |
/// | `offline`               | red    | `Offline`               |
///
/// † During `streaming`, the chip additionally carries a
/// `data-event-type` attribute echoing the current in-flight event's
/// wire tag (`llm_call`, `tool_call`, `thinking_block`, …). CSS rules
/// in `style.css` colour the chip to match the corresponding big-block
/// border colour: `--yellow` for `tool_call` / `tool_result`,
/// `--peach` for `llm_retry`, `--red` for error variants, `--llm`
/// (sapphire) for the LLM-side variants (the default). One source of
/// truth shared with the big-block renderer in `feed.rs`.
///
/// `pointer-events: none` in CSS — never intercepts clicks on the composer.
#[component]
fn StatusChip() -> impl IntoView {
    let store = use_context::<SessionStore>().expect("SessionStore must be provided");

    let status = move || {
        status_str(
            store.connected.get(),
            store.turn_state.get(),
            store.pre_committed.get(),
        )
    };

    // `(label, event_type_tag)` reactive memo. `None` outside the
    // streaming state (where the static `status_label` text wins) and
    // when nothing is in flight yet.
    let current = move || -> Option<(String, &'static str)> {
        if status() != "streaming" {
            return None;
        }
        let last_tool_name = store
            .streaming_tool_use
            .with(|m| m.iter().next_back().map(|(_, slot)| slot.name.clone()));
        let text_active = store.streaming_text.with(|m| !m.is_empty());
        let thinking_active = store.streaming_thinking.with(|m| !m.is_empty());
        store.events.with(|evs| {
            current_status_label(evs, text_active, thinking_active, last_tool_name.as_deref())
        })
    };

    let text = move || {
        current()
            .map(|(label, _)| label)
            .unwrap_or_else(|| status_label(status()).to_owned())
    };
    let event_type = move || current().map(|(_, tag)| tag);

    view! {
        <div
            class="status-chip"
            data-testid="leptos-status-chip"
            data-status=status
            data-event-type=event_type
        >
            {text}
        </div>
    }
}

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

// ---------------------------------------------------------------------------
// Tests (pure projections only; component is exercised by Playwright)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]

    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test;

    // ---- composer_action ---------------------------------------------------

    #[wasm_bindgen_test]
    fn composer_action_idle_is_send() {
        assert_eq!(
            composer_action(TurnState::Idle, false),
            ComposerAction::Send
        );
    }

    #[wasm_bindgen_test]
    fn composer_action_running_is_pause() {
        assert_eq!(
            composer_action(TurnState::Running, false),
            ComposerAction::Pause
        );
    }

    #[wasm_bindgen_test]
    fn composer_action_pause_requested_not_precommitted_is_continue() {
        assert_eq!(
            composer_action(TurnState::PauseRequested, false),
            ComposerAction::Continue
        );
    }

    #[wasm_bindgen_test]
    fn composer_action_pause_requested_precommitted_is_takeitback() {
        assert_eq!(
            composer_action(TurnState::PauseRequested, true),
            ComposerAction::TakeItBack
        );
    }

    #[wasm_bindgen_test]
    fn composer_action_paused_is_continue() {
        assert_eq!(
            composer_action(TurnState::Paused, false),
            ComposerAction::Continue
        );
    }

    // ---- action_label / action_tag ----------------------------------------

    #[wasm_bindgen_test]
    fn action_label_per_action_is_distinct() {
        assert_eq!(action_label(ComposerAction::Send), "Send ⏎");
        assert_eq!(action_label(ComposerAction::Pause), "Pause ⎋");
        assert_eq!(action_label(ComposerAction::Continue), "Continue ⏎");
        assert_eq!(action_label(ComposerAction::TakeItBack), "Take it back");
    }

    #[wasm_bindgen_test]
    fn action_label_values_are_pairwise_unique() {
        // Locks down the "every arm returns the same string" mutation.
        let all = [
            ComposerAction::Send,
            ComposerAction::Pause,
            ComposerAction::Continue,
            ComposerAction::TakeItBack,
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(action_label(*a), action_label(*b));
            }
        }
    }

    #[wasm_bindgen_test]
    fn action_tag_per_action_is_snake_case_lowercase() {
        assert_eq!(action_tag(ComposerAction::Send), "send");
        assert_eq!(action_tag(ComposerAction::Pause), "pause");
        assert_eq!(action_tag(ComposerAction::Continue), "continue");
        assert_eq!(action_tag(ComposerAction::TakeItBack), "takeitback");
    }

    #[wasm_bindgen_test]
    fn action_tag_values_are_pairwise_unique() {
        let all = [
            ComposerAction::Send,
            ComposerAction::Pause,
            ComposerAction::Continue,
            ComposerAction::TakeItBack,
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(action_tag(*a), action_tag(*b));
            }
        }
    }

    // ---- show_secondary_abort ----------------------------------------------

    #[wasm_bindgen_test]
    fn secondary_abort_visible_when_paused_or_pause_requested() {
        // Abort is now available whenever the turn is in a pause state.
        assert!(!show_secondary_abort(TurnState::Idle));
        assert!(!show_secondary_abort(TurnState::Running));
        assert!(show_secondary_abort(TurnState::PauseRequested));
        assert!(show_secondary_abort(TurnState::Paused));
    }

    // ---- selected_label_for ------------------------------------------------

    #[wasm_bindgen_test]
    fn selected_label_returns_label_for_known_value() {
        assert_eq!(selected_label_for(MODELS, "claude-sonnet-4-6"), "Sonnet");
        assert_eq!(selected_label_for(MODELS, "claude-opus-4-7"), "Opus 4.7");
        assert_eq!(selected_label_for(EFFORTS, "low"), "Low");
        assert_eq!(selected_label_for(EFFORTS, "max"), "Max");
    }

    #[wasm_bindgen_test]
    fn selected_label_falls_back_to_value_when_unknown() {
        // `xhigh` is not in the Sonnet EFFORTS list — falls back to value.
        assert_eq!(selected_label_for(EFFORTS, "xhigh"), "xhigh");
        // But `xhigh` IS in the Opus 4.7 list — returns its label.
        assert_eq!(selected_label_for(EFFORTS_OPUS47, "xhigh"), "XHigh");
        assert_eq!(selected_label_for(MODELS, "unknown-model"), "unknown-model");
    }

    #[wasm_bindgen_test]
    fn selected_label_handles_empty_options() {
        assert_eq!(selected_label_for(&[], "anything"), "anything");
    }

    #[wasm_bindgen_test]
    fn selected_label_returns_first_match_when_value_appears_twice() {
        // Defensive — duplicate values shouldn't happen but the
        // function must still terminate deterministically. Locks
        // down a mutation that scans the full slice instead of
        // early-returning.
        let dup: &[(&str, &str)] = &[("a", "first"), ("a", "second")];
        assert_eq!(selected_label_for(dup, "a"), "first");
    }

    // ---- turn_state_tag ----------------------------------------------------

    #[wasm_bindgen_test]
    fn turn_state_tag_per_state_is_distinct() {
        assert_eq!(turn_state_tag(TurnState::Idle), "idle");
        assert_eq!(turn_state_tag(TurnState::Running), "running");
        assert_eq!(turn_state_tag(TurnState::PauseRequested), "pause_requested");
        assert_eq!(turn_state_tag(TurnState::Paused), "paused");
    }

    #[wasm_bindgen_test]
    fn turn_state_tag_values_are_pairwise_unique() {
        let all = [
            TurnState::Idle,
            TurnState::Running,
            TurnState::PauseRequested,
            TurnState::Paused,
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(turn_state_tag(*a), turn_state_tag(*b));
            }
        }
    }

    // ---- status_str / status_label ----------------------------------------

    #[wasm_bindgen_test]
    fn status_str_offline_when_disconnected() {
        assert_eq!(status_str(false, TurnState::Idle, false), "offline");
        assert_eq!(status_str(false, TurnState::Running, false), "offline");
    }

    #[wasm_bindgen_test]
    fn status_str_streaming_when_running() {
        assert_eq!(status_str(true, TurnState::Running, false), "streaming");
    }

    #[wasm_bindgen_test]
    fn status_str_pausing_for_pause_requested_without_precommit() {
        assert_eq!(
            status_str(true, TurnState::PauseRequested, false),
            "pausing"
        );
    }

    #[wasm_bindgen_test]
    fn status_str_pausing_will_continue_for_pause_requested_with_precommit() {
        assert_eq!(
            status_str(true, TurnState::PauseRequested, true),
            "pausing-will-continue"
        );
    }

    #[wasm_bindgen_test]
    fn status_str_paused_when_paused() {
        assert_eq!(status_str(true, TurnState::Paused, false), "paused");
        // pre_committed has no effect in Paused (pause already landed).
        assert_eq!(status_str(true, TurnState::Paused, true), "paused");
    }

    #[wasm_bindgen_test]
    fn status_str_ready_when_idle_and_connected() {
        assert_eq!(status_str(true, TurnState::Idle, false), "ready");
    }

    #[wasm_bindgen_test]
    fn status_label_offline() {
        assert_eq!(status_label("offline"), "Offline");
    }

    #[wasm_bindgen_test]
    fn status_label_streaming() {
        assert_eq!(status_label("streaming"), "Streaming…");
    }

    #[wasm_bindgen_test]
    fn status_label_pausing() {
        assert_eq!(status_label("pausing"), "Pausing…");
    }

    #[wasm_bindgen_test]
    fn status_label_pausing_will_continue() {
        assert_eq!(
            status_label("pausing-will-continue"),
            "Pausing, will continue"
        );
    }

    #[wasm_bindgen_test]
    fn status_label_paused() {
        assert_eq!(status_label("paused"), "Paused");
    }

    #[wasm_bindgen_test]
    fn status_label_default_is_ready() {
        assert_eq!(status_label("ready"), "Ready");
        assert_eq!(status_label("other"), "Ready");
    }

    // ---- MODELS / EFFORTS hard-coded contents -----------------------------

    #[wasm_bindgen_test]
    fn models_list_contains_two_supported_models() {
        let values: Vec<&str> = MODELS.iter().map(|(v, _)| *v).collect();
        assert_eq!(values.len(), 2);
        assert!(values.contains(&"claude-sonnet-4-6"));
        assert!(values.contains(&"claude-opus-4-7"));
    }

    #[wasm_bindgen_test]
    fn efforts_list_contains_four_supported_levels() {
        let values: Vec<&str> = EFFORTS.iter().map(|(v, _)| *v).collect();
        assert_eq!(values.len(), 4);
        assert_eq!(values, vec!["low", "medium", "high", "max"]);
    }

    #[wasm_bindgen_test]
    fn efforts_opus47_list_contains_five_levels_including_xhigh() {
        let values: Vec<&str> = EFFORTS_OPUS47.iter().map(|(v, _)| *v).collect();
        assert_eq!(values.len(), 5);
        assert_eq!(values, vec!["low", "medium", "high", "xhigh", "max"]);
    }

    #[wasm_bindgen_test]
    fn efforts_for_model_returns_opus47_list_for_opus47() {
        assert_eq!(efforts_for_model("claude-opus-4-7"), EFFORTS_OPUS47);
    }

    #[wasm_bindgen_test]
    fn efforts_for_model_returns_standard_list_for_sonnet() {
        assert_eq!(efforts_for_model("claude-sonnet-4-6"), EFFORTS);
    }

    #[wasm_bindgen_test]
    fn efforts_for_model_returns_standard_list_for_unknown_model() {
        assert_eq!(efforts_for_model("unknown-model"), EFFORTS);
    }
}
