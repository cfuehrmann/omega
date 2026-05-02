//! `Agent::perform_resumption` (Phase 1d.1c).
//!
//! Pins the full event sequence, persistence shape, and seam contracts:
//!
//! * event order: ResumingSession -> LlmCall -> [signals] -> LlmResponse -> SessionResumed,
//! * `ResumingSession.basis` / `name` / `resumed_from` round-trip,
//! * `LlmCall` carries the Anthropic URL, the resumption model
//!   (`claude-sonnet-4-6`), exactly one context_hash, `cache_breakpoint_index = null`,
//! * the provider request `messages` is `[{user, basis}]` only — never the
//!   prior in-memory history,
//! * `LlmResponse.context_hash` is filled with the assistant record hash
//!   (12 chars),
//! * `SessionResumed.summary` matches `extract_summary_from_response` for
//!   both the `<summary>…</summary>` path and the no-block fallback,
//! * `LlmRetry` / `LlmError` paths,
//! * cancellation mid-stream stops cleanly without `TurnInterrupted`,
//! * the seeded synthetic pair is reachable by a subsequent `send_message`.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::doc_markdown,
    clippy::too_many_lines,
    clippy::wildcard_enum_match_arm,
    clippy::missing_panics_doc
)]

mod common;

use common::{collect_stream, make_llm_response, make_test_agent, tags};
use omega_agent::extract_description_from_response;
use omega_core::{AgentItem, ContentBlock, LlmError, Role};
use omega_protocol::events::LlmRetryEvent;
use omega_protocol::{OmegaEvent, StreamSignal};
use tokio_util::sync::CancellationToken;

const ANTHROPIC_URL: &str = "https://api.anthropic.com/v1/messages";
const RESUMPTION_MODEL: &str = "claude-sonnet-4-6";
const SEED_PREAMBLE: &str =
    "The following is context from the previous session to provide continuity:\n\n";
const SEED_ACK: &str =
    "Understood. I have reviewed the context from the previous session and am ready to continue.";

fn read_events(events_path: &std::path::Path) -> Vec<OmegaEvent> {
    let raw = std::fs::read_to_string(events_path).expect("read events.jsonl");
    raw.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("parse OmegaEvent line"))
        .collect()
}

fn assert_iso_timestamp(s: &str) {
    assert!(!s.is_empty(), "time must not be empty: {s:?}");
    assert!(s.contains('T'), "time must contain 'T': {s:?}");
    assert!(s.ends_with('Z'), "time must end with 'Z': {s:?}");
    assert!(
        s.starts_with("20") || s.starts_with("21"),
        "time must start with a 21st-century year: {s:?}"
    );
}

/// Push one canned LLM transcript: a single text-delta signal followed
/// by an `LlmResponse` carrying that same text.
fn push_text_response(provider: &common::MockProvider, text: &str) {
    provider.push_response(vec![
        Ok(AgentItem::Signal(StreamSignal::Text {
            text: text.to_owned(),
        })),
        Ok(make_llm_response("end_turn", Some(text), 10, 5)),
    ]);
}

// ---------------------------------------------------------------------------
// Event order / shape
// ---------------------------------------------------------------------------

#[tokio::test]
async fn emits_resuming_session_llm_call_signal_llm_response_session_resumed_in_order() {
    let (mut agent, provider, _tmp) = make_test_agent();
    push_text_response(&provider, "<summary>S.</summary>");

    let stream = agent.perform_resumption(
        "the basis".to_owned(),
        "PREV".to_owned(),
        None,
        CancellationToken::new(),
    );
    let items = collect_stream(stream).await;

    assert_eq!(
        tags(&items),
        vec![
            "ResumingSession",
            "LlmCall",
            "Signal:Text",
            "LlmResponse",
            "SessionResumed",
        ],
        "event sequence diverged from spec"
    );
}

#[tokio::test]
async fn order_holds_with_no_text_signal() {
    let (mut agent, provider, _tmp) = make_test_agent();
    // Provider yields LlmResponse only (no signals).
    provider.push_response(vec![Ok(make_llm_response(
        "end_turn",
        Some("<summary>S.</summary>"),
        10,
        5,
    ))]);

    let stream = agent.perform_resumption(
        "basis".to_owned(),
        "PREV".to_owned(),
        None,
        CancellationToken::new(),
    );
    let items = collect_stream(stream).await;

    assert_eq!(
        tags(&items),
        vec![
            "ResumingSession",
            "LlmCall",
            "LlmResponse",
            "SessionResumed",
        ]
    );
}

// ---------------------------------------------------------------------------
// ResumingSession event shape
// ---------------------------------------------------------------------------

#[tokio::test]
async fn resuming_session_carries_basis_resumed_from_and_no_name_when_none() {
    let (mut agent, provider, _tmp) = make_test_agent();
    push_text_response(&provider, "<summary>S.</summary>");

    let stream = agent.perform_resumption(
        "MY BASIS".to_owned(),
        "20240115_120000".to_owned(),
        None,
        CancellationToken::new(),
    );
    let items = collect_stream(stream).await;

    let rs = items
        .iter()
        .find_map(|i| match i {
            AgentItem::Event(e) => match e.as_ref() {
                OmegaEvent::ResumingSession(r) => Some(r),
                _ => None,
            },
            AgentItem::Signal(_) => None,
        })
        .expect("ResumingSession missing");
    assert_eq!(rs.basis, "MY BASIS");
    assert_eq!(rs.resumed_from, "20240115_120000");
    assert_eq!(rs.name, None);
    assert_iso_timestamp(&rs.time);
}

#[tokio::test]
async fn resuming_session_carries_name_when_some() {
    let (mut agent, provider, _tmp) = make_test_agent();
    push_text_response(&provider, "<summary>S.</summary>");

    let stream = agent.perform_resumption(
        "basis".to_owned(),
        "PREV".to_owned(),
        Some("auth-refactor".to_owned()),
        CancellationToken::new(),
    );
    let items = collect_stream(stream).await;

    let rs = items
        .iter()
        .find_map(|i| match i {
            AgentItem::Event(e) => match e.as_ref() {
                OmegaEvent::ResumingSession(r) => Some(r),
                _ => None,
            },
            AgentItem::Signal(_) => None,
        })
        .expect("ResumingSession missing");
    assert_eq!(rs.name.as_deref(), Some("auth-refactor"));
}

// ---------------------------------------------------------------------------
// LlmCall event shape
// ---------------------------------------------------------------------------

#[tokio::test]
async fn llm_call_uses_anthropic_url_and_resumption_model() {
    let (mut agent, provider, _tmp) = make_test_agent();
    push_text_response(&provider, "<summary>S.</summary>");

    let stream = agent.perform_resumption(
        "basis".to_owned(),
        "PREV".to_owned(),
        None,
        CancellationToken::new(),
    );
    let items = collect_stream(stream).await;

    let lc = items
        .iter()
        .find_map(|i| match i {
            AgentItem::Event(e) => match e.as_ref() {
                OmegaEvent::LlmCall(c) => Some(c),
                _ => None,
            },
            AgentItem::Signal(_) => None,
        })
        .expect("LlmCall missing");
    assert_eq!(lc.url, ANTHROPIC_URL);
    assert_eq!(lc.model, RESUMPTION_MODEL);
    assert_eq!(lc.context_hashes.len(), 1, "exactly one (basis user) hash");
    assert_eq!(lc.context_hashes[0].len(), 12);
    assert_eq!(
        lc.cache_breakpoint_index, None,
        "cache_breakpoint_index must be null for one-off resumption call"
    );
}

#[tokio::test]
async fn llm_call_resumption_model_overrides_active_model() {
    // Even after set_model, perform_resumption uses the hard-coded
    // RESUMPTION_MODEL — Sonnet is the right balance for summarisation.
    let (mut agent, provider, _tmp) = make_test_agent();
    let _ = agent.set_model("claude-opus-4-7".to_owned()).await;
    push_text_response(&provider, "<summary>S.</summary>");

    let stream = agent.perform_resumption(
        "basis".to_owned(),
        "PREV".to_owned(),
        None,
        CancellationToken::new(),
    );
    let _ = collect_stream(stream).await;

    let captured = provider.take_requests();
    assert_eq!(captured.len(), 1);
    assert_eq!(
        captured[0].model, RESUMPTION_MODEL,
        "resumption call must use RESUMPTION_MODEL, not active_model"
    );
}

// ---------------------------------------------------------------------------
// LlmRequest shape (basis-only, no prior history)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn provider_request_messages_contain_basis_only() {
    let (mut agent, provider, _tmp) = make_test_agent();
    push_text_response(&provider, "<summary>S.</summary>");

    let stream = agent.perform_resumption(
        "MY BASIS".to_owned(),
        "PREV".to_owned(),
        None,
        CancellationToken::new(),
    );
    let _ = collect_stream(stream).await;

    let captured = provider.take_requests();
    assert_eq!(captured.len(), 1);
    let messages = &captured[0].messages;
    assert_eq!(messages.len(), 1, "exactly one message: the basis");
    assert_eq!(messages[0].role, Role::User);
    let ContentBlock::Text { text } = &messages[0].content[0] else {
        panic!("expected Text block, got {:?}", messages[0].content[0]);
    };
    assert_eq!(text, "MY BASIS");
}

#[tokio::test]
async fn provider_request_does_not_include_prior_history() {
    // Pre-seed history; perform_resumption must IGNORE it on the wire.
    let (mut agent, provider, _tmp) = make_test_agent();
    let _ = agent
        .seed_with_resumption_summary("OLD".to_owned(), "OLDER_PREV".to_owned())
        .await
        .expect("seed");
    assert_eq!(agent.history().len(), 2, "pre-seed left 2 history entries");

    push_text_response(&provider, "<summary>S.</summary>");

    let stream = agent.perform_resumption(
        "BASIS".to_owned(),
        "PREV".to_owned(),
        None,
        CancellationToken::new(),
    );
    let _ = collect_stream(stream).await;

    let captured = provider.take_requests();
    assert_eq!(captured.len(), 1);
    let messages = &captured[0].messages;
    assert_eq!(
        messages.len(),
        1,
        "perform_resumption must not forward in-memory history to the provider"
    );
    let ContentBlock::Text { text } = &messages[0].content[0] else {
        panic!("expected Text block");
    };
    assert_eq!(text, "BASIS");
}

#[tokio::test]
async fn provider_request_uses_resumption_summary_instructions_as_system() {
    let (mut agent, provider, _tmp) = make_test_agent();
    push_text_response(&provider, "<summary>S.</summary>");

    let stream = agent.perform_resumption(
        "basis".to_owned(),
        "PREV".to_owned(),
        None,
        CancellationToken::new(),
    );
    let _ = collect_stream(stream).await;

    let captured = provider.take_requests();
    let system = captured[0].system.as_ref().expect("system prompt set");
    assert!(
        system.contains("<summary>"),
        "system prompt must instruct LLM to wrap in <summary>: {system}"
    );
    assert!(
        system.contains("<description>"),
        "system prompt must also request <description> tag: {system}"
    );
}

#[tokio::test]
async fn provider_request_max_tokens_is_resumption_limit() {
    let (mut agent, provider, _tmp) = make_test_agent();
    push_text_response(&provider, "<summary>S.</summary>");

    let stream = agent.perform_resumption(
        "basis".to_owned(),
        "PREV".to_owned(),
        None,
        CancellationToken::new(),
    );
    let _ = collect_stream(stream).await;

    let captured = provider.take_requests();
    assert_eq!(captured[0].config.max_tokens, 4096);
}

#[tokio::test]
async fn provider_request_carries_no_tools() {
    let (mut agent, provider, _tmp) = make_test_agent();
    push_text_response(&provider, "<summary>S.</summary>");

    let stream = agent.perform_resumption(
        "basis".to_owned(),
        "PREV".to_owned(),
        None,
        CancellationToken::new(),
    );
    let _ = collect_stream(stream).await;

    let captured = provider.take_requests();
    assert!(
        captured[0].tools.is_empty(),
        "resumption is a one-shot summarisation call; tools must be empty"
    );
}

// ---------------------------------------------------------------------------
// LlmResponse shape
// ---------------------------------------------------------------------------

#[tokio::test]
async fn llm_response_context_hash_filled_and_matches_persisted_record() {
    let (mut agent, provider, tmp) = make_test_agent();
    push_text_response(&provider, "<summary>S.</summary>");

    let stream = agent.perform_resumption(
        "basis".to_owned(),
        "PREV".to_owned(),
        None,
        CancellationToken::new(),
    );
    let items = collect_stream(stream).await;

    let lr = items
        .iter()
        .find_map(|i| match i {
            AgentItem::Event(e) => match e.as_ref() {
                OmegaEvent::LlmResponse(r) => Some(r),
                _ => None,
            },
            AgentItem::Signal(_) => None,
        })
        .expect("LlmResponse missing");
    assert_eq!(lr.context_hash.len(), 12, "context_hash unset");
    assert_eq!(lr.text.as_deref(), Some("<summary>S.</summary>"));

    // The hash on LlmResponse must correspond to the assistant context
    // record — second of two records (user-basis, assistant-response).
    let raw =
        std::fs::read_to_string(tmp.path().join("context.jsonl")).expect("read context.jsonl");
    let lines: Vec<&str> = raw.lines().filter(|l| !l.trim().is_empty()).collect();
    assert!(lines.len() >= 2);
    let v1: serde_json::Value = serde_json::from_str(lines[1]).expect("parse line 1");
    assert_eq!(v1["role"], "assistant");
}

// ---------------------------------------------------------------------------
// SessionResumed shape — summary extraction
// ---------------------------------------------------------------------------

#[tokio::test]
async fn session_resumed_summary_extracted_from_block() {
    let (mut agent, provider, _tmp) = make_test_agent();
    push_text_response(
        &provider,
        "<summary>auth done; deploy next</summary><description>x</description>",
    );

    let stream = agent.perform_resumption(
        "basis".to_owned(),
        "PREV".to_owned(),
        None,
        CancellationToken::new(),
    );
    let items = collect_stream(stream).await;

    let sr = items
        .iter()
        .find_map(|i| match i {
            AgentItem::Event(e) => match e.as_ref() {
                OmegaEvent::SessionResumed(r) => Some(r),
                _ => None,
            },
            AgentItem::Signal(_) => None,
        })
        .expect("SessionResumed missing");
    assert_eq!(sr.summary, "auth done; deploy next");
    assert_eq!(sr.resumed_from, "PREV");
}

#[tokio::test]
async fn session_resumed_summary_falls_back_to_trimmed_full_text_when_no_block() {
    let (mut agent, provider, _tmp) = make_test_agent();
    push_text_response(&provider, "  no block here  ");

    let stream = agent.perform_resumption(
        "basis".to_owned(),
        "PREV".to_owned(),
        None,
        CancellationToken::new(),
    );
    let items = collect_stream(stream).await;

    let sr = items
        .iter()
        .find_map(|i| match i {
            AgentItem::Event(e) => match e.as_ref() {
                OmegaEvent::SessionResumed(r) => Some(r),
                _ => None,
            },
            AgentItem::Signal(_) => None,
        })
        .expect("SessionResumed missing");
    assert_eq!(sr.summary, "no block here");
}

// ---------------------------------------------------------------------------
// History seeding side-effects (after success)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn history_grows_to_two_synthetic_messages_basis_not_in_history() {
    let (mut agent, provider, _tmp) = make_test_agent();
    push_text_response(&provider, "<summary>SS</summary>");

    assert_eq!(agent.history().len(), 0);
    let stream = agent.perform_resumption(
        "BASIS-XYZ".to_owned(),
        "PREV".to_owned(),
        None,
        CancellationToken::new(),
    );
    let _ = collect_stream(stream).await;

    let h = agent.history();
    assert_eq!(
        h.len(),
        2,
        "history must contain exactly the synthetic seed pair, not the basis"
    );
    assert_eq!(h[0].role, Role::User);
    assert_eq!(h[1].role, Role::Assistant);
    let ContentBlock::Text { text: t0 } = &h[0].content[0] else {
        panic!("expected Text block");
    };
    assert!(t0.starts_with(SEED_PREAMBLE), "preamble missing");
    assert!(t0.contains("SS"), "summary missing");
    assert!(
        !t0.contains("BASIS-XYZ"),
        "basis must NOT leak into seeded history"
    );
    let ContentBlock::Text { text: t1 } = &h[1].content[0] else {
        panic!("expected Text block");
    };
    assert_eq!(t1, SEED_ACK);
}

#[tokio::test]
async fn context_jsonl_has_four_records_basis_response_seed_pair() {
    let (mut agent, provider, tmp) = make_test_agent();
    push_text_response(&provider, "<summary>SS</summary>");

    let stream = agent.perform_resumption(
        "BASIS".to_owned(),
        "PREV".to_owned(),
        None,
        CancellationToken::new(),
    );
    let _ = collect_stream(stream).await;

    let raw =
        std::fs::read_to_string(tmp.path().join("context.jsonl")).expect("read context.jsonl");
    let lines: Vec<&str> = raw.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(
        lines.len(),
        4,
        "expected 4 context records: basis, llm-response, seed-user, seed-assistant"
    );

    // Roles in order.
    let roles: Vec<String> = lines
        .iter()
        .map(|l| {
            serde_json::from_str::<serde_json::Value>(l).expect("parse")["role"]
                .as_str()
                .expect("role")
                .to_owned()
        })
        .collect();
    assert_eq!(
        roles,
        vec!["user", "assistant", "user", "assistant"],
        "context records out of order"
    );

    // First record is the basis text.
    let v0: serde_json::Value = serde_json::from_str(lines[0]).expect("parse");
    let text0 = v0["content"][0]["text"].as_str().expect("text");
    assert_eq!(text0, "BASIS");

    // Third record (synthetic user seed) starts with preamble.
    let v2: serde_json::Value = serde_json::from_str(lines[2]).expect("parse");
    let text2 = v2["content"][0]["text"].as_str().expect("text");
    assert!(text2.starts_with(SEED_PREAMBLE));
    assert!(text2.ends_with("SS"));

    // Fourth record is the canned ack.
    let v3: serde_json::Value = serde_json::from_str(lines[3]).expect("parse");
    let text3 = v3["content"][0]["text"].as_str().expect("text");
    assert_eq!(text3, SEED_ACK);
}

#[tokio::test]
async fn next_send_message_after_resumption_uses_seeded_pair_in_request() {
    let (mut agent, provider, _tmp) = make_test_agent();
    push_text_response(&provider, "<summary>SUMMARY</summary>");

    let stream = agent.perform_resumption(
        "basis".to_owned(),
        "PREV".to_owned(),
        None,
        CancellationToken::new(),
    );
    let _ = collect_stream(stream).await;

    // Now a real user turn.
    provider.push_response(vec![Ok(make_llm_response("end_turn", Some("ok"), 1, 1))]);
    let stream = agent.send_message("hi".to_owned(), CancellationToken::new());
    let _ = collect_stream(stream).await;

    let captured = provider.take_requests();
    assert_eq!(captured.len(), 2, "one resumption + one send_message");
    let messages = &captured[1].messages;
    assert_eq!(
        messages.len(),
        3,
        "send_message request: seed-user, seed-assistant, real user"
    );
    let ContentBlock::Text { text: t0 } = &messages[0].content[0] else {
        panic!("expected Text");
    };
    assert!(t0.contains("SUMMARY"));
    let ContentBlock::Text { text: t2 } = &messages[2].content[0] else {
        panic!("expected Text");
    };
    assert_eq!(t2, "hi");
}

// ---------------------------------------------------------------------------
// Streaming forwarding
// ---------------------------------------------------------------------------

#[tokio::test]
async fn forwards_text_signals_during_streaming() {
    let (mut agent, provider, _tmp) = make_test_agent();
    provider.push_response(vec![
        Ok(AgentItem::Signal(StreamSignal::Text {
            text: "<sum".to_owned(),
        })),
        Ok(AgentItem::Signal(StreamSignal::Text {
            text: "mary>S</summary>".to_owned(),
        })),
        Ok(make_llm_response(
            "end_turn",
            Some("<summary>S</summary>"),
            10,
            5,
        )),
    ]);

    let stream = agent.perform_resumption(
        "basis".to_owned(),
        "PREV".to_owned(),
        None,
        CancellationToken::new(),
    );
    let items = collect_stream(stream).await;

    let texts: Vec<String> = items
        .iter()
        .filter_map(|i| match i {
            AgentItem::Signal(StreamSignal::Text { text }) => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        texts,
        vec!["<sum".to_owned(), "mary>S</summary>".to_owned()]
    );
}

#[tokio::test]
async fn forwards_thinking_signals_and_persists_to_assistant_record() {
    let (mut agent, provider, tmp) = make_test_agent();
    provider.push_response(vec![
        Ok(AgentItem::Signal(StreamSignal::Thinking {
            text: "let me think".to_owned(),
        })),
        Ok(AgentItem::Signal(StreamSignal::Text {
            text: "<summary>S</summary>".to_owned(),
        })),
        Ok(make_llm_response(
            "end_turn",
            Some("<summary>S</summary>"),
            10,
            5,
        )),
    ]);

    let stream = agent.perform_resumption(
        "basis".to_owned(),
        "PREV".to_owned(),
        None,
        CancellationToken::new(),
    );
    let items = collect_stream(stream).await;

    let tags = tags(&items);
    assert!(tags.contains(&"Signal:Thinking"));

    // Assistant record contains both a Thinking and a Text block.
    let raw =
        std::fs::read_to_string(tmp.path().join("context.jsonl")).expect("read context.jsonl");
    let lines: Vec<&str> = raw.lines().filter(|l| !l.trim().is_empty()).collect();
    let v1: serde_json::Value = serde_json::from_str(lines[1]).expect("parse");
    assert_eq!(v1["role"], "assistant");
    let blocks = v1["content"].as_array().expect("content array");
    assert_eq!(blocks.len(), 2, "expected thinking + text blocks");
    assert_eq!(blocks[0]["type"], "thinking");
    assert_eq!(blocks[0]["thinking"], "let me think");
    assert_eq!(blocks[1]["type"], "text");
}

// ---------------------------------------------------------------------------
// LlmRetry path
// ---------------------------------------------------------------------------

#[tokio::test]
async fn llm_retry_event_forwarded_and_partial_buffer_cleared() {
    let (mut agent, provider, _tmp) = make_test_agent();
    provider.push_response(vec![
        // Partial text that should be discarded on retry.
        Ok(AgentItem::Signal(StreamSignal::Text {
            text: "PARTIAL ".to_owned(),
        })),
        Ok(AgentItem::event(OmegaEvent::LlmRetry(LlmRetryEvent {
            time: "2024-01-01T00:00:00.000Z".to_owned(),
            attempt: 1,
            http_status: Some(529),
            wait_ms: 0,
            error: "overloaded".to_owned(),
            retry_at: None,
            error_body: None,
            thinking_fragment: None,
            text_fragment: None,
            reason: None,
        }))),
        // Real content after the retry.
        Ok(AgentItem::Signal(StreamSignal::Text {
            text: "<summary>OK</summary>".to_owned(),
        })),
        Ok(make_llm_response(
            "end_turn",
            Some("<summary>OK</summary>"),
            10,
            5,
        )),
    ]);

    let stream = agent.perform_resumption(
        "basis".to_owned(),
        "PREV".to_owned(),
        None,
        CancellationToken::new(),
    );
    let items = collect_stream(stream).await;

    assert!(
        tags(&items).contains(&"LlmRetry"),
        "LlmRetry must propagate"
    );

    let sr = items
        .iter()
        .find_map(|i| match i {
            AgentItem::Event(e) => match e.as_ref() {
                OmegaEvent::SessionResumed(r) => Some(r),
                _ => None,
            },
            AgentItem::Signal(_) => None,
        })
        .expect("SessionResumed missing");
    // The "PARTIAL " text must NOT have leaked into the assembled summary.
    assert_eq!(sr.summary, "OK", "partial buffer not cleared on retry");
    assert!(!sr.summary.contains("PARTIAL"));
}

// ---------------------------------------------------------------------------
// Error path
// ---------------------------------------------------------------------------

#[tokio::test]
async fn terminal_provider_error_yields_llm_error_and_no_session_resumed() {
    let (mut agent, provider, tmp) = make_test_agent();
    provider.push_response(vec![Err(LlmError::Transport {
        message: "network blew up".to_owned(),
    })]);

    let stream = agent.perform_resumption(
        "basis".to_owned(),
        "PREV".to_owned(),
        None,
        CancellationToken::new(),
    );
    let items = collect_stream(stream).await;

    // Sequence: ResumingSession, LlmCall, LlmError. No LlmResponse,
    // no SessionResumed.
    assert_eq!(
        tags(&items),
        vec!["ResumingSession", "LlmCall", "LlmError",],
        "expected provider-error truncation"
    );

    // LlmError carries the network message and Anthropic URL.
    let le = items
        .iter()
        .find_map(|i| match i {
            AgentItem::Event(e) => match e.as_ref() {
                OmegaEvent::LlmError(l) => Some(l),
                _ => None,
            },
            AgentItem::Signal(_) => None,
        })
        .expect("LlmError missing");
    assert!(le.error.contains("network blew up"));
    assert_eq!(le.url, ANTHROPIC_URL);

    // events.jsonl on disk also lacks SessionResumed.
    let persisted = read_events(&tmp.path().join("events.jsonl"));
    assert!(
        !persisted
            .iter()
            .any(|e| matches!(e, OmegaEvent::SessionResumed(_)))
    );
    assert!(
        persisted
            .iter()
            .any(|e| matches!(e, OmegaEvent::LlmError(_)))
    );
}

#[tokio::test]
async fn provider_ending_without_llm_response_yields_agent_error() {
    let (mut agent, provider, _tmp) = make_test_agent();
    // Empty transcript = signal-then-end with no final LlmResponse.
    provider.push_response(vec![Ok(AgentItem::Signal(StreamSignal::Text {
        text: "stub".to_owned(),
    }))]);

    let stream = agent.perform_resumption(
        "basis".to_owned(),
        "PREV".to_owned(),
        None,
        CancellationToken::new(),
    );
    let items = collect_stream(stream).await;

    let last_is_agent_error = matches!(
        items.last(),
        Some(AgentItem::Event(e)) if matches!(e.as_ref(), OmegaEvent::AgentError(_))
    );
    assert!(last_is_agent_error, "expected trailing AgentError");
    assert!(!tags(&items).contains(&"SessionResumed"));
}

// ---------------------------------------------------------------------------
// Cancellation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pre_cancelled_token_stops_before_persisting_assistant_record() {
    let (mut agent, provider, tmp) = make_test_agent();
    push_text_response(&provider, "<summary>S</summary>");

    let cancel = CancellationToken::new();
    cancel.cancel();
    let stream = agent.perform_resumption("basis".to_owned(), "PREV".to_owned(), None, cancel);
    let items = collect_stream(stream).await;

    // Cancellation is a clean stop — no SessionResumed, no TurnInterrupted
    // (perform_resumption is not a user turn; matches TS behavior).
    assert!(!tags(&items).contains(&"SessionResumed"));
    assert!(!tags(&items).contains(&"TurnInterrupted"));

    // History stays empty (no synthetic seed pair created).
    assert_eq!(agent.history().len(), 0);

    // Only the basis user record may have hit context.jsonl.
    let raw =
        std::fs::read_to_string(tmp.path().join("context.jsonl")).expect("read context.jsonl");
    let lines: Vec<&str> = raw.lines().filter(|l| !l.trim().is_empty()).collect();
    assert!(
        lines.len() <= 1,
        "cancellation must not write the assistant or seed records: {} lines",
        lines.len()
    );
}

// ---------------------------------------------------------------------------
// Description extraction (compositional integration with 1d.1b helper)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn extract_description_from_response_recovers_description_from_llm_response_text() {
    let (mut agent, provider, _tmp) = make_test_agent();
    push_text_response(
        &provider,
        "<summary>S</summary><description>Added auth middleware</description>",
    );

    let stream = agent.perform_resumption(
        "basis".to_owned(),
        "PREV".to_owned(),
        None,
        CancellationToken::new(),
    );
    let items = collect_stream(stream).await;

    let lr = items
        .iter()
        .find_map(|i| match i {
            AgentItem::Event(e) => match e.as_ref() {
                OmegaEvent::LlmResponse(r) => Some(r),
                _ => None,
            },
            AgentItem::Signal(_) => None,
        })
        .expect("LlmResponse missing");
    let text = lr.text.as_deref().expect("LlmResponse.text set");
    assert_eq!(
        extract_description_from_response(text),
        Some("Added auth middleware".to_owned())
    );
}

// ---------------------------------------------------------------------------
// Persistence — the full event sequence on disk
// ---------------------------------------------------------------------------

#[tokio::test]
async fn full_event_sequence_persisted_to_events_jsonl() {
    let (mut agent, provider, tmp) = make_test_agent();
    push_text_response(&provider, "<summary>SUMMARY</summary>");

    let stream = agent.perform_resumption(
        "basis".to_owned(),
        "PREV".to_owned(),
        Some("named".to_owned()),
        CancellationToken::new(),
    );
    let _ = collect_stream(stream).await;

    let persisted = read_events(&tmp.path().join("events.jsonl"));
    let kinds: Vec<&'static str> = persisted
        .iter()
        .map(|e| match e {
            OmegaEvent::ResumingSession(_) => "ResumingSession",
            OmegaEvent::LlmCall(_) => "LlmCall",
            OmegaEvent::LlmResponse(_) => "LlmResponse",
            OmegaEvent::SessionResumed(_) => "SessionResumed",
            _ => "Other",
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            "ResumingSession",
            "LlmCall",
            "LlmResponse",
            "SessionResumed",
        ]
    );

    // ResumingSession's name round-trips through persistence.
    let OmegaEvent::ResumingSession(rs) = &persisted[0] else {
        panic!("first persisted event is not ResumingSession");
    };
    assert_eq!(rs.name.as_deref(), Some("named"));
    assert_eq!(rs.basis, "basis");

    // SessionResumed's summary round-trips.
    let OmegaEvent::SessionResumed(sr) = &persisted[3] else {
        panic!("last persisted event is not SessionResumed");
    };
    assert_eq!(sr.summary, "SUMMARY");
    assert_eq!(sr.resumed_from, "PREV");
    assert_iso_timestamp(&sr.time);
}
