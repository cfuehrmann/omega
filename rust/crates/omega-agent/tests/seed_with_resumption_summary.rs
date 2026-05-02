//! `Agent::seed_with_resumption_summary` (Phase 1d.1c).
//!
//! Pins:
//!
//! * the returned `SessionResumed` event shape (time / resumed_from / summary),
//! * persistence of the event to `events.jsonl`,
//! * the two synthetic context records written to `context.jsonl`,
//! * history growth: exactly +2 (user with preamble+summary, assistant ack),
//! * `history` stays length-aligned with `context_hashes`,
//! * the canned preamble / canned acknowledgement strings (verbatim TS),
//! * the next `send_message` call sees the seeded conversation prefix.

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

use common::{collect_stream, make_llm_response, make_test_agent};
use omega_core::{ContentBlock, Role};
use omega_protocol::OmegaEvent;
use tokio_util::sync::CancellationToken;

/// Read every `OmegaEvent` from a session's `events.jsonl` (one JSON
/// object per line).
fn read_events(events_path: &std::path::Path) -> Vec<OmegaEvent> {
    let raw = std::fs::read_to_string(events_path).expect("read events.jsonl");
    raw.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("parse OmegaEvent line"))
        .collect()
}

fn assert_iso_timestamp(s: &str) {
    assert!(!s.is_empty(), "time field must not be empty: {s:?}");
    assert!(s.contains('T'), "time must contain 'T': {s:?}");
    assert!(s.ends_with('Z'), "time must end with 'Z': {s:?}");
    assert!(
        s.starts_with("20") || s.starts_with("21"),
        "time must start with a 21st-century year: {s:?}"
    );
}

const PREAMBLE: &str =
    "The following is context from the previous session to provide continuity:\n\n";
const ACK: &str =
    "Understood. I have reviewed the context from the previous session and am ready to continue.";

#[tokio::test]
async fn returns_session_resumed_event_with_summary_and_resumed_from() {
    let (mut agent, _provider, _tmp) = make_test_agent();

    let ev = agent
        .seed_with_resumption_summary("the summary".to_owned(), "20240115_120000".to_owned())
        .await
        .expect("seed should succeed");

    let OmegaEvent::SessionResumed(sr) = ev else {
        panic!("expected SessionResumed, got {ev:?}");
    };
    assert_eq!(sr.summary, "the summary");
    assert_eq!(sr.resumed_from, "20240115_120000");
    assert_iso_timestamp(&sr.time);
}

#[tokio::test]
async fn session_resumed_event_persisted_to_events_jsonl() {
    let (mut agent, _provider, tmp) = make_test_agent();

    let _ = agent
        .seed_with_resumption_summary("S".to_owned(), "PREV".to_owned())
        .await
        .expect("seed should succeed");

    let persisted = read_events(&tmp.path().join("events.jsonl"));
    assert_eq!(persisted.len(), 1, "exactly one event should be persisted");
    let OmegaEvent::SessionResumed(sr) = &persisted[0] else {
        panic!("persisted event is not SessionResumed: {:?}", persisted[0]);
    };
    assert_eq!(sr.summary, "S");
    assert_eq!(sr.resumed_from, "PREV");
    assert_iso_timestamp(&sr.time);
}

#[tokio::test]
async fn seeds_two_messages_into_history_user_then_assistant() {
    let (mut agent, _provider, _tmp) = make_test_agent();
    assert_eq!(agent.history().len(), 0);

    let _ = agent
        .seed_with_resumption_summary("S".to_owned(), "PREV".to_owned())
        .await
        .expect("seed should succeed");

    let h = agent.history();
    assert_eq!(h.len(), 2, "exactly two synthetic messages added");
    assert_eq!(h[0].role, Role::User);
    assert_eq!(h[1].role, Role::Assistant);
}

#[tokio::test]
async fn user_seed_message_is_preamble_concat_summary() {
    let (mut agent, _provider, _tmp) = make_test_agent();

    let _ = agent
        .seed_with_resumption_summary("MY SUMMARY".to_owned(), "PREV".to_owned())
        .await
        .expect("seed should succeed");

    let h = agent.history();
    let ContentBlock::Text { text } = &h[0].content[0] else {
        panic!("expected Text block, got {:?}", h[0].content[0]);
    };
    assert_eq!(text, &format!("{PREAMBLE}MY SUMMARY"));
    // Order matters — preamble must come before summary, not after.
    assert!(
        text.starts_with(PREAMBLE),
        "preamble must be at the start: {text:?}"
    );
    assert!(text.ends_with("MY SUMMARY"), "summary must be at the end");
}

#[tokio::test]
async fn assistant_seed_message_is_canned_acknowledgement() {
    let (mut agent, _provider, _tmp) = make_test_agent();

    let _ = agent
        .seed_with_resumption_summary("S".to_owned(), "PREV".to_owned())
        .await
        .expect("seed should succeed");

    let h = agent.history();
    assert_eq!(h[1].content.len(), 1, "assistant has exactly one block");
    let ContentBlock::Text { text } = &h[1].content[0] else {
        panic!("expected Text block, got {:?}", h[1].content[0]);
    };
    assert_eq!(text, ACK);
}

#[tokio::test]
async fn context_jsonl_records_user_then_assistant() {
    let (mut agent, _provider, tmp) = make_test_agent();

    let _ = agent
        .seed_with_resumption_summary("MY SUMMARY".to_owned(), "PREV".to_owned())
        .await
        .expect("seed should succeed");

    let raw =
        std::fs::read_to_string(tmp.path().join("context.jsonl")).expect("read context.jsonl");
    let lines: Vec<&str> = raw.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 2, "two context records expected");

    // Spot-check JSON shape: first record is role=user with preamble in text.
    let v0: serde_json::Value = serde_json::from_str(lines[0]).expect("parse line 0");
    assert_eq!(v0["role"], "user");
    let text0 = v0["content"][0]["text"].as_str().expect("text field");
    assert!(
        text0.starts_with(PREAMBLE),
        "user preamble missing: {text0:?}"
    );
    assert!(text0.contains("MY SUMMARY"), "user summary missing");

    let v1: serde_json::Value = serde_json::from_str(lines[1]).expect("parse line 1");
    assert_eq!(v1["role"], "assistant");
    let text1 = v1["content"][0]["text"].as_str().expect("text field");
    assert_eq!(text1, ACK);
}

#[tokio::test]
async fn context_hashes_stay_aligned_with_history() {
    let (mut agent, _provider, tmp) = make_test_agent();

    let _ = agent
        .seed_with_resumption_summary("S".to_owned(), "PREV".to_owned())
        .await
        .expect("seed should succeed");

    // We can't read context_hashes directly (private), but we can drive a
    // send_message that uses them — the cache_breakpoint_index in the
    // emitted LlmCall is computed from context_hashes.len()-1; the request
    // bodies that follow give us indirect evidence that hashes match
    // history.  Instead, prefer the simpler observation: history has the
    // same length as context.jsonl after the seed.
    let raw =
        std::fs::read_to_string(tmp.path().join("context.jsonl")).expect("read context.jsonl");
    let line_count = raw.lines().filter(|l| !l.trim().is_empty()).count();
    assert_eq!(line_count, agent.history().len(), "alignment broken");
}

#[tokio::test]
async fn next_send_message_includes_seeded_pair_in_request() {
    let (mut agent, provider, _tmp) = make_test_agent();

    let _ = agent
        .seed_with_resumption_summary("MY SUMMARY".to_owned(), "PREV".to_owned())
        .await
        .expect("seed should succeed");

    provider.push_response(vec![Ok(make_llm_response("end_turn", Some("ok"), 1, 1))]);
    let stream = agent.send_message("hi".to_owned(), CancellationToken::new());
    let _ = collect_stream(stream).await;

    let captured = provider.take_requests();
    assert_eq!(captured.len(), 1, "expected exactly one LLM call");
    let messages = &captured[0].messages;
    // Order: [synthetic user, synthetic assistant, real user "hi"].
    assert_eq!(
        messages.len(),
        3,
        "request must include seed pair + new user message"
    );
    assert_eq!(messages[0].role, Role::User);
    let ContentBlock::Text { text: t0 } = &messages[0].content[0] else {
        panic!("expected Text block in messages[0]");
    };
    assert!(t0.starts_with(PREAMBLE) && t0.contains("MY SUMMARY"));

    assert_eq!(messages[1].role, Role::Assistant);
    let ContentBlock::Text { text: t1 } = &messages[1].content[0] else {
        panic!("expected Text block in messages[1]");
    };
    assert_eq!(t1, ACK);

    assert_eq!(messages[2].role, Role::User);
    let ContentBlock::Text { text: t2 } = &messages[2].content[0] else {
        panic!("expected Text block in messages[2]");
    };
    assert_eq!(t2, "hi");
}

#[tokio::test]
async fn empty_summary_still_persists_event_and_seeds_history() {
    let (mut agent, _provider, tmp) = make_test_agent();

    let ev = agent
        .seed_with_resumption_summary(String::new(), "PREV".to_owned())
        .await
        .expect("seed should succeed even with empty summary");

    let OmegaEvent::SessionResumed(sr) = ev else {
        panic!("expected SessionResumed event");
    };
    assert_eq!(sr.summary, "");

    let h = agent.history();
    assert_eq!(h.len(), 2);
    let ContentBlock::Text { text } = &h[0].content[0] else {
        panic!("expected Text block");
    };
    // Empty summary still keeps the preamble.
    assert_eq!(text, PREAMBLE);

    let persisted = read_events(&tmp.path().join("events.jsonl"));
    assert_eq!(persisted.len(), 1);
}
