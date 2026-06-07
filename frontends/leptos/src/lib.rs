//! Phase 3.10 — Leptos UI library crate.
//!
//! ## Architecture
//!
//! ```text
//!   App
//!    ├── provide_context::<SessionStore>      (conversation state)
//!    ├── provide_context::<SessionListStore>  (picker state)
//!    ├── provide_context::<WsClient>          (write-path handle)
//!    ├── provide_context::<ContextModalState> (modal open/close)
//!    ├── provide_context::<TextModalState>    (generic text overlay — 3.10)
//!    ├── provide_context::<PickerOpen>        (picker open/close — 3.9)
//!    ├── Effect: WsClient::new(url, conv, list).connect()
//!    ├── SessionPicker     (3.2 + 3.5 resume button per row + 3.9 modal)
//!    ├── ConversationFeed  (3.3 + 3.5 LlmCallBlock + 3.6 MarkdownBody)
//!    ├── Composer          (3.4 + 3.9 Sessions button)
//!    ├── ContextModal      (3.5 — full-viewport overlay)
//!    ├── TextModal         (3.10 — generic text overlay)
//!    └── [cfg(debug_assertions)] <details data-testid="leptos-debug-panel">
//!         └── DebugView    (3.1 JSON dump — dropped from release builds 3.9)
//! ```
//!
//! ## Module lifetimes
//!
//! Pure target-agnostic modules (`event_view`, `markdown`,
//! `diff_render`, `completion`, `protocol`, `store`, `sessions`,
//! `context_modal`'s pure helpers) compile on both `wasm32` and the
//! host target. The remaining modules (`feed`, `picker`, `composer`,
//! `ws`, `http`) compile on host but their reactive runtime / JS
//! interop only execute under `wasm32` + `csr`.

pub mod completion;
pub mod composer;
pub mod context_modal;
pub mod diff_render;
pub mod dirty_modal;
pub mod event_view;
pub mod feed;
pub mod http;
pub mod markdown;
pub mod monitors_panel;
pub mod picker;
pub mod protocol;
pub mod queue_panel;
pub mod sessions;
pub mod store;
pub mod text_modal;
pub mod usage_panel;
pub mod ws;

use leptos::prelude::*;

use crate::composer::Composer;
use crate::context_modal::{ContextModal, ContextModalState};
use crate::dirty_modal::DirtyModal;
use crate::feed::ConversationFeed;
use crate::monitors_panel::{MonitorsBadge, MonitorsPanelOpen};
use crate::picker::{PickerOpen, SessionPicker};
use crate::protocol::TurnState;
pub use crate::queue_panel::QueueBadge;
use crate::queue_panel::QueuePanelOpen;
use crate::sessions::SessionListStore;
use crate::store::SessionStore;
use crate::text_modal::{TextModal, TextModalState};
use crate::usage_panel::{UsagePanel, UsagePanelOpen};
use crate::ws::{WsClient, ws_url_from_window};

/// Mount the [`App`] component into the document body. Called from
/// the binary's `main()`. Separated so that integration tests can
/// build the lib without invoking the mount path.
#[mutants::skip] // WASM entry-point wrapper; mounting verified by e2e smoke test.
pub fn run() {
    console_error_panic_hook::set_once();
    mount_to_body(App);
}

#[component]
pub fn App() -> impl IntoView {
    let store = SessionStore::new();
    let list_store = SessionListStore::new();
    let modal_state = ContextModalState::new();
    let text_modal_state = TextModalState::new();
    let picker_open = PickerOpen::new();
    let usage_panel_open = UsagePanelOpen::new();
    let monitors_panel_open = MonitorsPanelOpen::new();
    let queue_panel_open = QueuePanelOpen::new();
    provide_context(store);
    provide_context(list_store);
    provide_context(modal_state);
    provide_context(text_modal_state);
    provide_context(picker_open);
    provide_context(usage_panel_open);
    provide_context(monitors_panel_open);
    provide_context(queue_panel_open);

    let ws = WsClient::new(
        ws_url_from_window().unwrap_or_else(|err| {
            leptos::logging::error!("ws url derivation failed: {err:?}");
            String::new()
        }),
        store,
        list_store,
    );
    provide_context(ws);

    Effect::new(move |_| {
        ws.connect();
    });

    // Phase 3.10 TODO-E-1: auto-close the picker as soon as a turn
    // starts (or is requested to halt). Otherwise the modal overlay
    // (z-index 900) hides the composer's `Resume` button while the
    // turn is halted — the operator gets stuck. The picker only
    // re-opens via the `Sessions` button, never during a live turn.
    Effect::new(move |_| {
        if turn_is_active(store.turn_state.get()) {
            picker_open.close();
        }
    });

    // Phase 3.10 TODO-F: open the picker on a *fresh* server
    // connection (connected + no active session). Browser refresh
    // with an active session lands directly in the conversation
    // feed; only the first-time / post-server-restart case opens
    // the picker automatically.
    Effect::new(move |_| {
        if should_auto_open_picker(
            store.connected.get(),
            store.session_info.with(Option::is_none),
        ) {
            picker_open.open();
        }
    });

    view! {
        // `data-connected` — WS connected flag (Playwright: wait for WS ready).
        // `data-active-session-dir` — current session dir (Playwright: ground-truth
        //   active-dir read, replaces debug-store JSON parse). Both attributes
        // are always in the DOM; updated reactively from `SessionStore`.
        // Phase 3.9 TODO-4: debug panel cfg-gated; these attributes are the
        // replacement ground-truth surface for specs.
        <main
            data-connected=move || store.connected.get().to_string()
            data-active-session-dir=move || store.session_info.with(
                |si| si.as_ref().map(|s| s.dir.clone()).unwrap_or_default()
            )
        >
            <SessionPicker />
            <ConversationFeed />
            <UsagePanel />
            <MonitorsBadge />
            <QueueBadge />
            <Show when=move || session_has_loaded(store.session_info.with(Option::is_some)) fallback=|| ()>
                <Composer />
            </Show>
            <ContextModal />
            <TextModal />
            <DirtyModal />
            // Debug panel — compiled only in debug builds (cargo test,
            // `trunk serve` dev mode). `trunk build --release` strips
            // this block entirely so it never ships to production users.
            // Phase 3.9 TODO-4: specs migrated from debug-store reads to
            // `data-connected` + `data-session-dir` DOM attributes.
            {
                #[cfg(debug_assertions)]
                {
                    view! {
                        <details data-testid="leptos-debug-panel">
                            <summary>"debug: store snapshot"</summary>
                            <DebugView />
                        </details>
                    }.into_any()
                }
                #[cfg(not(debug_assertions))]
                { ().into_any() }
            }
        </main>
    }
}

/// Returns true when a turn is in progress (i.e. the session is not idle).
/// Extracted from the reactive Effect so the `!= Idle` condition is mutation-tested.
fn turn_is_active(state: TurnState) -> bool {
    state != TurnState::Idle
}

/// Returns true when the picker should open automatically:
/// server is connected and no session is active yet.
fn should_auto_open_picker(connected: bool, has_no_session: bool) -> bool {
    connected && has_no_session
}

/// Returns true when a session is active and the composer should be rendered.
/// Extracted so the condition is mutation-tested independently of the reactive Effect.
fn session_has_loaded(has_session: bool) -> bool {
    has_session
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test;

    #[wasm_bindgen_test]
    #[test]
    fn turn_is_active_true_for_running_halted_halt_requested() {
        assert!(turn_is_active(TurnState::Running));
        assert!(turn_is_active(TurnState::Halted));
        assert!(turn_is_active(TurnState::HaltRequested));
    }

    #[wasm_bindgen_test]
    #[test]
    fn turn_is_active_false_for_idle() {
        assert!(!turn_is_active(TurnState::Idle));
    }

    #[wasm_bindgen_test]
    #[test]
    fn session_has_loaded_true_when_session_info_present() {
        assert!(session_has_loaded(true));
        assert!(!session_has_loaded(false));
    }

    #[wasm_bindgen_test]
    #[test]
    fn should_auto_open_picker_true_only_when_connected_and_no_session() {
        assert!(should_auto_open_picker(true, true));
        assert!(!should_auto_open_picker(false, true));
        assert!(!should_auto_open_picker(true, false));
        assert!(!should_auto_open_picker(false, false));
    }
}

#[component]
fn DebugView() -> impl IntoView {
    let store = use_context::<SessionStore>().expect("SessionStore must be provided");

    let json = move || {
        store.connected.track();
        store.session_info.track();
        store.turn_state.track();
        store.streaming.track();
        store.events.track();
        store.streaming_text.track();
        store.streaming_thinking.track();
        store.transport_errors.track();
        let snap = store.snapshot();
        serde_json::to_string_pretty(&snap).unwrap_or_else(|e| format!("<serialise error: {e}>"))
    };

    view! {
        <section>
            <h3>
                "store snapshot ("
                <span data-testid="leptos-debug-event-count">
                    {move || store.events.with(Vec::len)}
                </span>
                " event(s) seen)"
            </h3>
            <pre data-testid="leptos-debug-store">{json}</pre>
        </section>
    }
}
