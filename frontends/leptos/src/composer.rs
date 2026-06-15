//! Composer component — the always-visible bottom bar.
//!
//! ```text
//!  <Composer>
//!   ├─ Sessions button  (opens session picker)
//!   ├─ PanelsMenuButton (toggles bottom-panel checkboxes)
//!   ├─ ModelSelect      (sends `set_model`)
//!   ├─ EffortSelect     (sends `set_effort`)
//!   ├─ Prompt button    (opens/triggers the PromptPanel)
//!   ├─ Turn-control group (Halt / Resume / Abort — conditional on turn state)
//!   └─ StatusChip       (connected + turn-state label)
//! ```
//!
//! The textarea, Send button, and file-completion popup have moved into
//! [`crate::prompt_panel::PromptPanel`], which is a collapsible panel
//! that slides up from above this bar without obscuring the event feed.
//!
//! Clicking the **Prompt** button:
//! - When an external editor is configured (server reports
//!   `editorConfigured: true`): opens the [`PromptPanel`] and immediately
//!   triggers the editor via an increment on `PromptPanelState::trigger_editor`.
//! - Otherwise: just expands the panel so the operator can type.
//!
//! ## Pure projection
//!
//! [`show_halt`] / [`show_resume`] / [`show_abort`] are the only places
//! the button-visibility matrix lives. [`status_str`] / [`status_label`]
//! are the only places the status-chip label mapping lives. All are pure,
//! mutation-tested, no DOM reads.

use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::event_view::current_status_label;
use crate::monitors_panel::{MonitorsPanelOpen, running_count, total_fired};
use crate::picker::PickerOpen;
use crate::prompt_panel::PromptPanelState;
use crate::protocol::{ClientFrame, TurnState};
use crate::queue_panel::{QueuePanelOpen, pending_count};
use crate::store::SessionStore;
use crate::usage_panel::UsagePanelOpen;
use crate::ws::WsClient;

// ---------------------------------------------------------------------------
// Hard-coded model / effort menus
// ---------------------------------------------------------------------------

/// Models the operator may choose between in the composer's model
/// dropdown. Hard-coded — see Phase 3.4 design notes for the
/// "discovery endpoint" rationale.
pub const MODELS: &[(&str, &str)] = &[("claude-sonnet-4-6", "Sonnet"), ("claude-opus-4-8", "Opus")];

/// Effort levels for Sonnet 4.6 (and Opus 4.6 if ever re-added).
/// These models support `low` / `medium` / `high` / `max`.
/// `xhigh` is not available on Sonnet.
pub const EFFORTS: &[(&str, &str)] = &[
    ("low", "Low"),
    ("medium", "Medium"),
    ("high", "High"),
    ("max", "Max"),
];

/// Effort levels for Claude Opus 4.7 and later, which additionally expose
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
/// `claude-opus-4-7` and `claude-opus-4-8` get the extended list
/// (including `xhigh`); all other models fall back to the standard
/// four-level list.
#[must_use]
pub fn efforts_for_model(model: &str) -> &'static [(&'static str, &'static str)] {
    if matches!(model, "claude-opus-4-7" | "claude-opus-4-8") {
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
// Pure control-visibility projections (§15 three-controls model)
// ---------------------------------------------------------------------------

/// Whether the **Halt** control should render. Halt = "stop advancing at
/// the next seam" and is only meaningful while the agent is actively
/// processing a block (`Running`).
#[must_use]
pub fn show_halt(turn_state: TurnState) -> bool {
    matches!(turn_state, TurnState::Running)
}

/// Whether the **Resume** control should render.
///
/// Resume = "carry on with no new input" and is meaningful in two states:
/// - `HaltRequested`: a halt has been requested but the loop hasn't parked
///   yet; clicking Resume cancels the pending halt so the agent continues.
/// - `Halted`: the loop is fully parked; clicking Resume wakes it with no
///   new input and the agent carries on from where it stopped.
#[must_use]
pub fn show_resume(turn_state: TurnState) -> bool {
    matches!(turn_state, TurnState::HaltRequested | TurnState::Halted)
}

/// Whether the **Abort** control should render.
///
/// Abort forcefully cancels the in-flight or parked block and is shown
/// alongside Resume whenever the loop is either mid-halt-flight
/// (`HaltRequested`) or fully parked (`Halted`).  While `Running` (no halt
/// pending) the sole secondary control is Halt; while `Idle` there is
/// nothing to abort.
#[must_use]
pub fn show_abort(turn_state: TurnState) -> bool {
    matches!(turn_state, TurnState::HaltRequested | TurnState::Halted)
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

/// Top-level always-visible bottom bar. Contains session/panel/model/effort
/// controls, the Prompt trigger button, turn controls, and the status chip.
///
/// The textarea, Send button, file completion, and editor invocation all
/// live in [`crate::prompt_panel::PromptPanel`], which is rendered above
/// this bar and can be collapsed without losing the draft.
///
/// Skipped from mutation testing: all mutations are in reactive signal
/// callbacks and DOM event handlers; exercised exclusively by the e2e harness.
#[mutants::skip]
#[component]
pub fn Composer() -> impl IntoView {
    let store = use_context::<SessionStore>().expect("SessionStore must be provided");
    let ws = use_context::<WsClient>().expect("WsClient must be provided");
    let prompt_panel =
        use_context::<PromptPanelState>().expect("PromptPanelState must be provided");

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

    // ---- turn controls -----------------------------------------------------

    let do_halt = move || {
        if let Err(err) = ws.send(&ClientFrame::Halt) {
            leptos::logging::warn!("composer halt failed: {err:?}");
        }
    };
    let do_resume = move || {
        if let Err(err) = ws.send(&ClientFrame::Resume) {
            leptos::logging::warn!("composer resume failed: {err:?}");
        }
    };
    let do_abort = move || {
        if let Err(err) = ws.send(&ClientFrame::Abort) {
            leptos::logging::warn!("composer abort failed: {err:?}");
        }
    };

    let on_halt_click = move |_| do_halt();
    let on_resume_click = move |_| do_resume();
    let on_abort_click = move |_| do_abort();

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

    // ---- Sessions picker ---------------------------------------------------

    let picker_open = use_context::<PickerOpen>().expect("PickerOpen must be provided");
    let on_sessions_click = move |_| {
        if picker_open.open.get_untracked() {
            picker_open.close();
        } else {
            picker_open.open();
        }
    };

    // ---- Prompt button -----------------------------------------------------
    //
    // Always opens the PromptPanel AND fires the editor immediately — same
    // effect as opening the panel then clicking its "Editor" button. The
    // button is disabled while the panel is already open to prevent a second
    // editor from launching.

    let on_prompt_click = move |_| {
        prompt_panel.open.set(true);
        prompt_panel.trigger_editor.update(|v| *v += 1);
    };

    // ---- view --------------------------------------------------------------

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
            <PanelsMenuButton />
            <ModelSelect active=active_model on_change=on_model_change />
            <EffortSelect active=active_effort active_model=active_model on_change=on_effort_change />
            <button
                class="leptos-composer-prompt"
                data-testid="leptos-composer-prompt"
                title="Open prompt panel — also opens the editor (Prompt)"
                disabled=move || prompt_panel.open.get() || prompt_panel.editor_in_flight.get()
                on:click=on_prompt_click
            >
                <Show when=move || !prompt_panel.draft.with(|d| d.trim().is_empty()) fallback=|| ()>
                    <span class="prompt-draft-dot" aria-label="unsent draft" />
                </Show>
                "Prompt"
            </button>
            <Show when=move || show_halt(store.turn_state.get()) fallback=|| ().into_any()>
                <button
                    class="leptos-composer-halt"
                    data-testid="leptos-composer-halt"
                    on:click=on_halt_click
                >
                    "▮▮"
                </button>
            </Show>
            <Show when=move || show_abort(store.turn_state.get()) fallback=|| ().into_any()>
                <button
                    class="leptos-composer-abort"
                    data-testid="leptos-composer-abort"
                    on:click=on_abort_click
                >
                    "■"
                </button>
            </Show>
            <Show when=move || show_resume(store.turn_state.get()) fallback=|| ().into_any()>
                <button
                    class="leptos-composer-resume"
                    data-testid="leptos-composer-resume"
                    on:click=on_resume_click
                >
                    "▶"
                </button>
            </Show>
            <StatusChip />
        </section>
    }
}

/// Stable wire string for `data-turn-state` (Playwright selector).
fn turn_state_tag(turn_state: TurnState) -> &'static str {
    match turn_state {
        TurnState::Idle => "idle",
        TurnState::Running => "running",
        TurnState::HaltRequested => "halt_requested",
        TurnState::Halted => "halted",
    }
}

// ---------------------------------------------------------------------------
// Pure control-visibility projections — Panels menu
// ---------------------------------------------------------------------------

/// Whether the "Panels" button should show an activity indicator.
///
/// Returns `true` when there is something noteworthy in the queue or the
/// monitor roster, so the operator knows to open those panels even when
/// the floating badges are gone.  The usage panel is excluded — it has no
/// "pending" state; its content is always historical token counts.
///
/// Conditions:
/// - `queue_count > 0`     → items are pending delivery at the next seam.
/// - `running_monitors > 0` → at least one monitor is actively running.
/// - `total_fired > 0`     → at least one monitor event has been fired.
#[must_use]
pub fn any_panel_activity(queue_count: usize, running_monitors: usize, total_fired: u64) -> bool {
    queue_count > 0 || running_monitors > 0 || total_fired > 0
}

// ---------------------------------------------------------------------------
// Sub-components
// ---------------------------------------------------------------------------

/// "Panels" menu button that toggles a dropdown with checkboxes for each
/// bottom panel (Usage / Queue / Monitors).  Each checkbox is bound to the
/// corresponding `*PanelOpen` context signal.  The dropdown closes when the
/// user clicks outside it (via a full-screen backdrop layer).
///
/// Marked `#[mutants::skip]` — component body is reactive/DOM glue;
/// `any_panel_activity` carries the mutation-test budget.
#[mutants::skip]
#[component]
fn PanelsMenuButton() -> impl IntoView {
    let store = use_context::<SessionStore>().expect("SessionStore must be provided");
    let usage_panel_open =
        use_context::<UsagePanelOpen>().expect("UsagePanelOpen must be provided");
    let queue_panel_open =
        use_context::<QueuePanelOpen>().expect("QueuePanelOpen must be provided");
    let monitors_panel_open =
        use_context::<MonitorsPanelOpen>().expect("MonitorsPanelOpen must be provided");

    let menu_open = RwSignal::new(false);

    // Derived counts for the menu-item labels.
    let queue_count = Memo::new(move |_| store.input_queue.with(|q| pending_count(q)));
    let running = Memo::new(move |_| store.roster.with(|r| running_count(r)));
    let fired = Memo::new(move |_| store.roster.with(|r| total_fired(r)));
    let has_activity =
        Memo::new(move |_| any_panel_activity(queue_count.get(), running.get(), fired.get()));

    let queue_label = move || {
        let c = queue_count.get();
        if c == 0 {
            "Queue".to_owned()
        } else {
            format!("Queue ({c} pending)")
        }
    };
    let monitors_label = move || {
        let r = running.get();
        let f = fired.get();
        if r == 0 && f == 0 {
            "Monitors".to_owned()
        } else if f == 0 {
            format!("Monitors ({r} running)")
        } else {
            format!("Monitors ({r} running, {f} fired)")
        }
    };

    view! {
        <div class="panels-menu-wrap" style="position:relative;display:inline-flex;">
            // Main "Panels" button.
            <button
                class="leptos-composer-panels"
                data-testid="panels-menu-btn"
                data-activity=move || has_activity.get().to_string()
                on:click=move |_| menu_open.update(|v| *v = !*v)
            >
                <Show when=move || has_activity.get() fallback=|| ()>
                    <span class="panels-activity-dot" aria-label="activity" />
                </Show>
                "Panels"
            </button>

            // Dropdown (shown when menu_open is true).
            <Show when=move || menu_open.get() fallback=|| ()>
                // Full-screen transparent backdrop — clicking outside the
                // menu closes it without triggering the button toggle.
                <div
                    style="position:fixed;inset:0;z-index:99;"
                    on:click=move |_| menu_open.set(false)
                />
                <div
                    class="panels-menu-dropdown"
                    data-testid="panels-menu-dropdown"
                >
                    <label class="panels-menu-item">
                        <input
                            type="checkbox"
                            data-testid="panels-usage-checkbox"
                            prop:checked=move || usage_panel_open.is_open()
                            on:change=move |_| usage_panel_open.toggle()
                        />
                        "Usage"
                    </label>
                    <label class="panels-menu-item">
                        <input
                            type="checkbox"
                            data-testid="panels-queue-checkbox"
                            prop:checked=move || queue_panel_open.is_open()
                            on:change=move |_| queue_panel_open.toggle()
                        />
                        {queue_label}
                    </label>
                    <label class="panels-menu-item">
                        <input
                            type="checkbox"
                            data-testid="panels-monitors-checkbox"
                            prop:checked=move || monitors_panel_open.is_open()
                            on:change=move |_| monitors_panel_open.toggle()
                        />
                        {monitors_label}
                    </label>
                </div>
            </Show>
        </div>
    }
}

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

/// Maps the connected/turn-state pair to a CSS `data-status` string.
///
/// | `data-status` | when               |
/// |---------------|--------------------|
/// | `offline`     | not connected      |
/// | `streaming`   | `Running`          |
/// | `halting`     | `HaltRequested`    |
/// | `halted`      | `Halted`           |
/// | `ready`       | `Idle`             |
pub(crate) fn status_str(connected: bool, turn_state: TurnState) -> &'static str {
    if !connected {
        "offline"
    } else {
        match turn_state {
            TurnState::Running => "streaming",
            TurnState::HaltRequested => "halting",
            TurnState::Halted => "halted",
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
        "halting" => "Halting…",
        "halted" => "Halted",
        _ => "Ready",
    }
}

/// Inline status badge rendered at the right end of the composer row.
///
/// Five base states driven by `store.connected` and `store.turn_state`:
///
/// | `data-status` | colour | text                 |
/// |---------------|--------|----------------------|
/// | `ready`       | teal   | `Ready`              |
/// | `streaming`   | llm†  | event label (dynamic) |
/// | `halting`     | yellow | `Halting…`           |
/// | `halted`      | yellow | `Halted`             |
/// | `offline`     | red    | `Offline`            |
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
#[mutants::skip]
#[component]
fn StatusChip() -> impl IntoView {
    let store = use_context::<SessionStore>().expect("SessionStore must be provided");

    let status = move || status_str(store.connected.get(), store.turn_state.get());

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

// ---------------------------------------------------------------------------
// Tests (pure projections only; component is exercised by Playwright)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]

    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test;

    // ---- show_halt / show_resume / show_abort ------------------------------

    #[wasm_bindgen_test]
    #[test]
    fn show_halt_only_while_running() {
        assert!(show_halt(TurnState::Running));
        assert!(!show_halt(TurnState::Idle));
        assert!(!show_halt(TurnState::HaltRequested));
        assert!(!show_halt(TurnState::Halted));
    }

    #[wasm_bindgen_test]
    #[test]
    fn show_resume_while_halt_requested_or_halted() {
        assert!(show_resume(TurnState::HaltRequested));
        assert!(show_resume(TurnState::Halted));
        assert!(!show_resume(TurnState::Idle));
        assert!(!show_resume(TurnState::Running));
    }

    #[wasm_bindgen_test]
    #[test]
    fn show_abort_while_halt_requested_or_halted() {
        assert!(show_abort(TurnState::HaltRequested));
        assert!(show_abort(TurnState::Halted));
        assert!(!show_abort(TurnState::Running));
        assert!(!show_abort(TurnState::Idle));
    }

    // ---- selected_label_for ------------------------------------------------

    #[wasm_bindgen_test]
    #[test]
    fn selected_label_returns_label_for_known_value() {
        assert_eq!(selected_label_for(MODELS, "claude-sonnet-4-6"), "Sonnet");
        assert_eq!(selected_label_for(MODELS, "claude-opus-4-8"), "Opus");
        assert_eq!(selected_label_for(EFFORTS, "low"), "Low");
        assert_eq!(selected_label_for(EFFORTS, "max"), "Max");
    }

    #[wasm_bindgen_test]
    #[test]
    fn selected_label_falls_back_to_value_when_unknown() {
        // `xhigh` is not in the Sonnet EFFORTS list — falls back to value.
        assert_eq!(selected_label_for(EFFORTS, "xhigh"), "xhigh");
        // But `xhigh` IS in the Opus 4.7/4.8 list — returns its label.
        assert_eq!(selected_label_for(EFFORTS_OPUS47, "xhigh"), "XHigh");
        assert_eq!(selected_label_for(MODELS, "unknown-model"), "unknown-model");
    }

    #[wasm_bindgen_test]
    #[test]
    fn selected_label_handles_empty_options() {
        assert_eq!(selected_label_for(&[], "anything"), "anything");
    }

    #[wasm_bindgen_test]
    #[test]
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
    #[test]
    fn turn_state_tag_per_state_is_distinct() {
        assert_eq!(turn_state_tag(TurnState::Idle), "idle");
        assert_eq!(turn_state_tag(TurnState::Running), "running");
        assert_eq!(turn_state_tag(TurnState::HaltRequested), "halt_requested");
        assert_eq!(turn_state_tag(TurnState::Halted), "halted");
    }

    #[wasm_bindgen_test]
    #[test]
    fn turn_state_tag_values_are_pairwise_unique() {
        let all = [
            TurnState::Idle,
            TurnState::Running,
            TurnState::HaltRequested,
            TurnState::Halted,
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(turn_state_tag(*a), turn_state_tag(*b));
            }
        }
    }

    // ---- status_str / status_label ----------------------------------------

    #[wasm_bindgen_test]
    #[test]
    fn status_str_offline_when_disconnected() {
        assert_eq!(status_str(false, TurnState::Idle), "offline");
        assert_eq!(status_str(false, TurnState::Running), "offline");
    }

    #[wasm_bindgen_test]
    #[test]
    fn status_str_streaming_when_running() {
        assert_eq!(status_str(true, TurnState::Running), "streaming");
    }

    #[wasm_bindgen_test]
    #[test]
    fn status_str_halting_for_halt_requested() {
        assert_eq!(status_str(true, TurnState::HaltRequested), "halting");
    }

    #[wasm_bindgen_test]
    #[test]
    fn status_str_halted_when_halted() {
        assert_eq!(status_str(true, TurnState::Halted), "halted");
    }

    #[wasm_bindgen_test]
    #[test]
    fn status_str_ready_when_idle_and_connected() {
        assert_eq!(status_str(true, TurnState::Idle), "ready");
    }

    #[wasm_bindgen_test]
    #[test]
    fn status_label_offline() {
        assert_eq!(status_label("offline"), "Offline");
    }

    #[wasm_bindgen_test]
    #[test]
    fn status_label_streaming() {
        assert_eq!(status_label("streaming"), "Streaming…");
    }

    #[wasm_bindgen_test]
    #[test]
    fn status_label_halting() {
        assert_eq!(status_label("halting"), "Halting…");
    }

    #[wasm_bindgen_test]
    #[test]
    fn status_label_halted() {
        assert_eq!(status_label("halted"), "Halted");
    }

    #[wasm_bindgen_test]
    #[test]
    fn status_label_default_is_ready() {
        assert_eq!(status_label("ready"), "Ready");
        assert_eq!(status_label("other"), "Ready");
    }

    // ---- MODELS / EFFORTS hard-coded contents -----------------------------

    #[wasm_bindgen_test]
    #[test]
    fn models_list_contains_two_supported_models() {
        let values: Vec<&str> = MODELS.iter().map(|(v, _)| *v).collect();
        assert_eq!(values.len(), 2);
        assert!(values.contains(&"claude-sonnet-4-6"));
        assert!(values.contains(&"claude-opus-4-8"));
    }

    #[wasm_bindgen_test]
    #[test]
    fn efforts_list_contains_four_supported_levels() {
        let values: Vec<&str> = EFFORTS.iter().map(|(v, _)| *v).collect();
        assert_eq!(values.len(), 4);
        assert_eq!(values, vec!["low", "medium", "high", "max"]);
    }

    #[wasm_bindgen_test]
    #[test]
    fn efforts_opus47_list_contains_five_levels_including_xhigh() {
        let values: Vec<&str> = EFFORTS_OPUS47.iter().map(|(v, _)| *v).collect();
        assert_eq!(values.len(), 5);
        assert_eq!(values, vec!["low", "medium", "high", "xhigh", "max"]);
    }

    #[wasm_bindgen_test]
    #[test]
    fn efforts_for_model_returns_opus47_list_for_opus47() {
        assert_eq!(efforts_for_model("claude-opus-4-7"), EFFORTS_OPUS47);
    }

    #[wasm_bindgen_test]
    #[test]
    fn efforts_for_model_returns_opus47_list_for_opus48() {
        assert_eq!(efforts_for_model("claude-opus-4-8"), EFFORTS_OPUS47);
    }

    #[wasm_bindgen_test]
    #[test]
    fn efforts_for_model_returns_standard_list_for_sonnet() {
        assert_eq!(efforts_for_model("claude-sonnet-4-6"), EFFORTS);
    }

    #[wasm_bindgen_test]
    #[test]
    fn efforts_for_model_returns_standard_list_for_unknown_model() {
        assert_eq!(efforts_for_model("unknown-model"), EFFORTS);
    }

    // ---- any_panel_activity -----------------------------------------------

    #[wasm_bindgen_test]
    #[test]
    fn any_panel_activity_false_when_all_zero() {
        assert!(!any_panel_activity(0, 0, 0));
    }

    #[wasm_bindgen_test]
    #[test]
    fn any_panel_activity_true_when_queue_non_empty() {
        assert!(any_panel_activity(1, 0, 0));
        assert!(any_panel_activity(5, 0, 0));
    }

    #[wasm_bindgen_test]
    #[test]
    fn any_panel_activity_true_when_running_monitor() {
        assert!(any_panel_activity(0, 1, 0));
        assert!(any_panel_activity(0, 3, 0));
    }

    #[wasm_bindgen_test]
    #[test]
    fn any_panel_activity_true_when_fired_events() {
        assert!(any_panel_activity(0, 0, 1));
        assert!(any_panel_activity(0, 0, 99));
    }

    #[wasm_bindgen_test]
    #[test]
    fn any_panel_activity_true_when_all_non_zero() {
        assert!(any_panel_activity(2, 1, 5));
    }

    #[wasm_bindgen_test]
    #[test]
    fn any_panel_activity_queue_only_drives_dot() {
        // One pending message, no monitors at all: still shows activity.
        assert!(any_panel_activity(1, 0, 0));
    }

    #[wasm_bindgen_test]
    #[test]
    fn any_panel_activity_fired_only_no_running_monitors() {
        // All monitors stopped but some fired events recorded: still shows activity.
        assert!(any_panel_activity(0, 0, 7));
    }
}
