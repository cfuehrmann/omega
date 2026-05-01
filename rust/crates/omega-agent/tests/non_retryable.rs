//! A non-retryable provider error (HTTP 400) terminates the turn with
//! `LlmError` + `AgentError` + `TurnInterrupted{Error}` events.

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

use common::{collect_stream, make_test_agent, tags};
use omega_core::{AgentItem, LlmError};
use omega_protocol::{InterruptReason, OmegaEvent};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn http_400_ends_turn_with_error_events() {
    let (mut agent, provider, _tmp) = make_test_agent();

    provider.push_response(vec![Err(LlmError::Http {
        status: 400,
        body: "bad request".to_owned(),
        retry_after: None,
    })]);

    let stream = agent.send_message("hi".to_owned(), CancellationToken::new());
    let items = collect_stream(stream).await;

    let t = tags(&items);
    assert_eq!(
        t,
        vec![
            "UserMessage",
            "LlmCall",
            "LlmError",
            "AgentError",
            "TurnInterrupted",
        ],
        "non-retryable error sequence diverged"
    );

    let agent_error = items
        .iter()
        .find_map(|i| match i {
            AgentItem::Event(e) => match e.as_ref() {
                OmegaEvent::AgentError(a) => Some(a),
                _ => None,
            },
            AgentItem::Signal(_) => None,
        })
        .expect("AgentError event");
    assert!(
        agent_error.error.contains("API error"),
        "expected generic API-error wording, got {:?}",
        agent_error.error
    );

    let interrupt = items
        .iter()
        .find_map(|i| match i {
            AgentItem::Event(e) => match e.as_ref() {
                OmegaEvent::TurnInterrupted(t) => Some(t),
                _ => None,
            },
            AgentItem::Signal(_) => None,
        })
        .expect("TurnInterrupted event");
    assert_eq!(interrupt.reason, Some(InterruptReason::Error));
}
