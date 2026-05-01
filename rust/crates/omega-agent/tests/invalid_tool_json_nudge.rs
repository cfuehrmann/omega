//! When the provider stream errors with the `malformed tool_use JSON`
//! marker, the agent must inject a corrective user message and
//! re-issue the LLM call (up to two times before giving up).

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
use omega_core::{AgentItem, LlmError, Role};
use omega_protocol::OmegaEvent;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn malformed_tool_json_triggers_nudge_and_retry() {
    let (mut agent, provider, _tmp) = make_test_agent();

    // Turn 1: stream errors out with the marker prefix.
    provider.push_response(vec![Err(LlmError::Stream {
        message: "malformed tool_use JSON: expected `,` at position 17".to_owned(),
    })]);
    // Turn 2: model now produces a clean text reply.
    provider.push_response(vec![Ok(make_llm_response("end_turn", Some("done"), 6, 2))]);

    let stream = agent.send_message("please".to_owned(), CancellationToken::new());
    let items = collect_stream(stream).await;

    let t = tags(&items);
    // Expected tags: UserMessage, LlmCall, LlmError, UserMessage(nudge),
    // LlmCall, LlmResponse, TurnEnd.
    assert_eq!(
        t,
        vec![
            "UserMessage",
            "LlmCall",
            "LlmError",
            "UserMessage",
            "LlmCall",
            "LlmResponse",
            "TurnEnd",
        ],
        "nudge sequence diverged"
    );

    // Two requests must have been issued to the provider, in order.
    let reqs = provider.take_requests();
    assert_eq!(reqs.len(), 2, "provider must be called exactly twice");

    // History order: original user, nudge user, assistant — three records.
    let history = agent.history();
    assert_eq!(history.len(), 3);
    assert_eq!(history[0].role, Role::User);
    assert_eq!(history[1].role, Role::User);
    assert_eq!(history[2].role, Role::Assistant);

    // The nudge user-message must be readable as the canonical phrase.
    let nudge_text = match &history[1].content[0] {
        omega_core::ContentBlock::Text { text } => text.as_str(),
        other => panic!("nudge block not text: {other:?}"),
    };
    assert!(
        nudge_text.contains("could not be parsed"),
        "nudge wording lost: {nudge_text:?}"
    );

    // The second LlmCall must include the nudge message in its messages
    // array (proving we re-sent the conversation).
    let second_request = &reqs[1];
    assert!(
        second_request.messages.iter().any(|m| {
            m.role == Role::User && m.content.iter().any(|b| {
                matches!(
                    b,
                    omega_core::ContentBlock::Text { text } if text.contains("could not be parsed")
                )
            })
        }),
        "nudge user message must appear in the re-sent request"
    );

    // For symmetry: the second-to-last item must be an LlmResponse with a
    // filled-in context_hash.
    let response = items
        .iter()
        .find_map(|i| match i {
            AgentItem::Event(e) => match e.as_ref() {
                OmegaEvent::LlmResponse(r) => Some(r),
                _ => None,
            },
            AgentItem::Signal(_) => None,
        })
        .expect("LlmResponse event");
    assert_eq!(response.context_hash.len(), 12);
}
