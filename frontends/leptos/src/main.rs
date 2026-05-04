//! Phase 3.3 — Leptos conversation feed.
//!
//! Architecture:
//! ```text
//!   App
//!    ├── provide_context::<SessionStore>      (conversation state)
//!    ├── provide_context::<SessionListStore>  (picker state)
//!    ├── provide_context::<WsClient>          (write-path handle)
//!    ├── Effect: WsClient::new(url, conv, list).connect()
//!    ├── SessionPicker     (3.2 — sibling of the feed)
//!    ├── ConversationFeed  (3.3 — primary surface)
//!    ├── StubComposer      (3.3 — temp; 3.4 replaces with real composer)
//!    └── <details data-testid="leptos-debug-panel">
//!         └── DebugView    (3.1 JSON dump, collapsed by default)
//! ```

mod event_view;
mod feed;
mod http;
mod picker;
mod protocol;
mod sessions;
mod store;
mod ws;

use leptos::prelude::*;

use crate::feed::{ConversationFeed, StubComposer};
use crate::picker::SessionPicker;
use crate::sessions::SessionListStore;
use crate::store::SessionStore;
use crate::ws::{WsClient, ws_url_from_window};

fn main() {
    console_error_panic_hook::set_once();
    mount_to_body(App);
}

#[component]
fn App() -> impl IntoView {
    let store = SessionStore::new();
    let list_store = SessionListStore::new();
    provide_context(store);
    provide_context(list_store);

    // Construct the WsClient once and provide it via context so the
    // picker, the feed (3.3), and 3.4+ composers can call
    // `WsClient::send`.
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
            <h1>"Omega (Leptos) — Phase 3.3"</h1>
            <SessionPicker />
            <ConversationFeed />
            <StubComposer />
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

    // Recompute on every relevant signal change. We touch each field
    // here so leptos's reactive graph subscribes us to all of them in
    // one shot — ergonomically equivalent to a `Memo` over `snapshot()`,
    // and the cost of one extra `to_string_pretty` per frame is fine
    // for a debug view.
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
