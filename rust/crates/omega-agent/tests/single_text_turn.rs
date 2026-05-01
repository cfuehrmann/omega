//! `Agent::send_message` happy path: model returns a single text reply
//! with no tool calls.  Verifies the basic event sequence and that the
//! `LlmResponse.context_hash` is filled in by the agent.

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
use omega_core::AgentItem;
use omega_protocol::{OmegaEvent, StreamSignal};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn single_text_turn_emits_expected_events_and_metrics() {
    let (mut agent, provider, _tmp) = make_test_agent();

    // One transcript: a streamed text delta + a final LlmResponse.
    provider.push_response(vec![
        Ok(AgentItem::Signal(StreamSignal::Text {
            text: "ok".to_owned(),
        })),
        Ok(make_llm_response("end_turn", Some("ok"), 5, 1)),
    ]);

    let stream = agent.send_message("hello".to_owned(), CancellationToken::new());
    let items = collect_stream(stream).await;

    assert_eq!(
        tags(&items),
        vec![
            "UserMessage",
            "LlmCall",
            "Signal:Text",
            "LlmResponse",
            "TurnEnd",
        ],
        "event sequence diverged from spec"
    );

    // The agent must replace the empty context_hash on LlmResponse with
    // the assistant record's hash.
    let llm_response = items
        .iter()
        .find_map(|i| match i {
            AgentItem::Event(e) => match e.as_ref() {
                OmegaEvent::LlmResponse(r) => Some(r),
                _ => None,
            },
            AgentItem::Signal(_) => None,
        })
        .expect("LlmResponse event missing");
    assert_eq!(llm_response.context_hash.len(), 12, "context_hash unset");

    // TurnEnd metrics should sum the single LlmResponse usage.
    let turn_end = items
        .iter()
        .find_map(|i| match i {
            AgentItem::Event(e) => match e.as_ref() {
                OmegaEvent::TurnEnd(t) => Some(t),
                _ => None,
            },
            AgentItem::Signal(_) => None,
        })
        .expect("TurnEnd event missing");
    assert_eq!(turn_end.metrics.input_tokens, 5);
    assert_eq!(turn_end.metrics.output_tokens, 1);

    // History grew: user + assistant.
    assert_eq!(agent.history().len(), 2);
}
