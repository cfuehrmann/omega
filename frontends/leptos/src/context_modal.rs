//! Context-inspector modal (Phase 3.5).
//!
//! ```text
//!  Operator clicks an `llm_call` block
//!    │
//!    └──► ContextModalState.0.set(Some(LlmCallEvent))
//!           │
//!           └──► <ContextModal/> mounts via the App-root `<Show>`,
//!                  fires `get_context(&event.context_hashes)` once,
//!                  renders each ContextRecord newest→oldest.
//! ```
//!
//! ## Shape of `ContextRecord` on the wire
//!
//! `omega-store::ContextRecord` is the canonical type, but the Leptos
//! crate doesn't (and can't) depend on `omega-store` — that crate
//! pulls in tokio + file I/O which don't compile to wasm32.
//!
//! Instead we mirror the wire JSON locally:
//!
//! ```jsonc
//! {
//!   "hash": "deadbeefcafe",
//!   "time": "2024-01-01T00:00:00.000Z",
//!   "role": "user",
//!   "content": [ { "type": "text", "text": "…" } ]
//! }
//! ```
//!
//! The `content` field is held as a `serde_json::Value` so the typed
//! enum stays in `omega-core` (where it belongs) and the wasm bundle
//! avoids the file-I/O dependency chain. The pure render helpers
//! ([`render_content`], [`render_block`]) project the JSON shape to a
//! display string — same dispatch as SolidJS's `renderContent`
//! (`src/web/client/App.tsx:418`).
//!
//! ## Pure mutation-test targets
//!
//! - [`build_hashes_param`] — `hashes.join(",")`. One-liner but the
//!   carve-out makes the choice of separator visible to `cargo
//!   mutants`. Replacing `","` with `""`/`"&"`/`";"` would silently
//!   break the server's `split(',')` parser.
//! - [`render_content`] — top-level dispatch (string vs array vs
//!   other).
//! - [`render_block`] — per-`type`-tag dispatch (text / thinking /
//!   tool_use / tool_result / unknown fallback).
//! - [`role_label`] — `role` → display string. Stable Playwright
//!   selector.
//!
//! ## JS-interop gaps (acknowledged, not closed)
//!
//! Same pattern as 3.1's `ws.rs`, 3.2's `picker.rs`, 3.3's `feed.rs`,
//! 3.4's `composer.rs`:
//!
//! - Modal close-button click is a `view!` glue line; mutation-tested
//!   indirectly via the Playwright spec.
//! - Click-outside-backdrop dismissal **not implemented** — the visible
//!   close button is the only dismissal vector. Phase 3.6 polish.
//! - Escape-key dismissal **not implemented** — same reason.
//! - Focus trap inside the modal **not implemented**. Phase 3.6.

use leptos::prelude::*;
use leptos::task::spawn_local;
use omega_types::events::LlmCallEvent;
use serde::{Deserialize, Serialize};

use crate::http::get_context;

// ---------------------------------------------------------------------------
// Wire shape
// ---------------------------------------------------------------------------

/// One entry in the `GET /api/context` JSON array.
///
/// Mirrors the on-disk `omega-store::ContextRecord` shape verbatim
/// (camelCase off — server-side this struct uses snake_case fields
/// without an explicit `rename_all`, see `omega-store/src/context_store.rs`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextRecord {
    pub hash: String,
    /// ISO-8601 UTC. Optional defensively — old logs predating 1c
    /// might omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<String>,
    /// `"user"` or `"assistant"`. Held as a string rather than an
    /// enum so a server-side enum extension doesn't break parsing —
    /// honest types: render the string we got.
    pub role: String,
    /// Array of content blocks in the omega-core `ContentBlock` shape;
    /// see module docs for why this is `Value` rather than a typed enum.
    #[serde(default)]
    pub content: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Pure helpers (cargo-mutants targets)
// ---------------------------------------------------------------------------

/// Project a list of context hashes to the value of the `hashes`
/// query-string parameter. `gloo-net` URL-encodes the value at the
/// fetch site, so callers pass the raw string verbatim.
///
/// Returns the empty string when `hashes` is empty — the caller in
/// [`get_context`] short-circuits in that case so no fetch is fired.
#[must_use]
pub fn build_hashes_param(hashes: &[String]) -> String {
    hashes.join(",")
}

/// Project a context record's `content` JSON to a flat display
/// string. Mirrors the SolidJS UI's `renderContent`
/// (`src/web/client/App.tsx:418`) byte-for-byte for visual parity.
///
/// - String → return as-is (legacy plain-text records).
/// - Array → map each element through [`render_block`], join with
///   newlines.
/// - Anything else → pretty-print as JSON.
#[must_use]
pub fn render_content(content: &serde_json::Value) -> String {
    if let Some(s) = content.as_str() {
        return s.to_owned();
    }
    if let Some(arr) = content.as_array() {
        return arr
            .iter()
            .map(render_block)
            .collect::<Vec<_>>()
            .join("\n");
    }
    serde_json::to_string_pretty(content).unwrap_or_else(|_| "<unrenderable content>".into())
}

/// Project one content block (an element of the `content` array) to
/// a display string. Dispatches on the `type` tag.
///
/// Matches SolidJS's per-block formatting:
/// - `text`        → the plain `text` field.
/// - `tool_use`    → `[tool_use: <name>]\n<pretty input>`.
/// - `tool_result` → `[tool_result]\n<flattened content>`.
/// - `thinking`    → `[thinking]\n<thinking text>`.
/// - anything else → pretty-printed JSON of the whole block.
///
/// Note: omega-core serialises `ToolResult.content` as a flat
/// `String`, but the Anthropic wire shape allows an array of
/// `{type, text}` objects. We handle both — the array branch is
/// defensive against legacy logs and direct-from-Anthropic captures.
#[must_use]
pub fn render_block(block: &serde_json::Value) -> String {
    let Some(obj) = block.as_object() else {
        return serde_json::to_string_pretty(block).unwrap_or_default();
    };
    let tag = obj.get("type").and_then(serde_json::Value::as_str);
    match tag {
        Some("text") => obj
            .get("text")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_owned(),
        Some("tool_use") => {
            let name = obj
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let input = obj.get("input").map_or_else(
                || "{}".to_owned(),
                |v| serde_json::to_string_pretty(v).unwrap_or_else(|_| "{}".to_owned()),
            );
            format!("[tool_use: {name}]\n{input}")
        }
        Some("tool_result") => {
            let content = obj.get("content").map_or_else(String::new, |c| {
                if let Some(s) = c.as_str() {
                    s.to_owned()
                } else if let Some(arr) = c.as_array() {
                    arr.iter()
                        .map(|el| {
                            el.get("text")
                                .and_then(serde_json::Value::as_str)
                                .map_or_else(
                                    || {
                                        serde_json::to_string(el)
                                            .unwrap_or_else(|_| String::new())
                                    },
                                    str::to_owned,
                                )
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                } else {
                    serde_json::to_string(c).unwrap_or_default()
                }
            });
            format!("[tool_result]\n{content}")
        }
        Some("thinking") => {
            let thinking = obj
                .get("thinking")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_owned();
            format!("[thinking]\n{thinking}")
        }
        _ => serde_json::to_string_pretty(block).unwrap_or_default(),
    }
}

/// Stable display label for a record's role. Held separately from the
/// `data-role` attribute so a styling tweak doesn't drift the spec
/// selector. Empty string on unknown role — preserved as-is so the
/// operator sees the wire value verbatim.
#[must_use]
pub fn role_label(role: &str) -> &str {
    match role {
        "user" => "user",
        "assistant" => "assistant",
        other => other,
    }
}

// ---------------------------------------------------------------------------
// Modal state
// ---------------------------------------------------------------------------

/// App-scoped reactive state for the context modal. `Some(event)` =
/// modal open with `event.context_hashes` driving the fetch. `None` =
/// modal closed.
///
/// Wrapped in a newtype `Copy` handle so `provide_context` /
/// `use_context` find a unique type — leptos's context lookup is
/// type-keyed.
#[derive(Debug, Clone, Copy)]
pub struct ContextModalState(pub RwSignal<Option<LlmCallEvent>>);

impl ContextModalState {
    /// Construct fresh state (modal closed). Must run inside a leptos
    /// reactive `Owner` scope.
    #[must_use]
    pub fn new() -> Self {
        Self(RwSignal::new(None))
    }

    /// Open the modal with the given LLM-call event.
    pub fn open(self, event: LlmCallEvent) {
        self.0.set(Some(event));
    }

    /// Open the modal for a single context hash (e.g. from an
    /// `llm_response` `[context]` button).  `request_bytes` is set to
    /// zero so the meta line omits the byte count.
    pub fn open_hash(self, hash: String) {
        self.0.set(Some(LlmCallEvent {
            time: String::new().into(),
            url: String::new(),
            model: String::new(),
            context_hashes: vec![hash],
            cache_breakpoint_index: None,
            request_bytes: 0,
            request_summary: None,
        }));
    }

    /// Close the modal.
    pub fn close(self) {
        self.0.set(None);
    }
}

impl Default for ContextModalState {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Fetch state (per-component; mutation-tested directly)
// ---------------------------------------------------------------------------

/// Reactive state for the modal's `GET /api/context` fetch lifecycle.
///
/// Same fetch-generation pattern as `SessionListStore` (Phase 3.2):
/// every transition (`begin`, `finish_if_current`, `fail_if_current`)
/// is a pure method on a `Copy` handle so the comparison logic is
/// directly mutation-testable. This carves the otherwise-opaque
/// `if fetch_seq != token` check out of the `spawn_local` closure
/// where `cargo mutants` cannot reach it.
///
/// ## Race scenario
///
/// User clicks llm_call A → fetch starts. User closes modal
/// (which the `Effect` interprets as a state reset) → user clicks
/// llm_call B → a second fetch starts. The first fetch's response
/// must NOT clobber the second fetch's pending state. The token
/// captured by the first `spawn_local` no longer matches the live
/// `fetch_seq`; `finish_if_current` and `fail_if_current` short-
/// circuit silently.
#[derive(Debug, Clone, Copy)]
struct ContextFetchState {
    records: RwSignal<Vec<ContextRecord>>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    fetch_seq: RwSignal<u64>,
}

impl ContextFetchState {
    fn new() -> Self {
        Self {
            records: RwSignal::new(Vec::new()),
            loading: RwSignal::new(false),
            error: RwSignal::new(None),
            fetch_seq: RwSignal::new(0),
        }
    }

    /// Reset every signal to its closed-modal default. Called when
    /// the modal closes so the next open doesn't briefly flash
    /// stale records.
    fn reset(self) {
        self.records.set(Vec::new());
        self.loading.set(false);
        self.error.set(None);
    }

    /// Begin a new fetch. Bumps the generation counter, returns the
    /// caller's token, sets `loading=true`, clears prior records and
    /// errors so the modal renders "Loading…" rather than stale
    /// content from a previous open.
    #[must_use]
    fn begin(self) -> u64 {
        let next = self.fetch_seq.get_untracked().wrapping_add(1);
        self.fetch_seq.set(next);
        self.loading.set(true);
        self.error.set(None);
        self.records.set(Vec::new());
        next
    }

    /// Apply a successful fetch result *iff* `token` is still the
    /// current generation. Stale results are silently discarded.
    /// Returns `true` iff the result was applied.
    fn finish_if_current(self, token: u64, items: Vec<ContextRecord>) -> bool {
        if self.fetch_seq.get_untracked() != token {
            return false;
        }
        self.records.set(items);
        self.loading.set(false);
        true
    }

    /// Record an error *iff* `token` is still the current generation.
    /// Stale errors are silently discarded. Returns `true` iff the
    /// error was recorded.
    fn fail_if_current(self, token: u64, message: String) -> bool {
        if self.fetch_seq.get_untracked() != token {
            return false;
        }
        self.error.set(Some(message));
        self.loading.set(false);
        true
    }
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

/// Top-level overlay. Renders nothing when the modal is closed. When
/// open, fires a single `get_context` fetch keyed on the event's
/// `context_hashes` and renders the records.
///
/// Mounted as a sibling of `<Composer/>` at the App root so its
/// `position: fixed` overlay stacks over every page surface
/// regardless of DOM nesting.
#[component]
pub fn ContextModal() -> impl IntoView {
    let state = use_context::<ContextModalState>().expect("ContextModalState must be provided");

    // Per-component fetch state. Carved out so the
    // `fetch_seq != token` stale-result check lives in directly
    // unit-testable methods (see `ContextFetchState` impl).
    let fetch = ContextFetchState::new();
    let records = fetch.records;
    let loading = fetch.loading;
    let error = fetch.error;

    Effect::new(move |_| {
        let opt = state.0.get();
        match opt {
            None => {
                // Modal closed — clear last result so the next open
                // doesn't briefly flash stale records.
                fetch.reset();
            }
            Some(event) => {
                let token = fetch.begin();
                let hashes = event.context_hashes.clone();
                spawn_local(async move {
                    match get_context(&hashes).await {
                        Ok(items) => {
                            fetch.finish_if_current(token, items);
                        }
                        Err(message) => {
                            fetch.fail_if_current(token, message);
                        }
                    }
                });
            }
        }
    });

    let on_close = move |_| state.close();

    view! {
        <Show
            when=move || state.0.with(Option::is_some)
            fallback=|| ().into_any()
        >
            // Phase 3.8: inline `style=` attributes were stripped
            // from this component so `frontends/leptos/style.css`
            // can fully own the modal's geometry + Mocha palette.
            // The previous inline styles hard-coded `background:#fff;
            // color:#000;` which is incompatible with the dark theme.
            <div
                class="leptos-context-modal-backdrop"
                data-testid="leptos-context-modal-backdrop"
            >
                <div
                    class="leptos-context-modal"
                    data-testid="leptos-context-modal"
                >
                    <header
                        class="leptos-context-modal-header"
                    >
                        <span class="leptos-context-modal-title">
                            "context records"
                        </span>
                        <button
                            class="leptos-context-modal-close"
                            data-testid="leptos-context-modal-close"
                            on:click=on_close
                        >
                            "✕"
                        </button>
                    </header>
                    <div
                        class="leptos-context-modal-meta"
                        data-testid="leptos-context-modal-meta"
                    >
                        {move || {
                            state.0.with(|opt| {
                                opt.as_ref().map_or_else(String::new, |event| {
                                    let n = event.context_hashes.len();
                                    if event.request_bytes > 0 {
                                        format!("{n} hash(es) · {} bytes", event.request_bytes)
                                    } else {
                                        format!("{n} hash(es)")
                                    }
                                })
                            })
                        }}
                    </div>
                    <Show
                        when=move || loading.get()
                        fallback=|| ().into_any()
                    >
                        <div
                            class="leptos-context-modal-loading"
                            data-testid="leptos-context-modal-loading"
                        >
                            "Loading…"
                        </div>
                    </Show>
                    <Show
                        when=move || error.with(Option::is_some)
                        fallback=|| ().into_any()
                    >
                        <div
                            class="leptos-context-modal-error"
                            data-testid="leptos-context-modal-error"
                        >
                            {move || error.with(|e| e.clone().unwrap_or_default())}
                        </div>
                    </Show>
                    <ul
                        class="leptos-context-modal-records"
                        data-testid="leptos-context-modal-records"
                    >
                        <For
                            each=move || {
                                // Reverse for newest→oldest display
                                // — matches SolidJS UI.
                                let mut v = records.get();
                                v.reverse();
                                v.into_iter().enumerate().collect::<Vec<_>>()
                            }
                            key=|(idx, rec): &(usize, ContextRecord)| (*idx, rec.hash.clone())
                            children=|(_, rec): (usize, ContextRecord)| {
                                view! {
                                    <li
                                        class=format!(
                                            "leptos-context-modal-record \
                                             leptos-context-modal-record-{}",
                                            rec.role,
                                        )
                                        data-testid="leptos-context-modal-record"
                                        data-role=rec.role.clone()
                                    >
                                        <span
                                            class="leptos-context-modal-record-role"
                                            data-testid="leptos-context-modal-record-role"
                                        >
                                            {role_label(&rec.role).to_owned()}
                                        </span>
                                        <Show
                                            when={
                                                let time = rec.time.clone();
                                                move || time.is_some()
                                            }
                                            fallback=|| ().into_any()
                                        >
                                            <span
                                                class="leptos-context-modal-record-time"
                                            >
                                                {rec.time.clone().unwrap_or_default()}
                                            </span>
                                        </Show>
                                        <pre
                                            class="leptos-context-modal-record-body"
                                            data-testid="leptos-context-modal-record-body"
                                        >
                                            {render_content(&rec.content)}
                                        </pre>
                                    </li>
                                }
                            }
                        />
                    </ul>
                    <Show
                        when=move || {
                            !loading.get()
                                && error.with(Option::is_none)
                                && records.with(Vec::is_empty)
                        }
                        fallback=|| ().into_any()
                    >
                        <div
                            class="leptos-context-modal-empty"
                            data-testid="leptos-context-modal-empty"
                        >
                            "(no context records returned)"
                        </div>
                    </Show>
                </div>
            </div>
        </Show>
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]

    use super::*;
    use serde_json::json;
    use wasm_bindgen_test::wasm_bindgen_test;

    // ---- build_hashes_param ------------------------------------------------

    #[wasm_bindgen_test]
    fn build_hashes_param_joins_with_comma() {
        let r = build_hashes_param(&["a".into(), "b".into(), "c".into()]);
        assert_eq!(r, "a,b,c");
    }

    #[wasm_bindgen_test]
    fn build_hashes_param_empty_input_returns_empty_string() {
        let r = build_hashes_param(&[]);
        assert_eq!(r, "");
    }

    #[wasm_bindgen_test]
    fn build_hashes_param_single_hash_has_no_separator() {
        // Boundary: one element → no comma. Catches `join(",")` →
        // `join(", ")` (extra space) and `join(",")` → `concat`
        // (which would still pass for a single element). Combined
        // with `joins_with_comma` above, both mutations die.
        let r = build_hashes_param(&["abc".into()]);
        assert_eq!(r, "abc");
        assert!(!r.contains(','));
    }

    #[wasm_bindgen_test]
    fn build_hashes_param_uses_comma_not_other_separator() {
        // Defends against `join(",")` → `join("&")` or `join(";")`
        // — the server splits on `','` (see
        // omega-server::router::ContextQuery), so any other
        // separator silently breaks the lookup.
        let r = build_hashes_param(&["x".into(), "y".into()]);
        assert!(r.contains(','));
        assert!(!r.contains('&'));
        assert!(!r.contains(';'));
        assert!(!r.contains(' '));
    }

    #[wasm_bindgen_test]
    fn build_hashes_param_preserves_order() {
        // Catches a `.sort()` insertion mutation — the server
        // preserves request order in the response, so the client
        // must too.
        let r = build_hashes_param(&["b".into(), "a".into(), "c".into()]);
        assert_eq!(r, "b,a,c");
    }

    // ---- render_block: per-tag dispatch ------------------------------------

    #[wasm_bindgen_test]
    fn render_block_text_returns_text_field() {
        assert_eq!(
            render_block(&json!({ "type": "text", "text": "hello" })),
            "hello"
        );
    }

    #[wasm_bindgen_test]
    fn render_block_text_missing_field_returns_empty() {
        assert_eq!(render_block(&json!({ "type": "text" })), "");
    }

    #[wasm_bindgen_test]
    fn render_block_tool_use_formats_with_name_and_input() {
        let r = render_block(&json!({
            "type": "tool_use",
            "name": "run_command",
            "input": { "command": "ls" },
        }));
        assert!(r.starts_with("[tool_use: run_command]\n"));
        assert!(r.contains("\"command\""));
        assert!(r.contains("\"ls\""));
    }

    #[wasm_bindgen_test]
    fn render_block_tool_use_missing_name_uses_empty_label() {
        let r = render_block(&json!({ "type": "tool_use", "input": {} }));
        assert!(r.starts_with("[tool_use: ]\n"));
    }

    #[wasm_bindgen_test]
    fn render_block_tool_result_string_content() {
        let r = render_block(&json!({
            "type": "tool_result",
            "tool_use_id": "id",
            "content": "ok",
        }));
        assert_eq!(r, "[tool_result]\nok");
    }

    #[wasm_bindgen_test]
    fn render_block_tool_result_array_content_flattens_text_blocks() {
        // Defensive: Anthropic-shaped content is an array. Each
        // entry's `text` field is concatenated with newlines;
        // matches the SolidJS UI's behaviour.
        let r = render_block(&json!({
            "type": "tool_result",
            "tool_use_id": "id",
            "content": [
                { "type": "text", "text": "line1" },
                { "type": "text", "text": "line2" },
            ],
        }));
        assert_eq!(r, "[tool_result]\nline1\nline2");
    }

    #[wasm_bindgen_test]
    fn render_block_thinking_formats_with_label() {
        let r = render_block(&json!({
            "type": "thinking",
            "thinking": "musing about the answer",
        }));
        assert_eq!(r, "[thinking]\nmusing about the answer");
    }

    #[wasm_bindgen_test]
    fn render_block_unknown_type_falls_back_to_pretty_json() {
        let r = render_block(&json!({ "type": "image", "source": "x.png" }));
        // Pretty-JSON output must include the type and the source
        // fields. Catches a fallback-arm deletion.
        assert!(r.contains("\"image\""));
        assert!(r.contains("\"source\""));
        assert!(r.contains('\n'), "pretty-print uses indentation");
    }

    #[wasm_bindgen_test]
    fn render_block_non_object_falls_back_to_pretty_json() {
        // E.g. `42` or `"raw string"` as a block — defensively
        // handled rather than panicking.
        assert_eq!(render_block(&json!(42)), "42");
        assert!(render_block(&json!("a")).contains('a'));
    }

    // ---- render_content: top-level dispatch --------------------------------

    #[wasm_bindgen_test]
    fn render_content_string_passthrough() {
        // Legacy plain-text record.
        assert_eq!(render_content(&json!("plain")), "plain");
    }

    #[wasm_bindgen_test]
    fn render_content_array_joins_blocks_with_newlines() {
        let r = render_content(&json!([
            { "type": "text", "text": "first" },
            { "type": "text", "text": "second" },
        ]));
        assert_eq!(r, "first\nsecond");
    }

    #[wasm_bindgen_test]
    fn render_content_empty_array_returns_empty_string() {
        // Boundary: empty array → empty string (no leading newline).
        // Catches `join("\n")` → `join("\n\n")` which would still
        // produce empty for empty input but split on the empty case.
        // The combined effect with the array-joins-blocks test above
        // pins both join-character mutations.
        assert_eq!(render_content(&json!([])), "");
    }

    #[wasm_bindgen_test]
    fn render_content_other_value_falls_back_to_pretty_json() {
        let r = render_content(&json!({ "shape": "object" }));
        assert!(r.contains("\"shape\""));
        assert!(r.contains("\"object\""));
    }

    // ---- role_label --------------------------------------------------------

    #[wasm_bindgen_test]
    fn role_label_known_roles_pass_through() {
        assert_eq!(role_label("user"), "user");
        assert_eq!(role_label("assistant"), "assistant");
    }

    #[wasm_bindgen_test]
    fn role_label_unknown_role_passes_through_verbatim() {
        // Honest types: render whatever the server sent us.
        assert_eq!(role_label("system"), "system");
        assert_eq!(role_label(""), "");
    }

    // ---- ContextRecord wire shape -----------------------------------------

    #[wasm_bindgen_test]
    fn context_record_round_trips_with_optional_time() {
        let json = r#"{
            "hash": "deadbeefcafe",
            "time": "2024-01-01T00:00:00.000Z",
            "role": "user",
            "content": [{"type":"text","text":"hi"}]
        }"#;
        let rec: ContextRecord = serde_json::from_str(json).unwrap();
        assert_eq!(rec.hash, "deadbeefcafe");
        assert_eq!(rec.time.as_deref(), Some("2024-01-01T00:00:00.000Z"));
        assert_eq!(rec.role, "user");
        assert!(rec.content.is_array());
    }

    #[wasm_bindgen_test]
    fn context_record_round_trips_without_optional_time() {
        let json = r#"{
            "hash": "deadbeefcafe",
            "role": "assistant",
            "content": "plain text"
        }"#;
        let rec: ContextRecord = serde_json::from_str(json).unwrap();
        assert!(rec.time.is_none());
        assert_eq!(rec.content.as_str(), Some("plain text"));
    }

    // ---- ContextModalState reactive surface --------------------------------

    use leptos::reactive::owner::Owner;

    fn with_owner<F: FnOnce()>(f: F) {
        let owner = Owner::new();
        owner.with(f);
    }

    fn fixture_event() -> LlmCallEvent {
        LlmCallEvent {
            time: "2024-01-01T00:00:00.000Z".into(),
            url: "https://api.example/v1/messages".into(),
            model: "claude-sonnet-4-6".into(),
            context_hashes: vec!["deadbeefcafe".into()],
            cache_breakpoint_index: Some(0),
            request_bytes: 1234,
            request_summary: None,
        }
    }

    #[wasm_bindgen_test]
    fn modal_state_starts_closed() {
        with_owner(|| {
            let s = ContextModalState::new();
            assert!(s.0.get_untracked().is_none());
        });
    }

    #[wasm_bindgen_test]
    fn modal_state_open_sets_event() {
        with_owner(|| {
            let s = ContextModalState::new();
            s.open(fixture_event());
            assert!(s.0.get_untracked().is_some());
        });
    }

    #[wasm_bindgen_test]
    fn modal_state_close_clears_event() {
        with_owner(|| {
            let s = ContextModalState::new();
            s.open(fixture_event());
            s.close();
            assert!(s.0.get_untracked().is_none());
        });
    }

    #[wasm_bindgen_test]
    fn modal_state_open_overwrites_previous_event() {
        with_owner(|| {
            let s = ContextModalState::new();
            let mut e1 = fixture_event();
            e1.context_hashes = vec!["aaa111111111".into()];
            s.open(e1);
            let mut e2 = fixture_event();
            e2.context_hashes = vec!["bbb222222222".into()];
            s.open(e2);
            assert_eq!(
                s.0.get_untracked().unwrap().context_hashes,
                vec!["bbb222222222".to_string()]
            );
        });
    }

    #[wasm_bindgen_test]
    fn modal_state_open_hash_sets_single_hash() {
        with_owner(|| {
            let s = ContextModalState::new();
            s.open_hash("deadbeef1234".into());
            let ev = s.0.get_untracked().expect("modal should be open after open_hash");
            assert_eq!(ev.context_hashes, vec!["deadbeef1234".to_string()]);
        });
    }

    // ---- ContextFetchState (token-comparison stale-fetch fix) -------------

    fn rec(hash: &str, role: &str) -> ContextRecord {
        ContextRecord {
            hash: hash.into(),
            time: None,
            role: role.into(),
            content: serde_json::Value::String("x".into()),
        }
    }

    #[wasm_bindgen_test]
    fn fetch_state_starts_idle() {
        with_owner(|| {
            let f = ContextFetchState::new();
            assert!(!f.loading.get_untracked());
            assert!(f.error.get_untracked().is_none());
            assert!(f.records.get_untracked().is_empty());
            assert_eq!(f.fetch_seq.get_untracked(), 0);
        });
    }

    #[wasm_bindgen_test]
    fn fetch_state_begin_bumps_seq_and_sets_loading() {
        with_owner(|| {
            let f = ContextFetchState::new();
            let t1 = f.begin();
            assert!(t1 > 0);
            assert!(f.loading.get_untracked());
            assert!(f.error.get_untracked().is_none());
            // Second begin produces a strictly larger token.
            let t2 = f.begin();
            assert!(t2 > t1, "second begin must bump: {t1} -> {t2}");
        });
    }

    #[wasm_bindgen_test]
    fn fetch_state_begin_clears_prior_records_and_error() {
        with_owner(|| {
            let f = ContextFetchState::new();
            // Prime stale state.
            f.records.set(vec![rec("old", "user")]);
            f.error.set(Some("prior".into()));
            let _ = f.begin();
            // Both prior values cleared so the modal renders
            // "Loading…" cleanly.
            assert!(f.records.get_untracked().is_empty());
            assert!(f.error.get_untracked().is_none());
        });
    }

    #[wasm_bindgen_test]
    fn fetch_state_finish_if_current_applies_when_token_matches() {
        with_owner(|| {
            let f = ContextFetchState::new();
            let t = f.begin();
            let applied = f.finish_if_current(t, vec![rec("a", "user")]);
            assert!(applied);
            assert!(!f.loading.get_untracked());
            assert_eq!(f.records.get_untracked().len(), 1);
        });
    }

    #[wasm_bindgen_test]
    fn fetch_state_finish_if_current_drops_stale_result() {
        // Race: fetch starts → second begin bumps the token → the
        // first fetch's result lands. Must NOT clobber the second
        // fetch's pending state. Catches the `!=` → `==` mutation
        // that was the only missed mutant before this carve-out.
        with_owner(|| {
            let f = ContextFetchState::new();
            let t1 = f.begin();
            // Second open bumps the seq.
            let _t2 = f.begin();
            let applied = f.finish_if_current(t1, vec![rec("stale", "user")]);
            assert!(!applied, "stale fetch must be dropped");
            // Records still empty (the second fetch's begin reset
            // them; the stale finish didn't repopulate).
            assert!(f.records.get_untracked().is_empty());
            // Loading still true (the second fetch is still in flight).
            assert!(f.loading.get_untracked());
        });
    }

    #[wasm_bindgen_test]
    fn fetch_state_fail_if_current_applies_when_token_matches() {
        with_owner(|| {
            let f = ContextFetchState::new();
            let t = f.begin();
            let applied = f.fail_if_current(t, "net error".into());
            assert!(applied);
            assert_eq!(f.error.get_untracked().as_deref(), Some("net error"));
            assert!(!f.loading.get_untracked());
        });
    }

    #[wasm_bindgen_test]
    fn fetch_state_fail_if_current_drops_stale_error() {
        // Same `!=` boundary as `finish_if_current_drops_stale_result`
        // but on the error path. Catches the `!=` → `==` mutation in
        // `fail_if_current`.
        with_owner(|| {
            let f = ContextFetchState::new();
            let t1 = f.begin();
            let _t2 = f.begin();
            let applied = f.fail_if_current(t1, "stale error".into());
            assert!(!applied);
            assert!(f.error.get_untracked().is_none());
        });
    }

    #[wasm_bindgen_test]
    fn fetch_state_reset_clears_records_loading_and_error() {
        with_owner(|| {
            let f = ContextFetchState::new();
            let t = f.begin();
            // Land a result.
            f.finish_if_current(t, vec![rec("a", "user")]);
            assert!(!f.records.get_untracked().is_empty());
            // Modal closes → reset.
            f.reset();
            assert!(f.records.get_untracked().is_empty());
            assert!(!f.loading.get_untracked());
            assert!(f.error.get_untracked().is_none());
            // fetch_seq must NOT be reset — a pending fetch from
            // before close must still be discarded as stale on a
            // subsequent open.
            assert!(f.fetch_seq.get_untracked() > 0);
        });
    }

    #[wasm_bindgen_test]
    fn fetch_state_reset_does_not_rewind_fetch_seq() {
        // Boundary: reset must NOT touch fetch_seq, otherwise a
        // pending fetch from before reset could land on a
        // post-reset open with a matching (zeroed) token. Catches
        // a defensive `fetch_seq.set(0)` insertion.
        with_owner(|| {
            let f = ContextFetchState::new();
            let t1 = f.begin();
            f.reset();
            // Begin again — must produce a strictly larger token.
            let t2 = f.begin();
            assert!(t2 > t1);
            // The pre-reset token must still be considered stale.
            assert!(!f.finish_if_current(t1, vec![rec("x", "user")]));
        });
    }
}
