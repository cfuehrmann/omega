//! Phase 3.1 — Leptos WS client + reactive store + debug dump view.
//!
//! Architecture:
//! ```text
//!   App
//!    ├── provide_context::<SessionStore>           ← descendants subscribe
//!    ├── Effect: WsClient::new(url, store).connect()
//!    └── DebugView                                  ← serialises store snapshot to JSON
//! ```
//!
//! No styling, no controls, no parity with the SolidJS UI.  The only
//! visible artefact is `[data-testid="leptos-debug-store"]` containing
//! a pretty-printed JSON dump of `SessionStore::snapshot()`, updated
//! reactively as WS frames arrive.

mod protocol;
mod store;
mod ws;

use leptos::prelude::*;

use crate::store::SessionStore;
use crate::ws::{WsClient, ws_url_from_window};

fn main() {
    console_error_panic_hook::set_once();
    mount_to_body(App);
}

#[component]
fn App() -> impl IntoView {
    // Single source of truth for the page; provided so any descendant
    // (3.2+) can `use_context::<SessionStore>()`.
    let store = SessionStore::new();
    provide_context(store);

    // Open the WebSocket once on mount. `Effect::new` runs after the
    // first render in CSR; the Effect closure runs once because we
    // never read a tracked signal inside it.
    Effect::new(move |_| match ws_url_from_window() {
        Ok(url) => WsClient::new(url, store).connect(),
        Err(err) => leptos::logging::error!("ws url derivation failed: {err:?}"),
    });

    view! {
        <main>
            <h1>"Omega (Leptos) — Phase 3.1 debug view"</h1>
            <p>
                "This page renders a live JSON snapshot of "
                <code>"SessionStore"</code>
                " — every typed "
                <code>"WsMessage"</code>
                " variant the server emits is applied through the "
                <code>"apply"</code>
                " reducer.  No controls; this is the protocol smoke surface."
            </p>
            <DebugView />
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
        // `track` each field so we re-render on any change.
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
            <h2>
                "store snapshot ("
                <span data-testid="leptos-debug-event-count">
                    {move || store.events.with(Vec::len)}
                </span>
                " event(s) seen)"
            </h2>
            <pre data-testid="leptos-debug-store">{json}</pre>
        </section>
    }
}
