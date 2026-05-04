//! Phase 3.0 hello-world: open a `/ws` connection and render the `type`
//! of every received frame. Out-of-scope: styling, forms, parity with
//! the SolidJS UI. The single `RwSignal<Vec<String>>` of frame types is
//! the entire state model for 3.0; 3.1 replaces it with proper protocol
//! deserialisation through `omega_protocol`.

use leptos::prelude::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{MessageEvent, WebSocket};

fn main() {
    // Surface wasm panics in the browser devtools console instead of a
    // silent abort.
    console_error_panic_hook::set_once();
    mount_to_body(App);
}

#[component]
fn App() -> impl IntoView {
    let frames: RwSignal<Vec<String>> = RwSignal::new(Vec::new());

    // Open the WS connection once on mount. `Effect::new` runs after the
    // first render in csr; the closure captures `frames` by clone.
    Effect::new(move |_| {
        if let Err(err) = open_ws(frames) {
            leptos::logging::error!("ws connect failed: {err:?}");
        }
    });

    view! {
        <main>
            <h1>"Omega (Leptos) — Phase 3.0"</h1>
            <p data-testid="leptos-status">
                "frames seen: " {move || frames.with(Vec::len)}
            </p>
            <ul data-testid="leptos-frames">
                {move || {
                    frames
                        .get()
                        .into_iter()
                        .map(|t| view! { <li>{t}</li> })
                        .collect_view()
                }}
            </ul>
        </main>
    }
}

/// Open a WebSocket to `/ws` on the same host the page was served from
/// and append each incoming frame's `type` field to `frames`.
///
/// Bubbles up `JsValue` errors from `WebSocket::new` so the caller can
/// log them; runtime parse failures are logged inline.
fn open_ws(frames: RwSignal<Vec<String>>) -> Result<(), JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
    let location = window.location();
    let host = location.host()?;
    let proto = if location.protocol()? == "https:" {
        "wss"
    } else {
        "ws"
    };
    let url = format!("{proto}://{host}/ws");

    let ws = WebSocket::new(&url)?;

    // Keep the closure alive for the life of the page by forgetting it —
    // the page is throw-away and the closure references the singleton
    // signal, so leaking is fine and avoids the usual `Closure::wrap`
    // ref-counting dance.
    let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |evt: MessageEvent| {
        let Some(text) = evt.data().as_string() else {
            return; // binary frames are not used by the Omega protocol
        };
        let frame_type = match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(v) => v
                .get("type")
                .and_then(|t| t.as_str())
                .unwrap_or("<no-type>")
                .to_owned(),
            Err(e) => {
                leptos::logging::warn!("ws parse: {e}");
                return;
            }
        };
        frames.update(|v| v.push(frame_type));
    });
    ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    on_message.forget();

    Ok(())
}
