//! Tests that thinking-block signatures are preserved in context and echoed
//! back in subsequent LLM calls.
//!
//! Regression test for the HTTP 400 `messages.1.content.0.thinking.signature:
//! Field required` error: when the first LLM response includes a thinking
//! block, the `signature` field must be stored in `context.jsonl` and
//! included in the next API request.

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
use omega_core::{AgentItem, ContentBlock, Role};
use omega_protocol::OmegaEvent;
use omega_protocol::StreamSignal;
use omega_protocol::events::ToolCallEvent;
use serde_json::json;
use tokio_util::sync::CancellationToken;

const THINKING_TEXT: &str = "Let me orient myself first.";
const THINKING_SIG: &str = "EqQBCgIYAhIM4Lm1nK8eFakeSignatureForTest==";

/// Build a first-turn transcript that contains thinking + tool_use, as
/// observed in the production session log that triggered the bug.
fn thinking_tool_use_turn() -> Vec<Result<AgentItem, omega_core::LlmError>> {
    vec![
        Ok(AgentItem::Signal(StreamSignal::Thinking {
            text: THINKING_TEXT.to_owned(),
        })),
        // ThinkingBlockComplete carries the Anthropic signature.
        Ok(AgentItem::Signal(StreamSignal::ThinkingBlockComplete {
            signature: THINKING_SIG.to_owned(),
        })),
        Ok(AgentItem::event(OmegaEvent::ToolCall(ToolCallEvent {
            time: "2024-01-01T00:00:00.000Z".to_owned(),
            id: "tu_1".to_owned(),
            name: "list_files".to_owned(),
            input: json!({ "path": "." }),
            context_hash: String::new(),
        }))),
        Ok(make_llm_response("tool_use", None, 20, 10)),
    ]
}

// ---------------------------------------------------------------------------
// Test 1 — signature is stored in context.jsonl
// ---------------------------------------------------------------------------

#[tokio::test]
async fn thinking_signature_persisted_to_context() {
    let (mut agent, provider, tmp) = make_test_agent();

    // Turn 1: thinking + tool_use
    provider.push_response(thinking_tool_use_turn());
    // Turn 2: final text reply
    provider.push_response(vec![Ok(make_llm_response("end_turn", Some("done"), 15, 5))]);

    let stream = agent.send_message("orient me".to_owned(), CancellationToken::new());
    collect_stream(stream).await;

    // The assistant context record (message index 1, role=assistant) must
    // contain a thinking block with the signature intact.
    let raw =
        std::fs::read_to_string(tmp.path().join("context.jsonl")).expect("read context.jsonl");
    let lines: Vec<&str> = raw.lines().filter(|l| !l.trim().is_empty()).collect();

    // Expect: user, assistant(thinking+tool_use), user(tool_results), assistant(text)
    assert!(lines.len() >= 2, "context.jsonl too short");

    let assistant1: serde_json::Value =
        serde_json::from_str(lines[1]).expect("parse assistant record");
    assert_eq!(assistant1["role"], "assistant");

    let blocks = assistant1["content"].as_array().expect("content array");
    let thinking_block = blocks
        .iter()
        .find(|b| b["type"] == "thinking")
        .expect("no thinking block found");

    assert_eq!(
        thinking_block["thinking"], THINKING_TEXT,
        "thinking text must be preserved"
    );
    assert_eq!(
        thinking_block["signature"], THINKING_SIG,
        "signature must be preserved — without it the next API call returns HTTP 400"
    );
}

// ---------------------------------------------------------------------------
// Test 2 — signature echoed in the second LLM request
// ---------------------------------------------------------------------------

#[tokio::test]
async fn thinking_signature_echoed_in_second_request() {
    let (mut agent, provider, _tmp) = make_test_agent();

    // Turn 1: thinking + tool_use
    provider.push_response(thinking_tool_use_turn());
    // Turn 2: final text reply
    provider.push_response(vec![Ok(make_llm_response("end_turn", Some("done"), 15, 5))]);

    let stream = agent.send_message("orient me".to_owned(), CancellationToken::new());
    collect_stream(stream).await;

    let requests = provider.take_requests();
    assert_eq!(requests.len(), 2, "expected exactly 2 LLM calls");

    // The second request must have the thinking block (with signature) in
    // its message history, at messages[1].content[0].
    let second_request = &requests[1];
    let messages = &second_request.messages;
    // messages[0] = user, messages[1] = assistant (thinking + tool_use)
    assert!(
        messages.len() >= 2,
        "second request must carry prior history"
    );

    let assistant_msg = &messages[1];
    assert_eq!(assistant_msg.role, Role::Assistant);

    let thinking_block = assistant_msg
        .content
        .iter()
        .find(|b| matches!(b, ContentBlock::Thinking { .. }))
        .expect("thinking block missing from second request");

    match thinking_block {
        ContentBlock::Thinking {
            thinking,
            signature,
        } => {
            assert_eq!(thinking, THINKING_TEXT);
            assert_eq!(
                signature.as_deref(),
                Some(THINKING_SIG),
                "signature must be echoed in second request — HTTP 400 without it"
            );
        }
        _ => panic!("unexpected block type"),
    }
}

// ---------------------------------------------------------------------------
// Test 3 — ThinkingBlockComplete is NOT forwarded to the UI
// ---------------------------------------------------------------------------

#[tokio::test]
async fn thinking_block_complete_signal_not_forwarded_to_ui() {
    let (mut agent, provider, _tmp) = make_test_agent();

    provider.push_response(vec![
        Ok(AgentItem::Signal(StreamSignal::Thinking {
            text: "hmm".to_owned(),
        })),
        Ok(AgentItem::Signal(StreamSignal::ThinkingBlockComplete {
            signature: "sig".to_owned(),
        })),
        Ok(make_llm_response("end_turn", Some("reply"), 5, 3)),
    ]);

    let stream = agent.send_message("hi".to_owned(), CancellationToken::new());
    let items = collect_stream(stream).await;

    let t = tags(&items);
    assert!(
        t.contains(&"Signal:Thinking"),
        "Thinking delta must be forwarded to UI"
    );
    assert!(
        !t.contains(&"Signal:ThinkingBlockComplete"),
        "ThinkingBlockComplete must NOT reach the UI"
    );
}
