//! Generic text-overlay modal (Phase 3.10 TODO-A / B / C).
//!
//! Re-usable centred overlay that shows an arbitrary `String` body
//! under a title.  Used by:
//!
//! * `llm_response` — `[thinking]` and `[payload]` buttons (TODO-A)
//! * `llm_call`     — `[payload]` button               (TODO-B)
//! * `tool_call`    — `[payload]` button               (TODO-C)
//! * `tool_result`  — `[payload]` button               (TODO-C)

use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;

// ---------------------------------------------------------------------------
// Modal state
// ---------------------------------------------------------------------------

/// App-scoped reactive state for the text modal.
///
/// `Some((title, body))` = modal open; `None` = modal closed.
#[derive(Debug, Clone, Copy)]
pub struct TextModalState(pub RwSignal<Option<(String, String)>>);

impl TextModalState {
    /// Create a new, closed text-modal state.
    #[must_use]
    pub fn new() -> Self {
        Self(RwSignal::new(None))
    }

    /// Open the modal with the given title and body text.
    pub fn open(self, title: impl Into<String>, body: impl Into<String>) {
        self.0.set(Some((title.into(), body.into())));
    }

    /// Close the modal.
    pub fn close(self) {
        self.0.set(None);
    }
}

impl Default for TextModalState {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

/// Generic text overlay.
///
/// Renders nothing when closed.  Reads open/close state from the
/// app-scoped [`TextModalState`] context (provided in `App` via
/// `provide_context`).
#[component]
pub fn TextModal() -> impl IntoView {
    let state =
        use_context::<TextModalState>().expect("TextModalState must be provided");
    let on_close = move |_: leptos::ev::MouseEvent| state.close();

    // Focusable backdrop: auto-focused on mount so Esc reaches the
    // keydown handler even before the operator clicks inside.
    let backdrop_ref = NodeRef::<html::Div>::new();
    Effect::new(move |_| {
        if backdrop_ref.get().is_some() {
            spawn_local(async move {
                if let Some(el) = backdrop_ref.get_untracked() {
                    let _ = el.focus();
                }
            });
        }
    });
    let on_keydown = move |evt: leptos::ev::KeyboardEvent| {
        if evt.key() == "Escape" {
            state.close();
        }
    };

    view! {
        <Show
            when=move || state.0.with(Option::is_some)
            fallback=|| ().into_any()
        >
            <div
                class="leptos-text-modal-backdrop"
                data-testid="leptos-text-modal-backdrop"
                node_ref=backdrop_ref
                tabindex="-1"
                on:keydown=on_keydown
            >
                <div
                    class="leptos-text-modal"
                    data-testid="leptos-text-modal"
                >
                    <header class="leptos-text-modal-header">
                        <span
                            class="leptos-text-modal-title"
                            data-testid="leptos-text-modal-title"
                        >
                            {move || {
                                state
                                    .0
                                    .with(|opt| opt.as_ref().map(|(t, _)| t.clone()).unwrap_or_default())
                            }}
                        </span>
                        <button
                            class="leptos-text-modal-close"
                            data-testid="leptos-text-modal-close"
                            aria-label="close"
                            on:click=on_close
                        >
                            "✕"
                        </button>
                    </header>
                    <pre
                        class="leptos-text-modal-body"
                        data-testid="leptos-text-modal-body"
                    >
                        {move || {
                            state
                                .0
                                .with(|opt| opt.as_ref().map(|(_, b)| b.clone()).unwrap_or_default())
                        }}
                    </pre>
                </div>
            </div>
        </Show>
    }
}

// ---------------------------------------------------------------------------
// Unit tests (wasm-bindgen-test)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use leptos::reactive::owner::Owner;
    use wasm_bindgen_test::wasm_bindgen_test;

    fn with_owner<F: FnOnce()>(f: F) {
        let owner = Owner::new();
        owner.with(f);
    }

    #[wasm_bindgen_test]
    fn text_modal_state_starts_closed() {
        with_owner(|| {
            let s = TextModalState::new();
            assert!(s.0.get_untracked().is_none());
        });
    }

    #[wasm_bindgen_test]
    fn text_modal_state_open_sets_content() {
        with_owner(|| {
            let s = TextModalState::new();
            s.open("my title", "my body");
            let opt = s.0.get_untracked();
            assert!(opt.is_some(), "expected Some after open");
            let (title, body) = opt.unwrap();
            assert_eq!(title, "my title");
            assert_eq!(body, "my body");
        });
    }

    #[wasm_bindgen_test]
    fn text_modal_state_close_clears_content() {
        with_owner(|| {
            let s = TextModalState::new();
            s.open("t", "b");
            s.close();
            assert!(s.0.get_untracked().is_none());
        });
    }

    #[wasm_bindgen_test]
    fn text_modal_state_open_overwrites_previous() {
        with_owner(|| {
            let s = TextModalState::new();
            s.open("t1", "b1");
            s.open("t2", "b2");
            let (title, body) = s.0.get_untracked().unwrap();
            assert_eq!(title, "t2");
            assert_eq!(body, "b2");
        });
    }
}
