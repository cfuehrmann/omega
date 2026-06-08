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
//!    ├── BottomPanelsContainer   ← Usage / Queue / Monitors stacked panels
//!    ├── Composer          (3.4 + 3.9 Sessions button + 6 Panels menu)
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
use crate::monitors_panel::{MonitorsPanel, MonitorsPanelOpen};
use crate::picker::{PickerOpen, SessionPicker};
use crate::protocol::TurnState;
use crate::queue_panel::{QueuePanel, QueuePanelOpen};
use crate::sessions::SessionListStore;
use crate::store::SessionStore;
use crate::text_modal::{TextModal, TextModalState};
use crate::usage_panel::{UsagePanel, UsagePanelOpen};
use crate::ws::{WsClient, ws_url_from_window};

/// localStorage key that persists which bottom panels are open across
/// browser refreshes.  Format written by [`serialize_panels_open`].
/// Only referenced in the wasm32 build path.
#[cfg(target_arch = "wasm32")]
pub(crate) const BOTTOM_PANELS_STORAGE_KEY: &str = "omega.bottom_panels";

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

    // Seed panel open-states from localStorage (best-effort; defaults to all
    // closed when the key is absent or unparseable).
    let (init_usage, init_queue, init_monitors) = load_panels_from_storage();
    let usage_panel_open = UsagePanelOpen(RwSignal::new(init_usage));
    let monitors_panel_open = MonitorsPanelOpen(RwSignal::new(init_monitors));
    let queue_panel_open = QueuePanelOpen(RwSignal::new(init_queue));

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

    // Persist the panel open-set to localStorage whenever any toggle changes.
    Effect::new(move |_| {
        let usage = usage_panel_open.is_open();
        let queue = queue_panel_open.is_open();
        let monitors = monitors_panel_open.is_open();
        let value = serialize_panels_open(usage, queue, monitors);
        save_panels_to_storage(&value);
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
            <TransportErrorBanner />
            <SessionPicker />
            <ConversationFeed />
            // Three collapsible panels stacked inside one scrollable container.
            // The outer div caps the combined height so they don't overwhelm the
            // viewport, and the overflow-y scrollbar lets the operator scroll
            // within the open panels without losing the composer row.
            <div class="bottom-panels-container" data-testid="bottom-panels-container">
                <UsagePanel />
                <QueuePanel />
                <MonitorsPanel />
            </div>
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

// ---------------------------------------------------------------------------
// Bottom-panels localStorage persistence
// ---------------------------------------------------------------------------

/// Serialize three panel open-state booleans to a localStorage string.
///
/// Format: `"usage:<u>,queue:<q>,monitors:<m>"` where each value is
/// `"true"` or `"false"`.  Chosen over JSON to avoid a serde dep in
/// this leaf module; parsed by [`parse_panels_open`].
pub(crate) fn serialize_panels_open(usage: bool, queue: bool, monitors: bool) -> String {
    format!("usage:{usage},queue:{queue},monitors:{monitors}")
}

/// Parse a localStorage string previously written by [`serialize_panels_open`].
///
/// Returns `(usage, queue, monitors)` booleans. Best-effort: any parse
/// failure (missing keys, unrecognised values, corrupt data) silently
/// returns `(false, false, false)` so the UI opens with all panels closed
/// rather than crashing on a bad persisted value.
pub(crate) fn parse_panels_open(stored: &str) -> (bool, bool, bool) {
    let mut usage = false;
    let mut queue = false;
    let mut monitors = false;
    for part in stored.split(',') {
        let mut kv = part.splitn(2, ':');
        let Some(key) = kv.next() else { continue };
        let val = kv.next() == Some("true");
        match key {
            "usage" => usage = val,
            "queue" => queue = val,
            "monitors" => monitors = val,
            _ => {}
        }
    }
    (usage, queue, monitors)
}

/// Read the stored panels open-set from localStorage (wasm32) or return
/// `(false, false, false)` on non-wasm targets / any storage failure.
///
/// Two separate `#[cfg]` branches — one wasm32, one not — avoid an
/// "unreachable code" error that a single block + fallback expression
/// would trigger in the wasm32 build.  The non-wasm32 branch calls
/// `parse_panels_open("")` (identical to returning all-false) to keep
/// `parse_panels_open` visible to the dead-code lint.
#[mutants::skip] // localStorage interop; correctness covered by serialize/parse unit tests.
fn load_panels_from_storage() -> (bool, bool, bool) {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()
            .and_then(|w| w.local_storage().ok().flatten())
            .and_then(|s| s.get_item(BOTTOM_PANELS_STORAGE_KEY).ok().flatten())
            .map(|s| parse_panels_open(&s))
            .unwrap_or((false, false, false))
    }
    // Non-wasm32 (SSR / host tests): no localStorage; all panels start closed.
    // parse_panels_open is called explicitly so the function is not
    // flagged as dead code in non-wasm32 compilation units.
    #[cfg(not(target_arch = "wasm32"))]
    parse_panels_open("")
}

/// Write the current panels open-set to localStorage.  Best-effort —
/// silently ignores any storage errors (quota exceeded, private mode, …).
#[mutants::skip] // localStorage interop; behaviour tested indirectly by e2e.
fn save_panels_to_storage(value: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
            let _ = storage.set_item(BOTTOM_PANELS_STORAGE_KEY, value);
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = value; // suppress unused-variable warning in non-wasm builds.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test;

    // ── turn_is_active ────────────────────────────────────────────────────

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

    // ── serialize_panels_open ─────────────────────────────────────────────

    #[wasm_bindgen_test]
    #[test]
    fn serialize_all_false() {
        assert_eq!(
            serialize_panels_open(false, false, false),
            "usage:false,queue:false,monitors:false"
        );
    }

    #[wasm_bindgen_test]
    #[test]
    fn serialize_all_true() {
        assert_eq!(
            serialize_panels_open(true, true, true),
            "usage:true,queue:true,monitors:true"
        );
    }

    #[wasm_bindgen_test]
    #[test]
    fn serialize_mixed_usage_only() {
        assert_eq!(
            serialize_panels_open(true, false, false),
            "usage:true,queue:false,monitors:false"
        );
    }

    #[wasm_bindgen_test]
    #[test]
    fn serialize_mixed_monitors_only() {
        assert_eq!(
            serialize_panels_open(false, false, true),
            "usage:false,queue:false,monitors:true"
        );
    }

    // ── parse_panels_open ─────────────────────────────────────────────────

    #[wasm_bindgen_test]
    #[test]
    fn parse_all_false() {
        assert_eq!(
            parse_panels_open("usage:false,queue:false,monitors:false"),
            (false, false, false)
        );
    }

    #[wasm_bindgen_test]
    #[test]
    fn parse_all_true() {
        assert_eq!(
            parse_panels_open("usage:true,queue:true,monitors:true"),
            (true, true, true)
        );
    }

    #[wasm_bindgen_test]
    #[test]
    fn parse_mixed_usage_and_monitors() {
        let (u, q, m) = parse_panels_open("usage:true,queue:false,monitors:true");
        assert!(u);
        assert!(!q);
        assert!(m);
    }

    #[wasm_bindgen_test]
    #[test]
    fn parse_empty_string_returns_all_false() {
        assert_eq!(parse_panels_open(""), (false, false, false));
    }

    #[wasm_bindgen_test]
    #[test]
    fn parse_corrupt_string_returns_all_false() {
        assert_eq!(parse_panels_open("garbage!!!"), (false, false, false));
    }

    #[wasm_bindgen_test]
    #[test]
    fn parse_unknown_key_ignored_known_keys_still_parsed() {
        let (u, q, m) = parse_panels_open("usage:true,unknown:true,monitors:true");
        assert!(u);
        assert!(!q); // queue key absent → false
        assert!(m);
    }

    #[wasm_bindgen_test]
    #[test]
    fn parse_partial_value_not_true_treated_as_false() {
        // "tru" is not "true" → treated as false.
        let (u, _q, _m) = parse_panels_open("usage:tru,queue:false,monitors:false");
        assert!(!u);
    }

    /// Round-trip: serialize → parse must be identity for all 8 combinations.
    #[wasm_bindgen_test]
    #[test]
    fn serialize_parse_round_trip_all_combinations() {
        for usage in [false, true] {
            for queue in [false, true] {
                for monitors in [false, true] {
                    let s = serialize_panels_open(usage, queue, monitors);
                    assert_eq!(
                        parse_panels_open(&s),
                        (usage, queue, monitors),
                        "round-trip failed for ({usage},{queue},{monitors}): {s}"
                    );
                }
            }
        }
    }
}

/// Always-visible banner that surfaces transport-level errors held in
/// [`SessionStore::transport_errors`] — in particular WS frames that
/// could not be parsed into a [`crate::protocol::WsMessage`] (§16).
/// Without this, an unparseable frame would only reach the dev console;
/// the debug panel is stripped from release builds, so the store error
/// state would otherwise be invisible to operators.
#[component]
#[mutants::skip] // view-only banner; behaviour covered by SessionStore::record_frame_parse_error tests + e2e.
fn TransportErrorBanner() -> impl IntoView {
    let store = use_context::<SessionStore>().expect("SessionStore must be provided");
    view! {
        <Show when=move || !store.transport_errors.with(Vec::is_empty) fallback=|| ()>
            <div
                data-testid="transport-error-banner"
                role="alert"
                style="background:#5c1a1a;color:#ffd7d7;padding:8px 12px;font-family:monospace;font-size:13px;border-bottom:2px solid #ff5252;"
            >
                {move || {
                    store
                        .transport_errors
                        .get()
                        .into_iter()
                        .map(|m| view! { <div>{m}</div> })
                        .collect_view()
                }}
            </div>
        </Show>
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
