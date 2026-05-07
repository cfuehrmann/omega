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
use omega_protocol::OmegaEvent;
use omega_protocol::events::{
    AgentErrorEvent, LlmCallEvent, LlmResponseEvent, LlmResponseUsage, ResumingSessionEvent,
    SessionResumedEvent, SessionStartedEvent, ToolCallEvent, ToolResultEvent, TurnEndEvent,
    TurnMetrics, UserMessageEvent,
};
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
    }
}

fn ev_assistant(text: &str) -> OmegaEvent {
    OmegaEvent::LlmResponse(LlmResponseEvent {
        time: "2025-01-01T00:00:01.000Z".into(),
        stop_reason: "end_turn".into(),
        cleared_tool_uses: None,
        cleared_input_tokens: None,
        usage: assistant_usage(),
        context_hash: "abcd1234ef56".into(),
        text: Some(text.into()),
        thinking: None,
        streaming_start: None,
        response_summary: None,
    })
}

fn ev_tool_call() -> OmegaEvent {
    OmegaEvent::ToolCall(ToolCallEvent {
        time: "2025-01-01T00:00:02.000Z".into(),
        id: "toolu_test".into(),
        name: "run_command".into(),
        input: serde_json::json!({ "command": "echo hi" }),
        context_hash: "abcd1234ef56".into(),
    })
}

fn ev_tool_result(out: &str, is_error: bool) -> OmegaEvent {
    OmegaEvent::ToolResult(ToolResultEvent {
        time: "2025-01-01T00:00:03.000Z".into(),
        id: "toolu_test".into(),
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
        session_id: "sid-test".into(),
        path: ".omega/sessions/2025-01-01T00-00-00-000-aaaaaaaa".into(),
        model: "claude-sonnet-4-6".into(),
        effort: "medium".into(),
        system_prompt: "system: test".into(),
        omega_commit: "abc1234".into(),
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
        context_hashes: vec!["aaaaaaaaaaaa".into(), "bbbbbbbbbbbb".into()],
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
fn snap_event_tool_call() {
    let html = render(|| {
        let ev = ev_tool_call();
        provide_context(TextModalState::new());
        view! { <EventBlock event=ev /> }
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
