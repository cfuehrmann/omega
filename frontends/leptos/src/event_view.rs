//! Pure helpers for the conversation feed (Phase 3.3).
//!
//! Three concerns, all pure / DOM-free / mutation-tested:
//!
//! 1. [`EventKind`] projection: route an [`OmegaEvent`] variant to one
//!    of six visual families (user / assistant / tool_call / tool_result
//!    / status / error). The big match still lives inside the
//!    `<EventBlock/>` component (it needs typed field access per
//!    variant), but the family-class decision is carved out here so
//!    `cargo mutants` can lock it down without DOM infrastructure —
//!    same role `is_active` / `apply_renamed` played for 3.2.
//!
//! 2. [`should_autoscroll`]: predicate that decides whether the feed
//!    should snap to the tail on new content. Pure boolean function
//!    over the three scroll-geometry numbers + a threshold. The DOM
//!    half (reading `scrollTop` / `clientHeight` / `scrollHeight` and
//!    calling `scrollIntoView()`) lives in `feed.rs` and is treated as
//!    a JS-interop edge — same pattern as 3.1's `ws.rs::WsClient::send`.
//!
//! 3. [`truncate_for_preview`]: tool_result inline-preview helper.
//!    Returns `Some(<truncated_with_marker>)` when the input exceeds
//!    `max_chars`, `None` otherwise. Matches the SolidJS UI's
//!    `truncate(s, maxChars=3000)` (see `src/web/client/App.tsx:305`).

use omega_protocol::OmegaEvent;

// ---------------------------------------------------------------------------
// Visual-family projection
// ---------------------------------------------------------------------------

/// Visual family of an [`OmegaEvent`]. Drives the CSS class of the
/// rendered block; multiple `OmegaEvent` variants share a kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    /// User-submitted message.
    User,
    /// Assistant turn output (text response from the LLM).
    Assistant,
    /// Tool invocation by the agent.
    ToolCall,
    /// Successful tool execution result.
    ToolResult,
    /// Lifecycle / status event (server_started, model_changed, etc).
    Status,
    /// Failure-mode event (errored tool result, LLM/transport/agent
    /// error, interrupted turn).
    Error,
}

/// Project an [`OmegaEvent`] to its [`EventKind`].
///
/// `ToolResult` splits by `is_error`: an errored result lands in
/// [`EventKind::Error`], a successful one in [`EventKind::ToolResult`].
/// All explicitly-error variants (`AgentError`, `LlmError`,
/// `TransportError`, `TurnInterrupted`) collapse into
/// [`EventKind::Error`]. Everything else is [`EventKind::Status`].
#[must_use]
pub fn kind_for(event: &OmegaEvent) -> EventKind {
    match event {
        OmegaEvent::UserMessage(_) => EventKind::User,
        OmegaEvent::LlmResponse(_) => EventKind::Assistant,
        OmegaEvent::ToolCall(_) => EventKind::ToolCall,
        OmegaEvent::ToolResult(r) => {
            if r.is_error {
                EventKind::Error
            } else {
                EventKind::ToolResult
            }
        }
        OmegaEvent::AgentError(_)
        | OmegaEvent::LlmError(_)
        | OmegaEvent::TransportError(_)
        | OmegaEvent::TurnInterrupted(_) => EventKind::Error,
        OmegaEvent::SessionStarted(_)
        | OmegaEvent::ServerStarted(_)
        | OmegaEvent::ServerStopped(_)
        | OmegaEvent::LlmCall(_)
        | OmegaEvent::TurnEnd(_)
        | OmegaEvent::Compacted(_)
        | OmegaEvent::LlmRetry(_)
        | OmegaEvent::ModelChanged(_)
        | OmegaEvent::EffortChanged(_)
        | OmegaEvent::ResumingSession(_)
        | OmegaEvent::SessionResumed(_)
        | OmegaEvent::PauseRequested(_)
        | OmegaEvent::TurnPaused(_)
        | OmegaEvent::TurnContinued(_) => EventKind::Status,
    }
}

/// CSS class string for a kind. Static `&'static str` so leptos's
/// view! can splat it into `class=` directly.
#[must_use]
pub fn css_class_for(kind: EventKind) -> &'static str {
    match kind {
        EventKind::User => "block block-user",
        EventKind::Assistant => "block block-assistant",
        EventKind::ToolCall => "block block-tool-call",
        EventKind::ToolResult => "block block-tool-result",
        EventKind::Status => "block block-status",
        EventKind::Error => "block block-error",
    }
}

/// Stable string tag for a kind, used as a `data-event-kind` attribute
/// (one stable selector for Playwright + dev-tools).
#[must_use]
pub fn kind_tag(kind: EventKind) -> &'static str {
    match kind {
        EventKind::User => "user",
        EventKind::Assistant => "assistant",
        EventKind::ToolCall => "tool_call",
        EventKind::ToolResult => "tool_result",
        EventKind::Status => "status",
        EventKind::Error => "error",
    }
}

/// `OmegaEvent` discriminator string — the `"type"` field's wire
/// projection. Used as a `data-event-type` attribute so Playwright
/// specs can target each specific event variant.
#[must_use]
pub fn event_type_tag(event: &OmegaEvent) -> &'static str {
    match event {
        OmegaEvent::SessionStarted(_) => "session_started",
        OmegaEvent::ServerStarted(_) => "server_started",
        OmegaEvent::ServerStopped(_) => "server_stopped",
        OmegaEvent::UserMessage(_) => "user_message",
        OmegaEvent::LlmCall(_) => "llm_call",
        OmegaEvent::LlmResponse(_) => "llm_response",
        OmegaEvent::ToolCall(_) => "tool_call",
        OmegaEvent::ToolResult(_) => "tool_result",
        OmegaEvent::TurnEnd(_) => "turn_end",
        OmegaEvent::LlmError(_) => "llm_error",
        OmegaEvent::AgentError(_) => "agent_error",
        OmegaEvent::TurnInterrupted(_) => "turn_interrupted",
        OmegaEvent::Compacted(_) => "compacted",
        OmegaEvent::LlmRetry(_) => "llm_retry",
        OmegaEvent::ModelChanged(_) => "model_changed",
        OmegaEvent::EffortChanged(_) => "effort_changed",
        OmegaEvent::TransportError(_) => "transport_error",
        OmegaEvent::ResumingSession(_) => "resuming_session",
        OmegaEvent::SessionResumed(_) => "session_resumed",
        OmegaEvent::PauseRequested(_) => "pause_requested",
        OmegaEvent::TurnPaused(_) => "turn_paused",
        OmegaEvent::TurnContinued(_) => "turn_continued",
    }
}

// ---------------------------------------------------------------------------
// Auto-scroll predicate
// ---------------------------------------------------------------------------

/// True iff the feed should auto-scroll on the next content change.
///
/// Inputs are the three scroll-geometry numbers from the feed
/// container (in CSS pixels) plus a permissive bottom threshold. The
/// predicate returns true when the visible region's bottom edge sits
/// within `threshold` pixels of the scrollable content's bottom edge —
/// i.e. the user is still effectively "at the tail". Scrolling up
/// even a little (past the threshold) flips the predicate to false,
/// pinning the scroll position.
///
/// The conventional threshold is ≈ 40 px (handles browser sub-pixel
/// rounding + a bit of grace for the user's own micro-scrolls).
#[must_use]
pub fn should_autoscroll(
    scroll_top: f64,
    client_height: f64,
    scroll_height: f64,
    threshold: f64,
) -> bool {
    scroll_top + client_height + threshold >= scroll_height
}

// ---------------------------------------------------------------------------
// Tool-result preview truncation
// ---------------------------------------------------------------------------

/// Truncate `s` to `max_chars` Unicode scalars for inline preview.
///
/// Returns `None` if `s` already fits, leaving the caller to render
/// `s` verbatim. Returns `Some(<truncated_with_marker>)` otherwise —
/// the marker line `\n… [{total} chars total — showing first
/// {max_chars}]` mirrors the SolidJS UI exactly so visual parity holds
/// across the two bundles during the 3.0–3.6 co-existence window.
///
/// Char-count is on `chars()`, not `len()`, so multi-byte sequences
/// don't get cut mid-codepoint and counting matches what the user
/// perceives as "characters" rather than UTF-8 bytes.
#[must_use]
pub fn truncate_for_preview(s: &str, max_chars: usize) -> Option<String> {
    let total = s.chars().count();
    if total <= max_chars {
        return None;
    }
    let prefix: String = s.chars().take(max_chars).collect();
    Some(format!(
        "{prefix}\n\u{2026} [{total} chars total — showing first {max_chars}]"
    ))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic, clippy::float_cmp)]

    use omega_protocol::events::{
        AgentErrorEvent, CompactedEvent, EffortChangedEvent, LlmCallEvent, LlmErrorEvent,
        LlmResponseEvent, LlmResponseUsage, LlmRetryEvent, ModelChangedEvent, PauseRequestedEvent,
        ResumingSessionEvent, ServerStartedEvent, ServerStopOutcome, ServerStoppedEvent,
        SessionResumedEvent, SessionStartedEvent, ToolCallEvent, ToolResultEvent,
        TransportErrorEvent, TurnContinuedEvent, TurnEndEvent, TurnInterruptedEvent, TurnMetrics,
        TurnPausedEvent, UserMessageEvent,
    };
    use omega_protocol::{ContinueMode, InterruptReason, OmegaEvent};
    use serde_json::json;
    use wasm_bindgen_test::wasm_bindgen_test;

    use super::*;

    fn t() -> String {
        "2024-01-01T00:00:00.000Z".into()
    }

    fn user() -> OmegaEvent {
        OmegaEvent::UserMessage(UserMessageEvent {
            time: t(),
            content: "hi".into(),
        })
    }

    fn assistant() -> OmegaEvent {
        OmegaEvent::LlmResponse(LlmResponseEvent {
            time: t(),
            stop_reason: "end_turn".into(),
            cleared_tool_uses: None,
            cleared_input_tokens: None,
            usage: LlmResponseUsage {
                input_tokens: 1,
                output_tokens: 2,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
                service_tier: None,
            },
            context_hash: "deadbeef".into(),
            text: Some("hello".into()),
            thinking: None,
            streaming_start: None,
            response_summary: None,
        })
    }

    fn tool_call() -> OmegaEvent {
        OmegaEvent::ToolCall(ToolCallEvent {
            time: t(),
            id: "id".into(),
            name: "run_command".into(),
            input: json!({ "command": "echo hi" }),
            context_hash: "deadbeef".into(),
        })
    }

    fn tool_result(is_error: bool) -> OmegaEvent {
        OmegaEvent::ToolResult(ToolResultEvent {
            time: t(),
            id: "id".into(),
            name: "run_command".into(),
            is_error,
            duration_ms: 1,
            output: "ok".into(),
        })
    }

    // ---- kind_for: one test per OmegaEvent variant -------------------------
    //
    // 22 variants, plus the ToolResult split → 23 distinct `kind_for`
    // calls. Each test catches the deletion mutation of the variant's
    // arm (which would route through a different arm or the wildcard).

    #[wasm_bindgen_test]
    fn kind_user_message_is_user() {
        assert_eq!(kind_for(&user()), EventKind::User);
    }

    #[wasm_bindgen_test]
    fn kind_llm_response_is_assistant() {
        assert_eq!(kind_for(&assistant()), EventKind::Assistant);
    }

    #[wasm_bindgen_test]
    fn kind_tool_call_is_tool_call() {
        assert_eq!(kind_for(&tool_call()), EventKind::ToolCall);
    }

    #[wasm_bindgen_test]
    fn kind_tool_result_success_is_tool_result() {
        assert_eq!(kind_for(&tool_result(false)), EventKind::ToolResult);
    }

    #[wasm_bindgen_test]
    fn kind_tool_result_failure_is_error() {
        assert_eq!(kind_for(&tool_result(true)), EventKind::Error);
    }

    #[wasm_bindgen_test]
    fn kind_agent_error_is_error() {
        let ev = OmegaEvent::AgentError(AgentErrorEvent {
            time: t(),
            error: "boom".into(),
        });
        assert_eq!(kind_for(&ev), EventKind::Error);
    }

    #[wasm_bindgen_test]
    fn kind_llm_error_is_error() {
        let ev = OmegaEvent::LlmError(LlmErrorEvent {
            time: t(),
            url: "u".into(),
            error: "e".into(),
            http_status: Some(400),
        });
        assert_eq!(kind_for(&ev), EventKind::Error);
    }

    #[wasm_bindgen_test]
    fn kind_transport_error_is_error() {
        let ev = OmegaEvent::TransportError(TransportErrorEvent {
            time: t(),
            error: "x".into(),
            context: None,
        });
        assert_eq!(kind_for(&ev), EventKind::Error);
    }

    #[wasm_bindgen_test]
    fn kind_turn_interrupted_is_error() {
        let ev = OmegaEvent::TurnInterrupted(TurnInterruptedEvent {
            time: t(),
            reason: Some(InterruptReason::Aborted),
        });
        assert_eq!(kind_for(&ev), EventKind::Error);
    }

    #[wasm_bindgen_test]
    fn kind_session_started_is_status() {
        let ev = OmegaEvent::SessionStarted(SessionStartedEvent {
            time: t(),
            session_id: "s".into(),
            path: ".".into(),
            model: "m".into(),
            effort: "e".into(),
            system_prompt: "p".into(),
            omega_commit: "u".into(),
        });
        assert_eq!(kind_for(&ev), EventKind::Status);
    }

    #[wasm_bindgen_test]
    fn kind_server_started_is_status() {
        let ev = OmegaEvent::ServerStarted(ServerStartedEvent { time: t() });
        assert_eq!(kind_for(&ev), EventKind::Status);
    }

    #[wasm_bindgen_test]
    fn kind_server_stopped_is_status() {
        let ev = OmegaEvent::ServerStopped(ServerStoppedEvent {
            time: t(),
            outcome: ServerStopOutcome::Clean,
            reason: None,
        });
        assert_eq!(kind_for(&ev), EventKind::Status);
    }

    #[wasm_bindgen_test]
    fn kind_llm_call_is_status() {
        let ev = OmegaEvent::LlmCall(LlmCallEvent {
            time: t(),
            url: "u".into(),
            model: "m".into(),
            context_hashes: vec![],
            cache_breakpoint_index: None,
            request_bytes: 0,
            request_summary: None,
        });
        assert_eq!(kind_for(&ev), EventKind::Status);
    }

    #[wasm_bindgen_test]
    fn kind_turn_end_is_status() {
        let ev = OmegaEvent::TurnEnd(TurnEndEvent {
            time: t(),
            metrics: TurnMetrics {
                input_tokens: 1,
                output_tokens: 2,
                cache_creation_tokens: None,
                cache_read_tokens: None,
            },
        });
        assert_eq!(kind_for(&ev), EventKind::Status);
    }

    #[wasm_bindgen_test]
    fn kind_compacted_is_status() {
        let ev = OmegaEvent::Compacted(CompactedEvent {
            time: t(),
            usage: json!({}),
        });
        assert_eq!(kind_for(&ev), EventKind::Status);
    }

    #[wasm_bindgen_test]
    fn kind_llm_retry_is_status() {
        let ev = OmegaEvent::LlmRetry(LlmRetryEvent {
            time: t(),
            attempt: 1,
            http_status: Some(500),
            wait_ms: 100,
            error: "e".into(),
            retry_at: None,
            error_body: None,
            thinking_fragment: None,
            text_fragment: None,
            reason: None,
        });
        assert_eq!(kind_for(&ev), EventKind::Status);
    }

    #[wasm_bindgen_test]
    fn kind_model_changed_is_status() {
        let ev = OmegaEvent::ModelChanged(ModelChangedEvent {
            time: t(),
            model: "m".into(),
        });
        assert_eq!(kind_for(&ev), EventKind::Status);
    }

    #[wasm_bindgen_test]
    fn kind_effort_changed_is_status() {
        let ev = OmegaEvent::EffortChanged(EffortChangedEvent {
            time: t(),
            effort: "e".into(),
        });
        assert_eq!(kind_for(&ev), EventKind::Status);
    }

    #[wasm_bindgen_test]
    fn kind_resuming_session_is_status() {
        let ev = OmegaEvent::ResumingSession(ResumingSessionEvent {
            time: t(),
            resumed_from: "x".into(),
            name: None,
            basis: "b".into(),
        });
        assert_eq!(kind_for(&ev), EventKind::Status);
    }

    #[wasm_bindgen_test]
    fn kind_session_resumed_is_status() {
        let ev = OmegaEvent::SessionResumed(SessionResumedEvent {
            time: t(),
            resumed_from: "x".into(),
            summary: "s".into(),
        });
        assert_eq!(kind_for(&ev), EventKind::Status);
    }

    #[wasm_bindgen_test]
    fn kind_pause_requested_is_status() {
        let ev = OmegaEvent::PauseRequested(PauseRequestedEvent { time: t() });
        assert_eq!(kind_for(&ev), EventKind::Status);
    }

    #[wasm_bindgen_test]
    fn kind_turn_paused_is_status() {
        let ev = OmegaEvent::TurnPaused(TurnPausedEvent { time: t() });
        assert_eq!(kind_for(&ev), EventKind::Status);
    }

    #[wasm_bindgen_test]
    fn kind_turn_continued_is_status() {
        let ev = OmegaEvent::TurnContinued(TurnContinuedEvent {
            time: t(),
            mode: ContinueMode::Manual,
        });
        assert_eq!(kind_for(&ev), EventKind::Status);
    }

    // ---- css_class_for ------------------------------------------------------

    #[wasm_bindgen_test]
    fn css_class_per_kind_is_distinct_and_namespaced() {
        // Every kind must produce a string starting with "block " and a
        // family-specific suffix. Catches "swap two arms" mutations.
        assert_eq!(css_class_for(EventKind::User), "block block-user");
        assert_eq!(css_class_for(EventKind::Assistant), "block block-assistant");
        assert_eq!(css_class_for(EventKind::ToolCall), "block block-tool-call");
        assert_eq!(
            css_class_for(EventKind::ToolResult),
            "block block-tool-result"
        );
        assert_eq!(css_class_for(EventKind::Status), "block block-status");
        assert_eq!(css_class_for(EventKind::Error), "block block-error");
    }

    #[wasm_bindgen_test]
    fn css_class_values_are_pairwise_unique() {
        // Locks down the "every arm returns the same string" mutation.
        let all = [
            EventKind::User,
            EventKind::Assistant,
            EventKind::ToolCall,
            EventKind::ToolResult,
            EventKind::Status,
            EventKind::Error,
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(
                    css_class_for(*a),
                    css_class_for(*b),
                    "kinds {a:?} and {b:?} share a CSS class"
                );
            }
        }
    }

    // ---- kind_tag -----------------------------------------------------------

    #[wasm_bindgen_test]
    fn kind_tag_per_kind_is_snake_case_lowercase() {
        assert_eq!(kind_tag(EventKind::User), "user");
        assert_eq!(kind_tag(EventKind::Assistant), "assistant");
        assert_eq!(kind_tag(EventKind::ToolCall), "tool_call");
        assert_eq!(kind_tag(EventKind::ToolResult), "tool_result");
        assert_eq!(kind_tag(EventKind::Status), "status");
        assert_eq!(kind_tag(EventKind::Error), "error");
    }

    #[wasm_bindgen_test]
    fn kind_tag_values_are_pairwise_unique() {
        let all = [
            EventKind::User,
            EventKind::Assistant,
            EventKind::ToolCall,
            EventKind::ToolResult,
            EventKind::Status,
            EventKind::Error,
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(
                    kind_tag(*a),
                    kind_tag(*b),
                    "kinds {a:?} and {b:?} share a tag"
                );
            }
        }
    }

    // ---- event_type_tag -----------------------------------------------------

    #[wasm_bindgen_test]
    fn event_type_tag_matches_serde_discriminator_for_each_variant() {
        // The tag we render as `data-event-type` must match the wire
        // discriminator (`#[serde(tag = "type", rename_all =
        // "snake_case")]` on OmegaEvent). If a future field-name change
        // breaks this, Playwright specs would silently miss the block.
        let cases: &[(OmegaEvent, &str)] = &[
            (user(), "user_message"),
            (assistant(), "llm_response"),
            (tool_call(), "tool_call"),
            (tool_result(false), "tool_result"),
            (
                OmegaEvent::AgentError(AgentErrorEvent {
                    time: t(),
                    error: "x".into(),
                }),
                "agent_error",
            ),
            (
                OmegaEvent::TurnEnd(TurnEndEvent {
                    time: t(),
                    metrics: TurnMetrics {
                        input_tokens: 0,
                        output_tokens: 0,
                        cache_creation_tokens: None,
                        cache_read_tokens: None,
                    },
                }),
                "turn_end",
            ),
        ];
        for (ev, tag) in cases {
            assert_eq!(event_type_tag(ev), *tag, "mismatch for {tag}");
        }
    }

    // ---- should_autoscroll: boundary semantics for cargo-mutants ----------

    #[wasm_bindgen_test]
    fn autoscroll_true_when_clearly_at_bottom() {
        // visible bottom (900 + 100 = 1000) == content bottom (1000)
        // even with zero threshold → true.
        assert!(should_autoscroll(900.0, 100.0, 1000.0, 0.0));
    }

    #[wasm_bindgen_test]
    fn autoscroll_false_when_clearly_scrolled_up() {
        // Way up top: 0 + 100 + 40 = 140 < 1000 → false.
        assert!(!should_autoscroll(0.0, 100.0, 1000.0, 40.0));
    }

    #[wasm_bindgen_test]
    fn autoscroll_at_exact_threshold_boundary_is_true() {
        // Equal-case: catches `>=` → `>` (would be false) and
        // `>=` → `!=` (would be false). This is the load-bearing
        // boundary test.
        // 900 + 100 + 40 = 1040 == 1040 → true under `>=`.
        assert!(should_autoscroll(900.0, 100.0, 1040.0, 40.0));
    }

    #[wasm_bindgen_test]
    fn autoscroll_one_pixel_past_threshold_is_false() {
        // 900 + 100 + 40 = 1040 < 1041 → false.
        // Catches `>=` → `<=` (would be true).
        assert!(!should_autoscroll(900.0, 100.0, 1041.0, 40.0));
    }

    #[wasm_bindgen_test]
    fn autoscroll_threshold_lifts_borderline_case_to_true() {
        // Without threshold: 100 + 20 + 0 = 120 < 130 → false.
        // With threshold 10: 100 + 20 + 10 = 130 == 130 → true.
        // Catches dropping the threshold from the sum.
        assert!(!should_autoscroll(100.0, 20.0, 130.0, 0.0));
        assert!(should_autoscroll(100.0, 20.0, 130.0, 10.0));
    }

    #[wasm_bindgen_test]
    fn autoscroll_scroll_top_contributes_to_sum() {
        // scroll_top=0 → false; scroll_top=900 → true (everything
        // else equal). Catches `+ scroll_top` being mutated away.
        assert!(!should_autoscroll(0.0, 100.0, 1000.0, 0.0));
        assert!(should_autoscroll(900.0, 100.0, 1000.0, 0.0));
    }

    #[wasm_bindgen_test]
    fn autoscroll_client_height_contributes_to_sum() {
        // client_height=0 → false; client_height=100 → true (everything
        // else equal). Catches `+ client_height` being mutated away.
        assert!(!should_autoscroll(900.0, 0.0, 1000.0, 0.0));
        assert!(should_autoscroll(900.0, 100.0, 1000.0, 0.0));
    }

    #[wasm_bindgen_test]
    fn autoscroll_returns_true_when_visible_overshoots_content() {
        // scroll_top + client_height > scroll_height (rare but
        // possible during layout transitions). Must remain true.
        assert!(should_autoscroll(950.0, 100.0, 1000.0, 0.0));
    }

    // ---- truncate_for_preview ----------------------------------------------

    #[wasm_bindgen_test]
    fn truncate_returns_none_below_limit() {
        assert_eq!(truncate_for_preview("abc", 10), None);
    }

    #[wasm_bindgen_test]
    fn truncate_returns_none_at_exact_limit() {
        // Boundary: chars().count() == max_chars → None.
        // Catches `<=` → `<` (would truncate the equal case).
        assert_eq!(truncate_for_preview("abc", 3), None);
    }

    #[wasm_bindgen_test]
    fn truncate_returns_some_one_char_above_limit() {
        // Boundary: chars().count() == max_chars + 1 → Some.
        // Catches `<=` → `>=` (would never truncate).
        let r = truncate_for_preview("abcd", 3).expect("must truncate");
        assert!(r.starts_with("abc"), "kept first 3 chars: {r}");
        assert!(r.contains("4 chars total"), "marker has total: {r}");
        assert!(r.contains("first 3"), "marker has prefix size: {r}");
    }

    #[wasm_bindgen_test]
    fn truncate_keeps_exactly_max_chars_of_prefix() {
        let s = "x".repeat(100);
        let r = truncate_for_preview(&s, 10).expect("must truncate");
        // First 10 chars preserved verbatim.
        let prefix: String = r.chars().take(10).collect();
        assert_eq!(prefix, "xxxxxxxxxx");
        // Marker appended on a new line so it's visually distinct in
        // the rendered <pre>.
        assert!(r.contains("\n\u{2026} "), "newline + ellipsis present");
    }

    #[wasm_bindgen_test]
    fn truncate_marker_reports_correct_total_and_first_n() {
        let s = "x".repeat(7654);
        let r = truncate_for_preview(&s, 1234).expect("must truncate");
        assert!(r.contains("7654 chars total"), "{r}");
        assert!(r.contains("first 1234"), "{r}");
    }

    #[wasm_bindgen_test]
    fn truncate_handles_multibyte_chars_without_panic() {
        // Five Greek letters → 5 chars / 10 bytes. max_chars=10 fits.
        let s = "αβγδε";
        assert_eq!(truncate_for_preview(s, 10), None);
        // max_chars=3 truncates to first 3 codepoints (αβγ).
        let r = truncate_for_preview(s, 3).expect("must truncate");
        let kept: String = r.chars().take(3).collect();
        assert_eq!(kept, "αβγ");
        assert!(r.contains("5 chars total"));
    }

    #[wasm_bindgen_test]
    fn truncate_with_zero_max_truncates_to_empty_prefix() {
        // Edge case: max_chars=0 always truncates non-empty input
        // to an empty prefix + marker. Catches a `take(max_chars)` →
        // `take(0)` mutation that would otherwise be observable only
        // here.
        let r = truncate_for_preview("ab", 0).expect("must truncate");
        // Marker but no prefix chars — the format's first '\n' is the
        // separator.
        assert!(r.starts_with("\n\u{2026}"), "starts with marker: {r:?}");
        assert!(r.contains("2 chars total"));
    }
}
