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
//!    │              └── ToolResult: <ToolResultBlock> with show-more toggle
//!    │                   └── pure `truncate_for_preview(s, 3000)`
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

use crate::event_view::{
    EventKind, css_class_for, event_type_tag, kind_for, kind_tag, should_autoscroll,
    truncate_for_preview,
};
use crate::store::SessionStore;

/// Match the SolidJS UI's inline preview cap. Larger payloads remain
/// reachable through the "show more" toggle (3.3) and, eventually,
/// the modal that 3.5 ports.
const TOOL_RESULT_PREVIEW_MAX_CHARS: usize = 3000;

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
#[component]
pub fn ConversationFeed() -> impl IntoView {
    let store = use_context::<SessionStore>().expect("SessionStore must be provided");

    let scroll_ref = NodeRef::<html::Section>::new();
    let sentinel_ref = NodeRef::<html::Div>::new();
    let auto_scroll = RwSignal::new(true);

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
    Effect::new(move |_| {
        let _ = store.events.with(Vec::len);
        let _ = store.streaming_text.with(String::len);
        let _ = store.streaming_thinking.with(String::len);
        if !auto_scroll.get_untracked() {
            return;
        }
        if let Some(el) = sentinel_ref.get() {
            el.scroll_into_view();
        }
    });

    let on_scroll = move |_ev: ev::Event| {
        let Some(el) = scroll_ref.get() else {
            return;
        };
        let scroll_top = f64::from(el.scroll_top());
        let client_height = f64::from(el.client_height());
        let scroll_height = f64::from(el.scroll_height());
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
        <section
            class="leptos-feed"
            data-testid="leptos-feed"
            data-auto-scroll=move || if auto_scroll.get() { "true" } else { "false" }
            node_ref=scroll_ref
            on:scroll=on_scroll
        >
            <For
                each=move || {
                    store
                        .events
                        .get()
                        .into_iter()
                        .enumerate()
                        .collect::<Vec<(usize, OmegaEvent)>>()
                }
                key=|(idx, _): &(usize, OmegaEvent)| *idx
                children=|(_, event): (usize, OmegaEvent)| view! { <EventBlock event=event /> }
            />
            <StreamingTail />
            <div class="leptos-feed-sentinel" data-testid="leptos-feed-sentinel" node_ref=sentinel_ref />
        </section>
    }
}

// ---------------------------------------------------------------------------
// Event block
// ---------------------------------------------------------------------------

/// Renders one `OmegaEvent` with the visual-family CSS class and a
/// typed body. `ToolResult` events delegate to [`ToolResultBlock`]
/// for show-more state.
#[component]
fn EventBlock(event: OmegaEvent) -> impl IntoView {
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
            {render_event_body(event)}
        </div>
    }
}

/// Per-variant body view. Returns `AnyView` so the match arms can each
/// produce their own concrete `view!` output and unify at the call
/// site. The big match here is necessary (each arm needs typed field
/// access); the *family decision* is carved out into the pure
/// `kind_for` in `event_view.rs`.
fn render_event_body(event: OmegaEvent) -> AnyView {
    match event {
        OmegaEvent::UserMessage(e) => view! {
            <span class="block-label">"user_message"</span>
            <pre class="block-body" data-testid="leptos-user-content">{e.content}</pre>
        }
        .into_any(),

        OmegaEvent::LlmResponse(e) => {
            let text = e.text.unwrap_or_default();
            let usage_line = format!(
                "in: {}  out: {}  ({})",
                e.usage.input_tokens, e.usage.output_tokens, e.stop_reason,
            );
            view! {
                <span class="block-label">"assistant"</span>
                <pre class="block-body" data-testid="leptos-assistant-text">{text}</pre>
                <span class="block-meta" data-testid="leptos-assistant-usage">{usage_line}</span>
            }
            .into_any()
        }

        OmegaEvent::ToolCall(e) => {
            let arg_preview = serde_json::to_string(&e.input).unwrap_or_else(|_| "{}".into());
            view! {
                <span class="block-label">"tool_call"</span>
                <span class="block-tool-name" data-testid="leptos-tool-name">{e.name}</span>
                <pre class="block-tool-input" data-testid="leptos-tool-input">{arg_preview}</pre>
            }
            .into_any()
        }

        OmegaEvent::ToolResult(e) => view! { <ToolResultBlock event=e /> }.into_any(),

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

        OmegaEvent::LlmCall(e) => {
            let line = format!(
                "{} · {} ctx record(s) · {} bytes",
                e.model,
                e.context_hashes.len(),
                e.request_bytes,
            );
            view! {
                <span class="block-label">"llm_call"</span>
                <span class="block-body">{line}</span>
            }
            .into_any()
        }

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
            <pre class="block-body">{e.summary}</pre>
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
// Tool-result block (per-row show-more toggle)
// ---------------------------------------------------------------------------

/// One tool_result row, with its own `expanded` signal so the
/// "show more" toggle is per-row state. The truncation decision is
/// the pure [`truncate_for_preview`] (mutation-tested); only the
/// visibility toggle and event handler live here.
#[component]
fn ToolResultBlock(event: omega_protocol::events::ToolResultEvent) -> impl IntoView {
    let expanded = RwSignal::new(false);
    // Compute the preview lazily so a long output isn't truncated
    // every render. This is fine to capture by clone — the event is
    // already owned by the closure.
    let full = event.output.clone();
    let truncated = truncate_for_preview(&full, TOOL_RESULT_PREVIEW_MAX_CHARS);
    let was_truncated = truncated.is_some();
    let preview = truncated.unwrap_or_else(|| full.clone());

    let label = if event.is_error {
        "tool_result · error"
    } else {
        "tool_result"
    };
    let duration_line = format!("{}ms · {}", event.duration_ms, event.name);

    view! {
        <span class="block-label">{label}</span>
        <span class="block-meta" data-testid="leptos-tool-result-meta">{duration_line}</span>
        <pre class="block-body" data-testid="leptos-tool-result-body">
            {move || if expanded.get() { full.clone() } else { preview.clone() }}
        </pre>
        <Show when=move || was_truncated fallback=|| ().into_any()>
            <button
                class="block-show-more"
                data-testid="leptos-tool-result-expand"
                on:click=move |_| expanded.update(|v| *v = !*v)
            >
                {move || if expanded.get() { "show less" } else { "show more" }}
            </button>
        </Show>
    }
}

// ---------------------------------------------------------------------------
// Streaming tail
// ---------------------------------------------------------------------------

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
