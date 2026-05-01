//! Two tool calls returned in one assistant turn must be dispatched
//! concurrently, their results bundled into a single user message, and
//! a follow-up LLM call made before the turn ends.

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
use omega_protocol::OmegaEvent;
use omega_protocol::events::ToolCallEvent;
use serde_json::json;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn parallel_tool_calls_dispatch_then_continue() {
    let (mut agent, provider, _tmp) = make_test_agent();

    // Turn 1: two ToolCall events emitted by the provider, then an
    // LlmResponse with stop_reason=tool_use.  We use `read_file` and
    // `list_files` because their stub tool bodies return immediately
    // (with is_error=true) — that's enough to drive the dispatch path.
    provider.push_response(vec![
        Ok(AgentItem::event(OmegaEvent::ToolCall(ToolCallEvent {
            time: "2024-01-01T00:00:00.000Z".to_owned(),
            id: "tu_a".to_owned(),
            name: "read_file".to_owned(),
            input: json!({ "path": "x.txt" }),
            context_hash: String::new(),
        }))),
        Ok(AgentItem::event(OmegaEvent::ToolCall(ToolCallEvent {
            time: "2024-01-01T00:00:00.000Z".to_owned(),
            id: "tu_b".to_owned(),
            name: "list_files".to_owned(),
            input: json!({ "path": "." }),
            context_hash: String::new(),
        }))),
        Ok(make_llm_response("tool_use", None, 7, 3)),
    ]);

    // Turn 2: model finishes after seeing the tool results.
    provider.push_response(vec![Ok(make_llm_response("end_turn", Some("done"), 9, 2))]);

    let stream = agent.send_message("please".to_owned(), CancellationToken::new());
    let items = collect_stream(stream).await;

    // Two tool calls executed concurrently, so their ToolResult order
    // is non-deterministic — assert tag *multiset* and key ordering
    // points instead of an exact sequence.
    let t = tags(&items);
    assert_eq!(t.first().copied(), Some("UserMessage"));
    assert_eq!(t.last().copied(), Some("TurnEnd"));
    let count = |needle: &str| t.iter().filter(|x| **x == needle).count();
    assert_eq!(count("LlmCall"), 2, "must call LLM twice");
    assert_eq!(count("ToolCall"), 2);
    assert_eq!(count("ToolResult"), 2);
    assert_eq!(count("LlmResponse"), 2);

    // The first LlmResponse must precede the ToolCalls.
    let first_response_idx = t
        .iter()
        .position(|x| *x == "LlmResponse")
        .expect("response 1");
    let first_toolcall_idx = t.iter().position(|x| *x == "ToolCall").expect("toolcall 1");
    assert!(
        first_response_idx < first_toolcall_idx,
        "tool calls must be emitted after the first LlmResponse"
    );

    // ToolCall events must carry the assistant context_hash filled in.
    let tool_call_events: Vec<&ToolCallEvent> = items
        .iter()
        .filter_map(|i| match i {
            AgentItem::Event(e) => match e.as_ref() {
                OmegaEvent::ToolCall(tc) => Some(tc),
                _ => None,
            },
            AgentItem::Signal(_) => None,
        })
        .collect();
    assert_eq!(tool_call_events.len(), 2);
    for tc in tool_call_events {
        assert_eq!(tc.context_hash.len(), 12, "context_hash unset on {tc:?}");
    }

    // History: user, assistant(tool_use), user(tool_results), assistant(final) = 4 records.
    assert_eq!(agent.history().len(), 4);
}
