//! Carve-out tests for genuinely agent-internal behaviour that is
//! awkward to provoke through a real LLM HTTP/SSE script.
//!
//! After TEST-ARCH-4 retired the bulk of `omega-agent/tests/`, the
//! agent loop is exercised end-to-end by:
//!
//! * `omega-cli/tests/cli.rs` (TEST-ARCH-1) — driving the real CLI
//!   binary against the `omega-test-fixtures` axum SSE fake.
//! * `omega-server/tests/{ws,ws_router}.rs` (TEST-ARCH-2) — driving the
//!   real server binary against the same fake, with raw-WS clients.
//! * `e2e/*.spec.ts` — Playwright against `omega-mock-server`.
//!
//! Coverage of `Agent::send_message` orchestration flows down from
//! those tiers.  This file exists only for the few internal contracts
//! that the HTTP/SSE fake cannot easily reproduce because they sit
//! inside the agent's reaction to *parsed* provider events:
//!
//! 1. **Dangling tool_use repair.** When the previous turn was
//!    interrupted between `LlmResponse` and tool dispatch, the
//!    in-memory history's last record is an assistant message with
//!    unmatched `tool_use` blocks.  `send_message` synthesises
//!    `is_error` `tool_result` blocks before the new user message
//!    lands, so the API doesn't reject the next request.  Reproducing
//!    this through real provider scripts requires crashing the agent
//!    mid-turn, persisting the half-resolved history, then resuming
//!    the same on-disk session — an order of magnitude more setup than
//!    the in-memory check below.
//!
//! 2. **Server-side compaction event.** The Anthropic SSE parser emits
//!    `OmegaEvent::Compacted` when the API decides to compact context
//!    server-side.  Reproducing the trigger via the fake would require
//!    extending it to emit Anthropic's compaction marker frames; the
//!    payload format is undocumented and only Anthropic's production
//!    backend ever generates it.  The downstream effect (history +
//!    context-hash clear) is purely a reaction inside `send_message`,
//!    so a direct injection test is the right shape.
//!
//! 3. **Malformed-tool-use JSON nudge.** When the SSE parser surfaces a
//!    `LlmError::Stream { message: "malformed tool_use JSON: ..." }`,
//!    the agent injects a corrective user message and re-issues the
//!    call (up to two times before giving up).  Reproducing this would
//!    require the fake to emit an SSE byte stream the parser rejects
//!    in exactly that shape.  Possible but brittle; the in-process
//!    error injection here is far more direct.
//!
//! All three tests use the `MockProvider` in `tests/common/mod.rs`.
//! Nothing else does — keeping that scaffolding alive for these three
//! tests is the explicit cost of the carve-out.

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
use omega_core::{AgentItem, ContentBlock, LlmError, Message, Role};
use omega_protocol::events::CompactedEvent;
use omega_protocol::{OmegaEvent, StreamSignal};
use omega_store::random_hash;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// 1. Dangling tool_use repair
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// 2. Server-side compaction
// ---------------------------------------------------------------------------

fn compacted_item(usage: Value) -> AgentItem {
    AgentItem::event(OmegaEvent::Compacted(CompactedEvent {
        time: "2024-06-01T00:00:00.000Z".to_owned(),
        usage,
    }))
}

fn read_events_jsonl(path: &std::path::Path) -> Vec<Value> {
    let raw = std::fs::read_to_string(path).expect("events.jsonl readable");
    raw.lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str(l).expect("valid JSON line"))
        .collect()
}

/// After two prior happy turns and a third turn that emits `Compacted`,
/// the agent's `history` must shrink to exactly one entry — the new
/// post-compaction assistant message.  `context_hashes` must follow
/// suit (one entry).  A subsequent turn must build on the cleared
/// history, and its `LlmCall.contextHashes` must contain only the
/// post-compaction hashes.  The `Compacted` event must be persisted to
/// `events.jsonl` with its `usage` payload preserved verbatim.
#[tokio::test]
async fn compacted_event_clears_history_and_persists_usage() {
    let (mut agent, provider, tmp) = make_test_agent();

    // Turn 1.
    provider.push_response(vec![
        Ok(AgentItem::Signal(StreamSignal::Text {
            text: "ok1".to_owned(),
        })),
        Ok(make_llm_response("end_turn", Some("ok1"), 100, 1)),
    ]);
    let _ = collect_stream(agent.send_message("first".to_owned(), CancellationToken::new())).await;

    // Turn 2.
    provider.push_response(vec![
        Ok(AgentItem::Signal(StreamSignal::Text {
            text: "ok2".to_owned(),
        })),
        Ok(make_llm_response("end_turn", Some("ok2"), 200, 2)),
    ]);
    let _ = collect_stream(agent.send_message("second".to_owned(), CancellationToken::new())).await;
    assert_eq!(agent.history().len(), 4);

    // Turn 3: provider emits Compacted with a rich usage payload, then a
    // post-compaction summary text + LlmResponse.
    let usage = json!({
        "input_tokens": 80_500,
        "output_tokens": 350,
        "service_tier": "standard",
        "iterations": [
            {"type": "compaction", "input_tokens": 80_000, "output_tokens": 300},
            {"type": "message",    "input_tokens": 500,    "output_tokens": 50}
        ]
    });
    provider.push_response(vec![
        Ok(compacted_item(usage.clone())),
        Ok(AgentItem::Signal(StreamSignal::Text {
            text: "summary".to_owned(),
        })),
        Ok(make_llm_response("end_turn", Some("summary"), 80_500, 250)),
    ]);
    let items =
        collect_stream(agent.send_message("third".to_owned(), CancellationToken::new())).await;

    // Compacted appears in the streamed items *before* the LlmResponse.
    let t = tags(&items);
    let cidx = t.iter().position(|x| *x == "Compacted").expect("Compacted");
    let ridx = t
        .iter()
        .position(|x| *x == "LlmResponse")
        .expect("LlmResponse");
    assert!(cidx < ridx, "Compacted must precede LlmResponse: {t:?}");

    // History collapsed to the lone post-compaction assistant message.
    assert_eq!(agent.history().len(), 1);
    assert!(matches!(agent.history()[0].role, Role::Assistant));

    // Turn 4 must build on the cleared history; its LlmCall must carry
    // only the 2 post-compaction context hashes.
    provider.push_response(vec![Ok(make_llm_response(
        "end_turn",
        Some("after"),
        50,
        3,
    ))]);
    let _ = collect_stream(agent.send_message("fourth".to_owned(), CancellationToken::new())).await;
    assert_eq!(agent.history().len(), 3);

    let events = read_events_jsonl(&tmp.path().join("events.jsonl"));
    let last_llm_call = events
        .iter()
        .filter(|v| v["type"] == "llm_call")
        .next_back()
        .expect("a final llm_call event");
    let hashes = last_llm_call["contextHashes"]
        .as_array()
        .expect("contextHashes array");
    assert_eq!(hashes.len(), 2);

    // Compacted persisted with usage verbatim (including `iterations[]`).
    let compacted = events
        .iter()
        .find(|v| v["type"] == "compacted")
        .expect("compacted event persisted");
    assert_eq!(compacted["usage"], usage);
}

// ---------------------------------------------------------------------------
// 3. Malformed tool_use JSON nudge
// ---------------------------------------------------------------------------

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
    assert_eq!(reqs.len(), 2);

    // History order: original user, nudge user, assistant — three records.
    let history = agent.history();
    assert_eq!(history.len(), 3);
    assert_eq!(history[0].role, Role::User);
    assert_eq!(history[1].role, Role::User);
    assert_eq!(history[2].role, Role::Assistant);

    let nudge_text = match &history[1].content[0] {
        ContentBlock::Text { text } => text.as_str(),
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
            m.role == Role::User
                && m.content.iter().any(|b| {
                    matches!(
                        b,
                        ContentBlock::Text { text } if text.contains("could not be parsed")
                    )
                })
        }),
        "nudge user message must appear in the re-sent request"
    );
}
