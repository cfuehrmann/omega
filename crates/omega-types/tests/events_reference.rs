//! All-26-variants `OmegaEvent` reference snapshot.
//!
//! This file is the living wire-format reference for `events.jsonl`.  It
//! contains exactly one example of every `OmegaEvent` variant, serialised
//! as JSON, so that any change to the persistence shape produces a visible
//! diff in `cargo insta review`.
//!
//! The snapshot deliberately includes a **correlated triple**:
//!
//! - `ToolCall` and `ToolResult` that share the same `id` (shown as `[id_1]`
//!   in both, proving the same value is in both events).
//! - `ToolCall` and `ToolResult` complete the tool-call lifecycle.
//!
//! The per-variant unit tests in `src/events.rs` stay — they pin specific
//! mutants the catalogue snapshot wouldn't reliably catch.
//!
//! SCHEMA-8 note: variants 21–26 cover the Phase 1b additive grammar
//! (`LlmResponseStarted`, `LlmResponseEnded`, `LlmResponseDiscarded`,
//! `TextBlock`, `ThinkingBlock`, `ToolUseBlock`).  Phase 6.5 removed
//! the legacy `LlmResponse` and `Compacted` variants.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use omega_types::OmegaEvent;
use omega_types::events::{
    AgentErrorEvent, ContinueMode, EffortChangedEvent, InterruptReason, LlmCallEvent,
    LlmErrorEvent, LlmResponseDiscardedEvent, LlmResponseEndedEvent, LlmResponseStartedEvent,
    LlmResponseUsage, LlmRetryEvent, LlmRetryReason, ModelChangedEvent, PauseRequestedEvent,
    ResumingSessionEvent, ServerStartedEvent, ServerStopOutcome, ServerStoppedEvent,
    SessionResumedEvent, SessionStartedEvent, TextBlockEvent, ThinkingBlockEvent, ToolCallEvent,
    ToolResultEvent, ToolUseBlockEvent, TransportErrorEvent, TurnContinuedEvent, TurnEndEvent,
    TurnInterruptedEvent, TurnMetrics, TurnPausedEvent, UsageIteration, UserMessageEvent,
};
use omega_types::ids::Origin;
use serde_json::json;

mod common;

// ---------------------------------------------------------------------------
// Shared fixtures
// ---------------------------------------------------------------------------

/// A fixed ISO timestamp — deterministic, human-readable.
const T: &str = "2024-01-15T12:00:00.000Z";

/// An example context hash (12 hex chars = 6 bytes of random).
const HASH: &str = "deadbeefcafe1234";

/// The shared Omega-issued correlation id for the
/// `ToolUseBlock` / `ToolCall` / `ToolResult` triple.
const CORR_ID: &str = "tc_ref_01";

/// LLM-issued `tool_use` id — only on `ToolUseBlockEvent`.
const TOOL_USE_ID: &str = "toolu_ref_01";

// ---------------------------------------------------------------------------
// Event factory
// ---------------------------------------------------------------------------

/// Build one representative example of every `OmegaEvent` variant.
///
/// The correlated pair (positions 6–7) uses the same `id` to demonstrate
/// id propagation.  Every other value is illustrative but realistic.
#[allow(clippy::too_many_lines)] // test fixture: 26 event variants, one per arm
fn all_26_events() -> Vec<OmegaEvent> {
    vec![
        // 1. SessionStarted
        OmegaEvent::SessionStarted(SessionStartedEvent {
            time: T.into(),
            session_id: "sess_abc123".into(),
            path: ".omega/sessions/20240115_120000".into(),
            model: "claude-sonnet-4-6".into(),
            effort: "medium".into(),
            system_prompt:
                "You are an expert assistant operating inside Omega, a software engineering agent harness."
                    .into(),
            omega_commit: "abc1234".into(),
            agent_time_zone: "Europe/Berlin".into(),
            origin: Origin::Root,
        }),
        // 2. ServerStarted
        OmegaEvent::ServerStarted(ServerStartedEvent { time: T.into() }),
        // 3. ServerStopped
        OmegaEvent::ServerStopped(ServerStoppedEvent {
            time: T.into(),
            outcome: ServerStopOutcome::Clean,
            reason: None,
        }),
        // 4. UserMessage
        OmegaEvent::UserMessage(UserMessageEvent {
            time: T.into(),
            content: "What files are in the current directory?".into(),
        }),
        // 5. LlmCall
        OmegaEvent::LlmCall(LlmCallEvent {
            time: T.into(),
            url: "https://api.anthropic.com/v1/messages".into(),
            model: "claude-sonnet-4-6".into(),
            context_hashes: vec![HASH.into()],
            cache_breakpoint_index: None,
            request_bytes: 1_234,
            request_summary: None,
        }),
        // 6. ToolCall — correlated triple, part 1
        OmegaEvent::ToolCall(ToolCallEvent {
            time: T.into(),
            tool_call_id: CORR_ID.into(),
            name: "list_dir".into(),
            input: json!({"path": "."}),
            context_hash: HASH.into(),
        }),
        // 7. ToolResult — correlated triple, part 2 (same tool_call_id)
        OmegaEvent::ToolResult(ToolResultEvent {
            time: T.into(),
            tool_call_id: CORR_ID.into(),
            name: "list_dir".into(),
            is_error: false,
            duration_ms: 8,
            output: "README.md\nsrc/\ntests/".into(),
        }),
        // 8. TurnEnd
        OmegaEvent::TurnEnd(TurnEndEvent {
            time: T.into(),
            metrics: TurnMetrics {
                input_tokens: 512,
                output_tokens: 64,
                cache_creation_tokens: Some(480),
                cache_read_tokens: None,
            },
        }),
        // 9. LlmError
        OmegaEvent::LlmError(LlmErrorEvent {
            time: T.into(),
            url: "https://api.anthropic.com/v1/messages".into(),
            error: "HTTP 429: rate limit exceeded".into(),
            http_status: Some(429),
        }),
        // 10. AgentError
        OmegaEvent::AgentError(AgentErrorEvent {
            time: T.into(),
            error: "Tool execution failed: permission denied".into(),
        }),
        // 11. TurnInterrupted
        OmegaEvent::TurnInterrupted(TurnInterruptedEvent {
            time: T.into(),
            reason: Some(InterruptReason::Aborted),
        }),
        // 12. LlmRetry
        OmegaEvent::LlmRetry(LlmRetryEvent {
            time: T.into(),
            attempt: 2,
            http_status: Some(429),
            wait_ms: 5_000,
            error: "rate limited".into(),
            retry_at: Some(T.into()),
            error_body: Some(json!({
                "type": "error",
                "error": {"type": "rate_limit_error", "message": "Too many requests"}
            })),
            reason: Some(LlmRetryReason::RetryAfter),
        }),
        // 13. ModelChanged
        OmegaEvent::ModelChanged(ModelChangedEvent {
            time: T.into(),
            model: "claude-opus-4-6".into(),
        }),
        // 14. EffortChanged
        OmegaEvent::EffortChanged(EffortChangedEvent {
            time: T.into(),
            effort: "high".into(),
        }),
        // 15. TransportError
        OmegaEvent::TransportError(TransportErrorEvent {
            time: T.into(),
            error: "WebSocket connection closed unexpectedly".into(),
            context: Some("client 192.168.1.42".into()),
        }),
        // 16. ResumingSession
        OmegaEvent::ResumingSession(ResumingSessionEvent {
            time: T.into(),
            resumed_from: "20240114_090000".into(),
            name: Some("prior session".into()),
            basis: "The agent fixed a bug in the parser.".into(),
        }),
        // 17. SessionResumed
        OmegaEvent::SessionResumed(SessionResumedEvent {
            time: T.into(),
            resumed_from: "20240114_090000".into(),
            summary: "Fixed a bug in the parser module.".into(),
        }),
        // 18. PauseRequested
        OmegaEvent::PauseRequested(PauseRequestedEvent { time: T.into() }),
        // 19. TurnPaused
        OmegaEvent::TurnPaused(TurnPausedEvent { time: T.into() }),
        // 20. TurnContinued
        OmegaEvent::TurnContinued(TurnContinuedEvent {
            time: T.into(),
            mode: ContinueMode::Manual,
        }),
        // ----- SCHEMA-8 additive variants ------------------------------------
        // 21. LlmResponseStarted — opener for a fresh provider stream.
        OmegaEvent::LlmResponseStarted(LlmResponseStartedEvent { time: T.into() }),
        // 22. LlmResponseEnded — successful close.  Carries usage with
        //     a populated `iterations` array (this is what makes a
        //     `Compacted` event redundant in the new grammar).
        OmegaEvent::LlmResponseEnded(LlmResponseEndedEvent {
            time: T.into(),
            stop_reason: "end_turn".into(),
            cleared_tool_uses: None,
            cleared_input_tokens: None,
            usage: LlmResponseUsage {
                input_tokens: 100,
                output_tokens: 50,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
                service_tier: None,
                iterations: Some(vec![
                    UsageIteration {
                        iteration_type: "compaction".into(),
                        input_tokens: 80,
                        output_tokens: 0,
                        cache_creation_input_tokens: Some(40),
                        cache_read_input_tokens: None,
                        service_tier: None,
                    },
                    UsageIteration {
                        iteration_type: "message".into(),
                        input_tokens: 20,
                        output_tokens: 50,
                        cache_creation_input_tokens: None,
                        cache_read_input_tokens: Some(40),
                        service_tier: None,
                    },
                ]),
            },
            context_hash: HASH.into(),
            response_summary: None,
        }),
        // 23. LlmResponseDiscarded — closer for an abandoned stream.
        OmegaEvent::LlmResponseDiscarded(LlmResponseDiscardedEvent { time: T.into() }),
        // 24. TextBlock — one complete text content block.
        OmegaEvent::TextBlock(TextBlockEvent {
            time: T.into(),
            text: "Hello, world.".into(),
            partial: false,
        }),
        // 25. ThinkingBlock — one complete thinking block (signature present).
        OmegaEvent::ThinkingBlock(ThinkingBlockEvent {
            time: T.into(),
            thinking: "Let me check the directory.".into(),
            signature: Some("sig_ref_01".into()),
            partial: false,
        }),
        // 26. ToolUseBlock — one complete tool_use content block.
        OmegaEvent::ToolUseBlock(ToolUseBlockEvent {
            time: T.into(),
            tool_call_id: CORR_ID.into(),
            tool_use_id: TOOL_USE_ID.into(),
            name: "list_dir".into(),
            input: json!({"path": "."}),
            partial: false,
        }),
    ]
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Snapshot every `OmegaEvent` variant in a single JSON array.
///
/// Two id fields appear on tool-related events:
///
///   * `tool_call_id` (Omega-issued) on `ToolUseBlockEvent`,
///     `ToolCallEvent`, and `ToolResultEvent` — the correlation key,
///     same value across the triple, redacted to `[id_1]`.
///   * `tool_use_id` (LLM-issued) on `ToolUseBlockEvent` only — the
///     transcript field from the provider's `tool_use` block,
///     redacted to `[id_2]`.
#[test]
fn all_26_variants_reference() {
    let events = all_26_events();
    assert_eq!(events.len(), 26, "exactly 26 OmegaEvent variants");

    let r = common::id_redactor();
    insta::assert_json_snapshot!(events, {
        "[].toolCallId" => r.redaction(),
        "[].toolUseId"  => r.redaction(),
    });
}
