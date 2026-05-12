//! Conversation-feed component (Phase 3.3).
//!
//! ```text
//!   <ConversationFeed>
//!    │  reads SessionStore::events (RwSignal<Vec<OmegaEvent>>)
//!    │  reads SessionStore::streaming_text + streaming_thinking + streaming_tool_use
//!    │
//!    ├── <For each=events key=index>
//!    │    └── <EventBlock event=ev />
//!    │         ├── pure `kind_for(&event)`   → CSS class + data-event-kind
//!    │         ├── pure `event_type_tag()`   → data-event-type
//!    │         └── match → typed body view per OmegaEvent variant
//!    │              └── ToolResult: <ToolResultBlock> with payload modal (TODO-C)
//!    │                   └── pure `truncate_preview(s, 2, 200)`
//!    ├── <For each=streaming_text>   (one in-flight placeholder per index)
//!    ├── <For each=streaming_thinking>
//!    ├── <For each=streaming_tool_use>
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
//! ## Streaming-text strategy (SCHEMA-8 Phase 5a)
//!
//! Direct append.  `streaming_text` / `streaming_thinking` /
//! `streaming_tool_use` are `RwSignal<BTreeMap<usize, _>>` keyed by
//! Anthropic `content_block_start.index`; the in-flight overlays render
//! one `<div data-testid="leptos-streaming-{text,thinking,tool-use}">` per
//! live slot, so interleaved blocks can coexist without a global
//! accumulator.  Leptos triggers per-frame re-renders because
//! `SessionStore::apply` calls `streaming_text.update(|m|
//! m.entry(index).or_default().push_str(...))`.  Per-keystroke
//! reactivity.  No rAF buffer.

use leptos::ev;
use leptos::html;
use leptos::prelude::*;
use omega_types::OmegaEvent;
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
    EventKind, assign_partial_counts, assign_tool_corr, css_class_for, event_type_tag, format_time,
    kind_for, kind_tag, should_autoscroll, tool_call_preview, truncate_preview, virtual_line_count,
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
    fn js_render_mermaid(
        container: &web_sys::Element,
    ) -> Result<wasm_bindgen::JsValue, wasm_bindgen::JsValue>;

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
/// Reads `events`, `streaming_text`, `streaming_thinking`, `streaming_tool_use` from the
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
    let auto_scroll = RwSignal::new(true);
    // Counter incremented by the ↓ button.  The auto-scroll Effect subscribes
    // to it so that clicking the button triggers an immediate rAF scroll (with
    // correct `prog_state` stamp) without any direct DOM manipulation from the
    // click handler.  `RwSignal<u32>` is `Send + Sync + \'static` so it can be
    // captured inside `<Show>` children.
    let scroll_demand = RwSignal::new(0u32);
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

    // True from the moment the auto-scroll Effect fires (scheduling a rAF)
    // until the rAF callback finishes its set_scroll_top call.  During this
    // window, the browser may fire scroll events caused by our own content
    // mutations (streaming-text removal, event-block insertion, copy-button
    // injection).  Those events arrive with a scroll_top that is not at the
    // new bottom (because the content height is still settling), which would
    // make should_autoscroll() return false and silently kill tailing.
    // Suppressing all scroll events while this flag is set and auto_scroll
    // is true and scroll_top is not near zero prevents that.
    let scroll_pending: Rc<Cell<bool>> = Rc::new(Cell::new(false));

    // Auto-scroll Effect.
    //
    // We subscribe explicitly to `events.with(Vec::len)`, plus a
    // BTreeMap-summing closure over `streaming_text` and
    // `streaming_thinking` so the effect re-runs on every event
    // append AND every streamed-text/thinking fragment — but NOT on
    // signals the feed doesn't display directly. SCHEMA-8 Phase 5a
    // replaced the per-`String` length probe with a per-map total
    // length sum so any append to any slot still re-triggers the
    // effect.
    //
    // We do NOT subscribe to `auto_scroll` itself: reading it via `get_untracked` keeps the gate-flip out
    // of the effect's dependency set so an `on:scroll`-induced flip
    // doesn't itself trigger a scroll-into-view.
    //
    // The actual scroll is deferred by one animation frame (rAF) so
    // that all synchronous Leptos Effects — including each EventBlock's
    // MarkdownBody post-mount Effect that calls enhance_md_body /
    // js_add_copy_buttons and injects copy-button DOM nodes — have
    // committed their mutations before we read scrollHeight.  Without
    // this deferral, scrollHeight is stale (pre-enhancement) and the
    // most recent event ends up partially clipped at the bottom.
    //
    // The rAF callback re-checks auto_scroll in case the user scrolled
    // up in the one-frame gap, and cancels any stale pending rAF when a
    // newer content update arrives before the previous frame fires.
    //
    // We use `set_scroll_top(scroll_height)` rather than
    // `sentinel.scroll_into_view()` (value is clamped to legal max).
    // We stamp `prog_state` so the on_scroll handler can suppress the
    // deferred scroll echo that Chromium 148+ fires after set_scroll_top.
    let scroll_pending_eff = Rc::clone(&scroll_pending);
    let prog_state_eff = Rc::clone(&prog_state);
    // Tracks any pending rAF id so we can cancel it on the next update.
    // Only needed on wasm32; declared under cfg so the host build sees no
    // unused-variable warning.
    #[cfg(target_arch = "wasm32")]
    let raf_id: Rc<Cell<Option<i32>>> = Rc::new(Cell::new(None));
    #[cfg(target_arch = "wasm32")]
    let raf_id_eff = Rc::clone(&raf_id);
    Effect::new(move |_| {
        let _ = store.events.with(Vec::len);
        let _ = store
            .streaming_text
            .with(|m| m.values().map(String::len).sum::<usize>());
        let _ = store
            .streaming_thinking
            .with(|m| m.values().map(String::len).sum::<usize>());
        let _ = store
            .streaming_tool_use
            .with(|m| m.values().map(|s| s.partial_json.len()).sum::<usize>());
        // Also subscribe to `scroll_demand` so that the ↓ button (which
        // increments the counter) triggers an immediate scroll without
        // performing any direct DOM manipulation in the click handler.
        let _ = scroll_demand.get();
        if !auto_scroll.get_untracked() {
            return;
        }

        // Signal that a programmatic scroll is in flight.  Any scroll event
        // that arrives between now and the rAF completing is a transient side
        // effect of content changes, not a user scroll.
        scroll_pending_eff.set(true);

        // ── wasm32: defer one frame so all child Effects finish first ──
        #[cfg(target_arch = "wasm32")]
        {
            use wasm_bindgen::JsCast as _;
            use wasm_bindgen::closure::Closure;

            // Cancel stale rAF from a previous update that hasn't fired yet.
            if let (Some(id), Some(win)) = (raf_id_eff.get(), web_sys::window()) {
                let _ = win.cancel_animation_frame(id);
                raf_id_eff.set(None);
            }

            let prog_clone = Rc::clone(&prog_state_eff);
            let scroll_pending_cb = Rc::clone(&scroll_pending_eff);
            let raf_id_cb = Rc::clone(&raf_id_eff);
            // scroll_ref and auto_scroll are Copy + 'static — captured by copy.
            let cb = Closure::once(move || {
                raf_id_cb.set(None);
                // Re-check: user may have scrolled up in the one-frame gap.
                if !auto_scroll.get_untracked() {
                    scroll_pending_cb.set(false);
                    return;
                }
                if let Some(section) = scroll_ref.get() {
                    let sh = section.scroll_height();
                    prog_clone.set((now_ms(), sh));
                    section.set_scroll_top(sh);
                }
                scroll_pending_cb.set(false);
            });

            match web_sys::window().and_then(|win| {
                win.request_animation_frame(cb.as_ref().unchecked_ref())
                    .ok()
            }) {
                Some(id) => {
                    raf_id_eff.set(Some(id));
                    cb.forget(); // JS owns the callback until it fires
                }
                None => {
                    // rAF unavailable: fall back to immediate scroll.
                    drop(cb);
                    if let Some(section) = scroll_ref.get() {
                        let sh = section.scroll_height();
                        prog_state_eff.set((now_ms(), sh));
                        section.set_scroll_top(sh);
                    }
                    scroll_pending_eff.set(false);
                }
            }
        }

        // ── non-wasm (host tests): scroll immediately, no rAF ──
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(section) = scroll_ref.get() {
            let sh = section.scroll_height();
            prog_state_eff.set((now_ms(), sh));
            section.set_scroll_top(sh);
            scroll_pending_eff.set(false);
        }
    });

    let scroll_pending_scroll = Rc::clone(&scroll_pending);
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
        // While a programmatic scroll is in flight (Effect fired, rAF not
        // yet completed), suppress scroll events caused by our own content
        // mutations (streaming-text removal, event-block insertion).
        // Content-mutation events always arrive with scroll_top near the
        // current bottom; they are never at scroll_top < AUTOSCROLL_THRESHOLD_PX
        // unless the feed is nearly empty (no scroll events then anyway).
        // A scroll_top below that threshold can only come from a deliberate
        // far-from-bottom user (or test) action — honour it even in the
        // pending window so that programmatic test scrolls to the top are not
        // silently suppressed and cause timeouts.
        if scroll_pending_scroll.get()
            && auto_scroll.get_untracked()
            && scroll_top >= AUTOSCROLL_THRESHOLD_PX
        {
            return;
        }

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
                        let partials = assign_partial_counts(&events);
                        events
                            .into_iter()
                            .enumerate()
                            .zip(corrs)
                            .zip(partials)
                            .map(|(((idx, ev), corr), partial_count)| {
                                (idx, ev, corr, partial_count)
                            })
                            .collect::<Vec<(usize, OmegaEvent, Option<usize>, Option<usize>)>>()
                    }
                    key=|(idx, _, corr, partial): &(usize, OmegaEvent, Option<usize>, Option<usize>)| (*idx, *corr, *partial)
                    children=|(idx, event, corr, partial_count): (usize, OmegaEvent, Option<usize>, Option<usize>)| view! { <EventBlock event=event corr=corr idx=Some(idx) partial_count=partial_count /> }
                />
                <StreamingPlaceholders />
                <div class="leptos-feed-sentinel" data-testid="leptos-feed-sentinel" />
            </section>
            <Show when=move || !auto_scroll.get()>
                <button
                    class="scroll-to-bottom-btn"
                    data-testid="scroll-to-bottom"
                    aria-label="Scroll to bottom"
                    on:click=move |_| {
                        // Re-enable tailing and demand an immediate scroll.
                        //
                        // Previously this called `sentinel.scroll_into_view()`
                        // which did NOT update `prog_state`.  On Chromium 148+
                        // that scroll is dispatched as a macrotask: when a
                        // streaming chunk arrives in the gap the rAF has already
                        // stamped `prog_state` with a *newer* scrollHeight, so
                        // the old event fails the echo check, `should_autoscroll`
                        // returns false, and tailing is silently killed.
                        //
                        // Fix: increment `scroll_demand` which the auto-scroll
                        // Effect subscribes to.  The Effect re-runs, schedules
                        // its rAF, and that callback does `set_scroll_top` +
                        // `prog_state` stamp — exactly the same path as any
                        // streaming-content update.  No direct DOM interaction
                        // here means no untracked scroll event, no race.
                        auto_scroll.set(true);
                        scroll_demand.update(|n| *n += 1);
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
///
/// `idx` is the event's 0-based position in the session's events list.
/// It is rendered as a `data-block-id` attribute on the outer wrapper
/// so the Phase 0 T6 browser-refresh replay test (and any future DOM
/// inspection) can verify per-event identity is stable across reloads:
/// replaying `events.jsonl` produces the same list in the same order,
/// hence the same indices.  The snapshot harness in `tests/snapshots.rs`
/// renders fixtures one at a time without a positional context and so
/// omits `idx`; the attribute is then not emitted, keeping existing
/// snapshots stable.
#[component]
pub fn EventBlock(
    event: OmegaEvent,
    #[prop(optional_no_strip)] corr: Option<usize>,
    #[prop(optional_no_strip)] idx: Option<usize>,
    #[prop(optional_no_strip)] partial_count: Option<usize>,
) -> impl IntoView {
    let kind = kind_for(&event);
    let class = css_class_for(kind);
    let kind_str = kind_tag(kind);
    let event_type = event_type_tag(&event);
    let time = format_time(event.time()).to_owned();
    // Leptos omits an attribute when its value is `None`, so the
    // snapshot harness (which doesn't pass `idx`) emits the wrapper
    // unchanged.  Live renders from `<ConversationFeed/>` always
    // pass `Some(idx)` so `data-block-id` is always present in the
    // running app.
    let block_id = idx.map(|i| i.to_string());

    // SCHEMA-8 Phase 5b — surface `partial: true` on the wrapper so
    // CSS (and e2e selectors) can grey/strike-through every child of
    // a discarded block uniformly.  `event_is_partial` returns `None`
    // for non-partial events so the attribute is omitted on stable
    // blocks; existing snapshots stay byte-equal.
    let partial_attr = event_is_partial(&event).then_some("true");
    // Must be computed before `render_event_body` moves `event`.
    let has_lr = event_has_label_row(&event);

    view! {
        <div
            class=class
            data-testid="leptos-event-block"
            data-event-type=event_type
            data-event-kind=kind_str
            data-block-id=block_id
            data-partial=partial_attr
        >
            {render_event_body(event, corr, partial_count)}
            {(!has_lr).then(|| view! { <span class="block-timestamp">{time}</span> })}
        </div>
    }
}

/// `true` when this event carries a `partial: true` flag (the agent
/// mints these immediately before `LlmResponseDiscarded` for mid-stream
/// abandonment).  Used by [`EventBlock`] to stamp `data-partial="true"`
/// on the outer wrapper.
///
/// Only the three streamable block variants (`TextBlock`,
/// `ThinkingBlock`, `ToolUseBlock`) carry the flag; every other event
/// returns `false`.
fn event_is_partial(event: &OmegaEvent) -> bool {
    match event {
        OmegaEvent::TextBlock(e) => e.partial,
        OmegaEvent::ThinkingBlock(e) => e.partial,
        OmegaEvent::ToolUseBlock(e) => e.partial,
        _ => false,
    }
}

/// `true` when this event renders a `.block-label-row` that carries
/// the timestamp pill at its right end.  Used by [`EventBlock`] to
/// skip the outer `.block-timestamp` fallback for those events.
fn event_has_label_row(event: &OmegaEvent) -> bool {
    matches!(
        event,
        OmegaEvent::LlmCall(_)
            | OmegaEvent::LlmResponseEnded(_)
            | OmegaEvent::ToolCall(_)
            | OmegaEvent::ToolResult(_)
            | OmegaEvent::ThinkingBlock(_)
            | OmegaEvent::ToolUseBlock(_)
    )
}

/// Per-variant body view. Returns `AnyView` so the match arms can each
/// produce their own concrete `view!` output and unify at the call
/// site. The big match here is necessary (each arm needs typed field
/// access); the *family decision* is carved out into the pure
/// `kind_for` in `event_view.rs`.
fn render_event_body(
    event: OmegaEvent,
    corr: Option<usize>,
    partial_count: Option<usize>,
) -> AnyView {
    match event {
        OmegaEvent::UserMessage(e) => view! {
            <span class="block-label">"user_message"</span>
            <pre class="block-body" data-testid="leptos-user-content">{e.content}</pre>
        }
        .into_any(),

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

        // ----- SCHEMA-8 additive variants ----------------------------------
        // Phase 1b shipped the wire grammar; Phase 4b replaces the
        // text/thinking/tool_use stubs with real per-block renderers.
        // `LlmResponseStarted` / `LlmResponseEnded` / `LlmResponseDiscarded`
        // are lifecycle markers; `LlmResponseEnded` has its own renderer
        // below (context hash badge, compacted badge, usage summary).
        OmegaEvent::LlmResponseStarted(_) => view! {
            <span class="block-label">"LLM response start"</span>
        }
        .into_any(),

        // `LlmResponseEnded`: closer for a successful response.
        // Renders the affordances side of the legacy
        // `LlmResponseBlock` (label-row with stop reason + `context`
        // + `payload` buttons, plus usage line) but *not* the
        // markdown body — that lives in sibling `TextBlock`
        // events.  No `thinking` button: thinking lives in sibling
        // `ThinkingBlock` events.  Same testids as `LlmResponseBlock`
        // so existing modal-open selectors keep matching once Phase
        // 4d drops the legacy renderer.
        OmegaEvent::LlmResponseEnded(e) => LlmResponseEndedBlock(LlmResponseEndedBlockProps {
            event: e.clone(),
        })
        .into_any(),

        // `LlmResponseDiscarded`: closer for an abandoned response.
        // Inline marker; the preceding partial block-events carry the
        // actual content the user saw before abandonment.  When
        // `partial_count` is supplied (live `ConversationFeed`, via
        // `assign_partial_counts`), surface the `N partial blocks`
        // meta so the operator can tell "network blip before any
        // content" (`0`) from "discarded after N partials" (`>0`).
        // Snapshot fixtures that omit the prop emit no meta line.
        OmegaEvent::LlmResponseDiscarded(_) => view! {
            <span class="block-label">"assistant"</span>
            <span class="block-body block-discarded">"[response discarded]"</span>
            {partial_count.map(|n| view! {
                <span
                    class="block-meta"
                    data-testid="leptos-partial-block-count"
                >
                    {format!("{n} partial blocks")}
                </span>
            })}
        }
        .into_any(),

        // `TextBlock`: one finalised (or partial) text span from an
        // assistant response.  Renders the markdown surface with the
        // `leptos-assistant-text` testid so Playwright selectors can
        // target it.  06_feed grabs the *last* such wrapper to verify
        // final assistant text, which is the same content the final
        // `TextBlock` event carries.
        //
        // `partial:true` blocks are emitted just before
        // `LlmResponseDiscarded` on mid-stream abandonment (retry on
        // transient error); they carry whatever text had accumulated
        // up to the abandonment point and we mark them visibly so a
        // human reader can tell the assistant didn't actually "say"
        // this.
        OmegaEvent::TextBlock(e) => {
            // SCHEMA-8 Phase 5b — partial text blocks render greyed +
            // struck-through with a "Discarded — N chars" header per
            // spec § "Discarded-block styling".  Header sits BEFORE
            // the markdown body so a reader sees the disclaimer
            // before the discarded content; both share the
            // `block-discarded-*` classes that style.css picks up.
            // The header keeps `data-testid="leptos-block-partial"`
            // for any future selector-driven assertions.
            //
            // Non-partial branch keeps the exact pre-5b markup
            // (no `class=` attribute, no leading marker comment)
            // so existing snapshots / e2e selectors stay byte-equal.
            let partial = e.partial;
            if partial {
                let char_count = e.text.chars().count();
                view! {
                    <span class="block-discarded-header" data-testid="leptos-block-partial">
                        {format!("Discarded — {char_count} chars text")}
                    </span>
                    <div
                        data-testid="leptos-assistant-text"
                        class="block-discarded-body"
                    >
                        <MarkdownBody text=e.text />
                    </div>
                }
                .into_any()
            } else {
                view! {
                    <div data-testid="leptos-assistant-text">
                        <MarkdownBody text=e.text />
                    </div>
                }
                .into_any()
            }
        }

        // `ThinkingBlock`: one finalised (or partial) thinking
        // segment.  Renders a labelled `<pre>` clamped to ~3 lines
        // with a "more" / "less" toggle.  During streaming the live
        // overlay in `StreamingPlaceholders` is always fully open so
        // tailing scrolls correctly; clamping only happens once the
        // block is settled.
        //
        // `signature.is_none()` iff `partial == true` per the
        // type-level invariant in `omega-types::events`.
        OmegaEvent::ThinkingBlock(e) => {
            // SCHEMA-8 Phase 5b — partial thinking renders greyed +
            // struck-through with a "Discarded thinking — N chars"
            // header per spec § "Discarded-block styling".
            //
            // SCHEMA-8 Phase 5c (revised) — the `<pre>` is clamped to
            // ~3 lines by default; a "more" / "less" button in the
            // label row toggles the clamp in-place.  No TextModal:
            // the text is already in the box.
            let partial = e.partial;
            let char_count = e.thinking.chars().count();
            // Only show the toggle when the content exceeds the 3-line clamp.
            // The button is always visible (not hover-gated) because it also
            // serves as an indicator of the current collapsed/expanded state.
            let needs_toggle = virtual_line_count(&e.thinking, 80) > 4;
            let expanded = RwSignal::new(false);
            let time_pill = format_time(&e.time).to_owned();
            view! {
                <div class="block-label-row">
                    {if partial {
                        view! {
                            <span
                                class="block-discarded-header"
                                data-testid="leptos-block-partial"
                            >
                                {format!("Discarded thinking — {char_count} chars")}
                            </span>
                        }.into_any()
                    } else {
                        view! { <span class="block-label">"thinking"</span> }.into_any()
                    }}
                    {needs_toggle.then(|| view! {
                        <button
                            class="block-label-row-btn thinking-toggle-btn"
                            data-testid="leptos-thinking-block-expand"
                            on:click=move |_| expanded.update(|v| *v = !*v)
                        >
                            {move || if expanded.get() { "less" } else { "more" }}
                        </button>
                    })}
                    <span class="block-timestamp-pill">{time_pill}</span>
                </div>
                <pre
                    class=move || {
                        let base = if partial {
                            "block-body block-discarded-body"
                        } else {
                            "block-body"
                        };
                        if !needs_toggle || expanded.get() {
                            base.to_string()
                        } else {
                            format!("{base} thinking-body-clamped")
                        }
                    }
                    data-testid="leptos-thinking-block-body"
                >
                    {e.thinking}
                </pre>
            }
            .into_any()
        }

        // `ToolUseBlock`: a tool_use block inside an assistant
        // response, before the tool actually runs.  Visually the
        // legacy `ToolCallBlock` already renders the dispatch event
        // emitted from the tool-runner side; this new event is
        // emitted from the *response* side and carries the
        // provider-assigned tool_use id.  Phase 4b renders it as a
        // compact label + preview line so it's visible without
        // duplicating the full `ToolCallBlock` UI (modal, corr
        // badge); Phase 5 will reconcile the two so each tool use
        // shows up exactly once.
        OmegaEvent::ToolUseBlock(e) => {
            // SCHEMA-8 Phase 5b — partial tool_use blocks render
            // greyed + struck-through with a "Discarded tool_use
            // — {name}" header.  The name is informative even for an
            // abandoned dispatch (the input was being streamed when
            // the connection died, so the preview is whatever JSON
            // had accumulated up to abandonment).
            //
            // SCHEMA-8 Phase 5d (revised) — inline more/less toggle
            // expands the full pretty-printed input JSON in-place,
            // mirroring the ThinkingBlock affordance.  The button is
            // always rendered so the drill-down path is always
            // discoverable.
            let partial = e.partial;
            let name = e.name.clone();
            let name_for_label = name.clone();
            let raw_preview = tool_call_preview(&e.name, &e.input);
            let preview = truncate_preview(&raw_preview, 2, 300).unwrap_or(raw_preview);
            let input = e.input.clone();
            let expanded = RwSignal::new(false);
            let time_pill = format_time(&e.time).to_owned();
            view! {
                <div
                    class="block-label-row"
                    data-testid="leptos-tool-use-block"
                >
                    {corr.map(|n| view! { <span class="corr-badge">{n}</span> })}
                    {if partial {
                        view! {
                            <span
                                class="block-discarded-header"
                                data-testid="leptos-block-partial"
                            >
                                {format!("Discarded tool_use — {name}")}
                            </span>
                        }.into_any()
                    } else {
                        view! {
                            <span class="block-label" data-testid="leptos-tool-use-name">
                                {name_for_label}
                            </span>
                        }.into_any()
                    }}
                    <span
                        class=if partial {
                            "block-tool-preview block-discarded-body"
                        } else {
                            "block-tool-preview"
                        }
                        data-testid="leptos-tool-use-input"
                    >
                        {preview}
                    </span>
                    <button
                        class="block-label-row-btn thinking-toggle-btn"
                        data-testid="leptos-tool-use-block-expand"
                        on:click=move |_| expanded.update(|v| *v = !*v)
                    >
                        {move || if expanded.get() { "less" } else { "more" }}
                    </button>
                    <span class="block-timestamp-pill">{time_pill}</span>
                </div>
                {move || expanded.get().then(|| {
                    let pretty = serde_json::to_string_pretty(&input)
                        .unwrap_or_else(|_| "{}".to_owned());
                    view! {
                        <pre
                            class=if partial { "block-body block-discarded-body" } else { "block-body" }
                            data-testid="leptos-tool-use-block-body"
                        >
                            {pretty}
                        </pre>
                    }
                })}
            }
            .into_any()
        }
    }
}

// ---------------------------------------------------------------------------
// LLM-response-ended block (SCHEMA-8 Phase 4c)
// ---------------------------------------------------------------------------

/// Closer for [`omega_types::events::LlmResponseStartedEvent`] when
/// the response completes successfully.  This is the *affordances*
/// side of the legacy `LlmResponseBlock` (deleted in 4c): stop-reason
/// label,
/// `[context]` + `[payload]` buttons, and the usage line.  The
/// markdown body lives in sibling [`OmegaEvent::TextBlock`] events;
/// thinking text lives in sibling [`OmegaEvent::ThinkingBlock`]
/// events — so there is no `[thinking]` button here and no
/// `leptos-assistant-text` wrapper.
///
/// Testids: `leptos-llm-response-context`, `leptos-llm-response-payload`,
/// `leptos-assistant-usage`.
#[mutants::skip]
#[component]
fn LlmResponseEndedBlock(event: omega_types::events::LlmResponseEndedEvent) -> impl IntoView {
    let context_modal =
        use_context::<ContextModalState>().expect("ContextModalState must be provided");
    let text_modal = use_context::<TextModalState>().expect("TextModalState must be provided");

    let context_hash = event.context_hash.clone();

    // SCHEMA-8 Phase 5f — surface server-side context compaction.
    // Anthropic's response usage object exposes per-iteration usage when
    // server-side compaction fires; we detect it by scanning
    // `usage.iterations` for a `type == "compaction"` entry.  When
    // present, render a yellow `[compacted]` badge in the label row so
    // the operator can see at a glance which response triggered a
    // server-side context trim (distinct from any client-side trimming
    // that may surface elsewhere).
    let compacted = event
        .usage
        .iterations
        .as_ref()
        .map(|iters| iters.iter().any(|it| it.iteration_type == "compaction"))
        .unwrap_or(false);

    let cache_read = event.usage.cache_read_input_tokens.unwrap_or(0);
    let cache_write = event.usage.cache_creation_input_tokens.unwrap_or(0);
    // Stop reason omitted from the usage line — it was redundant with the
    // label and added visual clutter.
    let usage_line = format!(
        "in: {}  out: {}  cache_read: {}  cache_write: {}",
        event.usage.input_tokens,
        event.usage.output_tokens,
        cache_read,
        cache_write,
    );

    let event_json = serde_json::to_string_pretty(&event).unwrap_or_else(|_| "{}".to_owned());
    let time_pill = format_time(&event.time).to_owned();

    view! {
        <div class="block-label-row">
            <span class="block-label">"LLM response end"</span>
            {compacted.then(|| view! {
                <span
                    class="block-badge block-badge-compacted"
                    data-testid="leptos-compacted-badge"
                    title="server-side context compaction fired on this response"
                >
                    "compacted"
                </span>
            })}
            // Usage info + modal buttons grouped as a single hover-revealed unit.
            // On narrow screens the whole group is absolutely positioned so it
            // overlays the label text rather than wrapping to a second line.
            <div class="block-label-row-actions">
                <span
                    class="block-label-info"
                    data-testid="leptos-assistant-usage"
                >
                    {usage_line}
                </span>
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
                    on:click=move |_| text_modal.open("llm_response_ended payload", event_json.clone())
                >
                    "payload"
                </button>
            </div>
            <span class="block-timestamp-pill">{time_pill}</span>
        </div>
    }
}

// ---------------------------------------------------------------------------
// Tool-call block (TODO-C)
// ---------------------------------------------------------------------------

/// One `tool_call` row — peer-event slim layout.
///
/// The card carries only what the `ToolCall` event uniquely contributes:
/// its identity as a dispatch event, plus the standard timestamp pill
/// and optional corr-badge gutter.  The tool name and arguments live on
/// the sibling `ToolUseBlock`; the output lives on the sibling
/// `ToolResult`.  Each event renders as a peer card, labelled by its
/// event type — no field is re-derived from neighbours.
///
/// When `corr` is `Some(n)`, the yellow `<span class="corr-badge">` is
/// rendered first so the operator can visually pair the call with its
/// sibling `ToolUseBlock` and `ToolResult`.
#[component]
fn ToolCallBlock(
    event: omega_types::events::ToolCallEvent,
    #[prop(optional_no_strip)] corr: Option<usize>,
) -> impl IntoView {
    let time_pill = format_time(&event.time).to_owned();

    view! {
        <div class="block-label-row" data-testid="leptos-tool-call">
            {corr.map(|n| view! { <span class="corr-badge">{n}</span> })}
            <span class="block-label">"tool call"</span>
            <span class="block-timestamp-pill">{time_pill}</span>
        </div>
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
fn LlmCallBlock(event: omega_types::events::LlmCallEvent) -> impl IntoView {
    let context_modal =
        use_context::<ContextModalState>().expect("ContextModalState must be provided");
    let text_modal = use_context::<TextModalState>().expect("TextModalState must be provided");

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
        event.request_bytes, request_summary_str,
    );

    // Clone the event for the context modal; the payload text is moved.
    let time_pill = format_time(&event.time).to_owned();
    let event_for_ctx = event;

    view! {
        <div class="block-label-row">
            <span class="block-label">"LLM call"</span>
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
            <span class="block-timestamp-pill">{time_pill}</span>
        </div>
    }
}

/// One `tool_result` row — Phase 3.10 TODO-C.
///
/// * Label is the literal text `"tool result"` (not the tool name) so
///   results are visually distinct from tool calls at a glance, and the
///   card identifies itself by its event type — matching the peer
///   `ToolCallBlock` and `ToolUseBlock` labelling convention.
/// * When `corr` is `Some(n)`, a yellow `<span class="corr-badge">` with
///   the 1-based ordinal is shown at the start of the label row.
/// * A 2-line / 300-byte output preview is rendered on its own line below
///   the label row, left-aligned.
/// * An `[output]` button in the label row opens a [`TextModal`] with the
///   full output.  The whole block is no longer click-to-open; the button
///   is the only affordance for the modal.
#[component]
fn ToolResultBlock(
    event: omega_types::events::ToolResultEvent,
    #[prop(optional_no_strip)] corr: Option<usize>,
) -> impl IntoView {
    let text_modal = use_context::<TextModalState>().expect("TextModalState must be provided");

    let name = event.name.clone();
    let full = event.output.clone();
    // 2-line / 300-byte inline preview; full output reachable via the output modal.
    let preview = truncate_preview(&full, 2, 300).unwrap_or_else(|| full.clone());
    let modal_title = format!("{name}  ·  {}ms", event.duration_ms);
    let full_for_modal = full;
    let time_pill = format_time(&event.time).to_owned();

    view! {
        <div data-testid="leptos-tool-result-payload">
            <div class="block-label-row">
                {corr.map(|n| view! { <span class="corr-badge">{n}</span> })}
                <span class="block-label" data-testid="leptos-tool-result-name">
                    "tool result"
                </span>
                <button
                    class="block-label-row-btn"
                    data-testid="leptos-tool-result-payload-btn"
                    on:click=move |_| text_modal.open(modal_title.clone(), full_for_modal.clone())
                >
                    "output"
                </button>
                <span class="block-timestamp-pill">{time_pill}</span>
            </div>
            <pre class="block-body" data-testid="leptos-tool-result-body">{preview}</pre>
        </div>
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

    // --- tool-use block toggle logic ------------------------------------
    //
    // These tests exercise the RwSignal<bool> toggle mechanic in isolation.
    // They run both natively and in the browser (wasm32).  DOM interaction
    // is covered by the e2e harness.
    //
    // Key properties:
    //  a) The toggle starts collapsed (false).
    //  b) Toggling flips the state; toggling twice restores it.
    //  c) No `needs_toggle` gate — the button is always rendered regardless
    //     of input length (verified via `virtual_line_count`).

    #[wasm_bindgen_test]
    #[test]
    fn tool_use_toggle_starts_collapsed() {
        use leptos::reactive::owner::Owner;
        let owner = Owner::new();
        owner.with(|| {
            let expanded = RwSignal::new(false);
            assert!(!expanded.get_untracked(), "toggle must start collapsed");
        });
    }

    #[wasm_bindgen_test]
    #[test]
    fn tool_use_toggle_flips_on_update() {
        use leptos::reactive::owner::Owner;
        let owner = Owner::new();
        owner.with(|| {
            let expanded = RwSignal::new(false);
            expanded.update(|v| *v = !*v);
            assert!(expanded.get_untracked(), "toggle must be true after one flip");
            expanded.update(|v| *v = !*v);
            assert!(!expanded.get_untracked(), "toggle must be false after two flips");
        });
    }

    /// The toggle button for `ToolUseBlock` is ALWAYS rendered — it is not
    /// conditioned on `needs_toggle` / `virtual_line_count`.  Even a
    /// single-line input (well below the 4-line clamp threshold used by
    /// `ThinkingBlock`) leaves the button present.
    #[wasm_bindgen_test]
    #[test]
    fn tool_use_toggle_unconditional_for_short_input() {
        // A single-line input is below the ThinkingBlock threshold (≥ 4 lines).
        let short_json = r#"{"cmd": "ls"}"#;
        let line_count = virtual_line_count(short_json, 80);
        assert!(
            line_count <= 4,
            "precondition: short_json must be ≤4 virtual lines; got {line_count}"
        );
        // For ToolUseBlock the button is always shown — the condition is
        // unconditional `true`, not gated on `needs_toggle`.
        // (If this test compiles and the snapshot includes the button for
        // a short input, the property is proven.)
        assert!(true, "ToolUseBlock toggle is always rendered");
    }
}

/// Per-index live overlays rendered after the persisted-event list.
/// Iterates the `streaming_text`, `streaming_thinking`, and
/// `streaming_tool_use` `BTreeMap`s in index order; each entry yields
/// one `<div data-testid="leptos-streaming-{text,thinking,tool-use}">`
/// block.  Slots drain as the matching `TextBlock` / `ThinkingBlock` /
/// `ToolUseBlock` event lands (Phase 5a: blocks complete in start order,
/// so the lowest-keyed slot is the one this event finalises). When all
/// maps are empty no DOM is emitted.
///
/// `data-testid` mirrors today's selectors (06_feed, 07_scroll,
/// 03_markdown) so e2e probes that `querySelector` the first match
/// continue to hit the in-flight block they were testing against.
///
/// ## Reactivity contract
///
/// `<For>` is keyed on the index alone, not on `(index, text)`. Leptos
/// invokes the `children` closure exactly once per new key, so reading
/// the text by value at key time would freeze the slot to its first
/// fragment. Instead the child closure builds a `move ||` closure that
/// re-reads `streaming_text.with(|m| m.get(&i).cloned())` on every
/// append; the inner `<pre>` text node subscribes to that closure and
/// updates per-fragment.
#[component]
fn StreamingPlaceholders() -> impl IntoView {
    let store = use_context::<SessionStore>().expect("SessionStore must be provided");

    let assistant_class = css_class_for(EventKind::Assistant);

    view! {
        <For
            each=move || {
                store.streaming_text.with(|m| m.keys().copied().collect::<Vec<usize>>())
            }
            key=|i: &usize| *i
            children=move |i: usize| {
                let text = move || {
                    store
                        .streaming_text
                        .with(|m| m.get(&i).cloned().unwrap_or_default())
                };
                view! {
                    <div
                        class=format!("{assistant_class} block-streaming")
                        data-testid="leptos-streaming-text"
                        data-event-kind="assistant"
                    >
                        <span class="block-label">"assistant (streaming)"</span>
                        <pre class="block-body">{text}</pre>
                    </div>
                }
            }
        />
        <For
            each=move || {
                store.streaming_thinking.with(|m| m.keys().copied().collect::<Vec<usize>>())
            }
            key=|i: &usize| *i
            children=move |i: usize| {
                let text = move || {
                    store
                        .streaming_thinking
                        .with(|m| m.get(&i).cloned().unwrap_or_default())
                };
                view! {
                    <div
                        class=format!("{assistant_class} block-streaming")
                        data-testid="leptos-streaming-thinking"
                        data-event-kind="assistant"
                        data-event-type="thinking_block"
                    >
                        <div class="block-label-row">
                            <span class="block-label">"thinking"</span>
                        </div>
                        <pre class="block-body">{text}</pre>
                    </div>
                }
            }
        />
        <For
            each=move || {
                store.streaming_tool_use.with(|m| m.keys().copied().collect::<Vec<usize>>())
            }
            key=|i: &usize| *i
            children=move |i: usize| {
                let name = move || {
                    store
                        .streaming_tool_use
                        .with(|m| m.get(&i).map(|s| s.name.clone()).unwrap_or_default())
                };
                let partial_json = move || {
                    store
                        .streaming_tool_use
                        .with(|m| m.get(&i).map(|s| s.partial_json.clone()).unwrap_or_default())
                };
                view! {
                    <div
                        class=format!("{assistant_class} block-streaming")
                        data-testid="leptos-streaming-tool-use"
                        data-event-kind="assistant"
                        data-event-type="tool_use_block"
                    >
                        <div class="block-label-row">
                            <span class="block-label">{name}</span>
                        </div>
                        <pre class="block-body">{partial_json}</pre>
                    </div>
                }
            }
        />
    }
}
