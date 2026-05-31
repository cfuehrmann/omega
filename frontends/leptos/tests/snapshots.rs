//! Phase 3.6 — Snapshot harness (TEST-ARCH-5).
//!
//! Runs on the **host** target with `--no-default-features --features ssr`
//! so leptos's host SSR codepath is available. The corresponding
//! `just web-leptos-snapshots` recipe drives this harness.
//!
//! Coverage:
//! * Per-`OmegaEvent` family for the feed (`<EventBlock />`).
//! * `<MarkdownBody />` over representative assistant turns (code
//!   block, list, table, mermaid, diff, embedded HTML).
//! * Per-`TurnState` for `<Composer />`.
//! * Per-modal-state for `<ContextModal />`.
//!
//! ## Why this differs from the original Phase 3.6 plan
//!
//! The plan suggested `leptos::ssr::render_to_string` from inside a
//! wasm32 test. That doesn't work: `csr` and `ssr` are mutually
//! exclusive leptos features. The cleanest split is option (a) from
//! the plan: split lib + bin, flip features at `cargo test
//! --features ssr` time. The lib code is feature-agnostic; only the
//! rendering harness picks a side.
//!
//! Leptos's SSR injects `data-hk="..."` hydration markers and
//! `<!--hk=...-->` comments. We strip them via [`scrub_dynamic`].

#![cfg(feature = "ssr")]

use leptos::prelude::*;
use leptos::reactive::owner::Owner;
use leptos::tachys::view::RenderHtml;
use omega_types::FeatureFlags;
use omega_types::OmegaEvent;
use omega_types::events::{
    AgentErrorEvent, ContextCompactedEvent, LlmCallEvent, LlmResponseDiscardedEvent,
    LlmResponseEndedEvent, LlmResponseUsage, ResumingSessionEvent, SessionResumedEvent,
    SessionStartedEvent, TextBlockEvent, ThinkingBlockEvent, ToolCallEvent, ToolResultEvent,
    ToolUseBlockEvent, TurnEndEvent, TurnMetrics, UsageIteration, UserMessageEvent,
};
use omega_types::ids::{Origin, SessionId};
use omega_web::context_modal::{ContextModal, ContextModalState};
use omega_web::feed::{EventBlock, MarkdownBody};
use omega_web::picker::PickerOpen;
use omega_web::protocol::{SessionInfoPayload, TurnState};
use omega_web::text_modal::TextModalState;
use omega_web::usage_panel::UsagePanelOpen;

// ---------------------------------------------------------------------------
// Scrubbing
// ---------------------------------------------------------------------------

fn scrub_dynamic(html: &str) -> String {
    // Walk char-by-char (not byte-by-byte) so multi-byte UTF-8 like
    // `·` and `✕` round-trip cleanly. Whenever we hit one of
    // the two leptos hydration markers, jump past it. Otherwise emit
    // the next char.
    let mut out = String::with_capacity(html.len());
    let mut idx = 0;
    while idx < html.len() {
        let rest = &html[idx..];
        if let Some(stripped) = rest.strip_prefix(" data-hk=\"")
            && let Some(end) = stripped.find('"')
        {
            idx += rest.len() - stripped.len() + end + 1;
            continue;
        }
        if rest.starts_with("<!--hk=")
            && let Some(close) = rest.find("-->")
        {
            idx += close + 3;
            continue;
        }
        let ch = rest.chars().next().unwrap();
        out.push(ch);
        idx += ch.len_utf8();
    }
    out
}

fn render<F, V>(builder: F) -> String
where
    F: FnOnce() -> V + 'static,
    V: IntoView + RenderHtml + 'static,
{
    let owner = Owner::new();
    let html = owner.with(|| builder().to_html());
    drop(owner);
    scrub_dynamic(&html)
}

// ---------------------------------------------------------------------------
// Fixtures — one per OmegaEvent family
// ---------------------------------------------------------------------------

fn ev_user(content: &str) -> OmegaEvent {
    OmegaEvent::UserMessage(UserMessageEvent {
        time: "2025-01-01T00:00:00.000Z".into(),
        content: content.into(),
    })
}

fn assistant_usage() -> LlmResponseUsage {
    LlmResponseUsage {
        input_tokens: 10,
        output_tokens: 5,
        cache_read_input_tokens: None,
        cache_creation_input_tokens: None,
        service_tier: None,
        iterations: None,
    }
}

/// SCHEMA-8 Phase 4c: assistant text now lives in `TextBlockEvent`,
/// not in `LlmResponseEvent.text` (band-aid is still on the wire but
/// the renderer is muted).  All `snap_event_assistant_*` snapshots
/// drive the new `TextBlock` renderer here — same `MarkdownBody`
/// surface, same `data-testid="leptos-assistant-text"` wrapper.
fn ev_assistant(text: &str) -> OmegaEvent {
    OmegaEvent::TextBlock(TextBlockEvent {
        time: "2025-01-01T00:00:01.000Z".into(),
        text: text.into(),
        partial: false,
    })
}

/// SCHEMA-8 Phase 5b — fixtures for the three partial-block
/// renderers.  The agent mints `partial: true` block events just
/// before `LlmResponseDiscarded` on mid-stream abandonment
/// (retry-on-transient-error).  The renderers stamp `data-partial=
/// "true"` on the outer wrapper plus `block-discarded-{header,body}`
/// classes on the inner pieces so CSS can grey + strike-through the
/// content while keeping the disclaimer readable.
fn ev_assistant_partial(text: &str) -> OmegaEvent {
    OmegaEvent::TextBlock(TextBlockEvent {
        time: "2025-01-01T00:00:01.000Z".into(),
        text: text.into(),
        partial: true,
    })
}

fn ev_thinking_partial(thinking: &str) -> OmegaEvent {
    OmegaEvent::ThinkingBlock(ThinkingBlockEvent {
        time: "2025-01-01T00:00:01.250Z".into(),
        thinking: thinking.into(),
        signature: None,
        partial: true,
    })
}

fn ev_thinking(thinking: &str) -> OmegaEvent {
    OmegaEvent::ThinkingBlock(ThinkingBlockEvent {
        time: "2025-01-01T00:00:01.250Z".into(),
        thinking: thinking.into(),
        signature: Some("sig_xyz".into()),
        partial: false,
    })
}

fn ev_tool_use_partial(name: &str, input: serde_json::Value) -> OmegaEvent {
    OmegaEvent::ToolUseBlock(ToolUseBlockEvent {
        time: "2025-01-01T00:00:01.500Z".into(),
        tool_call_id: "tc_partial".into(),
        tool_use_id: "toolu_partial".into(),
        name: name.into(),
        input,
        partial: true,
    })
}

fn ev_tool_use(name: &str, input: serde_json::Value) -> OmegaEvent {
    OmegaEvent::ToolUseBlock(ToolUseBlockEvent {
        time: "2025-01-01T00:00:01.500Z".into(),
        tool_call_id: "tc_test".into(),
        tool_use_id: "toolu_complete".into(),
        name: name.into(),
        input,
        partial: false,
    })
}

/// New affordance row: stop-reason label + `[context]` + `[payload]`
/// + usage line.  No body, no thinking button (those live in sibling
/// `TextBlock` / `ThinkingBlock` events).
fn ev_llm_response_ended() -> OmegaEvent {
    OmegaEvent::LlmResponseEnded(LlmResponseEndedEvent {
        time: "2025-01-01T00:00:01.500Z".into(),
        stop_reason: "end_turn".into(),
        cleared_tool_uses: None,
        cleared_input_tokens: None,
        usage: assistant_usage(),
        context_hash: "abcd1234ef560000".into(),
        response_summary: None,
    })
}

/// SCHEMA-8 Phase 5f — LlmResponseEnded with a server-side compaction
/// entry in `usage.iterations`.  Drives the `[compacted]` badge in the
/// label row.
fn ev_llm_response_ended_compacted() -> OmegaEvent {
    OmegaEvent::LlmResponseEnded(LlmResponseEndedEvent {
        time: "2025-01-01T00:00:01.500Z".into(),
        stop_reason: "end_turn".into(),
        cleared_tool_uses: None,
        cleared_input_tokens: None,
        usage: LlmResponseUsage {
            input_tokens: 32,
            output_tokens: 7,
            cache_read_input_tokens: None,
            cache_creation_input_tokens: None,
            service_tier: None,
            iterations: Some(vec![
                UsageIteration {
                    iteration_type: "compaction".into(),
                    input_tokens: 4096,
                    output_tokens: 32,
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                    service_tier: None,
                },
                UsageIteration {
                    iteration_type: "message".into(),
                    input_tokens: 32,
                    output_tokens: 7,
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                    service_tier: None,
                },
            ]),
        },
        context_hash: "abcd1234ef560000".into(),
        response_summary: None,
    })
}

fn ev_tool_call() -> OmegaEvent {
    OmegaEvent::ToolCall(ToolCallEvent {
        time: "2025-01-01T00:00:02.000Z".into(),
        tool_call_id: "tc_test".into(),
        name: "run_command".into(),
        input: serde_json::json!({ "command": "echo hi" }),
        context_hash: "abcd1234ef560000".into(),
    })
}

fn ev_tool_result(out: &str, is_error: bool) -> OmegaEvent {
    OmegaEvent::ToolResult(ToolResultEvent {
        time: "2025-01-01T00:00:03.000Z".into(),
        tool_call_id: "tc_test".into(),
        name: "run_command".into(),
        output: out.into(),
        is_error,
        duration_ms: 42,
    })
}

fn ev_turn_end() -> OmegaEvent {
    OmegaEvent::TurnEnd(TurnEndEvent {
        time: "2025-01-01T00:00:04.000Z".into(),
        metrics: TurnMetrics {
            input_tokens: 100,
            output_tokens: 20,
            cache_creation_tokens: None,
            cache_read_tokens: None,
        },
    })
}

fn ev_session_started() -> OmegaEvent {
    OmegaEvent::SessionStarted(SessionStartedEvent {
        time: "2025-01-01T00:00:00.000Z".into(),
        session_id: "018f4c2e-3a1b-7d00-8000-abcdef012345"
            .parse::<SessionId>()
            .unwrap(),
        path: ".omega/sessions/2025-01-01T00-00-00-000-aaaaaaaa".into(),
        model: "claude-sonnet-4-6".into(),
        effort: "medium".into(),
        system_prompt: "system: test".into(),
        omega_commit: "abc1234".into(),
        agent_time_zone: "Europe/Berlin".into(),
        origin: Origin::Root,
        features: FeatureFlags::default(),
        tool_selection: Vec::new(),
    })
}

fn ev_agent_error() -> OmegaEvent {
    OmegaEvent::AgentError(AgentErrorEvent {
        time: "2025-01-01T00:00:09.000Z".into(),
        error: "something exploded".into(),
    })
}

fn ev_llm_call() -> OmegaEvent {
    OmegaEvent::LlmCall(LlmCallEvent {
        time: "2025-01-01T00:00:01.000Z".into(),
        url: "https://api.anthropic.com/v1/messages".into(),
        model: "claude-sonnet-4-6".into(),
        context_hashes: vec!["aaaaaaaaaaaa0000".into(), "bbbbbbbbbbbb0000".into()],
        cache_breakpoint_index: Some(2),
        request_bytes: 1234,
        request_summary: Some(serde_json::json!({"system": "test"})),
    })
}

fn ev_resuming() -> OmegaEvent {
    OmegaEvent::ResumingSession(ResumingSessionEvent {
        time: "2025-01-01T00:00:05.000Z".into(),
        resumed_from: "2024-01-01T00-00-00-000-source".into(),
        name: None,
        basis: "ABCD".into(),
    })
}

fn ev_session_resumed() -> OmegaEvent {
    OmegaEvent::SessionResumed(SessionResumedEvent {
        time: "2025-01-01T00:00:06.000Z".into(),
        resumed_from: "2024-01-01T00-00-00-000-source".into(),
        summary: "**Resumed** with progress.".into(),
    })
}

/// Phase 2.0 (F11): server-side context compaction event.
fn ev_context_compacted() -> OmegaEvent {
    OmegaEvent::ContextCompacted(ContextCompactedEvent {
        time: "2025-01-01T00:00:01.750Z".into(),
        tokens_before: 80_000,
        tokens_after: 500,
        summary_tokens: 300,
    })
}

// ---------------------------------------------------------------------------
// EventBlock — per-OmegaEvent family
// ---------------------------------------------------------------------------

#[test]
fn snap_event_user_message() {
    let html = render(|| {
        let ev = ev_user("hello world");
        view! { <EventBlock event=ev /> }
    });
    insta::assert_snapshot!(html);
}

#[test]
fn snap_event_assistant_plain_text() {
    let html = render(|| {
        let ev = ev_assistant("plain assistant text");
        provide_context(ContextModalState::new());
        provide_context(TextModalState::new());
        view! { <EventBlock event=ev /> }
    });
    insta::assert_snapshot!(html);
}

#[test]
fn snap_event_assistant_markdown_code_block() {
    let html = render(|| {
        let ev = ev_assistant("Here is code:\n\n```rust\nlet x = 1;\n```\n");
        provide_context(ContextModalState::new());
        provide_context(TextModalState::new());
        view! { <EventBlock event=ev /> }
    });
    insta::assert_snapshot!(html);
}

#[test]
fn snap_event_assistant_markdown_list() {
    let html = render(|| {
        let ev = ev_assistant("Steps:\n\n- first\n- second\n- third\n");
        provide_context(ContextModalState::new());
        provide_context(TextModalState::new());
        view! { <EventBlock event=ev /> }
    });
    insta::assert_snapshot!(html);
}

#[test]
fn snap_event_assistant_markdown_table() {
    let html = render(|| {
        let ev = ev_assistant("| a | b |\n|---|---|\n| 1 | 2 |\n");
        provide_context(ContextModalState::new());
        provide_context(TextModalState::new());
        view! { <EventBlock event=ev /> }
    });
    insta::assert_snapshot!(html);
}

#[test]
fn snap_event_assistant_mermaid() {
    let html = render(|| {
        let ev = ev_assistant("```mermaid\ngraph LR\n  A --> B\n```\n");
        provide_context(ContextModalState::new());
        provide_context(TextModalState::new());
        view! { <EventBlock event=ev /> }
    });
    insta::assert_snapshot!(html);
}

#[test]
fn snap_event_assistant_diff() {
    let html = render(|| {
        let ev = ev_assistant("```diff\n+ added\n- removed\n```\n");
        provide_context(ContextModalState::new());
        provide_context(TextModalState::new());
        view! { <EventBlock event=ev /> }
    });
    insta::assert_snapshot!(html);
}

#[test]
fn snap_event_text_block_partial_discarded() {
    let html = render(|| {
        let ev = ev_assistant_partial("partial text never settled");
        provide_context(ContextModalState::new());
        provide_context(TextModalState::new());
        view! { <EventBlock event=ev /> }
    });
    insta::assert_snapshot!(html);
}

#[test]
fn snap_event_thinking_block_partial_discarded() {
    let html = render(|| {
        let ev = ev_thinking_partial("thinking interrupted mid-stream");
        provide_context(ContextModalState::new());
        provide_context(TextModalState::new());
        view! { <EventBlock event=ev /> }
    });
    insta::assert_snapshot!(html);
}

#[test]
fn snap_event_thinking_block_collapsed() {
    // Settled ThinkingBlock with enough virtual lines to exceed the
    // virtual_line_count(_, 80) > 4 toggle gate: renders clamped with an
    // always-visible "more" button; no TextModal.
    let html = render(|| {
        let ev = ev_thinking(
            "step one\nstep two\nstep three\nstep four\nstep five — exceeds 4-line clamp",
        );
        provide_context(ContextModalState::new());
        provide_context(TextModalState::new());
        view! { <EventBlock event=ev /> }
    });
    assert!(
        html.contains("data-testid=\"leptos-thinking-block-expand\""),
        "toggle button must be present when virtual_line_count > 4:\n{html}",
    );
    insta::assert_snapshot!(html);
}

#[test]
fn snap_event_thinking_block_no_toggle_at_four_lines() {
    // Four short hard lines → virtual_line_count(text, 80) == 4 → NOT > 4
    // → more/less button must NOT be rendered.
    let html = render(|| {
        let ev = ev_thinking("line one\nline two\nline three\nline four");
        provide_context(ContextModalState::new());
        provide_context(TextModalState::new());
        view! { <EventBlock event=ev /> }
    });
    assert!(
        !html.contains("data-testid=\"leptos-thinking-block-expand\""),
        "toggle button must be absent when virtual_line_count == 4:\n{html}",
    );
    insta::assert_snapshot!(html);
}

#[test]
fn snap_event_tool_use_block_partial_discarded() {
    let html = render(|| {
        let ev = ev_tool_use_partial(
            "run_command",
            serde_json::json!({ "command": "echo partial" }),
        );
        provide_context(ContextModalState::new());
        provide_context(TextModalState::new());
        view! { <EventBlock event=ev /> }
    });
    insta::assert_snapshot!(html);
}

#[test]
fn snap_event_tool_use_block_with_toggle() {
    // SCHEMA-8 Phase 5d (revised) — non-partial ToolUseBlock renders
    // an unconditional more/less toggle button.  The body <pre> is absent
    // in the collapsed (initial) state; the toggle button is always present.
    let html = render(|| {
        let ev = ev_tool_use(
            "run_command",
            serde_json::json!({ "command": "ls -la", "timeout_s": 30 }),
        );
        provide_context(ContextModalState::new());
        provide_context(TextModalState::new());
        view! { <EventBlock event=ev /> }
    });
    insta::assert_snapshot!(html);
}

#[test]
fn snap_event_tool_use_python_repl_default_timeout_collapsed() {
    // Phase 2.3 — python_repl ToolUseBlock: first non-blank line shown as
    // preview; no timeout chip (default timeout omitted).  Collapsed state.
    let html = render(|| {
        let ev = ev_tool_use(
            "python_repl",
            serde_json::json!({ "code": "out, err, rc = sh(\"ls -la\")\nprint(out)" }),
        );
        provide_context(ContextModalState::new());
        provide_context(TextModalState::new());
        view! { <EventBlock event=ev /> }
    });
    insta::assert_snapshot!(html);
}

#[test]
fn snap_event_tool_use_python_repl_non_default_timeout_chip() {
    // Phase 2.3 — timeout chip appears when timeout != default (60 s).
    let html = render(|| {
        let ev = ev_tool_use(
            "python_repl",
            serde_json::json!({ "code": "import time\ntime.sleep(1)", "timeout": 1800 }),
        );
        provide_context(ContextModalState::new());
        provide_context(TextModalState::new());
        view! { <EventBlock event=ev /> }
    });
    insta::assert_snapshot!(html);
}

#[test]
fn snap_event_tool_use_python_repl_empty_code() {
    // Phase 2.3 — empty code: label row still renders cleanly (no preview text).
    let html = render(|| {
        let ev = ev_tool_use("python_repl", serde_json::json!({ "code": "" }));
        provide_context(ContextModalState::new());
        provide_context(TextModalState::new());
        view! { <EventBlock event=ev /> }
    });
    insta::assert_snapshot!(html);
}

#[test]
fn snap_event_assistant_html_is_escaped() {
    let html = render(|| {
        let ev = ev_assistant("hello <script>alert(1)</script>");
        provide_context(ContextModalState::new());
        provide_context(TextModalState::new());
        view! { <EventBlock event=ev /> }
    });
    assert!(
        !html.contains("<script>"),
        "raw HTML survived markdown render: {html}",
    );
    insta::assert_snapshot!(html);
}

#[test]
fn snap_event_llm_response_ended() {
    let html = render(|| {
        let ev = ev_llm_response_ended();
        provide_context(ContextModalState::new());
        provide_context(TextModalState::new());
        view! { <EventBlock event=ev /> }
    });
    insta::assert_snapshot!(html);
}

#[test]
fn snap_event_llm_response_discarded_with_partial_count() {
    // SCHEMA-8 Phase 5g — a `LlmResponseDiscarded` event rendered
    // alongside `partial_count=Some(3)` (as the live `ConversationFeed`
    // computes via `assign_partial_counts`) must surface an `N partial
    // blocks` meta line so the operator can tell "network blip before
    // any content" (0) from "discarded after N partials" (>0).
    let html = render(|| {
        let ev = OmegaEvent::LlmResponseDiscarded(LlmResponseDiscardedEvent {
            time: "2025-01-01T00:00:01.000Z".into(),
        });
        view! { <EventBlock event=ev partial_count=Some(3) /> }
    });
    insta::assert_snapshot!(html);
}

#[test]
fn snap_event_llm_response_discarded_zero_partials() {
    // Zero partials — the meta line still renders with `0 partial blocks`
    // to disambiguate "abandoned immediately after `LlmResponseStarted`"
    // (no content streamed) from "never had a `partial_count` to begin
    // with" (snapshot-harness fixtures that omit the prop).
    let html = render(|| {
        let ev = OmegaEvent::LlmResponseDiscarded(LlmResponseDiscardedEvent {
            time: "2025-01-01T00:00:01.000Z".into(),
        });
        view! { <EventBlock event=ev partial_count=Some(0) /> }
    });
    insta::assert_snapshot!(html);
}

#[test]
fn snap_event_llm_response_ended_compacted() {
    // SCHEMA-8 Phase 5f — a response whose usage carries an
    // `iterations` array including a `type="compaction"` entry must
    // surface a yellow `[compacted]` badge in the label row.  This
    // pins the badge presence + ordering and verifies the
    // `[context]` / `[payload]` buttons remain on the row.
    let html = render(|| {
        let ev = ev_llm_response_ended_compacted();
        provide_context(ContextModalState::new());
        provide_context(TextModalState::new());
        view! { <EventBlock event=ev /> }
    });
    insta::assert_snapshot!(html);
}

#[test]
fn snap_event_context_compacted() {
    let html = render(|| {
        let ev = ev_context_compacted();
        view! { <EventBlock event=ev /> }
    });
    insta::assert_snapshot!(html);
}

#[test]
fn snap_event_tool_call() {
    let html = render(|| {
        let ev = ev_tool_call();
        provide_context(TextModalState::new());
        view! { <EventBlock event=ev /> }
    });
    insta::assert_snapshot!(html);
}

#[test]
fn snap_event_tool_call_with_corr_badge() {
    // Peer-event slim ToolCallBlock with a corr badge for a
    // multi-call group.  Verifies the layout is
    // [corr-badge] "tool call" timestamp  (no name, no preview, no on:click).
    let html = render(|| {
        let ev = ev_tool_call();
        provide_context(TextModalState::new());
        view! { <EventBlock event=ev corr=Some(2) /> }
    });
    insta::assert_snapshot!(html);
}

#[test]
fn snap_event_tool_use_block_with_corr_badge() {
    // SCHEMA-8 Phase 5e — ToolUseBlock paired with its sibling ToolCall
    // via the same provider tool_use_id; the corr badge is rendered at
    // the start of the row alongside the modal-opening label.
    let html = render(|| {
        let ev = ev_tool_use("run_command", serde_json::json!({ "command": "ls -la" }));
        provide_context(ContextModalState::new());
        provide_context(TextModalState::new());
        view! { <EventBlock event=ev corr=Some(2) /> }
    });
    insta::assert_snapshot!(html);
}

#[test]
fn snap_event_tool_result_ok() {
    let html = render(|| {
        let ev = ev_tool_result("hi\n", false);
        provide_context(TextModalState::new());
        view! { <EventBlock event=ev /> }
    });
    insta::assert_snapshot!(html);
}

#[test]
fn snap_event_tool_result_error() {
    let html = render(|| {
        let ev = ev_tool_result("boom\n", true);
        provide_context(TextModalState::new());
        view! { <EventBlock event=ev /> }
    });
    insta::assert_snapshot!(html);
}

#[test]
fn snap_event_turn_end() {
    let html = render(|| {
        let ev = ev_turn_end();
        view! { <EventBlock event=ev /> }
    });
    insta::assert_snapshot!(html);
}

#[test]
fn snap_event_session_started() {
    let html = render(|| {
        let ev = ev_session_started();
        provide_context(TextModalState::new());
        view! { <EventBlock event=ev /> }
    });
    insta::assert_snapshot!(html);
}

#[test]
fn snap_event_agent_error() {
    let html = render(|| {
        let ev = ev_agent_error();
        view! { <EventBlock event=ev /> }
    });
    insta::assert_snapshot!(html);
}

#[test]
fn snap_event_llm_call() {
    let html = render(|| {
        let ev = ev_llm_call();
        provide_context(ContextModalState::new());
        provide_context(TextModalState::new());
        view! { <EventBlock event=ev /> }
    });
    insta::assert_snapshot!(html);
}

#[test]
fn snap_event_resuming_session() {
    let html = render(|| {
        let ev = ev_resuming();
        view! { <EventBlock event=ev /> }
    });
    insta::assert_snapshot!(html);
}

#[test]
fn snap_event_session_resumed_markdown() {
    let html = render(|| {
        let ev = ev_session_resumed();
        view! { <EventBlock event=ev /> }
    });
    // Mid-summary `**bold**` should render as `<strong>` because
    // session_resumed now pipes through MarkdownBody.
    assert!(html.contains("<strong>Resumed</strong>"), "{html}");
    insta::assert_snapshot!(html);
}

// ---------------------------------------------------------------------------
// MarkdownBody — standalone fixtures
// ---------------------------------------------------------------------------

#[test]
fn snap_markdown_body_paragraph() {
    let html = render(|| view! { <MarkdownBody text=String::from("hello") /> });
    insta::assert_snapshot!(html);
}

#[test]
fn snap_markdown_body_code_block_no_lang() {
    let html = render(|| {
        view! { <MarkdownBody text=String::from("```\nplain code\n```\n") /> }
    });
    insta::assert_snapshot!(html);
}

#[test]
fn snap_markdown_body_inline_code() {
    let html = render(|| {
        view! { <MarkdownBody text=String::from("call `foo()` and `bar`") /> }
    });
    insta::assert_snapshot!(html);
}

#[test]
fn snap_markdown_body_link() {
    let html = render(|| {
        view! { <MarkdownBody text=String::from("see [omega](https://example.com)") /> }
    });
    insta::assert_snapshot!(html);
}

// ---------------------------------------------------------------------------
// ContextModal — per-modal-state
// ---------------------------------------------------------------------------

#[test]
fn snap_modal_closed_renders_nothing_visible() {
    let html = render(|| {
        let state = ContextModalState::new();
        provide_context(state);
        view! { <ContextModal /> }
    });
    insta::assert_snapshot!(html);
}

#[test]
fn snap_modal_open_loading() {
    let html = render(|| {
        let state = ContextModalState::new();
        let llm_call = match ev_llm_call() {
            OmegaEvent::LlmCall(e) => e,
            _ => unreachable!(),
        };
        state.open(llm_call);
        provide_context(state);
        view! { <ContextModal /> }
    });
    insta::assert_snapshot!(html);
}

// ---------------------------------------------------------------------------
// Composer — per-TurnState
// ---------------------------------------------------------------------------

mod composer_states {
    use super::*;
    use omega_web::composer::Composer;
    use omega_web::sessions::SessionListStore;
    use omega_web::store::SessionStore;
    use omega_web::ws::WsClient;

    fn install_app_context(turn_state: TurnState, pre_committed: bool) {
        let store = SessionStore::new();
        store.session_info.set(Some(SessionInfoPayload {
            dir: "2025-01-01T00-00-00-000-aaaa".into(),
            model: "claude-sonnet-4-6".into(),
            effort: "medium".into(),
            cwd: "/work".into(),
            sessions_root: "/work/.omega/sessions".into(),
            turn_state,
            has_pending_changes: false,
            name: None,
        }));
        store.turn_state.set(turn_state);
        store.pre_committed.set(pre_committed);
        provide_context(store);
        let list_store = SessionListStore::new();
        provide_context(list_store);
        let ws = WsClient::new(String::new(), store, list_store);
        provide_context(ws);
        // Phase 3.9: PickerOpen is required by <Composer /> (Sessions button).
        provide_context(PickerOpen::new());
        // UsagePanelOpen is required by <Composer /> (Usage button).
        provide_context(UsagePanelOpen::new());
    }

    #[test]
    fn snap_composer_idle() {
        let html = render(|| {
            install_app_context(TurnState::Idle, false);
            view! { <Composer /> }
        });
        insta::assert_snapshot!(html);
    }

    #[test]
    fn snap_composer_running() {
        let html = render(|| {
            install_app_context(TurnState::Running, false);
            view! { <Composer /> }
        });
        insta::assert_snapshot!(html);
    }

    #[test]
    fn snap_composer_pause_requested() {
        let html = render(|| {
            install_app_context(TurnState::PauseRequested, false);
            view! { <Composer /> }
        });
        insta::assert_snapshot!(html);
    }

    /// PauseRequested + pre_committed=true: primary should be "Take it back"
    /// with the secondary Abort ⎋ still present.
    #[test]
    fn snap_composer_pause_requested_pre_committed() {
        let html = render(|| {
            install_app_context(TurnState::PauseRequested, true);
            view! { <Composer /> }
        });
        insta::assert_snapshot!(html);
    }

    #[test]
    fn snap_composer_paused() {
        let html = render(|| {
            install_app_context(TurnState::Paused, false);
            view! { <Composer /> }
        });
        insta::assert_snapshot!(html);
    }
}

// ---------------------------------------------------------------------------
// Tool-selection panel (Phase 2.1 Commit B)
// ---------------------------------------------------------------------------
//
// These snapshots pin the rendered tool-picker body in three states:
//
//   1. Fresh open with the *Standard* preset.
//   2. After clicking the *REPL-centric* preset chip.
//   3. After unchecking one tool from Standard — *Custom* chip active.
//
// `<ToolSelectionPanel/>` reads its initial state from the `PickerOpen`
// context, so the tests pre-populate `tool_selection` and then render
// the panel directly (no need to drive a click sequence).
mod tool_picker_states {
    use super::*;
    use omega_web::picker::ToolSelectionPanel;
    use omega_web::protocol::PRESETS;
    use omega_web::sessions::SessionListStore;
    use omega_web::store::SessionStore;
    use omega_web::ws::WsClient;

    /// Install the minimal context the panel needs.  Unlike
    /// `composer_states::install_app_context` we don't materialise an
    /// active session — the panel doesn't read `SessionStore` itself,
    /// but `WsClient::new` borrows it, so we provide one anyway.
    fn install_picker_context(initial_selection: Vec<String>) {
        let store = SessionStore::new();
        provide_context(store);
        let list_store = SessionListStore::new();
        provide_context(list_store);
        let ws = WsClient::new(String::new(), store, list_store);
        provide_context(ws);
        let picker_open = PickerOpen::new();
        picker_open.open.set(true);
        picker_open.show_tool_picker.set(true);
        picker_open.tool_selection.set(initial_selection);
        provide_context(picker_open);
    }

    #[test]
    fn snap_tool_picker_standard() {
        let html = render(|| {
            // Standard preset — the freshly-opened state with no prior
            // localStorage value.
            let standard: Vec<String> = PRESETS[0].tools.iter().map(|s| (*s).to_owned()).collect();
            install_picker_context(standard);
            view! { <ToolSelectionPanel /> }
        });
        insta::assert_snapshot!(html);
    }

    #[test]
    fn snap_tool_picker_repl_centric() {
        let html = render(|| {
            // After clicking the *REPL-centric* preset chip.
            let repl_centric: Vec<String> = PRESETS
                .iter()
                .find(|p| p.id == "repl-centric")
                .expect("repl-centric preset must exist")
                .tools
                .iter()
                .map(|s| (*s).to_owned())
                .collect();
            install_picker_context(repl_centric);
            view! { <ToolSelectionPanel /> }
        });
        insta::assert_snapshot!(html);
    }

    #[test]
    fn snap_tool_picker_custom() {
        let html = render(|| {
            // Standard preset minus one tool — Custom chip should be active.
            let mut sel: Vec<String> = PRESETS[0].tools.iter().map(|s| (*s).to_owned()).collect();
            sel.retain(|s| s != "fetch_url");
            install_picker_context(sel);
            view! { <ToolSelectionPanel /> }
        });
        insta::assert_snapshot!(html);
    }
}
