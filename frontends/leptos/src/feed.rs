//! Conversation-feed component (Phase 3.3).
//!
//! ```text
//!   <ConversationFeed>
//!    │  reads SessionStore::events (RwSignal<Vec<OmegaEvent>>)
//!    │  reads SessionStore::streaming_text + streaming_thinking
//!    │
//!    ├── <For each=events key=index>
//!    │    └── <EventBlock event=ev />
//!    │         ├── pure `kind_for(&event)`   → CSS class + data-event-kind
//!    │         ├── pure `event_type_tag()`   → data-event-type
//!    │         └── match → typed body view per OmegaEvent variant
//!    │              └── ToolResult: <ToolResultBlock> with payload modal (TODO-C)
//!    │                   └── pure `truncate_preview(s, 2, 200)`
//!    ├── <StreamingTail>     (live append into the active assistant block)
//!    └── <div sentinel/>     (Effect-driven scrollIntoView seam)
//! ```
//!
//! ## Mutation-test carve-outs
//!
//! Pure helpers in [`crate::event_view`] are individually mutation-
//! tested. Component glue (NodeRef reads, scrollIntoView calls, view!
//! macro expansions, event handlers) is the JS-interop edge — same
//! mutation-gap pattern as 3.1's `ws.rs::WsClient::send` and 3.2's
//! `picker.rs` event handlers.
//!
//! ## Streaming-text strategy
//!
//! Direct append: `StreamingTail` reads `streaming_text` once per
//! re-render, which leptos triggers per-`Text`-frame because
//! `SessionStore::apply` calls `streaming_text.update(|s|
//! s.push_str(...))`. Per-keystroke reactivity. No rAF buffer; if 3.6
//! introduces markdown rendering and per-frame work becomes expensive,
//! that's the point at which a buffer earns its keep.

use leptos::ev;
use leptos::html;
use leptos::prelude::*;
use omega_protocol::OmegaEvent;
use std::cell::Cell;
use std::rc::Rc;

/// Returns the current time in milliseconds (epoch on wasm32; always 0 on
/// host targets where the auto-scroll code never executes).
#[inline]
fn now_ms() -> f64 {
    #[cfg(target_arch = "wasm32")]
    {
        js_sys::Date::now()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        0.0
    }
}

use crate::context_modal::ContextModalState;
#[cfg(target_arch = "wasm32")]
use crate::diff_render::render_diff_html;
use crate::event_view::{
    EventKind, assign_tool_corr, css_class_for, event_type_tag, kind_for, kind_tag,
    should_autoscroll, truncate_preview,
};
use crate::markdown;
use crate::store::SessionStore;
use crate::text_modal::TextModalState;

// ---------------------------------------------------------------------------
// Mermaid + copy-button JS interop (Phase 3.6)
// ---------------------------------------------------------------------------
//
// The mermaid lazy-load shim lives in `src/mermaid.js` so it can
// dynamically `import("mermaid")` from a CDN — wasm-bindgen would
// otherwise need a built-time dep on a 600 KB JS library and the
// SolidJS UI mirrors this exact lazy-load pattern
// (App.tsx:122-132). The two-function surface (`renderMermaid` +
// `addCopyButtons`) is enough to mirror SolidJS's
// `enhanceCodeBlocks` + `renderMermaidBlocks` end-to-end.
//
// Module path is crate-relative: wasm-bindgen rewrites it at link
// time so the resulting `.js` lives next to the wasm output. On
// host targets these externs compile but the functions are never
// invoked (the Effects that call them only fire under `csr`).
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(module = "/src/mermaid.js")]
extern "C" {
    #[wasm_bindgen(js_name = "renderMermaid", catch)]
    fn js_render_mermaid(container: &web_sys::Element) -> Result<wasm_bindgen::JsValue, wasm_bindgen::JsValue>;

    #[wasm_bindgen(js_name = "addCopyButtons")]
    fn js_add_copy_buttons(container: &web_sys::Element);
}

#[cfg(not(target_arch = "wasm32"))]
#[allow(dead_code)]
fn js_render_mermaid(_container: &()) -> Result<(), ()> {
    Ok(())
}
#[cfg(not(target_arch = "wasm32"))]
#[allow(dead_code)]
fn js_add_copy_buttons(_container: &()) {}

/// Pixels of grace at the bottom of the feed before the user is
/// considered to have scrolled up. Mirrors the SolidJS UI's `8` px
/// magic but bumped to `40` (per task spec) — handles bigger
/// micro-scrolls and sub-pixel rounding on hi-DPI displays.
const AUTOSCROLL_THRESHOLD_PX: f64 = 40.0;

// ---------------------------------------------------------------------------
// Top-level feed
// ---------------------------------------------------------------------------

/// Primary visible surface of the Leptos UI.
///
/// Reads `events`, `streaming_text`, `streaming_thinking` from the
/// `SessionStore` context. Renders one `<EventBlock/>` per event,
/// then a streaming overlay (text and/or thinking), then a sentinel
/// `<div>` that the auto-scroll Effect targets via `scrollIntoView()`.
/// Mutations skipped: auto-scroll reactive conditions require live DOM;
/// scrolling behaviour is verified by the e2e harness.
#[mutants::skip]
#[component]
pub fn ConversationFeed() -> impl IntoView {
    let store = use_context::<SessionStore>().expect("SessionStore must be provided");

    let scroll_ref = NodeRef::<html::Section>::new();
    let sentinel_ref = NodeRef::<html::Div>::new();
    let auto_scroll = RwSignal::new(true);
    // Chromium 148+ dispatches scroll events as macrotasks, so the
    // event can fire *after* new content has been appended (growing
    // scrollHeight), making should_autoscroll() spuriously return false.
    //
    // Fix: record (timestamp_ms, scrollHeight) of each programmatic
    // set_scroll_top.  In on_scroll, if elapsed < grace AND the observed
    // scrollTop matches the expected clamped value (prog_sh - clientH)
    // AND scrollHeight has grown since then, it's a deferred echo — ignore
    // it.  A genuine user scroll-to-top produces a completely different
    // scrollTop (≈0), so the position check rejects it even within the
    // grace window.
    let prog_state: Rc<Cell<(f64, i32)>> = Rc::new(Cell::new((-10_000.0, 0)));
    const PROG_SCROLL_GRACE_MS: f64 = 150.0;

    // Auto-scroll Effect.
    //
    // We subscribe explicitly to `events.with(Vec::len)`,
    // `streaming_text.with(String::len)`, and
    // `streaming_thinking.with(String::len)` so the effect re-runs on
    // every event append AND every streamed-text fragment AND every
    // streamed-thinking fragment — but NOT on signals that the feed
    // doesn't display directly. We do NOT subscribe to `auto_scroll`
    // itself: reading it via `get_untracked` keeps the gate-flip out
    // of the effect's dependency set so an `on:scroll`-induced flip
    // doesn't itself trigger a scroll-into-view.
    //
    // We use `set_scroll_top(scroll_height)` instead of
    // `sentinel.scroll_into_view()` to keep the scroll synchronous
    // (value is clamped by the browser to the legal maximum).
    // We also stamp `last_prog_scroll_ts` so the on_scroll handler
    // can suppress the deferred scroll event that Chromium 148+ fires
    // after new content has already been appended (see comment above).
    let prog_state_eff = Rc::clone(&prog_state);
    Effect::new(move |_| {
        let _ = store.events.with(Vec::len);
        let _ = store.streaming_text.with(String::len);
        let _ = store.streaming_thinking.with(String::len);
        if !auto_scroll.get_untracked() {
            return;
        }
        if let Some(section) = scroll_ref.get() {
            let sh = section.scroll_height();
            prog_state_eff.set((now_ms(), sh));
            section.set_scroll_top(sh);
        }
    });

    let prog_state_scroll = Rc::clone(&prog_state);
    let on_scroll = move |_ev: ev::Event| {
        let Some(el) = scroll_ref.get() else {
            return;
        };
        let scroll_top = f64::from(el.scroll_top());
        let client_height = f64::from(el.client_height());
        let scroll_height = f64::from(el.scroll_height());

        // Guard against deferred scroll-event echoes from our own
        // set_scroll_top calls (Chromium 148+ fires scroll events as
        // macrotasks — new content can arrive between the call and the
        // event, making should_autoscroll() spuriously return false).
        //
        // We only suppress the event when ALL three conditions hold:
        //   1. It arrived within the grace window after a programmatic scroll.
        //   2. We're already in tailing mode (auto_scroll = true).
        //   3. The scroll position matches the expected clamped value (the
        //      deferred echo of prog_sh − clientH), AND scrollHeight has
        //      grown since we set it (confirming new content arrived).
        // A genuine user scroll-to-top yields scroll_top ≈ 0 which never
        // satisfies condition 3, so it is always processed.
        let (prog_ts, prog_sh) = prog_state_scroll.get();
        let elapsed = now_ms() - prog_ts;
        if elapsed < PROG_SCROLL_GRACE_MS && auto_scroll.get_untracked() {
            let expected_st = f64::from(prog_sh) - client_height;
            let is_echo = (scroll_top - expected_st).abs() <= 5.0
                && scroll_height > f64::from(prog_sh) + 10.0;
            if is_echo {
                return;
            }
        }

        let next = should_autoscroll(
            scroll_top,
            client_height,
            scroll_height,
            AUTOSCROLL_THRESHOLD_PX,
        );
        if auto_scroll.get_untracked() != next {
            auto_scroll.set(next);
        }
    };

    view! {
        <div class="feed-wrapper">
            <section
                class="leptos-feed"
                data-testid="leptos-feed"
                data-auto-scroll=move || if auto_scroll.get() { "true" } else { "false" }
                node_ref=scroll_ref
                on:scroll=on_scroll
            >
                <For
                    each=move || {
                        let events = store.events.get();
                        let corrs = assign_tool_corr(&events);
                        events
                            .into_iter()
                            .enumerate()
                            .zip(corrs)
                            .map(|((idx, ev), corr)| (idx, ev, corr))
                            .collect::<Vec<(usize, OmegaEvent, Option<usize>)>>()
                    }
                    key=|(idx, _, _): &(usize, OmegaEvent, Option<usize>)| *idx
                    children=|(_, event, corr): (usize, OmegaEvent, Option<usize>)| view! { <EventBlock event=event corr=corr /> }
                />
                <StreamingTail />
                <div class="leptos-feed-sentinel" data-testid="leptos-feed-sentinel" node_ref=sentinel_ref />
            </section>
            <Show when=move || !auto_scroll.get()>
                <button
                    class="scroll-to-bottom-btn"
                    data-testid="scroll-to-bottom"
                    aria-label="Scroll to bottom"
                    on:click=move |_| {
                        auto_scroll.set(true);
                        if let Some(el) = sentinel_ref.get_untracked() {
                            el.scroll_into_view();
                        }
                    }
                >
                    "↓"
                </button>
            </Show>
        </div>
    }
}

// ---------------------------------------------------------------------------
// Event block
// ---------------------------------------------------------------------------

/// Renders one `OmegaEvent` with the visual-family CSS class and a
/// typed body. `ToolResult` events delegate to [`ToolResultBlock`]
/// for show-more state.
///
/// `pub` so the host-target snapshot harness in `tests/snapshots.rs`
/// (Phase 3.6 TEST-ARCH-5) can render fixtures directly. The wasm
/// runtime mounts it from `<ConversationFeed/>`.
///
/// `corr` is the 1-based correlation integer for tool-call / tool-result
/// pairs within the same `LlmCall` group (see [`assign_tool_corr`]).
/// When called from the snapshot harness without a corr argument it
/// defaults to `None` and no superscript is rendered.
#[component]
pub fn EventBlock(event: OmegaEvent, #[prop(optional_no_strip)] corr: Option<usize>) -> impl IntoView {
    let kind = kind_for(&event);
    let class = css_class_for(kind);
    let kind_str = kind_tag(kind);
    let event_type = event_type_tag(&event);

    view! {
        <div
            class=class
            data-testid="leptos-event-block"
            data-event-type=event_type
            data-event-kind=kind_str
        >
            {render_event_body(event, corr)}
        </div>
    }
}

/// Per-variant body view. Returns `AnyView` so the match arms can each
/// produce their own concrete `view!` output and unify at the call
/// site. The big match here is necessary (each arm needs typed field
/// access); the *family decision* is carved out into the pure
/// `kind_for` in `event_view.rs`.
fn render_event_body(event: OmegaEvent, corr: Option<usize>) -> AnyView {
    match event {
        OmegaEvent::UserMessage(e) => view! {
            <span class="block-label">"user_message"</span>
            <pre class="block-body" data-testid="leptos-user-content">{e.content}</pre>
        }
        .into_any(),

        OmegaEvent::LlmResponse(e) => view! { <LlmResponseBlock event=e /> }.into_any(),

        OmegaEvent::ToolCall(e) => view! { <ToolCallBlock event=e corr=corr /> }.into_any(),

        OmegaEvent::ToolResult(e) => view! { <ToolResultBlock event=e corr=corr /> }.into_any(),

        OmegaEvent::TurnEnd(e) => {
            let m = &e.metrics;
            let line = format!(
                "turn end · in: {} · out: {}",
                m.input_tokens, m.output_tokens,
            );
            view! {
                <span class="block-label">"turn_end"</span>
                <span class="block-body">{line}</span>
            }
            .into_any()
        }

        OmegaEvent::LlmCall(e) => view! { <LlmCallBlock event=e /> }.into_any(),

        OmegaEvent::LlmError(e) => {
            let status = e
                .http_status
                .map_or_else(String::new, |s| format!("HTTP {s} · "));
            view! {
                <span class="block-label">"llm_error"</span>
                <pre class="block-body">{format!("{status}{}", e.error)}</pre>
            }
            .into_any()
        }

        OmegaEvent::AgentError(e) => view! {
            <span class="block-label">"agent_error"</span>
            <pre class="block-body">{e.error}</pre>
        }
        .into_any(),

        OmegaEvent::TransportError(e) => {
            let ctx = e.context.unwrap_or_default();
            view! {
                <span class="block-label">"transport_error"</span>
                <pre class="block-body">{format!("{} · {ctx}", e.error)}</pre>
            }
            .into_any()
        }

        OmegaEvent::TurnInterrupted(e) => {
            let reason = e
                .reason
                .map(|r| format!("{r:?}"))
                .unwrap_or_else(|| "unknown".into());
            view! {
                <span class="block-label">"turn_interrupted"</span>
                <span class="block-body">{format!("reason: {reason}")}</span>
            }
            .into_any()
        }

        OmegaEvent::SessionStarted(e) => view! {
            <span class="block-label">"session_started"</span>
            <span class="block-body">{format!("model: {} · effort: {}", e.model, e.effort)}</span>
        }
        .into_any(),

        OmegaEvent::ServerStarted(_) => view! {
            <span class="block-label">"server_started"</span>
        }
        .into_any(),

        OmegaEvent::ServerStopped(e) => view! {
            <span class="block-label">"server_stopped"</span>
            <span class="block-body">{format!("{:?}", e.outcome)}</span>
        }
        .into_any(),

        OmegaEvent::Compacted(e) => {
            let line = serde_json::to_string(&e.usage).unwrap_or_else(|_| "{}".into());
            view! {
                <span class="block-label">"compacted"</span>
                <pre class="block-body">{line}</pre>
            }
            .into_any()
        }

        OmegaEvent::LlmRetry(e) => view! {
            <span class="block-label">"llm_retry"</span>
            <span class="block-body">{format!("attempt {} · wait {}ms · {}", e.attempt, e.wait_ms, e.error)}</span>
        }
        .into_any(),

        OmegaEvent::ModelChanged(e) => view! {
            <span class="block-label">"model_changed"</span>
            <span class="block-body">{format!("model: {}", e.model)}</span>
        }
        .into_any(),

        OmegaEvent::EffortChanged(e) => view! {
            <span class="block-label">"effort_changed"</span>
            <span class="block-body">{format!("effort: {}", e.effort)}</span>
        }
        .into_any(),

        OmegaEvent::ResumingSession(e) => view! {
            <span class="block-label">"resuming_session"</span>
            <span class="block-body">{format!("from: {} · basis: {}", e.resumed_from, e.basis)}</span>
        }
        .into_any(),

        OmegaEvent::SessionResumed(e) => view! {
            <span class="block-label">"session_resumed"</span>
            <MarkdownBody text=e.summary />
        }
        .into_any(),

        OmegaEvent::PauseRequested(_) => view! {
            <span class="block-label">"pause_requested"</span>
        }
        .into_any(),

        OmegaEvent::TurnPaused(_) => view! {
            <span class="block-label">"turn_paused"</span>
        }
        .into_any(),

        OmegaEvent::TurnContinued(e) => view! {
            <span class="block-label">"turn_continued"</span>
            <span class="block-body">{format!("mode: {:?}", e.mode)}</span>
        }
        .into_any(),
    }
}

// ---------------------------------------------------------------------------
// LLM-response block (Phase 3.10 TODO-A)
// ---------------------------------------------------------------------------

/// One `llm_response` row with four affordances:
///
/// 1. Stop reason inline in the label (`assistant  (end_turn)`).
/// 2. `[context]` button — opens the context modal for the response's
///    `context_hash`.
/// 3. `[payload]` button — opens the text modal with the full event JSON.
/// 4. `[thinking]` button — visible only when `thinking` is non-empty;
///    opens the text modal with the thinking text; always the leftmost button.
///
/// Usage line shows all four token buckets including the cache breakdown
/// required by TODO-A-5 (BUG-C regression detector).
/// Mutations skipped: `has_thinking` bool-inversion requires DOM to observe;
/// thinking-block display is verified by the e2e harness.
#[mutants::skip]
#[component]
fn LlmResponseBlock(event: omega_protocol::events::LlmResponseEvent) -> impl IntoView {
    let context_modal =
        use_context::<ContextModalState>().expect("ContextModalState must be provided");
    let text_modal =
        use_context::<TextModalState>().expect("TextModalState must be provided");

    // Extract all fields before any moves into closures.
    let stop_reason = event.stop_reason.clone();
    let context_hash = event.context_hash.clone();

    let thinking_text = event.thinking.clone().unwrap_or_default();
    let has_thinking = !thinking_text.is_empty();

    let cache_read = event.usage.cache_read_input_tokens.unwrap_or(0);
    let cache_write = event.usage.cache_creation_input_tokens.unwrap_or(0);
    let usage_line = format!(
        "in: {}  out: {}  cache_read: {}  cache_write: {}  ({})",
        event.usage.input_tokens,
        event.usage.output_tokens,
        cache_read,
        cache_write,
        event.stop_reason,
    );

    // Serialise *before* the text move so the whole event is captured.
    let event_json = serde_json::to_string_pretty(&event)
        .unwrap_or_else(|_| "{}".to_owned());

    // The outer `<div data-testid="leptos-assistant-text">` wraps
    // MarkdownBody so existing Playwright selectors still work after
    // Phase 3.6 swapped the inner `<pre>` for the markdown surface.
    let text = event.text.unwrap_or_default();

    view! {
        <div class="block-label-row">
            <span class="block-label">
                "assistant"
                <span class="block-stop-reason">
                    {format!("  ({stop_reason})")}
                </span>
            </span>
            <Show when=move || has_thinking fallback=|| ().into_any()>
                <button
                    class="block-label-row-btn"
                    data-testid="leptos-llm-response-thinking"
                    on:click={
                        let t = thinking_text.clone();
                        move |_| text_modal.open("thinking", t.clone())
                    }
                >
                    "thinking"
                </button>
            </Show>
            <button
                class="block-label-row-btn"
                data-testid="leptos-llm-response-context"
                on:click=move |_| context_modal.open_hash(context_hash.clone())
            >
                "context"
            </button>
            <button
                class="block-label-row-btn"
                data-testid="leptos-llm-response-payload"
                on:click=move |_| text_modal.open("llm_response payload", event_json.clone())
            >
                "payload"
            </button>
        </div>
        <div data-testid="leptos-assistant-text">
            <MarkdownBody text=text />
        </div>
        <span class="block-meta" data-testid="leptos-assistant-usage">{usage_line}</span>
    }
}

// ---------------------------------------------------------------------------
// Tool-call block (TODO-C)
// ---------------------------------------------------------------------------

/// One `tool_call` row — Phase 3.10 TODO-C.
///
/// Label is the tool name (not the literal string `"tool_call"`).
/// When `corr` is `Some(n)`, the 1-based integer *n* appears as a
/// superscript correlation hint so the user can pair calls with
/// their results. A 2-line / 200-byte JSON preview appears inline;
/// an `[input]` button opens the full JSON in a [`TextModal`].
#[component]
fn ToolCallBlock(
    event: omega_protocol::events::ToolCallEvent,
    #[prop(optional_no_strip)] corr: Option<usize>,
) -> impl IntoView {
    let text_modal =
        use_context::<TextModalState>().expect("TextModalState must be provided");

    let name = event.name.clone();

    let full_input = serde_json::to_string_pretty(&event.input)
        .unwrap_or_else(|_| "{}".to_owned());
    // 2-line / 200-byte preview; full JSON reachable via the input modal.
    let preview = truncate_preview(&full_input, 2, 200)
        .unwrap_or_else(|| full_input.clone());
    let full_for_modal = full_input;
    let modal_title = format!("tool_call: {name}");

    view! {
        <div class="block-label-row">
            <span class="block-label">
                <span data-testid="leptos-tool-name">{name.clone()}</span>
                {corr.map(|n| view! { <sup class="block-tool-id">{n.to_string()}</sup> })}
            </span>
            <button
                class="block-label-row-btn"
                data-testid="leptos-tool-call-input"
                on:click=move |_| text_modal.open(modal_title.clone(), full_for_modal.clone())
            >
                "input"
            </button>
        </div>
        <pre class="block-tool-input" data-testid="leptos-tool-input">{preview}</pre>
    }
}

// ---------------------------------------------------------------------------
// LLM-call block (TODO-B: label-row layout, context + payload buttons)
// ---------------------------------------------------------------------------

/// One `llm_call` row — Phase 3.10 TODO-B.
///
/// Label row (flex `.block-label-row`) contains:
///   `llm_call` label · ctx-count + bytes meta · `[context]` button · `[payload]` button
///
/// The `[context]` button opens the [`ContextModal`] overlay.
/// The `[payload]` button opens a [`TextModal`] with
/// cache_breakpoint_index, request_bytes, and the request_summary JSON.
///
/// The old native `<details>` expander (Phase 3.5) is removed
/// and replaced by the payload modal so the inline block stays
/// compact.
#[component]
fn LlmCallBlock(event: omega_protocol::events::LlmCallEvent) -> impl IntoView {
    let context_modal = use_context::<ContextModalState>()
        .expect("ContextModalState must be provided");
    let text_modal =
        use_context::<TextModalState>().expect("TextModalState must be provided");

    // Inline meta: ctx count · bytes (shown on the label row).
    let inline_meta = format!(
        "{} ctx · {} bytes",
        event.context_hashes.len(),
        event.request_bytes,
    );

    // Payload text: request_bytes + request_summary JSON.
    // cache_control placement is visible inside request_summary itself
    // (system/tools/messages labels each note their ephemeral marker).
    let request_summary_str = event.request_summary.as_ref().map_or_else(
        || "(not available)".to_owned(),
        |v| serde_json::to_string_pretty(v).unwrap_or_else(|_| "(unrenderable)".to_owned()),
    );
    let payload_text = format!(
        "request_bytes: {}\n\n--- request_summary ---\n{}",
        event.request_bytes,
        request_summary_str,
    );

    // Clone the event for the context modal; the payload text is moved.
    let event_for_ctx = event;

    view! {
        <div class="block-label-row">
            <span class="block-label">"llm_call"</span>
            <span class="block-meta" data-testid="leptos-llm-call-summary">{inline_meta}</span>
            <button
                class="block-label-row-btn"
                data-testid="leptos-llm-call-open-modal"
                on:click=move |_| context_modal.open(event_for_ctx.clone())
            >
                "context"
            </button>
            <button
                class="block-label-row-btn"
                data-testid="leptos-llm-call-payload"
                on:click=move |_| text_modal.open("llm_call payload", payload_text.clone())
            >
                "payload"
            </button>
        </div>
    }
}

/// One `tool_result` row — Phase 3.10 TODO-C.
///
/// * Label is the tool name (not `"tool_result"`).
/// * When `corr` is `Some(n)`, the same integer shown on the
///   matching [`ToolCallBlock`] appears as a superscript.
/// * Inline preview is truncated to the first 2 lines / 200 bytes.
/// * A `[payload]` button opens a [`TextModal`] with the full output.
/// * The old `[show more]` toggle and the `duration_ms` meta line are
///   removed from the inline view; duration appears in the modal title.
#[component]
fn ToolResultBlock(
    event: omega_protocol::events::ToolResultEvent,
    #[prop(optional_no_strip)] corr: Option<usize>,
) -> impl IntoView {
    let text_modal =
        use_context::<TextModalState>().expect("TextModalState must be provided");

    let name = event.name.clone();
    let full = event.output.clone();
    // 2-line / 200-byte inline preview; full output reachable via the payload modal.
    let preview = truncate_preview(&full, 2, 200)
        .unwrap_or_else(|| full.clone());
    let modal_title = format!("{name}  ·  {}ms", event.duration_ms);
    let full_for_modal = full;

    view! {
        <div class="block-label-row">
            <span class="block-label" data-testid="leptos-tool-result-name">
                {name}
                {corr.map(|n| view! { <sup class="block-tool-id">{n.to_string()}</sup> })}
            </span>
            <button
                class="block-label-row-btn"
                data-testid="leptos-tool-result-payload"
                on:click=move |_| text_modal.open(modal_title.clone(), full_for_modal.clone())
            >
                "payload"
            </button>
        </div>
        <pre class="block-body" data-testid="leptos-tool-result-body">{preview}</pre>
    }
}

// ---------------------------------------------------------------------------
// MarkdownBody (Phase 3.6)
// ---------------------------------------------------------------------------

/// Wire-shape helper: the language tag pulldown-cmark emits as the
/// `class=` attribute on the `<code>` element. Pure; mutation-tested
/// via `tests/snapshots.rs` and the markdown unit tests.
///
/// Returns the bare language string (e.g. `"mermaid"` for
/// `class="language-mermaid"`), or `None` for code blocks that have
/// no language tag.
#[must_use]
pub fn language_from_class(class_attr: &str) -> Option<&str> {
    // pulldown-cmark emits `class="language-foo"` for ``` foo blocks.
    // Multiple classes are space-separated; the language one is
    // always the `language-*` token.
    class_attr
        .split_ascii_whitespace()
        .find_map(|tok| tok.strip_prefix("language-"))
}

/// Decide whether a fenced-code language tag should be diff-rendered.
/// Mirrors `App.tsx::enhanceCodeBlocks`'s
/// `cls.includes("language-diff") || cls.includes("language-patch")`
/// check. Pure; mutation-tested.
#[must_use]
pub fn is_diff_language(lang: &str) -> bool {
    lang == "diff" || lang == "patch"
}

/// Decide whether a fenced-code language tag should be lazy-rendered
/// as a Mermaid diagram. Pure; mutation-tested.
#[must_use]
pub fn is_mermaid_language(lang: &str) -> bool {
    lang == "mermaid"
}

/// Render markdown text as a `<div class="block-body md-body">`.
/// Mirrors `App.tsx::MdBody` exactly:
///
/// 1. `inner_html` is set from [`markdown::render_to_html`] (raw
///    HTML in source is escaped by the rendering pipeline).
/// 2. After mount, an `Effect` walks the rendered DOM, marks
///    mermaid blocks for lazy rendering, applies diff colouring on
///    `language-diff` / `language-patch` blocks, and asks the JS
///    shim to add copy buttons + render mermaid.
///
/// The post-mount mutation surface (DOM walking, mermaid invocation)
/// is JS-interop and lives behind a `cfg(target_arch = "wasm32")`
/// gate; the host build compiles it as a no-op so the snapshot
/// tests render the static HTML straight from `inner_html`.
#[component]
pub fn MarkdownBody(text: String) -> impl IntoView {
    let html = markdown::render_to_html(&text);
    let node_ref: NodeRef<html::Div> = NodeRef::new();

    Effect::new(move |_| {
        if let Some(_el) = node_ref.get() {
            #[cfg(target_arch = "wasm32")]
            {
                use wasm_bindgen::JsCast as _;
                let el_ref: &web_sys::Element = _el.unchecked_ref();
                enhance_md_body(el_ref);
                js_add_copy_buttons(el_ref);
                if let Err(err) = js_render_mermaid(el_ref) {
                    leptos::logging::error!("renderMermaid failed: {err:?}");
                }
            }
        }
    });

    view! {
        <div
            class="block-body md-body"
            data-testid="md-body"
            inner_html=html
            node_ref=node_ref
        />
    }
}

/// Walk every `<pre>` in `container` once and apply the SolidJS
/// UI's post-mount enhancements:
///
/// * `language-mermaid` — add `mermaid-pending` class and stash the
///   raw source in `data-mermaid-source` so the JS shim can replace
///   the `<pre>` with the rendered SVG wrapper.
/// * `language-diff` / `language-patch` — replace `<code>.innerHTML`
///   with the per-line span output of [`render_diff_html`] and add
///   the `diff-block` marker class + `data-testid="diff-block"` to
///   the parent `<pre>`.
/// * Any other language — leave alone; the JS shim's
///   `addCopyButtons` injects the copy button after the wasm side
///   returns.
///
/// Idempotent via `data-enhanced="1"`. Skipped on host targets.
/// DOM-only function; skip mutation to avoid undetectable DOM-state mutations.
/// Covered by e2e harness markdown / mermaid / diff tests.
#[cfg(target_arch = "wasm32")]
#[mutants::skip]
fn enhance_md_body(container: &web_sys::Element) {
    use wasm_bindgen::JsCast as _;
    let pres = container.query_selector_all("pre").unwrap_or_else(|err| {
        leptos::logging::error!("querySelectorAll(pre) failed: {err:?}");
        web_sys::NodeList::from(wasm_bindgen::JsValue::NULL).unchecked_into()
    });
    let len = pres.length();
    for i in 0..len {
        let Some(node) = pres.item(i) else { continue };
        let Ok(pre) = node.dyn_into::<web_sys::HtmlElement>() else {
            continue;
        };
        // Idempotency guard — if the JS shim already enhanced this
        // block (copy button added), don't double-process.
        if pre.dataset().get("mdEnhanced").is_some() {
            continue;
        }
        let _ = pre.dataset().set("mdEnhanced", "1");

        let code = pre.query_selector("code").ok().flatten();
        let lang = code.as_ref().and_then(|c| {
            let cls = c.class_name();
            language_from_class(&cls).map(str::to_owned)
        });

        let raw_text = code
            .as_ref()
            .map(|c| c.text_content().unwrap_or_default())
            .unwrap_or_else(|| pre.text_content().unwrap_or_default());

        match lang.as_deref() {
            Some(l) if is_mermaid_language(l) => {
                let _ = pre.dataset().set("mermaidSource", &raw_text);
                pre.class_list()
                    .add_1("mermaid-pending")
                    .unwrap_or_else(|err| {
                        leptos::logging::error!("add mermaid-pending failed: {err:?}");
                    });
            }
            Some(l) if is_diff_language(l) => {
                if let Some(c) = code {
                    c.set_inner_html(&render_diff_html(&raw_text));
                }
                pre.class_list().add_1("diff-block").unwrap_or_else(|err| {
                    leptos::logging::error!("add diff-block failed: {err:?}");
                });
                pre.set_attribute("data-testid", "diff-block")
                    .unwrap_or_else(|err| {
                        leptos::logging::error!("set data-testid failed: {err:?}");
                    });
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Tests — pure helpers in this module
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]

    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test;

    // --- language_from_class -----------------------------------------

    #[wasm_bindgen_test]
    #[test]
    fn language_from_class_extracts_simple_language() {
        assert_eq!(language_from_class("language-rust"), Some("rust"));
    }

    #[wasm_bindgen_test]
    #[test]
    fn language_from_class_handles_no_language() {
        assert_eq!(language_from_class(""), None);
        assert_eq!(language_from_class("hljs"), None);
    }

    #[wasm_bindgen_test]
    #[test]
    fn language_from_class_finds_in_multi_class() {
        assert_eq!(
            language_from_class("hljs language-mermaid foo"),
            Some("mermaid"),
        );
    }

    #[wasm_bindgen_test]
    #[test]
    fn language_from_class_strips_only_first_token() {
        // Defensive: pulldown-cmark always uses a single token; pin
        // that we don't accidentally combine multiple language- prefixes.
        assert_eq!(
            language_from_class("language-foo language-bar"),
            Some("foo"),
        );
    }

    #[wasm_bindgen_test]
    #[test]
    fn language_from_class_does_not_match_partial_prefix() {
        // A class `language` (no dash) does not start with the full
        // `language-` prefix so it doesn't strip.
        assert_eq!(language_from_class("language"), None);
    }

    // --- is_diff_language --------------------------------------------

    #[wasm_bindgen_test]
    #[test]
    fn is_diff_language_matches_diff() {
        assert!(is_diff_language("diff"));
    }

    #[wasm_bindgen_test]
    #[test]
    fn is_diff_language_matches_patch() {
        assert!(is_diff_language("patch"));
    }

    #[wasm_bindgen_test]
    #[test]
    fn is_diff_language_rejects_other() {
        assert!(!is_diff_language(""));
        assert!(!is_diff_language("rust"));
        assert!(!is_diff_language("DIFF")); // case-sensitive on purpose
        assert!(!is_diff_language("diff-tree"));
    }

    // --- is_mermaid_language ----------------------------------------

    #[wasm_bindgen_test]
    #[test]
    fn is_mermaid_language_matches() {
        assert!(is_mermaid_language("mermaid"));
    }

    #[wasm_bindgen_test]
    #[test]
    fn is_mermaid_language_rejects_other() {
        assert!(!is_mermaid_language(""));
        assert!(!is_mermaid_language("Mermaid"));
        assert!(!is_mermaid_language("mermaid2"));
        assert!(!is_mermaid_language("flow"));
    }
}


/// Live overlay rendered after the persisted-event list. Two
/// conditional blocks: streaming text (assistant family) and streaming
/// thinking (status family). Each is a plain `<Show>` over the
/// corresponding signal's emptiness; per-keystroke reactivity comes
/// from leptos's signal subscription on the inner text node.
#[component]
fn StreamingTail() -> impl IntoView {
    let store = use_context::<SessionStore>().expect("SessionStore must be provided");

    let assistant_class = css_class_for(EventKind::Assistant);
    let status_class = css_class_for(EventKind::Status);

    view! {
        <Show
            when=move || !store.streaming_text.with(String::is_empty)
            fallback=|| ().into_any()
        >
            <div
                class=format!("{assistant_class} block-streaming")
                data-testid="leptos-streaming-text"
                data-event-kind="assistant"
            >
                <span class="block-label">"assistant (streaming)"</span>
                <pre class="block-body">{move || store.streaming_text.get()}</pre>
            </div>
        </Show>
        <Show
            when=move || !store.streaming_thinking.with(String::is_empty)
            fallback=|| ().into_any()
        >
            <div
                class=format!("{status_class} block-streaming")
                data-testid="leptos-streaming-thinking"
                data-event-kind="status"
            >
                <span class="block-label">"thinking (streaming)"</span>
                <pre class="block-body">{move || store.streaming_thinking.get()}</pre>
            </div>
        </Show>
    }
}
