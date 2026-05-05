//! Phase 3.6 \u2014 Leptos UI library crate.
//!
//! Splitting the previous `[[bin]]`-only crate into a lib + bin lets
//! host-target snapshot tests pull in the components without
//! depending on the binary entrypoint. The bin (`main.rs`) is now a
//! 5-line shim around [`run`].
//!
//! ## Architecture
//!
//! ```text
//!   App
//!    \u251c\u2500\u2500 provide_context::<SessionStore>      (conversation state)
//!    \u251c\u2500\u2500 provide_context::<SessionListStore>  (picker state)
//!    \u251c\u2500\u2500 provide_context::<WsClient>          (write-path handle)
//!    \u251c\u2500\u2500 provide_context::<ContextModalState> (modal open/close)
//!    \u251c\u2500\u2500 Effect: WsClient::new(url, conv, list).connect()
//!    \u251c\u2500\u2500 SessionPicker     (3.2 + 3.5 resume button per row)
//!    \u251c\u2500\u2500 ConversationFeed  (3.3 + 3.5 LlmCallBlock + 3.6 MarkdownBody)
//!    \u251c\u2500\u2500 Composer          (3.4)
//!    \u251c\u2500\u2500 ContextModal      (3.5 \u2014 full-viewport overlay)
//!    \u2514\u2500\u2500 <details data-testid=\"leptos-debug-panel\">
//!         \u2514\u2500\u2500 DebugView    (3.1 JSON dump)
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
pub mod ws;

use leptos::prelude::*;

use crate::composer::Composer;
use crate::context_modal::{ContextModal, ContextModalState};
use crate::feed::ConversationFeed;
use crate::picker::SessionPicker;
use crate::sessions::SessionListStore;
use crate::store::SessionStore;
use crate::ws::{WsClient, ws_url_from_window};

/// Mount the [`App`] component into the document body. Called from
/// the binary's `main()`. Separated so that integration tests can
/// build the lib without invoking the mount path.
pub fn run() {
    console_error_panic_hook::set_once();
    mount_to_body(App);
}

#[component]
pub fn App() -> impl IntoView {
    let store = SessionStore::new();
    let list_store = SessionListStore::new();
    let modal_state = ContextModalState::new();
    provide_context(store);
    provide_context(list_store);
    provide_context(modal_state);

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

    view! {
        <main>
            <h1>"Omega (Leptos) — Phase 3.6"</h1>
            <SessionPicker />
            <ConversationFeed />
            <Composer />
            <ContextModal />
            <details data-testid="leptos-debug-panel">
                <summary>"debug: store snapshot"</summary>
                <DebugView />
            </details>
        </main>
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
