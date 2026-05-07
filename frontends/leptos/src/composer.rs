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
//!   └─ <button> based on `composer_action(turn_state)`:
//!        Idle            → "Send"     ⇒ ClientFrame::UserMessage
//!        Running         → "Pause"    ⇒ ClientFrame::Pause
//!        PauseRequested  → "Abort"    ⇒ ClientFrame::Abort
//!        Paused          → "Continue" ⇒ ClientFrame::Continue { content }
//!      [+ a secondary "Abort" button only in Paused, so the operator
//!         can always escalate.]
//! ```
//!
//! ## Pure projection
//!
//! [`composer_action`] is the only place the four-state mapping lives.
//! It mirrors 3.3's `kind_for` / 3.2's `is_active` pattern: pure,
//! mutation-tested, no DOM reads. The component wraps it with the
//! `WsClient::send` calls and signal updates.
//!
//! ## Continue with interjection
//!
//! Per 3.4 spec: when the action is `Continue` (turn paused), the
//! textarea draft (if non-empty) is forwarded as
//! `ClientFrame::Continue { content: Some(draft) }`. Empty drafts
//! send `Continue { content: None }`. The wire shape supports both
//! verbatim. The "preCommitted / take it back" SolidJS UX is
//! intentionally dropped — one less RwSignal, one less race window;
//! the operator can still reach the same outcome by pausing then
//! continuing.
//!
//! ## Mutation-test carve-out
//!
//! `composer_action` is the carved-out pure projection.
//! `selected_label_for` (model + effort short labels) is also pure
//! and mutation-tested. Component glue (textarea events, dropdown
//! reactivity, completion popup positioning, focus management,
//! NodeRef DOM reads) is the JS-interop edge — same gap pattern as
//! 3.1's `ws.rs` / 3.2's `picker.rs` / 3.3's `feed.rs`.

use leptos::ev;
use leptos::html;
use leptos::prelude::*;
use leptos::reactive::owner::LocalStorage;
use leptos::task::spawn_local;
use wasm_bindgen::JsCast;
use web_sys::HtmlTextAreaElement;

use crate::completion::{accept_completion, at_token_at_cursor, next_highlight, selected_item};
use crate::http::get_files;
use crate::picker::PickerOpen;
use crate::protocol::{ClientFrame, TurnState};
use crate::store::SessionStore;
use crate::ws::WsClient;

// ---------------------------------------------------------------------------
// Hard-coded model / effort menus
// ---------------------------------------------------------------------------

/// Models the operator may choose between in the composer's model
/// dropdown. Hard-coded — see Phase 3.4 design notes for the
/// "discovery endpoint" rationale.
pub const MODELS: &[(&str, &str)] = &[
    ("claude-sonnet-4-6", "Sonnet"),
    ("claude-opus-4-6", "Opus 4.6"),
    ("claude-opus-4-7", "Opus 4.7"),
];

/// Effort levels offered. Server's `cap_effort_for_model` downcasts
/// `max` on Sonnet to `high`, so no client-side gating is required.
pub const EFFORTS: &[(&str, &str)] = &[
    ("low", "Low"),
    ("medium", "Medium"),
    ("high", "High"),
    ("max", "Max"),
];

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

/// Primary action the composer should offer for the given
/// server-reported turn state.
///
/// - `Idle`           → `Send`
/// - `Running`        → `Pause`
/// - `PauseRequested` → `Abort` (escalation while server hasn't
///   actually paused yet — `Continue` would be ambiguous because the
///   agent is mid-streaming)
/// - `Paused`         → `Continue` (with optional interjection content
///   from the textarea draft)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposerAction {
    Send,
    Pause,
    Abort,
    Continue,
}

/// Pure projection from server-reported turn state to the primary
/// composer action. Same pattern as 3.3's `kind_for`.
#[must_use]
pub fn composer_action(turn_state: TurnState) -> ComposerAction {
    match turn_state {
        TurnState::Idle => ComposerAction::Send,
        TurnState::Running => ComposerAction::Pause,
        TurnState::PauseRequested => ComposerAction::Abort,
        TurnState::Paused => ComposerAction::Continue,
    }
}

/// Stable visible label for each action.
#[must_use]
pub fn action_label(action: ComposerAction) -> &'static str {
    match action {
        ComposerAction::Send => "Send",
        ComposerAction::Pause => "Pause",
        ComposerAction::Abort => "Abort",
        ComposerAction::Continue => "Continue",
    }
}

/// Stable `data-action` attribute string for each action — Playwright
/// spec selector.
#[must_use]
pub fn action_tag(action: ComposerAction) -> &'static str {
    match action {
        ComposerAction::Send => "send",
        ComposerAction::Pause => "pause",
        ComposerAction::Abort => "abort",
        ComposerAction::Continue => "continue",
    }
}

/// Whether a secondary `Abort` button should render alongside the
/// primary one. We only show it in `Paused` — when the primary is
/// `Continue` and the operator might want to escalate. In `Running`
/// the primary is already `Pause` (Abort requires pausing first); in
/// `PauseRequested` the primary is already `Abort`; in `Idle` there
/// is no turn to abort.
#[must_use]
pub fn show_secondary_abort(turn_state: TurnState) -> bool {
    matches!(turn_state, TurnState::Paused)
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

/// Top-level composer surface. Reads from `SessionStore` (turn_state,
/// session_info) and `WsClient` (send) via context.
#[component]
pub fn Composer() -> impl IntoView {
    let store = use_context::<SessionStore>().expect("SessionStore must be provided");
    let ws = use_context::<WsClient>().expect("WsClient must be provided");

    let textarea_ref = NodeRef::<html::Textarea>::new();

    // Draft text. The textarea is the canonical source of truth for
    // visible text via `prop:value`; we mirror it into `draft` for
    // the send/continue handlers.
    let draft = RwSignal::new(String::new());

    // File-completion popup state.
    let completion_items = RwSignal::new(Vec::<String>::new());
    let completion_highlight = RwSignal::new(-1_i32); // cargo-mutants: skip — completion-popup init value; popup behaviour verified by e2e harness.
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

    // Primary action derived from turn state — mutation-testable
    // projection lives in `composer_action`.
    let action = Memo::new(move |_| composer_action(store.turn_state.get()));

    #[allow(unused_variables)]
    let close_completion = move || {
        completion_open.set(false);
        completion_items.set(Vec::new());
        completion_highlight.set(-1); // cargo-mutants: skip — completion-popup reset; verified by e2e harness.
    };

    // Fire a /api/files fetch for `prefix`. Stale fetches are
    // discarded by comparing the seq token at completion time.
    let query_completion = move |prefix: String| {
        let next = completion_seq.with_value(|v| v.wrapping_add(1));
        completion_seq.set_value(next);
        spawn_local(async move {
            match get_files(&prefix).await {
                Ok(items) => {
                    if completion_seq.with_value(|v| *v) != next { // cargo-mutants: skip — stale-fetch guard; async behaviour not exercisable without wasm fetch mock.
                        return; // stale
                    }
                    let any = !items.is_empty(); // cargo-mutants: skip — completion-popup guard; exercised by e2e file-completion test.
                    completion_items.set(items);
                    completion_highlight.set(-1); // cargo-mutants: skip — completion-popup reset; verified by e2e harness.
                    completion_open.set(any);
                }
                Err(_) => {
                    if completion_seq.with_value(|v| *v) != next { // cargo-mutants: skip — stale-fetch guard; async behaviour not exercisable without wasm fetch mock.
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

    let do_continue = move || {
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
    };

    let on_primary_click = move |_| match action.get() {
        ComposerAction::Send => do_send(),
        ComposerAction::Pause => do_pause(),
        ComposerAction::Abort => do_abort(),
        ComposerAction::Continue => do_continue(),
    };

    let on_secondary_abort = move |_| do_abort();

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
            if key == "Escape" { // cargo-mutants: skip — DOM key handler; exercised by e2e harness.
                evt.prevent_default();
                close_completion();
                return;
            }
            if key == "Enter" { // cargo-mutants: skip — DOM key handler; exercised by e2e harness.
                evt.prevent_default();
                if completion_highlight.get_untracked() >= 0 { // cargo-mutants: skip — DOM key handler; exercised by e2e harness.
                    accept_highlighted();
                } else {
                    close_completion();
                }
                return;
            }
            if key == "ArrowDown" || (key == "Tab" && !shift) { // cargo-mutants: skip — DOM key handler; exercised by e2e harness.
                evt.prevent_default();
                let len = completion_items.with_untracked(Vec::len);
                completion_highlight.update(|h| *h = next_highlight(*h, len, 1));
                return;
            }
            if key == "ArrowUp" || (key == "Tab" && shift) { // cargo-mutants: skip — DOM key handler; exercised by e2e harness.
                evt.prevent_default();
                let len = completion_items.with_untracked(Vec::len);
                completion_highlight.update(|h| *h = next_highlight(*h, len, -1)); // cargo-mutants: skip — DOM key handler; exercised by e2e harness.
                return;
            }
            // Other keys fall through to the textarea (typing narrows
            // the prefix; on_input fires next).
        }

        // Out of popup: Enter (no Shift) fires the primary action.
        if key == "Enter" && !shift { // cargo-mutants: skip — DOM key handler; exercised by e2e harness.
            evt.prevent_default();
            on_primary_click(ev::MouseEvent::new("click").unwrap_or_else(|_| {
                // Synthesising a MouseEvent is overkill here; just
                // dispatch the action directly.
                ev::MouseEvent::new("click").expect("MouseEvent fallback")
            }));
        }
    };

    // ---- view --------------------------------------------------------------

    // "Sessions" button opens the picker (Phase 3.9 TODO-1).
    let picker_open = use_context::<PickerOpen>().expect("PickerOpen must be provided");
    let on_sessions_click = move |_| picker_open.open();

    view! {
        <section
            class="leptos-composer"
            data-testid="leptos-composer"
            data-turn-state=move || turn_state_tag(store.turn_state.get())
        >
            <button
                class="leptos-composer-sessions"
                data-testid="leptos-composer-sessions"
                on:click=on_sessions_click
            >
                "Sessions"
            </button>
            <ModelSelect active=active_model on_change=on_model_change />
            <EffortSelect active=active_effort on_change=on_effort_change />
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
            <button
                class="leptos-composer-primary"
                data-testid="leptos-composer-primary"
                data-action=move || action_tag(action.get())
                on:click=on_primary_click
            >
                {move || action_label(action.get())}
            </button>
            <Show when=move || show_secondary_abort(store.turn_state.get()) fallback=|| ().into_any()>
                <button
                    class="leptos-composer-abort"
                    data-testid="leptos-composer-abort"
                    on:click=on_secondary_abort
                >
                    "Abort"
                </button>
            </Show>
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
fn EffortSelect<F>(active: Memo<String>, on_change: F) -> impl IntoView
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
            {EFFORTS.iter().map(|(value, label)| view! {
                <option value=*value>{*label}</option>
            }).collect_view()}
        </select>
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
        assert_eq!(composer_action(TurnState::Idle), ComposerAction::Send);
    }

    #[wasm_bindgen_test]
    fn composer_action_running_is_pause() {
        assert_eq!(composer_action(TurnState::Running), ComposerAction::Pause);
    }

    #[wasm_bindgen_test]
    fn composer_action_pause_requested_is_abort() {
        assert_eq!(
            composer_action(TurnState::PauseRequested),
            ComposerAction::Abort
        );
    }

    #[wasm_bindgen_test]
    fn composer_action_paused_is_continue() {
        assert_eq!(composer_action(TurnState::Paused), ComposerAction::Continue);
    }

    // ---- action_label / action_tag ----------------------------------------

    #[wasm_bindgen_test]
    fn action_label_per_action_is_distinct() {
        assert_eq!(action_label(ComposerAction::Send), "Send");
        assert_eq!(action_label(ComposerAction::Pause), "Pause");
        assert_eq!(action_label(ComposerAction::Abort), "Abort");
        assert_eq!(action_label(ComposerAction::Continue), "Continue");
    }

    #[wasm_bindgen_test]
    fn action_label_values_are_pairwise_unique() {
        // Locks down the "every arm returns the same string" mutation.
        let all = [
            ComposerAction::Send,
            ComposerAction::Pause,
            ComposerAction::Abort,
            ComposerAction::Continue,
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
        assert_eq!(action_tag(ComposerAction::Abort), "abort");
        assert_eq!(action_tag(ComposerAction::Continue), "continue");
    }

    #[wasm_bindgen_test]
    fn action_tag_values_are_pairwise_unique() {
        let all = [
            ComposerAction::Send,
            ComposerAction::Pause,
            ComposerAction::Abort,
            ComposerAction::Continue,
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(action_tag(*a), action_tag(*b));
            }
        }
    }

    // ---- show_secondary_abort ----------------------------------------------

    #[wasm_bindgen_test]
    fn secondary_abort_only_visible_when_paused() {
        assert!(!show_secondary_abort(TurnState::Idle));
        assert!(!show_secondary_abort(TurnState::Running));
        assert!(!show_secondary_abort(TurnState::PauseRequested));
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
        // E.g. server-set effort `xhigh` not in the dropdown.
        assert_eq!(selected_label_for(EFFORTS, "xhigh"), "xhigh");
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

    // ---- MODELS / EFFORTS hard-coded contents -----------------------------

    #[wasm_bindgen_test]
    fn models_list_contains_three_supported_models() {
        let values: Vec<&str> = MODELS.iter().map(|(v, _)| *v).collect();
        assert_eq!(values.len(), 3);
        assert!(values.contains(&"claude-sonnet-4-6"));
        assert!(values.contains(&"claude-opus-4-6"));
        assert!(values.contains(&"claude-opus-4-7"));
    }

    #[wasm_bindgen_test]
    fn efforts_list_contains_four_supported_levels() {
        let values: Vec<&str> = EFFORTS.iter().map(|(v, _)| *v).collect();
        assert_eq!(values.len(), 4);
        assert_eq!(values, vec!["low", "medium", "high", "max"]);
    }
}
