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
pub mod event_view;
pub mod feed;
pub mod http;
pub mod markdown;
pub mod picker;
pub mod protocol;
pub mod sessions;
pub mod store;
pub mod text_modal;
pub mod ws;

use leptos::prelude::*;

use crate::composer::Composer;
use crate::context_modal::{ContextModal, ContextModalState};
use crate::feed::ConversationFeed;
use crate::picker::{PickerOpen, SessionPicker};
use crate::protocol::TurnState;
use crate::sessions::SessionListStore;
use crate::store::SessionStore;
use crate::text_modal::{TextModal, TextModalState};
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
    provide_context(store);
    provide_context(list_store);
    provide_context(modal_state);
    provide_context(text_modal_state);
    provide_context(picker_open);

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
    // starts (or is requested to pause). Otherwise the modal overlay
    // (z-index 900) hides the composer's `Continue` button while the
    // turn is paused — the operator gets stuck. The picker only
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
        if should_auto_open_picker(store.connected.get(), store.session_info.with(Option::is_none)) {
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
            <Composer />
            <ContextModal />
            <TextModal />
            <StatusChip />
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

/// Phase 3.10 TODO-D — persistent status chip in the bottom-right corner.
///
/// Four states driven by `store.connected` and `store.turn_state`:
///
/// | `data-status` | colour | text         |
/// |---------------|--------|--------------|
/// | `ready`       | teal   | `Ready`      |
/// | `streaming`   | llm    | `Streaming…` |
/// | `paused`      | yellow | `Paused`     |
/// | `offline`     | red    | `Offline`    |
///
/// CSS lives in `style.css` (`.status-chip` + `[data-status="*"]` rules
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

/// written in Phase 3.10 planning). The chip is `pointer-events: none`
/// so it never intercepts clicks on the feed or composer beneath it.
/// Maps the connected/turn-state pair to a CSS `data-status` string.
fn status_str(connected: bool, turn_state: TurnState) -> &'static str {
    if !connected {
        "offline"
    } else {
        match turn_state {
            TurnState::Running => "streaming",
            TurnState::Paused | TurnState::PauseRequested => "paused",
            TurnState::Idle => "ready",
        }
    }
}

/// Maps a status string to its human-readable chip label.
fn status_label(status: &str) -> &'static str {
    match status {
        "offline" => "Offline",
        "streaming" => "Streaming…",
        "paused" => "Paused",
        _ => "Ready",
    }
}

#[component]
fn StatusChip() -> impl IntoView {
    let store = use_context::<SessionStore>().expect("SessionStore must be provided");

    let status = move || status_str(store.connected.get(), store.turn_state.get());
    let text = move || status_label(status());

    view! {
        <div
            class="status-chip"
            data-testid="leptos-status-chip"
            data-status=status
        >
            {text}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test;

    #[wasm_bindgen_test]
    #[test]
    fn turn_is_active_true_for_running_paused_pause_requested() {
        assert!(turn_is_active(TurnState::Running));
        assert!(turn_is_active(TurnState::Paused));
        assert!(turn_is_active(TurnState::PauseRequested));
    }

    #[wasm_bindgen_test]
    #[test]
    fn turn_is_active_false_for_idle() {
        assert!(!turn_is_active(TurnState::Idle));
    }

    #[wasm_bindgen_test]
    #[test]
    fn should_auto_open_picker_true_only_when_connected_and_no_session() {
        assert!(should_auto_open_picker(true, true));
        assert!(!should_auto_open_picker(false, true));
        assert!(!should_auto_open_picker(true, false));
        assert!(!should_auto_open_picker(false, false));
    }

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
    fn status_str_paused_for_paused_and_pause_requested() {
        assert_eq!(status_str(true, TurnState::Paused), "paused");
        assert_eq!(status_str(true, TurnState::PauseRequested), "paused");
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
    fn status_label_paused() {
        assert_eq!(status_label("paused"), "Paused");
    }

    #[wasm_bindgen_test]
    #[test]
    fn status_label_default_is_ready() {
        assert_eq!(status_label("ready"), "Ready");
        assert_eq!(status_label("other"), "Ready");
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
        serde_json::to_string_pretty(&snap)
            .unwrap_or_else(|e| format!("<serialise error: {e}>"))
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
