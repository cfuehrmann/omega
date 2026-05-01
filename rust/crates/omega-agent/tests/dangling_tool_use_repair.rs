//! When the previous turn was interrupted between LlmResponse and tool
//! dispatch, the in-memory history's last record is an assistant
//! message containing `tool_use` blocks with no matching `tool_result`s.
//! `Agent::send_message` must synthesise `is_error` tool_results before
//! the new user message lands so the Anthropic API doesn't reject the
//! payload.

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
use omega_core::{AgentItem, ContentBlock, Message, Role};
use omega_protocol::OmegaEvent;
use omega_store::random_hash;
use serde_json::json;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn dangling_tool_use_synthesises_is_error_tool_results() {
    let (mut agent, provider, _tmp) = make_test_agent();

    // Pre-seed history: a user msg + an assistant msg whose tool_use was
    // never resolved.  Hashes are placeholders — the agent only re-sends
    // them in LlmCall.context_hashes.
    let user_msg = Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "first request".to_owned(),
        }],
    };
    let assistant_msg = Message {
        role: Role::Assistant,
        content: vec![ContentBlock::ToolUse {
            id: "tu_orphan".to_owned(),
            name: "read_file".to_owned(),
            input: json!({ "path": "missing.txt" }),
        }],
    };
    agent.seed_history(
        vec![user_msg, assistant_msg],
        vec![random_hash(), random_hash()],
    );

    // Provider just returns a clean reply for the resumed turn.
    provider.push_response(vec![Ok(make_llm_response(
        "end_turn",
        Some("resumed"),
        3,
        1,
    ))]);

    let stream = agent.send_message("continue".to_owned(), CancellationToken::new());
    let items = collect_stream(stream).await;

    let t = tags(&items);
    // Expected: ToolResult (synthetic), UserMessage, LlmCall, LlmResponse, TurnEnd.
    assert_eq!(
        t,
        vec![
            "ToolResult",
            "UserMessage",
            "LlmCall",
            "LlmResponse",
            "TurnEnd",
        ],
        "dangling-repair sequence diverged"
    );

    // The synthetic tool_result must reference the orphan id and be
    // marked as an error so the model knows it didn't actually run.
    let synthetic = items
        .iter()
        .find_map(|i| match i {
            AgentItem::Event(e) => match e.as_ref() {
                OmegaEvent::ToolResult(r) => Some(r),
                _ => None,
            },
            AgentItem::Signal(_) => None,
        })
        .expect("synthetic ToolResult event");
    assert_eq!(synthetic.id, "tu_orphan");
    assert_eq!(synthetic.name, "read_file");
    assert!(synthetic.is_error);
    assert!(
        synthetic.output.contains("not executed"),
        "expected 'not executed' marker in synthetic output, got: {:?}",
        synthetic.output
    );

    // History grew correctly: 2 (seeded) + 1 (synthetic user tool_results)
    // + 1 (new user message) + 1 (assistant final) = 5 records.
    let history = agent.history();
    assert_eq!(history.len(), 5);

    // The third record (the synthetic insertion) must be a User message
    // with a single is_error tool_result block.
    match &history[2].role {
        Role::User => {}
        other => panic!("expected User role at idx 2, got {other:?}"),
    }
    match &history[2].content[0] {
        ContentBlock::ToolResult {
            tool_use_id,
            is_error,
            ..
        } => {
            assert_eq!(tool_use_id, "tu_orphan");
            assert!(is_error);
        }
        other => panic!("expected ToolResult block, got {other:?}"),
    }

    // The first request the agent sent to the provider must include all
    // four messages in its `messages` array (seeded user, seeded
    // assistant, synthetic user, new user).  Proves the repair landed
    // before the LLM call.
    let reqs = provider.take_requests();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].messages.len(), 4);
}
