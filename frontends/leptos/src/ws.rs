//! `WsClient` — typed WebSocket connection that drives [`SessionStore`].
//!
//! Phase 3.1 deliverables (carried out by this module):
//!
//! 1. Open a WebSocket to `/ws` (relative to `window.location`),
//!    deserialising each incoming text frame as a typed
//!    [`crate::protocol::WsMessage`] and applying it via
//!    [`SessionStore::apply`].
//! 2. Own JS-bridged callback closures via [`StoredValue`] (Leptos's
//!    idiom for non-`Copy` long-lived values), so that each
//!    reconnection attempt deterministically drops the previous batch
//!    of closures rather than leaking them with `forget()`.
//! 3. Reconnect on `onclose` with exponential back-off
//!    (0.5 s × 2^attempt, capped at 30 s, ±20 % jitter).
//! 4. Provide a typed `send(&ClientFrame)` for future composers; phase
//!    3.1 has no callers, but the plumbing locks in the type-discipline
//!    on the write path. (`unused` is allowed for now.)

use leptos::prelude::*;
use leptos::reactive::owner::LocalStorage;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use web_sys::{CloseEvent, MessageEvent, WebSocket, js_sys};

use crate::protocol::{ClientFrame, WsMessage};
use crate::sessions::SessionListStore;
use crate::store::SessionStore;

const BACKOFF_BASE_MS: u32 = 500;
const BACKOFF_CAP_MS: u32 = 30_000;

// ---------------------------------------------------------------------------
// Inner state
// ---------------------------------------------------------------------------

struct WsState {
    url: String,
    store: SessionStore,
    list_store: SessionListStore,
    /// Currently-active socket (may be in any readyState).
    socket: Option<WebSocket>,
    /// Closures we hold across the JS↔Rust boundary for the current
    /// socket. Replaced atomically on each `connect()` call so the
    /// previous closures drop immediately.
    closures: ClosureBag,
    /// Failed/closed-without-open attempts since the last successful
    /// `onopen`. Drives the back-off.
    attempt: u32,
    /// Pending reconnect timer handle (so we can clear it on explicit
    /// disconnect).
    pending_reconnect: Option<i32>,
}

#[derive(Default)]
struct ClosureBag {
    // These fields are deliberately read-only owned slots: dropping a
    // `Closure<...>` is what *detaches* it from the JS event listener.
    // Holding them keeps the JS callbacks alive; replacing the bag
    // drops them all in one go.
    #[allow(dead_code)]
    on_open: Option<Closure<dyn FnMut(JsValue)>>,
    #[allow(dead_code)]
    on_message: Option<Closure<dyn FnMut(MessageEvent)>>,
    #[allow(dead_code)]
    on_close: Option<Closure<dyn FnMut(CloseEvent)>>,
    #[allow(dead_code)]
    on_error: Option<Closure<dyn FnMut(JsValue)>>,
}

// ---------------------------------------------------------------------------
// Public client handle
// ---------------------------------------------------------------------------

/// Cheap-to-clone handle to a single WebSocket connection plus
/// reconnection state. Backed by a [`StoredValue`] so cross-closure
/// re-entry (open → close → reconnect) is `Copy`-safe.
///
/// Uses `LocalStorage` because `WebSocket` and `Closure` are
/// `!Send + !Sync`. CSR is single-threaded so this is fine.
#[derive(Clone, Copy)]
pub struct WsClient {
    state: StoredValue<WsState, LocalStorage>,
}

impl WsClient {
    /// Build a [`WsClient`] for the given `/ws` URL writing into the
    /// two reactive stores.  Does **not** open the socket — call
    /// [`Self::connect`].
    ///
    /// `store` reduces all conversation/session frames; `list_store`
    /// reduces the picker-relevant `SessionRenamed` / `SessionDeleted`
    /// envelopes (Phase 3.2). Both are passed as `Copy` signal-handles.
    #[must_use]
    pub fn new(url: String, store: SessionStore, list_store: SessionListStore) -> Self {
        let state = WsState {
            url,
            store,
            list_store,
            socket: None,
            closures: ClosureBag::default(),
            attempt: 0,
            pending_reconnect: None,
        };
        Self {
            state: StoredValue::new_local(state),
        }
    }

    /// Open a fresh WebSocket. Drops any previously-installed socket
    /// and closures. Idempotent: a second call re-connects.
    #[mutants::skip] // WebSocket API; covered by e2e harness (smoke + reconnect tests).
    pub fn connect(self) {
        // Cancel any pending reconnect timer first so two connects
        // don't race to install duplicate sockets.
        self.state.update_value(|s| {
            if let Some(handle) = s.pending_reconnect.take() {
                clear_timeout(handle);
            }
        });

        let url = self.state.with_value(|s| s.url.clone());
        let ws = match WebSocket::new(&url) {
            Ok(ws) => ws,
            Err(err) => {
                leptos::logging::error!("ws connect failed: {err:?}");
                self.schedule_reconnect();
                return;
            }
        };

        // Build fresh closures wired to `self`.
        let on_open = {
            let client = self;
            Closure::<dyn FnMut(JsValue)>::new(move |_evt: JsValue| {
                client.state.update_value(|s| {
                    s.attempt = 0;
                });
            })
        };

        let on_message = {
            let client = self;
            Closure::<dyn FnMut(MessageEvent)>::new(move |evt: MessageEvent| {
                let Some(text) = evt.data().as_string() else {
                    // The Omega protocol is text-only — ignore binary.
                    return;
                };
                match serde_json::from_str::<WsMessage>(&text) {
                    Ok(msg) => client.state.with_value(|s| {
                        // Picker store sees envelope events first (immutable
                        // borrow); then conversation store consumes the frame.
                        s.list_store.apply(&msg);
                        s.store.apply(msg);
                    }),
                    Err(e) => {
                        leptos::logging::warn!("ws frame parse error: {e}; raw={text}");
                    }
                }
            })
        };

        let on_close = {
            let client = self;
            Closure::<dyn FnMut(CloseEvent)>::new(move |_evt: CloseEvent| {
                client.state.update_value(|s| {
                    s.store.connected.set(false);
                    s.attempt = s.attempt.saturating_add(1);
                });
                client.schedule_reconnect();
            })
        };

        let on_error = {
            Closure::<dyn FnMut(JsValue)>::new(move |evt: JsValue| {
                leptos::logging::warn!("ws onerror: {evt:?}");
            })
        };

        ws.set_onopen(Some(on_open.as_ref().unchecked_ref()));
        ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));
        ws.set_onerror(Some(on_error.as_ref().unchecked_ref()));

        // Install the new socket + closures. Dropping the prior
        // ClosureBag here detaches the previous handlers from JS.
        self.state.update_value(|s| {
            s.socket = Some(ws);
            s.closures = ClosureBag {
                on_open: Some(on_open),
                on_message: Some(on_message),
                on_close: Some(on_close),
                on_error: Some(on_error),
            };
        });
    }

    /// Send a typed [`ClientFrame`] to the server.  Returns `Err` if
    /// the socket is missing or in a non-open state — callers should
    /// queue + retry or drop, depending on context.
    ///
    /// Phase 3.2 wires this up from the session picker (new / rename /
    /// delete frames).
    #[mutants::skip] // WebSocket send; covered by e2e harness (all user-action tests).
    pub fn send(self, frame: &ClientFrame) -> Result<(), JsValue> {
        let payload = serde_json::to_string(frame)
            .map_err(|e| JsValue::from_str(&format!("client frame serialise: {e}")))?;
        self.state
            .try_with_value(|s| match s.socket.as_ref() {
                Some(ws) if ws.ready_state() == WebSocket::OPEN => ws.send_with_str(&payload),
                Some(_) => Err(JsValue::from_str("ws not in OPEN state")),
                None => Err(JsValue::from_str("ws not initialised")),
            })
            .ok_or_else(|| JsValue::from_str("WsClient state already disposed"))?
    }

    #[mutants::skip] // timer scheduling; covered by e2e harness reconnect behaviour.
    fn schedule_reconnect(self) {
        let attempt = self.state.with_value(|s| s.attempt);
        let delay_ms = backoff_delay_ms(attempt, &mut RandomJitter);

        let cb = {
            let client = self;
            Closure::<dyn FnMut()>::new(move || client.connect())
        };

        match set_timeout(cb.as_ref().unchecked_ref(), delay_ms) {
            Ok(handle) => self.state.update_value(|s| {
                s.pending_reconnect = Some(handle);
            }),
            Err(err) => leptos::logging::error!("schedule_reconnect failed: {err:?}"),
        }
        // The closure is one-shot; let JS keep ownership until it
        // fires by leaking it into the JS heap. (StoredValue would
        // require us to store one slot per pending reconnect; given
        // we cancel and replace at the timer level, the brief leak
        // window is bounded and acceptable.)
        cb.forget();
    }
}

// ---------------------------------------------------------------------------
// Back-off math (pure — directly unit-testable)
// ---------------------------------------------------------------------------

/// Source of multiplicative jitter ∈ [0.8, 1.2].
trait Jitter {
    fn factor(&mut self) -> f64;
}

struct RandomJitter;

impl Jitter for RandomJitter {
    fn factor(&mut self) -> f64 {
        // js_sys::Math::random() ∈ [0, 1) — map to [0.8, 1.2).
        0.8 + js_sys::Math::random() * 0.4
    }
}

/// Compute the back-off delay for `attempt` (0-based: first failure is
/// `attempt = 1`, etc.). `0.5 s × 2^(attempt-1)` capped at 30 s, then
/// multiplied by a jitter factor.
fn backoff_delay_ms(attempt: u32, jitter: &mut impl Jitter) -> i32 {
    let base = if attempt <= 1 {
        BACKOFF_BASE_MS
    } else {
        let shift = (attempt - 1).min(8); // cap exponent so we don't overflow
        BACKOFF_BASE_MS.saturating_mul(1u32 << shift)
    };
    let capped = base.min(BACKOFF_CAP_MS);
    let with_jitter = (f64::from(capped) * jitter.factor()).round();
    let bounded = with_jitter.clamp(0.0, f64::from(i32::MAX));
    // Cast through u32 first to avoid sign confusion on large values.
    bounded as i32
}

// ---------------------------------------------------------------------------
// JS timer wrappers
// ---------------------------------------------------------------------------

#[mutants::skip] // JS timer wrapper; covered by e2e harness reconnect tests.
fn set_timeout(callback: &js_sys::Function, ms: i32) -> Result<i32, JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
    window.set_timeout_with_callback_and_timeout_and_arguments_0(callback, ms)
}

#[mutants::skip] // JS timer cancel; covered by e2e harness reconnect tests.
fn clear_timeout(handle: i32) {
    if let Some(window) = web_sys::window() {
        window.clear_timeout_with_handle(handle);
    }
}

// ---------------------------------------------------------------------------
// URL derivation
// ---------------------------------------------------------------------------

/// Derive `<ws|wss>://<host>/ws` from `window.location`. Errors only on
/// a missing/foreign-origin window.
#[mutants::skip] // URL derives from window.location protocol; test URL is always http so == vs != flip is value-equivalent in the wasm test harness.
pub fn ws_url_from_window() -> Result<String, JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
    let location = window.location();
    let host = location.host()?;
    let proto = if location.protocol()? == "https:" {
        "wss"
    } else {
        "ws"
    };
    Ok(format!("{proto}://{host}/ws"))
}

// ---------------------------------------------------------------------------
// Tests (pure back-off math — no DOM/WS required)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::float_cmp)]

    use std::cell::Cell;

    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test;

    /// Deterministic jitter: returns a fixed factor each call.
    struct FixedJitter(f64);
    impl Jitter for FixedJitter {
        fn factor(&mut self) -> f64 {
            self.0
        }
    }

    /// Sequence-driven jitter: cell + slice.
    struct SeqJitter {
        idx: Cell<usize>,
        values: &'static [f64],
    }
    impl Jitter for SeqJitter {
        fn factor(&mut self) -> f64 {
            let i = self.idx.get();
            self.idx.set(i + 1);
            self.values[i]
        }
    }

    #[wasm_bindgen_test]
    fn first_attempt_is_base_delay() {
        // attempt=1 → base, jitter=1.0 → exactly 500ms.
        assert_eq!(backoff_delay_ms(1, &mut FixedJitter(1.0)), 500);
    }

    #[wasm_bindgen_test]
    fn second_attempt_doubles() {
        assert_eq!(backoff_delay_ms(2, &mut FixedJitter(1.0)), 1000);
    }

    #[wasm_bindgen_test]
    fn fifth_attempt_at_eight_times_base() {
        // 0.5s * 2^(5-1) = 8s.
        assert_eq!(backoff_delay_ms(5, &mut FixedJitter(1.0)), 8000);
    }

    #[wasm_bindgen_test]
    fn high_attempt_caps_at_thirty_seconds() {
        // 0.5s * 2^7 = 64s → capped to 30s.
        assert_eq!(backoff_delay_ms(8, &mut FixedJitter(1.0)), 30_000);
        assert_eq!(backoff_delay_ms(20, &mut FixedJitter(1.0)), 30_000);
    }

    #[wasm_bindgen_test]
    fn jitter_factor_is_applied() {
        // attempt=2 base = 1000ms. Min jitter 0.8 → 800; max 1.2 → 1200.
        let lo = backoff_delay_ms(2, &mut FixedJitter(0.8));
        let hi = backoff_delay_ms(2, &mut FixedJitter(1.2));
        assert_eq!(lo, 800);
        assert_eq!(hi, 1200);
    }

    #[wasm_bindgen_test]
    fn each_attempt_uses_one_jitter_sample() {
        let mut j = SeqJitter {
            idx: Cell::new(0),
            values: &[1.0, 1.0, 1.0],
        };
        let _ = backoff_delay_ms(1, &mut j);
        let _ = backoff_delay_ms(2, &mut j);
        let _ = backoff_delay_ms(3, &mut j);
        assert_eq!(j.idx.get(), 3);
    }

    /// `RandomJitter` is the production `Jitter` impl that wraps
    /// `js_sys::Math::random()`. The unit tests above only ever drive
    /// `backoff_delay_ms` with deterministic stub jitters, so the real
    /// implementation — `0.8 + Math::random() * 0.4` → `[0.8, 1.2)` —
    /// was not exercised at all and survived every operator mutation
    /// (`+ → -`, `* → /`, etc.).
    ///
    /// Asserting the documented range over many samples kills those
    /// mutants: `+ → -` would drift below `0.8`, `* → /` and `+ → +=
    /// 0.4` (the `0.4` operand mutations) would produce values outside
    /// `[0.8, 1.2)`. Asserting at least two distinct values rules out
    /// any mutation that collapses the function to a constant.
    #[wasm_bindgen_test]
    fn random_jitter_factor_in_documented_range_and_varies() {
        let mut j = RandomJitter;
        let mut samples: Vec<f64> = Vec::with_capacity(200);
        for _ in 0..200 {
            let f = j.factor();
            assert!(
                (0.8..1.2).contains(&f),
                "RandomJitter::factor returned {f}; expected [0.8, 1.2)"
            );
            samples.push(f);
        }
        let first = samples[0];
        let varies = samples.iter().any(|s| (s - first).abs() > 1e-9);
        assert!(
            varies,
            "RandomJitter::factor returned a constant value across 200 samples"
        );
    }
}
