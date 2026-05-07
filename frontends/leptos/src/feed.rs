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
//!    │                   └── pure `truncate_to_lines(s, 2)`
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

use crate::context_modal::ContextModalState;
#[cfg(target_arch = "wasm32")]
use crate::diff_render::render_diff_html;
use crate::event_view::{
    EventKind, css_class_for, event_type_tag, kind_for, kind_tag, should_autoscroll,
    truncate_to_lines,
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
///
/// `pub` so the host-target snapshot harness in `tests/snapshots.rs`
/// (Phase 3.6 TEST-ARCH-5) can render fixtures directly. The wasm
/// runtime mounts it from `<ConversationFeed/>`.
#[component]
pub fn EventBlock(event: OmegaEvent) -> impl IntoView {
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

        OmegaEvent::LlmResponse(e) => view! { <LlmResponseBlock event=e /> }.into_any(),

        OmegaEvent::ToolCall(e) => view! { <ToolCallBlock event=e /> }.into_any(),

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
///    opens the text modal with the thinking text.
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
            <button
                class="block-label-row-btn"
                data-testid="leptos-llm-response-context"
                on:click=move |_| context_modal.open_hash(context_hash.clone())
            >
                "[context]"
            </button>
            <button
                class="block-label-row-btn"
                data-testid="leptos-llm-response-payload"
                on:click=move |_| text_modal.open("llm_response payload", event_json.clone())
            >
                "[payload]"
            </button>
            <Show when=move || has_thinking fallback=|| ().into_any()>
                <button
                    class="block-label-row-btn"
                    data-testid="leptos-llm-response-thinking"
                    on:click={
                        let t = thinking_text.clone();
                        move |_| text_modal.open("thinking", t.clone())
                    }
                >
                    "[thinking]"
                </button>
            </Show>
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
/// The last four characters of the tool-use `id` appear in superscript
/// muted text as a correlation hint when parallel tool calls share an
/// `llm_call`. A 2-line JSON preview appears inline; a `[payload]`
/// button opens the full JSON in a [`TextModal`].
/// Returns the last `n` characters (Unicode scalar values) of `id`, or
/// the whole string if `id` is shorter than `n`.  Used to build the
/// short correlation hint shown in the tool-call block header.
fn tool_id_suffix(id: &str) -> String {
    // saturating_sub avoids the off-by-one boundary: skip(0) == clone for len==4
    let offset = id.chars().count().saturating_sub(4);
    id.chars().skip(offset).collect()
}

#[component]
fn ToolCallBlock(event: omega_protocol::events::ToolCallEvent) -> impl IntoView {
    let text_modal =
        use_context::<TextModalState>().expect("TextModalState must be provided");

    let name = event.name.clone();
    // Last 4 chars of the tool-use id as a correlation hint.
    let id_suffix = tool_id_suffix(&event.id);

    let full_input = serde_json::to_string_pretty(&event.input)
        .unwrap_or_else(|_| "{}".to_owned());
    // 2-line preview; full JSON reachable via the payload modal.
    let preview = truncate_to_lines(&full_input, 2)
        .unwrap_or_else(|| full_input.clone());
    let full_for_modal = full_input;
    let modal_title = format!("tool_call: {name}");

    view! {
        <div class="block-label-row">
            <span class="block-label">
                <span data-testid="leptos-tool-name">{name.clone()}</span>
                <sup class="block-tool-id">{id_suffix}</sup>
            </span>
            <button
                class="block-label-row-btn"
                data-testid="leptos-tool-call-payload"
                on:click=move |_| text_modal.open(modal_title.clone(), full_for_modal.clone())
            >
                "[payload]"
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
///   `llm_call` label · `[context]` button · `[payload]` button
///
/// The `[context]` button opens the [`ContextModal`] overlay.
/// The `[payload]` button opens a [`TextModal`] with the full
/// event metadata: model, cache_breakpoint_index, request_bytes,
/// context_hashes, and the request_summary JSON.
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

    // Summary line: model · ctx count · bytes.
    let summary_line = format!(
        "{} · {} ctx · {} bytes",
        event.model,
        event.context_hashes.len(),
        event.request_bytes,
    );

    // Payload text: all four metadata fields joined in a readable block.
    let cache_bp = event
        .cache_breakpoint_index
        .map_or_else(|| "none".to_owned(), |i| i.to_string());
    let hashes_text = if event.context_hashes.is_empty() {
        "  (none)".to_owned()
    } else {
        event
            .context_hashes
            .iter()
            .map(|h| format!("  {h}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let request_summary_str = event.request_summary.as_ref().map_or_else(
        || "(not available)".to_owned(),
        |v| serde_json::to_string_pretty(v).unwrap_or_else(|_| "(unrenderable)".to_owned()),
    );
    let payload_text = format!(
        "model: {}\ncache_breakpoint_index: {}\nrequest_bytes: {}\ncontext_hashes:\n{}\n\n--- request_summary ---\n{}",
        event.model,
        cache_bp,
        event.request_bytes,
        hashes_text,
        request_summary_str,
    );

    // Clone the event for the context modal; the payload text is moved.
    let event_for_ctx = event;

    view! {
        <div class="block-label-row">
            <span class="block-label">"llm_call"</span>
            <button
                class="block-label-row-btn"
                data-testid="leptos-llm-call-open-modal"
                on:click=move |_| context_modal.open(event_for_ctx.clone())
            >
                "[context]"
            </button>
            <button
                class="block-label-row-btn"
                data-testid="leptos-llm-call-payload"
                on:click=move |_| text_modal.open("llm_call payload", payload_text.clone())
            >
                "[payload]"
            </button>
        </div>
        <span class="block-body" data-testid="leptos-llm-call-summary">{summary_line}</span>
    }
}

/// One `tool_result` row — Phase 3.10 TODO-C.
///
/// * Label is the tool name (not `"tool_result"`).
/// * Inline preview is truncated to the first 2 lines.
/// * A `[payload]` button opens a [`TextModal`] with the full output.
/// * The old `[show more]` toggle and the `duration_ms` meta line are
///   removed from the inline view; duration appears in the modal title.
#[component]
fn ToolResultBlock(event: omega_protocol::events::ToolResultEvent) -> impl IntoView {
    let text_modal =
        use_context::<TextModalState>().expect("TextModalState must be provided");

    let name = event.name.clone();
    let full = event.output.clone();
    // 2-line inline preview; full output reachable via the payload modal.
    let preview = truncate_to_lines(&full, 2)
        .unwrap_or_else(|| full.clone());
    let modal_title = format!("{name}  ·  {}ms", event.duration_ms);
    let full_for_modal = full;

    view! {
        <div class="block-label-row">
            <span class="block-label" data-testid="leptos-tool-result-name">{name}</span>
            <button
                class="block-label-row-btn"
                data-testid="leptos-tool-result-payload"
                on:click=move |_| text_modal.open(modal_title.clone(), full_for_modal.clone())
            >
                "[payload]"
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

    // --- tool_id_suffix ---------------------------------------------

    #[wasm_bindgen_test]
    #[test]
    fn tool_id_suffix_returns_full_id_when_short() {
        assert_eq!(tool_id_suffix(""), "");
        assert_eq!(tool_id_suffix("ab"), "ab");
        assert_eq!(tool_id_suffix("abcd"), "abcd");
    }

    #[wasm_bindgen_test]
    #[test]
    fn tool_id_suffix_returns_last_four_chars_when_longer() {
        assert_eq!(tool_id_suffix("abcde"), "bcde");
        assert_eq!(tool_id_suffix("abcdefgh"), "efgh");
        assert_eq!(tool_id_suffix("toolu_01ABC"), "1ABC");
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
