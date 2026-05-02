//! Integration tests for [`Agent::init`].
//!
//! Verifies that `init` writes a `server_started` event followed by a
//! `session_started` event to `events.jsonl`, using the session directory
//! and model/effort from the agent configuration.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

mod common;

use omega_protocol::OmegaEvent;

/// `init` must write two events: `server_started` then `session_started`.
#[tokio::test]
async fn init_writes_server_and_session_started_events() {
    let (agent, _provider, tmp) = common::make_test_agent();

    agent.init().await.expect("init should succeed");

    // Read back what was persisted.
    let content = std::fs::read_to_string(tmp.path().join("events.jsonl"))
        .expect("events.jsonl should exist");
    let lines: Vec<&str> = content.lines().collect();

    assert_eq!(lines.len(), 2, "exactly two events must be written");

    let first: OmegaEvent = serde_json::from_str(lines[0]).expect("line 0 should be valid JSON");
    assert!(
        matches!(first, OmegaEvent::ServerStarted(_)),
        "first event must be ServerStarted, got {first:?}"
    );

    let second: OmegaEvent = serde_json::from_str(lines[1]).expect("line 1 should be valid JSON");
    assert!(
        matches!(second, OmegaEvent::SessionStarted(_)),
        "second event must be SessionStarted, got {second:?}"
    );
}

/// `init` records the configured model in the `session_started` event.
#[tokio::test]
async fn init_session_started_contains_model() {
    let (agent, _provider, tmp) = common::make_test_agent();

    agent.init().await.expect("init should succeed");

    let content = std::fs::read_to_string(tmp.path().join("events.jsonl")).expect("events.jsonl");
    let second_line = content.lines().nth(1).expect("second line");
    let ev: OmegaEvent = serde_json::from_str(second_line).expect("parse");

    if let OmegaEvent::SessionStarted(ev) = ev {
        assert_eq!(
            ev.model, "claude-sonnet-4-6",
            "model must match AgentConfig"
        );
        assert!(
            !ev.system_prompt.is_empty(),
            "system prompt must be non-empty"
        );
    } else {
        panic!("expected SessionStarted, got {ev:?}");
    }
}
