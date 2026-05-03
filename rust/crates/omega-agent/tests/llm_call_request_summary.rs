//! Verifies that `LlmCall` events carry a populated `request_summary`
//! with elided (non-wall-of-text) descriptors for `system`, `tools`, and
//! `messages`.
//!
//! Mirrors the TypeScript `elideAnthropicRequest` tests in
//! `src/agent-integration.test.ts`:
//!
//! * `llm_call carries url and requestSummary`
//! * `llm_call requestSummary has elided messages descriptor`
//! * `llm_call requestSummary includes all top-level request fields`

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::doc_markdown,
    clippy::missing_panics_doc
)]

mod common;

use common::{collect_stream, make_llm_response, make_test_agent};
use omega_core::AgentItem;
use omega_protocol::{OmegaEvent, StreamSignal};
use tokio_util::sync::CancellationToken;

fn llm_call_summary(items: &[AgentItem]) -> serde_json::Value {
    for item in items {
        if let AgentItem::Event(ev) = item {
            if let OmegaEvent::LlmCall(e) = ev.as_ref() {
                return e
                    .request_summary
                    .clone()
                    .expect("request_summary must be Some");
            }
        }
    }
    panic!("no LlmCall event found in items");
}

// ---------------------------------------------------------------------------
// send_message path
// ---------------------------------------------------------------------------

#[tokio::test]
async fn send_message_llm_call_has_request_summary() {
    let (mut agent, provider, _tmp) = make_test_agent();
    provider.push_response(vec![
        Ok(AgentItem::Signal(StreamSignal::Text {
            text: "hello".to_owned(),
        })),
        Ok(make_llm_response("end_turn", Some("hello"), 10, 2)),
    ]);

    let stream = agent.send_message("hi".to_owned(), CancellationToken::new());
    let items = collect_stream(stream).await;

    let summary = llm_call_summary(&items);

    // model is present and correct
    assert_eq!(summary["model"], "claude-sonnet-4-6");

    // max_tokens is present
    assert!(
        summary["max_tokens"].is_number(),
        "max_tokens must be a number"
    );

    // thinking is present (adaptive thinking is always enabled)
    assert_eq!(
        summary["thinking"]["type"], "adaptive",
        "thinking.type must be 'adaptive'"
    );

    // messages is a descriptor string, not an array
    let messages = summary["messages"]
        .as_str()
        .expect("messages must be a string");
    assert!(
        messages.contains("message"),
        "messages descriptor must contain 'message': {messages}"
    );
    assert!(
        messages.contains("chars"),
        "messages descriptor must contain 'chars': {messages}"
    );

    // system is a descriptor string, not the raw text
    let system = summary["system"].as_str().expect("system must be a string");
    assert!(
        system.starts_with('['),
        "system must be an elided descriptor: {system}"
    );
    assert!(
        system.contains("block"),
        "system descriptor must mention 'block': {system}"
    );
    // singular: first send_message has exactly 1 system block
    assert!(
        system.starts_with("[1 block,"),
        "system must use singular 'block' for a single block: {system}"
    );

    // tools is an array of elided objects
    let tools = summary["tools"].as_array().expect("tools must be an array");
    assert!(!tools.is_empty(), "tools array must not be empty");
    let first = &tools[0];
    assert!(first["name"].is_string(), "tool must have a name");
    assert_eq!(
        first["input_schema"], "[elided]",
        "tool input_schema must be elided"
    );
    let desc = first["description"]
        .as_str()
        .expect("description must be string");
    assert!(
        desc.ends_with("chars]"),
        "description must be elided: {desc}"
    );
}

#[tokio::test]
async fn send_message_messages_descriptor_reflects_count() {
    let (mut agent, provider, _tmp) = make_test_agent();
    provider.push_response(vec![Ok(make_llm_response("end_turn", Some("ok"), 5, 1))]);

    let stream = agent.send_message("hello".to_owned(), CancellationToken::new());
    let items = collect_stream(stream).await;

    let summary = llm_call_summary(&items);
    let messages = summary["messages"]
        .as_str()
        .expect("messages must be a string");
    // The first send_message adds exactly 1 user message before the call.
    assert!(
        messages.starts_with("[1 message,"),
        "expected '[1 message, …]' for a single message, got: {messages}"
    );
}

// ---------------------------------------------------------------------------
// perform_resumption path
// ---------------------------------------------------------------------------

#[tokio::test]
async fn resumption_llm_call_has_request_summary() {
    let (mut agent, provider, tmp) = make_test_agent();

    // Resumption response
    let resumption_text = "<summary>Prior summary.</summary><description>Did work</description>";
    provider.push_response(vec![
        Ok(AgentItem::Signal(StreamSignal::Text {
            text: resumption_text.to_owned(),
        })),
        Ok(make_llm_response("end_turn", Some(resumption_text), 10, 3)),
    ]);

    let basis_path = tmp.path().join("basis.txt");
    std::fs::write(&basis_path, "basis content").unwrap();

    let stream = agent.perform_resumption(
        "basis content".to_owned(),
        basis_path.to_string_lossy().into_owned(),
        None,
        CancellationToken::new(),
    );
    let items = collect_stream(stream).await;

    // Find the LlmCall event
    let summary = llm_call_summary(&items);

    assert!(
        summary["model"].is_string(),
        "model must be present in resumption request_summary"
    );
    assert!(
        summary["max_tokens"].is_number(),
        "max_tokens must be present"
    );

    // The resumption call has no tools
    assert!(
        summary.get("tools").is_none()
            || summary["tools"].as_array().map_or(true, |a| a.is_empty()),
        "resumption call must have no tools"
    );

    // messages is elided
    let messages = summary["messages"]
        .as_str()
        .expect("messages must be a string");
    assert!(
        messages.contains("message"),
        "messages must be elided: {messages}"
    );
}
