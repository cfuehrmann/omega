//! `Agent::set_model` / `Agent::set_effort` (Phase 1d.1a).
//!
//! Pins:
//!
//! * the field/event mutation,
//! * persistence to `events.jsonl`,
//! * the next `send_message` actually uses the new model on the wire,
//! * defaults: `active_model` = `config.model`, `active_effort` = `"medium"`,
//! * `set_effort` does not perturb the active model (and vice versa).

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
use omega_agent::DEFAULT_EFFORT;
use omega_protocol::OmegaEvent;
use tokio_util::sync::CancellationToken;

/// Read every `OmegaEvent` from a session's `events.jsonl` (one JSON
/// object per line).  Used by tests to verify persistence.
fn read_events(events_path: &std::path::Path) -> Vec<OmegaEvent> {
    let raw = std::fs::read_to_string(events_path).expect("read events.jsonl");
    raw.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("parse OmegaEvent line"))
        .collect()
}

/// Cheap check that `time` looks like the RFC3339-with-`Z` shape that
/// `now_iso` produces (e.g. `2024-01-01T12:34:56.789Z`).  Used to pin
/// the timestamp through to the persisted event so a mutation that
/// replaces `now_iso` with an empty / arbitrary string is killed.
fn assert_iso_timestamp(s: &str) {
    assert!(!s.is_empty(), "time field must not be empty: {s:?}");
    assert!(s.contains('T'), "time must contain 'T': {s:?}");
    assert!(s.ends_with('Z'), "time must end with 'Z': {s:?}");
    // Year prefix sanity (cheap, locale-free).
    assert!(
        s.starts_with("20") || s.starts_with("21"),
        "time must start with a 21st-century year: {s:?}"
    );
}

#[tokio::test]
async fn default_active_model_matches_config_model() {
    let (agent, _provider, _tmp) = make_test_agent();
    assert_eq!(agent.active_model(), "claude-sonnet-4-6");
}

#[tokio::test]
async fn default_active_effort_is_medium() {
    let (agent, _provider, _tmp) = make_test_agent();
    assert_eq!(agent.active_effort(), DEFAULT_EFFORT);
    assert_eq!(agent.active_effort(), "medium");
}

#[tokio::test]
async fn set_model_changes_active_model_and_returns_event() {
    let (mut agent, _provider, tmp) = make_test_agent();
    let ev = agent.set_model("claude-opus-4-7".to_owned()).await;

    assert_eq!(agent.active_model(), "claude-opus-4-7");

    let OmegaEvent::ModelChanged(mc) = ev else {
        panic!("expected ModelChanged event, got {ev:?}");
    };
    assert_eq!(mc.model, "claude-opus-4-7");
    assert_iso_timestamp(&mc.time);

    // The event must also be on disk.
    let persisted = read_events(&tmp.path().join("events.jsonl"));
    assert_eq!(persisted.len(), 1, "exactly one event should be persisted");
    let OmegaEvent::ModelChanged(persisted_mc) = &persisted[0] else {
        panic!("persisted event is not ModelChanged: {:?}", persisted[0]);
    };
    assert_eq!(persisted_mc.model, "claude-opus-4-7");
    assert_iso_timestamp(&persisted_mc.time);
}

#[tokio::test]
async fn set_effort_changes_active_effort_and_returns_event() {
    let (mut agent, _provider, tmp) = make_test_agent();
    let ev = agent.set_effort("high".to_owned()).await;

    assert_eq!(agent.active_effort(), "high");

    let OmegaEvent::EffortChanged(ec) = ev else {
        panic!("expected EffortChanged event, got {ev:?}");
    };
    assert_eq!(ec.effort, "high");
    assert_iso_timestamp(&ec.time);

    let persisted = read_events(&tmp.path().join("events.jsonl"));
    assert_eq!(persisted.len(), 1);
    let OmegaEvent::EffortChanged(persisted_ec) = &persisted[0] else {
        panic!("persisted event is not EffortChanged: {:?}", persisted[0]);
    };
    assert_eq!(persisted_ec.effort, "high");
    assert_iso_timestamp(&persisted_ec.time);
}

#[tokio::test]
async fn set_model_does_not_change_active_effort() {
    let (mut agent, _provider, _tmp) = make_test_agent();
    let _ = agent.set_model("claude-opus-4-7".to_owned()).await;
    assert_eq!(agent.active_effort(), DEFAULT_EFFORT);
}

#[tokio::test]
async fn set_effort_does_not_change_active_model() {
    let (mut agent, _provider, _tmp) = make_test_agent();
    let _ = agent.set_effort("high".to_owned()).await;
    assert_eq!(agent.active_model(), "claude-sonnet-4-6");
}

#[tokio::test]
async fn last_set_model_call_wins() {
    let (mut agent, _provider, tmp) = make_test_agent();
    let _ = agent.set_model("claude-opus-4-6".to_owned()).await;
    let _ = agent.set_model("claude-opus-4-7".to_owned()).await;
    assert_eq!(agent.active_model(), "claude-opus-4-7");

    // Both events must be persisted, in order.
    let persisted = read_events(&tmp.path().join("events.jsonl"));
    assert_eq!(persisted.len(), 2);
    let models: Vec<&str> = persisted
        .iter()
        .filter_map(|e| match e {
            OmegaEvent::ModelChanged(m) => Some(m.model.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(models, vec!["claude-opus-4-6", "claude-opus-4-7"]);
}

#[tokio::test]
async fn next_send_message_uses_new_model_on_the_wire() {
    let (mut agent, provider, _tmp) = make_test_agent();
    let _ = agent.set_model("claude-opus-4-7".to_owned()).await;

    provider.push_response(vec![Ok(make_llm_response("end_turn", Some("ok"), 1, 1))]);

    let stream = agent.send_message("hi".to_owned(), CancellationToken::new());
    let _ = collect_stream(stream).await;

    let captured = provider.take_requests();
    assert_eq!(captured.len(), 1, "expected exactly one LLM call");
    assert_eq!(
        captured[0].model, "claude-opus-4-7",
        "send_message did not pick up the new active_model"
    );
}

#[tokio::test]
async fn send_message_without_set_model_uses_config_model() {
    let (mut agent, provider, _tmp) = make_test_agent();
    provider.push_response(vec![Ok(make_llm_response("end_turn", Some("ok"), 1, 1))]);

    let stream = agent.send_message("hi".to_owned(), CancellationToken::new());
    let _ = collect_stream(stream).await;

    let captured = provider.take_requests();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].model, "claude-sonnet-4-6");
}
