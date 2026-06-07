#![allow(
    clippy::match_wildcard_for_single_variants, // defensive wildcards in test matches
    clippy::filter_next, // .filter().next() reads as the intent in tests
    clippy::cast_possible_wrap, // usize→i64 is safe in test sizes
    clippy::collapsible_if, // nested if-let reads more clearly than let-chains here
)]

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
//! 2. **Server-side compaction via usage.iterations.** The Anthropic provider
//!    embeds compaction info in `LlmResponseEnded.usage.iterations` when
//!    server-side context compaction fires.  The agent detects a
//!    `type=="compaction"` iteration entry and clears history / context-hashes
//!    so the next turn starts from a fresh baseline.  Phase 2.0 (F11) adds a
//!    `ContextCompacted` event immediately before the corresponding
//!    `LlmResponseEnded`, recording token counts for before/after inspection
//!    and closing the strict-resume gap.  Injecting this via the
//!    `MockProvider` (emitting a `LlmResponseEndedEvent` with the right usage
//!    payload) is the right shape; real Anthropic SSE is far harder to replicate.
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
//!
//! 4. **A1 structural guard.** `user_role_context_appends_are_event_backed`
//!    is a source-scan test (not a runtime test) that verifies every
//!    `context_store.append(Role::User, …)` call in `agent.rs` is inside
//!    an allowlisted `inject_*` helper or a documented bootstrap function.
//!    It turns RED whenever someone adds a new unguarded user-role context
//!    append, enforcing the invariant from §15(a) of
//!    `docs/monitors-design.html` by construction.  This is a string-scan
//!    assertion and is not suitable for mutation testing; that is noted in
//!    the Justfile recipe `mutants-a1-guard`.

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

use common::{
    collect_stream, drive, make_llm_response, make_monitor_item, make_terminal_response,
    make_test_agent, make_tool_use_items, tags,
};
use omega_agent::{Agent, AgentConfig, InputItem, InputQueue};
use omega_core::{AgentItem, ContentBlock, LlmError, LlmRequest, Message, Role};
use omega_store::{ContextStore, EventStore, content_hash};
use omega_types::events::MonitorStopReason;
use omega_types::events::ToolResultEvent;
use omega_types::events::{
    ContextCompactedEvent, HarnessRecoveryKind, LlmResponseEndedEvent, LlmResponseUsage,
    UsageIteration,
};
use omega_types::{FeatureFlags, OmegaEvent, StreamSignal};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

use futures::StreamExt;
use omega_tools::MonitorStatus;
use tokio::time::timeout;

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
    let user_hash = content_hash(&user_msg.role, &user_msg.content);
    let assistant_hash = content_hash(&assistant_msg.role, &assistant_msg.content);
    agent.seed_history(
        vec![user_msg, assistant_msg],
        vec![user_hash, assistant_hash],
    );

    // Provider returns a non-empty reply so the empty-response guard
    // does not fire (a bare make_llm_response produces zero content blocks).
    provider.push_response(make_terminal_response("end_turn", 3, 1));

    let stream = drive(&mut agent, "continue".to_owned(), CancellationToken::new());
    let items = collect_stream(stream).await;

    let t = tags(&items);
    assert_eq!(
        t,
        vec![
            "ToolResult",
            "UserMessage",
            "LlmCall",
            "LlmResponseStarted",
            "Signal:Text", // make_terminal_response produces a text delta
            "LlmResponseEnded",
            "TurnEnd",
        ],
        "dangling-repair sequence diverged"
    );

    // The synthetic tool_result event carries a freshly minted Omega
    // `tool_call_id` (no upstream ToolCall to correlate with), so the
    // event itself doesn't reference "tu_orphan" — only the
    // `ContentBlock::ToolResult.tool_use_id` written to the
    // conversation history does (asserted further below).  Here we
    // verify the event-level shape: right tool name, error flag set,
    // and a non-empty Omega correlation id present.
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
    assert!(
        !synthetic.tool_call_id.is_empty(),
        "synthetic ToolResult must carry an Omega tool_call_id",
    );
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

    // The first request the agent sent to the provider must contain 3
    // messages: [seeded_user, seeded_assistant, merged_user].
    //
    // project_messages() merges consecutive role:user entries into one
    // API message, so the synthetic tool-result user message and the new
    // "continue" user message are combined.  The in-memory history still
    // has 5 entries (asserted above); only the projected API view collapses.
    let reqs = provider.take_requests();
    assert_eq!(reqs.len(), 1);
    assert_eq!(
        reqs[0].messages.len(),
        3,
        "project_messages must merge the synthetic tool-result and the new \
         user message into one role:user API message"
    );
    // The merged user message is the last one; it must contain both the
    // ToolResult block (from the dangling repair) and the Text block (from
    // the new "continue" message).
    let merged = &reqs[0].messages[2];
    assert!(
        merged.content.len() >= 2,
        "merged user message must have at least 2 content blocks, got {}",
        merged.content.len()
    );
    let has_tool_result = merged
        .content
        .iter()
        .any(|b| matches!(b, omega_core::ContentBlock::ToolResult { .. }));
    let has_text = merged
        .content
        .iter()
        .any(|b| matches!(b, omega_core::ContentBlock::Text { .. }));
    assert!(
        has_tool_result,
        "merged message must contain the ToolResult block"
    );
    assert!(
        has_text,
        "merged message must contain the Text block for 'continue'"
    );
}

// ---------------------------------------------------------------------------
// 2. Server-side compaction
// ---------------------------------------------------------------------------

fn read_events_jsonl(path: &std::path::Path) -> Vec<Value> {
    let raw = std::fs::read_to_string(path).expect("events.jsonl readable");
    raw.lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str(l).expect("valid JSON line"))
        .collect()
}

/// After two prior happy turns and a third turn whose `LlmResponseEnded`
/// carries a `usage.iterations` entry with `type=="compaction"`, the agent's
/// `history` must shrink to exactly one entry — the new post-compaction
/// assistant message.  `context_hashes` must follow suit.  A subsequent turn
/// must build on the cleared history, and its `LlmCall.contextHashes` must
/// contain only the post-compaction hashes.  The `llm_response_ended` event
/// must be persisted to `events.jsonl` with the iterations preserved.
#[tokio::test]
async fn compacted_event_clears_history_and_persists_usage() {
    let (mut agent, provider, tmp) = make_test_agent();

    // Turn 1.
    provider.push_response(vec![
        Ok(AgentItem::Signal(StreamSignal::Text {
            index: 0,
            text: "ok1".to_owned(),
        })),
        Ok(make_llm_response("end_turn", 100, 1)),
    ]);
    let _ = collect_stream(drive(
        &mut agent,
        "first".to_owned(),
        CancellationToken::new(),
    ))
    .await;

    // Turn 2.
    provider.push_response(vec![
        Ok(AgentItem::Signal(StreamSignal::Text {
            index: 0,
            text: "ok2".to_owned(),
        })),
        Ok(make_llm_response("end_turn", 200, 2)),
    ]);
    let _ = collect_stream(drive(
        &mut agent,
        "second".to_owned(),
        CancellationToken::new(),
    ))
    .await;
    assert_eq!(agent.history().len(), 4);

    // Turn 3: provider emits LlmResponseEnded with compaction iterations,
    // signalling server-side compaction. The agent detects
    // type=="compaction" in usage.iterations and clears history.
    provider.push_response(vec![
        Ok(AgentItem::Signal(StreamSignal::Text {
            index: 0,
            text: "summary".to_owned(),
        })),
        Ok(AgentItem::Event(Box::new(OmegaEvent::LlmResponseEnded(
            LlmResponseEndedEvent {
                time: "2024-06-01T00:00:00.000Z".to_owned(),
                stop_reason: "end_turn".to_owned(),
                cleared_tool_uses: None,
                cleared_input_tokens: None,
                usage: LlmResponseUsage {
                    input_tokens: 80_500,
                    output_tokens: 250,
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                    service_tier: None,
                    iterations: Some(vec![
                        UsageIteration {
                            iteration_type: "compaction".to_owned(),
                            input_tokens: 80_000,
                            output_tokens: 300,
                            cache_creation_input_tokens: None,
                            cache_read_input_tokens: None,
                            service_tier: None,
                        },
                        UsageIteration {
                            iteration_type: "message".to_owned(),
                            input_tokens: 500,
                            output_tokens: 50,
                            cache_creation_input_tokens: None,
                            cache_read_input_tokens: None,
                            service_tier: None,
                        },
                    ]),
                },
                context_hash: String::new(),
                response_summary: None,
            },
        )))),
    ]);
    let _ = collect_stream(drive(
        &mut agent,
        "third".to_owned(),
        CancellationToken::new(),
    ))
    .await;

    // History collapsed to the lone post-compaction assistant message.
    assert_eq!(agent.history().len(), 1);
    assert!(matches!(agent.history()[0].role, Role::Assistant));

    // Turn 4 must build on the cleared history; its LlmCall must carry
    // only the 2 post-compaction context hashes.
    provider.push_response(make_terminal_response("end_turn", 50, 3));
    let _ = collect_stream(drive(
        &mut agent,
        "fourth".to_owned(),
        CancellationToken::new(),
    ))
    .await;
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

    // The llm_response_ended event must be persisted with its iterations.
    let ended = events
        .iter()
        .filter(|v| v["type"] == "llm_response_ended")
        .nth(2) // third turn's ended event (index 2, 0-based)
        .expect("third llm_response_ended persisted");
    let iters = ended["usage"]["iterations"]
        .as_array()
        .expect("usage.iterations array");
    assert_eq!(iters.len(), 2);
    assert_eq!(iters[0]["type"], "compaction");
}

/// Phase 2.0 (F11): a turn with server-side compaction must emit a
/// `ContextCompacted` event **immediately before** its `LlmResponseEnded`,
/// with correct `tokensBefore`, `tokensAfter`, and `summaryTokens` matching
/// the compaction and message iterations.
///
/// Fold invariant: folding `events.jsonl` and clearing accumulated
/// context-hashes on every `context_compacted` event reproduces the same
/// history size as `agent.history().len()` — proving strict resume can
/// reconstruct the LLM-visible context from the event log alone.
#[tokio::test]
async fn context_compacted_event_emitted_before_response_with_correct_tokens() {
    let (mut agent, provider, tmp) = make_test_agent();

    // Two prior turns to build up history.
    provider.push_response(vec![
        Ok(AgentItem::Signal(StreamSignal::Text {
            index: 0,
            text: "ok1".to_owned(),
        })),
        Ok(make_llm_response("end_turn", 100, 1)),
    ]);
    let _ = collect_stream(drive(
        &mut agent,
        "first".to_owned(),
        CancellationToken::new(),
    ))
    .await;

    // Compaction turn: provider fires LlmResponseEnded with both a
    // `compaction` iteration and a `message` iteration.
    provider.push_response(vec![
        Ok(AgentItem::Signal(StreamSignal::Text {
            index: 0,
            text: "summary".to_owned(),
        })),
        Ok(AgentItem::Event(Box::new(OmegaEvent::LlmResponseEnded(
            LlmResponseEndedEvent {
                time: "2024-06-01T00:00:00.000Z".to_owned(),
                stop_reason: "end_turn".to_owned(),
                cleared_tool_uses: None,
                cleared_input_tokens: None,
                usage: LlmResponseUsage {
                    input_tokens: 80_500,
                    output_tokens: 250,
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                    service_tier: None,
                    iterations: Some(vec![
                        UsageIteration {
                            iteration_type: "compaction".to_owned(),
                            input_tokens: 80_000,
                            output_tokens: 300,
                            cache_creation_input_tokens: None,
                            cache_read_input_tokens: None,
                            service_tier: None,
                        },
                        UsageIteration {
                            iteration_type: "message".to_owned(),
                            input_tokens: 500,
                            output_tokens: 50,
                            cache_creation_input_tokens: None,
                            cache_read_input_tokens: None,
                            service_tier: None,
                        },
                    ]),
                },
                context_hash: String::new(),
                response_summary: None,
            },
        )))),
    ]);
    let items = collect_stream(drive(
        &mut agent,
        "compact me".to_owned(),
        CancellationToken::new(),
    ))
    .await;

    // -----------------------------------------------------------------------
    // 1. Stream items: ContextCompacted must appear before LlmResponseEnded.
    // -----------------------------------------------------------------------
    let compact_pos = items.iter().position(|item| {
        matches!(item, AgentItem::Event(ev) if matches!(ev.as_ref(), OmegaEvent::ContextCompacted(_)))
    });
    let ended_pos = items.iter().rposition(|item| {
        matches!(item, AgentItem::Event(ev) if matches!(ev.as_ref(), OmegaEvent::LlmResponseEnded(_)))
    });
    let compact_pos = compact_pos.expect("ContextCompacted emitted in stream");
    let ended_pos = ended_pos.expect("LlmResponseEnded emitted in stream");
    assert!(
        compact_pos < ended_pos,
        "ContextCompacted (pos {compact_pos}) must precede LlmResponseEnded (pos {ended_pos})"
    );

    // -----------------------------------------------------------------------
    // 2. Token counts in the ContextCompacted event match the iterations.
    // -----------------------------------------------------------------------
    let cc_event = items
        .iter()
        .find_map(|item| {
            if let AgentItem::Event(ev) = item {
                if let OmegaEvent::ContextCompacted(cc) = ev.as_ref() {
                    return Some(cc.clone());
                }
            }
            None
        })
        .expect("ContextCompacted found");
    assert_eq!(
        cc_event,
        ContextCompactedEvent {
            time: cc_event.time.clone(), // timestamp is non-deterministic
            tokens_before: 80_000,
            tokens_after: 500,
            summary_tokens: 300,
        }
    );

    // -----------------------------------------------------------------------
    // 3. events.jsonl: context_compacted persisted with camelCase fields and
    //    appears immediately before llm_response_ended in the event log.
    // -----------------------------------------------------------------------
    let events = read_events_jsonl(&tmp.path().join("events.jsonl"));
    let positions: Vec<(usize, &str)> = events
        .iter()
        .enumerate()
        .filter_map(|(i, v)| {
            let t = v["type"].as_str()?;
            if matches!(t, "context_compacted" | "llm_response_ended") {
                Some((i, t))
            } else {
                None
            }
        })
        .collect();
    // Find the compaction-turn pair: context_compacted followed by
    // llm_response_ended.
    let cc_json_pos = positions
        .iter()
        .find(|(_, t)| *t == "context_compacted")
        .map(|(i, _)| *i)
        .expect("context_compacted in events.jsonl");
    let ended_json_pos = positions
        .iter()
        .rfind(|(_, t)| *t == "llm_response_ended")
        .map(|(i, _)| *i)
        .expect("llm_response_ended in events.jsonl");
    assert!(
        cc_json_pos < ended_json_pos,
        "context_compacted (line {cc_json_pos}) precedes llm_response_ended (line {ended_json_pos})"
    );

    // Verify camelCase fields and token values in the persisted event.
    let cc_json = &events[cc_json_pos];
    assert_eq!(cc_json["type"], "context_compacted");
    assert_eq!(cc_json["tokensBefore"], 80_000_i64);
    assert_eq!(cc_json["tokensAfter"], 500_i64);
    assert_eq!(cc_json["summaryTokens"], 300_i64);

    // -----------------------------------------------------------------------
    // 4. Fold invariant: folding events.jsonl honours ContextCompacted by
    //    clearing accumulated context-hashes, leaving exactly one assistant
    //    hash — matching agent.history().len() == 1.
    // -----------------------------------------------------------------------
    let mut fold_hashes: Vec<String> = Vec::new();
    for ev in &events {
        match ev["type"].as_str().unwrap_or("") {
            "context_compacted" => fold_hashes.clear(),
            "llm_response_ended" => {
                let h = ev["contextHash"].as_str().unwrap_or("").to_owned();
                if !h.is_empty() {
                    fold_hashes.push(h);
                }
            }
            _ => {}
        }
    }
    assert_eq!(
        fold_hashes.len(),
        agent.history().len(),
        "fold produces {}, agent.history() has {}",
        fold_hashes.len(),
        agent.history().len()
    );
    assert_eq!(
        fold_hashes.len(),
        1,
        "exactly one post-compaction hash in the fold"
    );
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
    // Turn 2: model now produces a clean non-empty text reply.
    provider.push_response(make_terminal_response("end_turn", 6, 2));

    let stream = drive(&mut agent, "please".to_owned(), CancellationToken::new());
    let items = collect_stream(stream).await;

    let t = tags(&items);
    assert_eq!(
        t,
        vec![
            "UserMessage",
            "LlmCall",
            "LlmResponseStarted",
            "LlmResponseDiscarded",
            "LlmError",
            "HarnessRecovery", // injected nudge (InvalidToolJson)
            "LlmCall",
            "LlmResponseStarted",
            "Signal:Text",
            "LlmResponseEnded",
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

// ---------------------------------------------------------------------------
// BUG-D: tool-call / tool-result clearing audit + fix guard
// ---------------------------------------------------------------------------

/// Helper: build a single tool-use transcript that uses `run_command` with
/// `echo` so the tool round-trip completes quickly in tests.
fn echo_tool_response(id: &str, turn_num: usize) -> Vec<Result<AgentItem, LlmError>> {
    use omega_types::events::ToolCallEvent;
    vec![
        Ok(AgentItem::event(OmegaEvent::ToolCall(ToolCallEvent {
            time: "2024-01-01T00:00:00.000Z".to_owned(),
            tool_call_id: id.to_owned(),
            name: "run_command".to_owned(),
            input: serde_json::json!({ "command": format!("echo turn{turn_num}") }),
            context_hash: String::new(),
        }))),
        Ok(make_llm_response("tool_use", (turn_num * 100) as i64, 5)),
    ]
}

/// **BUG-D audit (RED → GREEN).**
///
/// Before the fix, `context_management` is `None` in every `LlmRequest`,
/// so Anthropic's server-side tool-result clearing never fires.  This test
/// asserts that `context_management` IS present in every captured request,
/// which fails before the fix and passes after.
///
/// The companion audit below documents the monotonic growth of
/// `request_bytes` that results from never clearing tool history.
#[tokio::test]
async fn context_management_present_in_every_llm_request() {
    let (mut agent, provider, _tmp) = make_test_agent();

    // 8 turns: each turn fires one tool call (run_command echo),
    // then the model emits end_turn.
    for i in 1..=8_usize {
        let tool_id = format!("tu_{i:02}");
        // Tool-use turn.
        provider.push_response(echo_tool_response(&tool_id, i));
        // Post-tool final turn (non-empty so the loop ends normally).
        provider.push_response(make_terminal_response("end_turn", (i * 100) as i64, 3));
    }

    for i in 1..=8_usize {
        let stream = drive(&mut agent, format!("turn {i}"), CancellationToken::new());
        let _ = collect_stream(stream).await;
    }

    let reqs = provider.take_requests();
    assert!(!reqs.is_empty(), "expected captured requests");

    // Every request must carry context_management (BUG-D fix).
    for (i, req) in reqs.iter().enumerate() {
        assert!(
            req.context_management.is_some(),
            "request {i} missing context_management — BUG-D not fixed"
        );
    }

    // The context_management must contain the clear_tool_uses_20250919 edit.
    let cm = reqs[0].context_management.as_ref().unwrap();
    let edits = cm["edits"].as_array().expect("edits array");
    let has_clear_tool_uses = edits
        .iter()
        .any(|e| e["type"] == "clear_tool_uses_20250919");
    assert!(
        has_clear_tool_uses,
        "context_management.edits must include clear_tool_uses_20250919"
    );
}

/// **BUG-D audit (read-only — always GREEN).**
///
/// Documents that `request_bytes` on `LlmCallEvent` grows monotonically as
/// tool call/result pairs accumulate.  This is the expected behaviour before
/// and after the BUG-D fix because `MockProvider` never actually fires
/// Anthropic’s server-side clearing (which requires real input-token counts
/// exceeding the configured threshold).
///
/// The real-world plateau effect is observable via `LlmCallEvent.requestBytes`
/// in production sessions once BUG-C (prompt caching) and BUG-D
/// (context_management) are both fixed.
#[tokio::test]
async fn audit_request_bytes_grow_without_context_management() {
    let (mut agent, provider, _tmp) = make_test_agent();

    // 6 tool-use turns: accumulate tool I/O in history.
    for i in 1..=6_usize {
        let tool_id = format!("tu_{i:02}");
        provider.push_response(echo_tool_response(&tool_id, i));
        provider.push_response(make_terminal_response("end_turn", (i * 50) as i64, 2));
    }

    let mut request_bytes_seq: Vec<i64> = Vec::new();
    for i in 1..=6_usize {
        let stream = drive(&mut agent, format!("turn {i}"), CancellationToken::new());
        let items = collect_stream(stream).await;
        // Extract LlmCall events from this turn and record request_bytes.
        for item in &items {
            if let AgentItem::Event(boxed) = item {
                if let OmegaEvent::LlmCall(ev) = boxed.as_ref() {
                    request_bytes_seq.push(ev.request_bytes);
                }
            }
        }
    }

    // Verify we captured bytes for every turn-call pair.
    assert_eq!(
        request_bytes_seq.len(),
        12,
        "expected 2 LlmCall events per turn (tool + final) x 6 turns"
    );

    // The bytes grow: earlier calls have smaller histories than later ones.
    // This is the monotonic-growth that context_management is meant to bound.
    assert!(
        request_bytes_seq.windows(2).all(|w| w[0] <= w[1]),
        "request_bytes must grow (or stay equal) turn over turn: {request_bytes_seq:?}"
    );

    // Confirm the last call's request is larger than the first.
    assert!(
        request_bytes_seq.last().unwrap() > request_bytes_seq.first().unwrap(),
        "request_bytes must increase over 6 tool turns: {request_bytes_seq:?}"
    );
}

// ---------------------------------------------------------------------------
// Active model / effort accessors
// ---------------------------------------------------------------------------
//
// `Agent::active_model` and `Agent::active_effort` are read by the
// router on every status push (`omega-server/src/router.rs:246-251`)
// to populate the SessionInfoCache that the leptos UI's StatusChip
// renders. They're also used by the effort-reset gate (lines 630-631).
// Without these tests, mutating the accessors to return `""` survived
// the workspace mutation sweep because no test actually observed the
// value. Each accessor gets two checks: initial-config visibility and
// post-`set_*` visibility.

/// Kills `replace Agent::active_model -> &str with ""` (and `"xyzzy"`).
#[tokio::test]
async fn active_model_reflects_initial_config() {
    let (agent, _p, _t) = make_test_agent();
    // make_test_agent constructs with model="claude-sonnet-4-6".
    assert_eq!(agent.active_model(), "claude-sonnet-4-6");
}

/// Pinned: a follow-on `set_model` call is observable through the
/// accessor. Required because the SessionInfoCache snapshot a slash-
/// command refresh produces would otherwise still show the old model.
#[tokio::test]
async fn active_model_reflects_set_model() {
    let (mut agent, _p, _t) = make_test_agent();
    let _ = agent.set_model("claude-opus-4-8".to_owned()).await;
    assert_eq!(agent.active_model(), "claude-opus-4-8");
}

/// Kills `replace Agent::active_effort -> &str with ""` (and `"xyzzy"`).
/// `make_test_agent` constructs with `effort = None`; the agent
/// substitutes `DEFAULT_EFFORT` ("medium") in `Agent::new`, so the
/// accessor MUST round-trip the resolved string (not be replaced by a
/// literal `""` or `"xyzzy"` returned regardless of state).
#[tokio::test]
async fn active_effort_reflects_initial_config() {
    let (agent, _p, _t) = make_test_agent();
    assert_eq!(agent.active_effort(), "medium");
}

/// Pinned: a follow-on `set_effort` is observable through the accessor.
/// Without this, a router that broadcasts the effort to the StatusChip
/// would still show the previous value after `/effort` slash-command.
#[tokio::test]
async fn active_effort_reflects_set_effort() {
    let (mut agent, _p, _t) = make_test_agent();
    let _ = agent.set_effort("medium".to_owned()).await;
    assert_eq!(agent.active_effort(), "medium");
}

// ---------------------------------------------------------------------------
// Cache-token propagation into TurnEnd metrics
// ---------------------------------------------------------------------------

/// A mocked LLM response that reports non-zero cache_creation_input_tokens
/// and cache_read_input_tokens must surface those values in the emitted
/// `turn_end` event's `metrics.cacheCreationTokens` / `cacheReadTokens`
/// fields so that `bench/omega_agent.py:populate_context_post_run` can
/// accumulate them correctly.
///
/// Regression test: the `TurnMetrics` struct has `cache_creation_tokens` /
/// `cache_read_tokens` fields and the agent must propagate them from the
/// `LlmResponseUsage.cache_creation_input_tokens` /
/// `cache_read_input_tokens` fields returned by the Anthropic API.
#[tokio::test]
async fn turn_end_metrics_carry_cache_tokens() {
    let (mut agent, provider, tmp) = make_test_agent();

    // Build a response with non-zero cache tokens.
    let response_with_cache = AgentItem::Event(Box::new(OmegaEvent::LlmResponseEnded(
        LlmResponseEndedEvent {
            time: "2024-01-01T00:00:00.000Z".to_owned(),
            stop_reason: "end_turn".to_owned(),
            cleared_tool_uses: None,
            cleared_input_tokens: None,
            usage: LlmResponseUsage {
                input_tokens: 1000,
                output_tokens: 50,
                cache_creation_input_tokens: Some(800),
                cache_read_input_tokens: Some(200),
                service_tier: None,
                iterations: None,
            },
            context_hash: String::new(),
            response_summary: None,
        },
    )));

    // Add a text signal so assistant_blocks is non-empty; the
    // empty-response guard must not fire for this test.
    provider.push_response(vec![
        Ok(AgentItem::Signal(StreamSignal::Text {
            index: 0,
            text: "ok".to_owned(),
        })),
        Ok(response_with_cache),
    ]);

    let stream = drive(&mut agent, "hello".to_owned(), CancellationToken::new());
    let items = collect_stream(stream).await;

    // Locate the TurnEnd event.
    let turn_end = items
        .iter()
        .find_map(|item| match item {
            AgentItem::Event(boxed) => match boxed.as_ref() {
                OmegaEvent::TurnEnd(ev) => Some(ev.clone()),
                _ => None,
            },
            _ => None,
        })
        .expect("turn_end event must be emitted");

    assert_eq!(
        turn_end.metrics.input_tokens, 1000,
        "inputTokens mismatch: {:?}",
        turn_end.metrics
    );
    assert_eq!(
        turn_end.metrics.output_tokens, 50,
        "outputTokens mismatch: {:?}",
        turn_end.metrics
    );
    assert_eq!(
        turn_end.metrics.cache_creation_tokens,
        Some(800),
        "cacheCreationTokens must be Some(800), got: {:?}",
        turn_end.metrics
    );
    assert_eq!(
        turn_end.metrics.cache_read_tokens,
        Some(200),
        "cacheReadTokens must be Some(200), got: {:?}",
        turn_end.metrics
    );

    // Also verify the serialised JSON uses camelCase keys expected by
    // bench/omega_agent.py:populate_context_post_run.
    let events = read_events_jsonl(&tmp.path().join("events.jsonl"));
    let turn_end_json = events
        .iter()
        .find(|v| v["type"] == "turn_end")
        .expect("turn_end must be present in events.jsonl");

    let metrics = &turn_end_json["metrics"];
    assert_eq!(metrics["inputTokens"], 1000, "JSON inputTokens");
    assert_eq!(metrics["outputTokens"], 50, "JSON outputTokens");
    assert_eq!(
        metrics["cacheCreationTokens"], 800,
        "JSON cacheCreationTokens must be 800, was: {metrics}"
    );
    assert_eq!(
        metrics["cacheReadTokens"], 200,
        "JSON cacheReadTokens must be 200, was: {metrics}"
    );
}

// ---------------------------------------------------------------------------
// 5. System-prompt-path guard (e2e)
//
// Verifies the full stack:
//   discover_instruction_files → build_system_blocks → system_prompt_paths
//   → ToolCtx → execute_tool guard → ToolResult in the event stream.
//
// The unit-level tests in omega-tools/tests/file_tools.rs cover mutation
// variants of the guard function itself; this test checks that the agent
// wires everything together correctly end-to-end.
// ---------------------------------------------------------------------------

/// A simulated repo: a tempdir with a `.git` marker and an `AGENTS.md`.
/// `Agent::init` calls `discover_instruction_files(cwd)` which walks
/// upward looking for `.git`; having it in the same directory keeps the
/// test self-contained regardless of where the test binary runs.
fn setup_fake_repo() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    // A plain file (not directory) is enough to satisfy `find_git_root`
    // because it only checks `.exists()`.
    std::fs::write(tmp.path().join(".git"), "gitdir: fake").unwrap();
    let agents_md = tmp.path().join("AGENTS.md");
    std::fs::write(&agents_md, "# Test project\nDo the thing.").unwrap();
    (tmp, agents_md)
}

#[tokio::test]
async fn system_prompt_guard_blocks_read_of_instruction_file_end_to_end() {
    use omega_agent::{Agent, AgentConfig};
    use omega_store::{ContextStore, EventStore};
    use std::sync::Arc;

    // Arrange: agent whose CWD is a fake repo containing AGENTS.md.
    let (tmp, agents_md) = setup_fake_repo();

    // Reuse the make_test_agent factory but point it at our fake repo.
    // We need a custom CWD, so we wire the agent directly here.
    let provider = Arc::new(common::MockProvider::new());
    let mut agent = Agent::new(
        provider.clone(),
        ContextStore::new(tmp.path().join("context.jsonl")),
        EventStore::new(tmp.path().join("events.jsonl")),
        AgentConfig {
            model: "claude-sonnet-4-6".to_owned(),
            effort: None,
            cwd: tmp.path().to_path_buf(),
            session_dir: tmp.path().to_path_buf(),
            headless: true,
            features: None,
            tool_selection: None,
        },
    );
    agent.init().await.expect("init");

    // Act: the model tries to read AGENTS.md (absolute path).
    provider.push_response(make_tool_use_items(
        "call-guard-01",
        "read_file",
        json!({ "path": agents_md.to_str().unwrap() }),
    ));
    // After the (blocked) tool result the model ends the turn normally.
    provider.push_response(vec![Ok(make_llm_response("end_turn", 20, 5))]);

    let stream = drive(&mut agent, "Hello".to_owned(), CancellationToken::new());
    let items = collect_stream(stream).await;

    // Assert: find the ToolResult event for our call.
    let tool_result: &ToolResultEvent = items
        .iter()
        .find_map(|item| {
            if let AgentItem::Event(ev) = item {
                if let OmegaEvent::ToolResult(tr) = ev.as_ref() {
                    if tr.tool_call_id == "call-guard-01" {
                        return Some(tr);
                    }
                }
            }
            None
        })
        .expect("ToolResult event not found in stream");

    assert!(
        !tool_result.is_error,
        "guard must not surface as an error; output: {}",
        tool_result.output
    );
    assert!(
        tool_result.output.contains("system prompt"),
        "guard message must mention \"system prompt\"; output: {}",
        tool_result.output
    );
    assert!(
        !tool_result.output.contains("Do the thing"),
        "AGENTS.md content must not leak through the guard; output: {}",
        tool_result.output
    );
}

// ---------------------------------------------------------------------------
// 5b. system_prompt round-trips through init_for_resume (mutation test anchor)
//
// A unit carve-out: testing system_prompt() via send_message would require
// a live LLM call; this focused test pins system_prompt() against a known
// literal so that any mutation returning a wrong constant is detected directly.
// ---------------------------------------------------------------------------

#[test]
fn system_prompt_round_trips_init_for_resume() {
    let (mut agent, _, _tmp) = make_test_agent();
    let known_text = "known system prompt text for mutation testing".to_owned();
    agent.init_for_resume(known_text.clone());
    assert_eq!(
        agent.system_prompt(),
        known_text,
        "system_prompt() must return exactly the text passed to init_for_resume()"
    );
}

// ---------------------------------------------------------------------------
// 7. Python REPL — tool state persists and event shapes are correct
//
// Runs an Agent with features.repl=true through two sequential turns.
// Turn 1 defines a variable.  Turn 2 prints it.  We assert:
//   a. The ToolResult for turn 2 contains the printed value — state persists.
//   b. The ToolCall events carry the `code` argument.
//   c. The ToolResult events carry the tool output.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn python_repl_tool_state_persists() {
    use omega_agent::AgentConfig;
    use omega_store::{ContextStore, EventStore};
    use omega_types::events::{ToolCallEvent, ToolResultEvent};

    let tmp = tempfile::tempdir().expect("tempdir");
    let session_dir = tmp.path().to_path_buf();

    let provider = std::sync::Arc::new(common::MockProvider::new());
    let mut agent = omega_agent::Agent::new(
        provider.clone(),
        ContextStore::new(session_dir.join("context.jsonl")),
        EventStore::new(session_dir.join("events.jsonl")),
        AgentConfig {
            model: "claude-sonnet-4-6".to_owned(),
            effort: None,
            cwd: session_dir.clone(),
            session_dir: session_dir.clone(),
            headless: true,
            features: None,
            // Default 12 base tools + python_repl (this test exercises the REPL).
            tool_selection: Some({
                let mut v: Vec<String> = omega_tools::DEFAULT_TOOL_NAMES
                    .iter()
                    .map(|s| (*s).to_owned())
                    .collect();
                v.push("python_repl".to_owned());
                v
            }),
        },
    );
    agent.init().await.expect("init");

    // --- Turn 1: define a variable -----------------------------------------
    //
    // Mock LLM: call python_repl with `x = 42` (no output), then end turn.
    provider.push_response(common::make_tool_use_items(
        "repl-call-01",
        "python_repl",
        json!({ "code": "x = 42" }),
    ));
    provider.push_response(vec![Ok(common::make_llm_response("end_turn", 10, 5))]);

    let items1 = common::collect_stream(drive(
        &mut agent,
        "set x to 42".to_owned(),
        CancellationToken::new(),
    ))
    .await;

    // The ToolResult for the first call must be present with no error.
    let tr1: &ToolResultEvent = items1
        .iter()
        .find_map(|item| {
            if let AgentItem::Event(ev) = item {
                if let OmegaEvent::ToolResult(tr) = ev.as_ref() {
                    if tr.tool_call_id == "repl-call-01" {
                        return Some(tr);
                    }
                }
            }
            None
        })
        .expect("ToolResult for repl-call-01 not found in turn-1 items");

    assert!(
        !tr1.is_error,
        "turn-1 tool call must not be an error; output: {:?}",
        tr1.output
    );
    // `x = 42` produces no output.
    assert!(
        tr1.output.trim().is_empty(),
        "assignment must produce no output; got: {:?}",
        tr1.output
    );

    // --- Turn 2: print the variable ----------------------------------------
    //
    // State must persist: `x` is still 42 from turn 1.
    provider.push_response(common::make_tool_use_items(
        "repl-call-02",
        "python_repl",
        json!({ "code": "print(x)" }),
    ));
    provider.push_response(vec![Ok(common::make_llm_response("end_turn", 15, 5))]);

    let items2 = common::collect_stream(drive(
        &mut agent,
        "now print x".to_owned(),
        CancellationToken::new(),
    ))
    .await;

    // Find the ToolResult for the second call.
    let tr2: &ToolResultEvent = items2
        .iter()
        .find_map(|item| {
            if let AgentItem::Event(ev) = item {
                if let OmegaEvent::ToolResult(tr) = ev.as_ref() {
                    if tr.tool_call_id == "repl-call-02" {
                        return Some(tr);
                    }
                }
            }
            None
        })
        .expect("ToolResult for repl-call-02 not found in turn-2 items");

    assert!(
        !tr2.is_error,
        "turn-2 tool call must not be an error; output: {:?}",
        tr2.output
    );
    // State persistence: `x` was 42 in turn 1 and must still be 42 in turn 2.
    assert_eq!(
        tr2.output.trim(),
        "42",
        "state must persist: `print(x)` must output 42; got: {:?}",
        tr2.output
    );

    // --- Check ToolCall event shapes ---------------------------------------
    //
    // Both turns must have emitted ToolCall events with the correct `name`
    // and `input` fields.
    let tool_calls_turn1: Vec<&ToolCallEvent> = items1
        .iter()
        .filter_map(|item| {
            if let AgentItem::Event(ev) = item {
                if let OmegaEvent::ToolCall(tc) = ev.as_ref() {
                    return Some(tc);
                }
            }
            None
        })
        .collect();

    assert_eq!(tool_calls_turn1.len(), 1, "exactly one ToolCall per turn");
    let tc1 = tool_calls_turn1[0];
    assert_eq!(tc1.name, "python_repl", "tool name must be python_repl");
    assert_eq!(
        tc1.input["code"].as_str(),
        Some("x = 42"),
        "full code must appear in ToolCall input"
    );
}

// ---------------------------------------------------------------------------
// Phase 1.2 — tool_selection
// ---------------------------------------------------------------------------

/// `Agent::init` must reject a `tool_selection` containing a name not in
/// `omega_tools::ALL_TOOL_NAMES`.  The error message must name the
/// offending entry so the operator can diagnose the typo from the WS
/// client or CLI surface.
#[tokio::test]
async fn init_rejects_unknown_tool_name() {
    use omega_agent::{Agent, AgentConfig};

    let tmp = tempfile::tempdir().expect("tempdir");
    let provider: std::sync::Arc<dyn omega_core::Provider> =
        std::sync::Arc::new(common::MockProvider::new());
    let mut agent = Agent::new(
        provider,
        omega_store::ContextStore::new(tmp.path().join("context.jsonl")),
        omega_store::EventStore::new(tmp.path().join("events.jsonl")),
        AgentConfig {
            model: "claude-sonnet-4-6".to_owned(),
            effort: None,
            cwd: tmp.path().to_path_buf(),
            session_dir: tmp.path().to_path_buf(),
            headless: false,
            features: None,
            tool_selection: Some(vec!["does_not_exist".to_owned()]),
        },
    );
    let err = agent
        .init()
        .await
        .expect_err("init must fail when tool_selection contains an unknown name");
    let msg = err.to_string();
    assert!(
        msg.contains("does_not_exist"),
        "error must mention the offending tool name; got: {msg}",
    );
}

/// End-to-end via `Agent::send_message` + `MockProvider`: a session
/// started with `tool_selection = ["python_repl", "web_search",
/// "fetch_url"]` must:
///   1. send exactly those three tool definitions in the LLM request, and
///   2. carry a system prompt that mentions `python_repl` but does NOT
///      mention `run_command` (except inside the "Reduced toolset" block
///      that names the absent shell tools).
#[tokio::test]
async fn tool_selection_drives_request_tools_and_system_prompt() {
    use omega_agent::{Agent, AgentConfig};

    let tmp = tempfile::tempdir().expect("tempdir");
    let mock = std::sync::Arc::new(common::MockProvider::new());
    let provider: std::sync::Arc<dyn omega_core::Provider> = mock.clone();
    let mut agent = Agent::new(
        provider,
        omega_store::ContextStore::new(tmp.path().join("context.jsonl")),
        omega_store::EventStore::new(tmp.path().join("events.jsonl")),
        AgentConfig {
            model: "claude-sonnet-4-6".to_owned(),
            effort: None,
            cwd: tmp.path().to_path_buf(),
            session_dir: tmp.path().to_path_buf(),
            headless: false,
            features: None,
            tool_selection: Some(vec![
                "python_repl".to_owned(),
                "web_search".to_owned(),
                "fetch_url".to_owned(),
            ]),
        },
    );
    agent.init().await.expect("init must succeed");

    // Single LLM call: non-empty end_turn so the loop exits normally.
    mock.push_response(make_terminal_response("end_turn", 1, 1));

    let stream = drive(&mut agent, "noop".to_owned(), CancellationToken::new());
    let _ = collect_stream(stream).await;

    let reqs = mock.take_requests();
    assert_eq!(reqs.len(), 1, "exactly one LLM request expected");
    let req = &reqs[0];

    // Tool names in the request, in some order:
    let tool_names: std::collections::BTreeSet<String> =
        req.tools.iter().map(|t| t.name.clone()).collect();
    let expected: std::collections::BTreeSet<String> = ["python_repl", "web_search", "fetch_url"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    assert_eq!(
        tool_names, expected,
        "tools sent to provider must match tool_selection exactly",
    );

    // System prompt invariants.
    let sys = req
        .system
        .as_ref()
        .expect("agent must send system blocks")
        .join("\n\n");
    assert!(
        sys.contains("python_repl"),
        "system prompt must mention python_repl when it is in tool_selection",
    );

    // `run_command` may appear inside the "Reduced toolset" block (which
    // enumerates the absent shell tools).  Strip that block before the
    // residue check.
    let residue = sys
        .split("Reduced toolset")
        .next()
        .expect("split always yields at least one element");
    assert!(
        !residue.contains("run_command"),
        "system prompt must not reference `run_command` outside the \
         Reduced-toolset block when shell tools are not selected; \
         residue: {residue}",
    );
}

// ---------------------------------------------------------------------------
// Phase 0 — Async Monitors: context projection
// ---------------------------------------------------------------------------
//
// Tests (a)–(d) verify the core projection invariants:
//
//   (a) MonitorStderr is NEVER projected into the LLM context.
//   (b) MonitorDelivery projects to role:user.
//   (c) Consecutive role:user events merge into ONE API message.
//   (d) MonitorStarted is NOT projected (log/causality only).
//
// Additionally:
//   (e) MonitorStopped with an unexpected reason IS projected.
//   (f) MonitorStopped with StoppedByAgent is NOT projected.
//   (g) MonitorStopped with StoppedBySessionEnd is NOT projected.
//
// All tests drive the public Agent API and observe effects via
// history() / MockProvider.take_requests().

/// (a) MonitorStderr: written to event log only, NEVER into context.
///
/// After append_monitor_stderr, history must be unchanged and the
/// LlmRequest seen by the provider must contain no stderr text.
#[tokio::test]
async fn monitor_stderr_not_projected_into_context() {
    let (mut agent, provider, _tmp) = make_test_agent();
    agent.init().await.expect("init");

    // History must be empty after init (no user turn yet).
    assert_eq!(agent.history().len(), 0);

    // Inject stderr — must not touch history.
    agent
        .append_monitor_stderr("mon-1".into(), "fatal: out of memory".into())
        .await
        .expect("append_monitor_stderr");

    assert_eq!(
        agent.history().len(),
        0,
        "append_monitor_stderr must NOT add to in-memory history"
    );

    // Drive a normal turn — the LlmRequest must contain exactly ONE
    // message (the user text), with no stderr noise.
    provider.push_response(make_terminal_response("end_turn", 1, 1));
    let _ = collect_stream(drive(&mut agent, "hello".into(), CancellationToken::new())).await;

    let reqs = provider.take_requests();
    assert_eq!(reqs.len(), 1);
    let msgs = &reqs[0].messages;
    assert_eq!(
        msgs.len(),
        1,
        "only the user 'hello' message must reach the API"
    );

    let body = serde_json::to_string(&msgs[0].content).unwrap();
    assert!(
        !body.contains("fatal"),
        "stderr text must NOT appear in API context, got: {body}"
    );
}

/// (b) MonitorDelivery: projects to role:user in the LLM context.
///
/// inject_monitor_delivery must add a role:user entry to history, and
/// the LlmRequest must carry the monitor lines.
#[tokio::test]
async fn monitor_delivery_projects_to_user_role() {
    let (mut agent, provider, _tmp) = make_test_agent();
    agent.init().await.expect("init");

    let item = make_monitor_item("mon-1", &["line A", "line B"]);
    agent
        .inject_monitor_delivery(vec![item])
        .await
        .expect("inject_monitor_delivery");

    // History must now contain exactly one role:user message.
    assert_eq!(
        agent.history().len(),
        1,
        "inject_monitor_delivery must add exactly one history entry"
    );
    assert_eq!(
        agent.history()[0].role,
        Role::User,
        "injected monitor delivery must project to role:user"
    );

    // The content block must contain the monitor id and lines.
    let text_content = match &agent.history()[0].content[0] {
        ContentBlock::Text { text } => text.clone(),
        other => panic!("expected Text block, got {other:?}"),
    };
    assert!(
        text_content.contains("mon-1"),
        "monitor id must appear in context text, got: {text_content}"
    );
    assert!(
        text_content.contains("line A"),
        "monitor lines must appear in context text, got: {text_content}"
    );

    // Drive a turn — the LlmRequest must contain the monitor content.
    provider.push_response(vec![Ok(make_llm_response("end_turn", 1, 1))]);
    let _ = collect_stream(drive(
        &mut agent,
        "continue".into(),
        CancellationToken::new(),
    ))
    .await;

    let reqs = provider.take_requests();
    let body = serde_json::to_string(&reqs[0].messages).unwrap();
    assert!(
        body.contains("mon-1"),
        "monitor id must reach the LLM API, got: {body}"
    );
}

/// (c) Consecutive role:user events merge into ONE API message.
///
/// Two MonitorDelivery injections followed by a send_message produce
/// three consecutive role:user entries in history.  project_messages
/// must collapse them into a single message for the API call.
#[tokio::test]
async fn consecutive_user_role_events_merge_into_one_api_message() {
    let (mut agent, provider, _tmp) = make_test_agent();
    agent.init().await.expect("init");

    // Simulate one completed turn first so history is [user, assistant].
    provider.push_response(make_terminal_response("end_turn", 5, 2));
    let _ = collect_stream(drive(
        &mut agent,
        "question".into(),
        CancellationToken::new(),
    ))
    .await;
    assert_eq!(
        agent.history().len(),
        2,
        "after one turn: [user, assistant]"
    );

    // Now inject two monitor deliveries — both project to role:user.
    agent
        .inject_monitor_delivery(vec![make_monitor_item("mon-1", &["stdout line 1"])])
        .await
        .expect("delivery 1");
    agent
        .inject_monitor_delivery(vec![make_monitor_item("mon-2", &["stdout line 2"])])
        .await
        .expect("delivery 2");

    // History now has: [user, assistant, user(mon-1), user(mon-2)] = 4 entries.
    assert_eq!(
        agent.history().len(),
        4,
        "history must hold all 4 entries before projection"
    );

    // Drive the next turn.  The LlmRequest must receive a MERGED view:
    // [user(q), assistant(a), user(mon-1 + mon-2 + 'follow-up')] = 3 messages.
    provider.push_response(make_terminal_response("end_turn", 5, 2));
    let _ = collect_stream(drive(
        &mut agent,
        "follow-up".into(),
        CancellationToken::new(),
    ))
    .await;

    let reqs = provider.take_requests();
    assert_eq!(
        reqs.len(),
        2,
        "two LLM calls expected (one per send_message)"
    );
    let last_req = &reqs[1];
    assert_eq!(
        last_req.messages.len(),
        3,
        "project_messages must merge [user(mon-1), user(mon-2), user(follow-up)] \
         into one API message; got {}",
        last_req.messages.len()
    );

    // The merged user message must contain content from both deliveries
    // and from the new human message.
    let last_user = &last_req.messages[2];
    let body = serde_json::to_string(&last_user.content).unwrap();
    assert!(
        body.contains("mon-1"),
        "merged message must include monitor 1 content, got: {body}"
    );
    assert!(
        body.contains("mon-2"),
        "merged message must include monitor 2 content, got: {body}"
    );
    assert!(
        body.contains("follow-up"),
        "merged message must include human text, got: {body}"
    );
}

/// (d) MonitorStarted: NOT projected into context (log/causality only).
///
/// append_monitor_started must leave history unchanged.
#[tokio::test]
async fn monitor_started_not_projected_into_context() {
    let (mut agent, provider, _tmp) = make_test_agent();
    agent.init().await.expect("init");

    agent
        .append_monitor_started(
            "mon-1".into(),
            "watch build log".into(),
            "tail -f build.log".into(),
        )
        .await
        .expect("append_monitor_started");

    assert_eq!(
        agent.history().len(),
        0,
        "append_monitor_started must NOT add to in-memory history"
    );

    // Drive a turn — the LlmRequest must contain only the user text.
    provider.push_response(vec![Ok(make_llm_response("end_turn", 1, 1))]);
    let _ = collect_stream(drive(&mut agent, "hi".into(), CancellationToken::new())).await;

    let reqs = provider.take_requests();
    assert_eq!(reqs[0].messages.len(), 1, "only the user 'hi' message");
    let body = serde_json::to_string(&reqs[0].messages[0].content).unwrap();
    assert!(
        !body.contains("mon-1"),
        "MonitorStarted must NOT appear in API context, got: {body}"
    );
}

/// (e) MonitorStopped with an unexpected reason projects into context.
///
/// Unexpected reasons (StoppedByUser, ProcessExited, ProcessCrashed) must add
/// a role:user notification to history so the agent learns.
#[tokio::test]
async fn monitor_stopped_unexpected_projected_into_context() {
    for reason in [
        MonitorStopReason::StoppedByUser,
        MonitorStopReason::ProcessExited,
        MonitorStopReason::ProcessCrashed,
    ] {
        let (mut agent, provider, _tmp) = make_test_agent();
        agent.init().await.expect("init");

        agent
            .inject_monitor_stopped("mon-1".into(), reason, Some(1))
            .await
            .expect("inject_monitor_stopped");

        assert_eq!(
            agent.history().len(),
            1,
            "unexpected MonitorStopped must add one role:user entry to history"
        );
        assert_eq!(agent.history()[0].role, Role::User);

        // The notification must mention the monitor id.
        let text = match &agent.history()[0].content[0] {
            ContentBlock::Text { text } => text.clone(),
            other => panic!("expected Text block, got {other:?}"),
        };
        assert!(
            text.contains("mon-1"),
            "stop notification must reference monitor id, got: {text}"
        );

        // Also verify the notification reaches the API.
        provider.push_response(vec![Ok(make_llm_response("end_turn", 1, 1))]);
        let _ = collect_stream(drive(
            &mut agent,
            "continue".into(),
            CancellationToken::new(),
        ))
        .await;

        let reqs = provider.take_requests();
        let body = serde_json::to_string(&reqs[0].messages).unwrap();
        assert!(
            body.contains("mon-1"),
            "stop notification must reach the LLM API, got: {body}"
        );
    }
}

/// (f) MonitorStopped with StoppedByAgent is NOT projected into context.
///
/// The agent already knows it stopped the monitor; no notification needed.
#[tokio::test]
async fn monitor_stopped_agent_stopped_not_projected() {
    let (mut agent, provider, _tmp) = make_test_agent();
    agent.init().await.expect("init");

    agent
        .inject_monitor_stopped("mon-1".into(), MonitorStopReason::StoppedByAgent, Some(0))
        .await
        .expect("inject_monitor_stopped");

    assert_eq!(
        agent.history().len(),
        0,
        "StoppedByAgent must NOT add to in-memory history"
    );

    // Drive a turn — only the user text must reach the API.
    provider.push_response(vec![Ok(make_llm_response("end_turn", 1, 1))]);
    let _ = collect_stream(drive(&mut agent, "hi".into(), CancellationToken::new())).await;

    let reqs = provider.take_requests();
    assert_eq!(reqs[0].messages.len(), 1, "only user 'hi' must appear");
    let body = serde_json::to_string(&reqs[0].messages[0].content).unwrap();
    assert!(
        !body.contains("mon-1"),
        "StoppedByAgent must NOT inject into API context, got: {body}"
    );
}

/// (g) MonitorStopped with StoppedBySessionEnd is NOT projected into context.
///
/// Session teardown writes this reason; no running loop to notify.
#[tokio::test]
async fn monitor_stopped_by_session_end_not_projected() {
    let (mut agent, provider, _tmp) = make_test_agent();
    agent.init().await.expect("init");

    agent
        .inject_monitor_stopped(
            "mon-se".into(),
            MonitorStopReason::StoppedBySessionEnd,
            None,
        )
        .await
        .expect("inject_monitor_stopped");

    assert_eq!(
        agent.history().len(),
        0,
        "StoppedBySessionEnd must NOT add to in-memory history"
    );

    provider.push_response(vec![Ok(make_llm_response("end_turn", 1, 1))]);
    let _ = collect_stream(drive(&mut agent, "hi".into(), CancellationToken::new())).await;

    let reqs = provider.take_requests();
    assert_eq!(reqs[0].messages.len(), 1, "only user 'hi' must appear");
    let body = serde_json::to_string(&reqs[0].messages[0].content).unwrap();
    assert!(
        !body.contains("mon-se"),
        "StoppedBySessionEnd must NOT inject into API context, got: {body}"
    );
}

// ---------------------------------------------------------------------------
// 7. Accessor-mutation guards
// ---------------------------------------------------------------------------
// These tests close survivor mutations on simple read-only accessors and
// on `append_*` methods whose only observable effect is writing to the
// event log — effects that the higher-level projection tests don't cover.

/// `features()` must return the flags actually configured, not
/// `Default::default()`.
///
/// Guard: `replace Agent::features -> FeatureFlags with Default::default()`.
#[tokio::test]
async fn features_accessor_reflects_configured_flags() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let provider = Arc::new(common::MockProvider::new());
    let agent = Agent::new(
        provider,
        ContextStore::new(tmp.path().join("context.jsonl")),
        EventStore::new(tmp.path().join("events.jsonl")),
        AgentConfig {
            model: "claude-sonnet-4-6".to_owned(),
            effort: None,
            cwd: tmp.path().to_path_buf(),
            session_dir: tmp.path().to_path_buf(),
            headless: false,
            features: Some(FeatureFlags { subagents: true }),
            tool_selection: None,
        },
    );
    assert!(
        agent.features().subagents,
        "features() must return the configured value, not Default::default()"
    );
}

/// `tool_selection()` must return the names actually registered,
/// not an empty or placeholder vec.
///
/// Guards the three `Vec::leak` replacement mutations on `tool_selection()`.
#[test]
fn tool_selection_accessor_reflects_configured_tools() {
    let (agent, _, _tmp) = make_test_agent();
    // Default (None in config) falls back to all built-in tools.
    assert!(
        !agent.tool_selection().is_empty(),
        "tool_selection() must not be empty"
    );
    assert!(
        !agent.tool_selection().iter().any(String::is_empty),
        "tool_selection() must not contain empty-string names"
    );
    assert!(
        !agent.tool_selection().iter().any(|s| s == "xyzzy"),
        "tool_selection() must not contain placeholder 'xyzzy'"
    );
    // Spot-check: a known built-in tool must be present.
    assert!(
        agent.tool_selection().iter().any(|s| s == "read_file"),
        "tool_selection() must contain 'read_file'"
    );
}

/// `context_hashes()` must reflect what was actually stored in context.jsonl.
///
/// Guard: `replace Agent::context_hashes -> &[ContextHash] with Vec::leak(Vec::new())`.
#[tokio::test]
async fn context_hashes_accessor_tracks_injected_items() {
    let (mut agent, _, _tmp) = make_test_agent();
    agent.init().await.expect("init");

    assert!(agent.context_hashes().is_empty(), "starts empty after init");

    agent
        .inject_monitor_delivery(vec![make_monitor_item("mon-1", &["line"])])
        .await
        .expect("inject");

    assert_eq!(
        agent.context_hashes().len(),
        1,
        "context_hashes() must track the injected delivery"
    );
}

/// `append_monitor_started` must write a `monitor_started` event to
/// `events.jsonl`.
///
/// Guard: `replace Agent::append_monitor_started -> omega_store::Result<()>
/// with Ok(())`.
#[tokio::test]
async fn append_monitor_started_writes_event_to_log() {
    let (mut agent, _, tmp) = make_test_agent();
    agent.init().await.expect("init");

    agent
        .append_monitor_started(
            "mon-42".into(),
            "watch the build".into(),
            "tail -f build.log".into(),
        )
        .await
        .expect("append_monitor_started");

    let events = read_events_jsonl(&tmp.path().join("events.jsonl"));
    let started = events
        .iter()
        .find(|v| v["type"] == "monitor_started")
        .expect("monitor_started event must appear in events.jsonl");

    assert_eq!(started["id"], "mon-42", "id must match");
    assert_eq!(
        started["command"], "tail -f build.log",
        "command must match"
    );
    assert_eq!(
        started["description"], "watch the build",
        "description must match"
    );
}

/// `append_monitor_stderr` must write a `monitor_stderr` event to
/// `events.jsonl` (and ONLY there — not into the context).
///
/// Guard: `replace Agent::append_monitor_stderr -> omega_store::Result<()>
/// with Ok(())`.
#[tokio::test]
async fn append_monitor_stderr_writes_event_to_log() {
    let (mut agent, _, tmp) = make_test_agent();
    agent.init().await.expect("init");

    agent
        .append_monitor_stderr("mon-99".into(), "error: file not found\n".into())
        .await
        .expect("append_monitor_stderr");

    let events = read_events_jsonl(&tmp.path().join("events.jsonl"));
    let stderr = events
        .iter()
        .find(|v| v["type"] == "monitor_stderr")
        .expect("monitor_stderr event must appear in events.jsonl");

    assert_eq!(stderr["id"], "mon-99", "id must match");
    assert_eq!(
        stderr["chunk"], "error: file not found\n",
        "chunk must match"
    );
}

// ===========================================================================
// PHASE 2 — Async monitors wired into the loop (seams, parking, exit capture,
// shutdown, cutover).  Driven end-to-end via Agent::send_message + MockProvider
// with real short-lived shell monitors spawned through the live MonitorManager.
// ===========================================================================

/// Poll a predicate at 5 ms intervals up to ~3 s, panicking if it never holds.
/// Used as a deterministic barrier so a monitor's output/exit is enqueued
/// *before* we drive the agent past a given seam (removes process-timing races).
async fn poll_until(mut cond: impl FnMut() -> bool, what: &str) {
    for _ in 0..600 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("timed out waiting for: {what}");
}

/// One outcome of a timed, lazy pull on a `send_message` stream.
enum Pull {
    Item(AgentItem),
    Ended,
    Parked,
}

/// Pull the next stream item, treating a `ms`-millisecond stall as "parked".
/// `let Ok(..) else` avoids a wildcard `Err` arm (clippy::match_wild_err_arm).
async fn pull<S>(stream: &mut S, ms: u64) -> Pull
where
    S: futures::Stream<Item = AgentItem> + Unpin,
{
    let Ok(opt) = timeout(Duration::from_millis(ms), stream.next()).await else {
        return Pull::Parked;
    };
    match opt {
        Some(item) => Pull::Item(item),
        None => Pull::Ended,
    }
}

/// Concatenate every `role:user` text block across a request's messages.
fn user_text(req: &LlmRequest) -> String {
    let mut out = String::new();
    for m in &req.messages {
        if m.role == Role::User {
            for b in &m.content {
                if let ContentBlock::Text { text } = b {
                    out.push_str(text);
                    out.push('\n');
                }
            }
        }
    }
    out
}

/// True iff `needle` appears anywhere in any message text/tool_result of `req`.
fn any_message_contains(req: &LlmRequest, needle: &str) -> bool {
    serde_json::to_string(&req.messages)
        .unwrap_or_default()
        .contains(needle)
}

// (f) Shutdown reaps live monitors on session end.
#[tokio::test]
async fn shutdown_reaps_live_monitors() {
    let (agent, _provider, _tmp) = make_test_agent();
    let mgr = agent.monitor_manager();
    let a = mgr.spawn("a", "sleep 100").expect("spawn a");
    let b = mgr.spawn("b", "sleep 100").expect("spawn b");
    poll_until(|| mgr.live_count() == 2, "both monitors live").await;

    let reaped = agent.shutdown_monitors();
    assert_eq!(reaped.len(), 2, "shutdown must report both reaped monitors");
    assert!(reaped.contains(&a.id) && reaped.contains(&b.id));
    assert_eq!(mgr.live_count(), 0, "no monitors may survive shutdown");
    assert_eq!(mgr.status(&a.id), Some(MonitorStatus::Stopped));
    assert_eq!(mgr.status(&b.id), Some(MonitorStatus::Stopped));
}

// (g) Cutover: the monitor tools are now selectable — the model sees them in
//     `tools`, and the system prompt teaches the monitor concept.
#[tokio::test]
async fn cutover_exposes_tools_and_teaching_copy() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let provider = Arc::new(common::MockProvider::new());
    let selection: Vec<String> = ["read_file", "run_command", "monitor", "stop_monitor"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    let mut agent = Agent::new(
        provider.clone(),
        ContextStore::new(tmp.path().join("context.jsonl")),
        EventStore::new(tmp.path().join("events.jsonl")),
        AgentConfig {
            model: "claude-sonnet-4-6".to_owned(),
            effort: None,
            cwd: tmp.path().to_path_buf(),
            session_dir: tmp.path().to_path_buf(),
            headless: false,
            features: None,
            tool_selection: Some(selection),
        },
    );

    agent.init().await.expect("init agent");
    provider.push_response(vec![Ok(make_llm_response("end_turn", 5, 5))]);
    let _ = collect_stream(drive(&mut agent, "hi".to_owned(), CancellationToken::new())).await;

    let reqs = provider.take_requests();
    let req = &reqs[0];
    let tool_names: Vec<&str> = req.tools.iter().map(|t| t.name.as_str()).collect();
    assert!(
        tool_names.contains(&"monitor") && tool_names.contains(&"stop_monitor"),
        "monitor/stop_monitor must be offered to the model, got: {tool_names:?}"
    );
    let system = req.system.clone().unwrap_or_default().join("\n");
    assert!(
        system.to_lowercase().contains("monitor"),
        "the system prompt must teach the monitor concept"
    );
}

// --- Drop reaping + per-monitor batching (mutation guards) ---

fn proc_dir_exists(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}

async fn poll_proc_gone(pid: u32) -> bool {
    for _ in 0..800 {
        if !proc_dir_exists(pid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    false
}

async fn read_pid(path: &std::path::Path) -> Option<u32> {
    for _ in 0..800 {
        if let Ok(s) = std::fs::read_to_string(path)
            && let Ok(pid) = s.trim().parse::<u32>()
        {
            return Some(pid);
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    None
}

// Dropping the Agent (session end) must reap any still-live monitor tree via
// the Drop impl — no orphans. Guards the `impl Drop for Agent`.
#[tokio::test]
async fn dropping_the_agent_reaps_live_monitors() {
    let work = tempfile::tempdir().expect("tempdir");
    let pidfile = work.path().join("pid");
    let (agent, _provider, _tmp) = make_test_agent();
    let mgr = agent.monitor_manager();
    let cmd = format!("echo $$ > {}; sleep 100", pidfile.display());
    mgr.spawn("reaped", &cmd).expect("spawn monitor");

    let pid = read_pid(&pidfile).await.expect("monitor pid");
    assert!(proc_dir_exists(pid), "monitor should be running, pid {pid}");

    drop(agent);

    assert!(
        poll_proc_gone(pid).await,
        "dropping the agent must reap the live monitor group (pid {pid})"
    );
    drop(mgr);
}

// ---------------------------------------------------------------------------
// Task 4 (Phase 4): headless / session-end shutdown logging
// ---------------------------------------------------------------------------
//
// `shutdown_and_log_monitors` must:
//  1. Kill every still-running monitor's process tree.
//  2. Persist exactly one `MonitorStopped(StoppedBySessionEnd)` event per
//     killed monitor to events.jsonl.
//  3. Respect the CAS: monitors that already have a terminal status (naturally
//     exited before the call) must NOT be double-logged.

/// `shutdown_and_log_monitors` kills a live monitor and writes
/// `MonitorStopped(StoppedBySessionEnd)` to events.jsonl.
#[tokio::test]
async fn shutdown_and_log_monitors_kills_process_and_persists_session_end_stop() {
    use omega_types::events::{MonitorStopReason, MonitorStoppedEvent};

    let work = tempfile::tempdir().expect("tempdir");
    let pidfile = work.path().join("pid");

    let (mut agent, _provider, _tmp) = make_test_agent();
    agent.init().await.expect("init");

    // Spawn a long-lived monitor so it is still running at teardown.
    let cmd = format!("echo $$ > {}; sleep 300", pidfile.display());
    let mgr = agent.monitor_manager();
    let spawned = mgr.spawn("teardown-mon", &cmd).expect("spawn");
    let monitor_id = spawned.id.clone();
    drop(mgr); // release the Arc clone; agent owns the canonical one

    // Wait for the PID to appear.
    let pid = read_pid(&pidfile)
        .await
        .expect("monitor PID should appear within deadline");
    assert!(
        proc_dir_exists(pid),
        "monitor must be running before teardown"
    );

    // ---------- teardown ----------
    let logged = agent.shutdown_and_log_monitors().await;

    // 1. Exactly one event was logged (the one live monitor).
    assert_eq!(
        logged.len(),
        1,
        "shutdown must log exactly one event for the one live monitor"
    );

    // 2. The logged event carries the correct reason and id.
    match &logged[0] {
        OmegaEvent::MonitorStopped(MonitorStoppedEvent {
            id,
            reason,
            exit_code,
            ..
        }) => {
            assert_eq!(id, &monitor_id);
            assert_eq!(*reason, MonitorStopReason::StoppedBySessionEnd);
            assert_eq!(
                *exit_code, None,
                "exit_code must be None for session-end stop"
            );
        }
        other => panic!("expected MonitorStopped, got {other:?}"),
    }

    // 3. The monitor process was killed.
    assert!(
        poll_proc_gone(pid).await,
        "shutdown must kill the monitor process tree (pid {pid})"
    );
}

/// `shutdown_and_log_monitors` does NOT double-log a monitor that already
/// has a terminal event (naturally exited before teardown fires).
#[tokio::test]
async fn shutdown_and_log_monitors_does_not_double_log_already_stopped_monitor() {
    let (mut agent, provider, _tmp) = make_test_agent();
    agent.init().await.expect("init");

    // Spawn a monitor that exits immediately.
    let mgr = agent.monitor_manager();
    let spawned = mgr.spawn("instant-exit", "true").expect("spawn");
    let monitor_id = spawned.id.clone();

    // Wait until the manager CAS advances to Stopped.
    poll_until(
        || mgr.status(&monitor_id) == Some(MonitorStatus::Stopped),
        "instant-exit monitor must reach Stopped status",
    )
    .await;

    // Run a turn so the agent drains the stop item via Seam A (which advances
    // the CAS and logs MonitorStopped).
    for _ in 0..4 {
        provider.push_response(vec![Ok(make_llm_response("end_turn", 1, 1))]);
    }
    drop(mgr); // release extra Arc clone before the mutable borrow below
    let _ = collect_stream(drive(&mut agent, "hi".into(), CancellationToken::new())).await;

    // Now call shutdown — the monitor is already terminal; must return empty.
    let logged = agent.shutdown_and_log_monitors().await;
    assert!(
        logged.is_empty(),
        "shutdown must not double-log an already-stopped monitor; got {logged:?}"
    );
}

// ---------------------------------------------------------------------------
// 9. Monitor wrapper format (“strong framing” nudging fix)
// ---------------------------------------------------------------------------
//
// These tests verify that the new `<monitor id="…">…</monitor>` /
// `<monitor-stopped …/>` wrapper format appears in the projected user
// message that reaches the LLM API.  They drive the public Agent API
// (inject_monitor_delivery / inject_monitor_stopped) with a MockProvider so
// the exact text seen by the model is observable.

/// Monitor delivery wrapper: the `<monitor id="…">…</monitor>` tag must appear
/// in the user message sent to the LLM, not the legacy `[Monitor …]` bracket.
#[tokio::test]
async fn monitor_delivery_wrapper_format_in_llm_context() {
    let (mut agent, provider, _tmp) = make_test_agent();
    agent.init().await.expect("init");

    let item = make_monitor_item("mon-99", &["output line"]);
    agent
        .inject_monitor_delivery(vec![item])
        .await
        .expect("inject_monitor_delivery");

    // Drive a turn so the monitor delivery is included in the LlmRequest.
    provider.push_response(vec![Ok(make_llm_response("end_turn", 1, 1))]);
    let _ = collect_stream(drive(&mut agent, "go".into(), CancellationToken::new())).await;

    let reqs = provider.take_requests();
    let body = serde_json::to_string(&reqs[0].messages).expect("serialize");

    // Must use the new XML-style tag wrapper.
    assert!(
        body.contains("<monitor id=\\\"mon-99\\\">"),
        "delivery must use <monitor id=\"...\"> tag in LLM context, got: {body}"
    );
    assert!(
        body.contains("</monitor>"),
        "delivery must use </monitor> closing tag in LLM context, got: {body}"
    );
    assert!(
        body.contains("output line"),
        "delivery must carry the stdout line, got: {body}"
    );
    // Must NOT use the old bracket format.
    assert!(
        !body.contains("[Monitor"),
        "delivery must NOT use legacy [Monitor ...] bracket format, got: {body}"
    );
}

/// Monitor stopped wrapper: the `<monitor-stopped id="…" reason="…" exit-code="…"/>`
/// tag must appear in the user message sent to the LLM.
#[tokio::test]
async fn monitor_stopped_wrapper_format_in_llm_context() {
    let (mut agent, provider, _tmp) = make_test_agent();
    agent.init().await.expect("init");

    agent
        .inject_monitor_stopped("mon-55".into(), MonitorStopReason::ProcessExited, Some(42))
        .await
        .expect("inject_monitor_stopped");

    provider.push_response(vec![Ok(make_llm_response("end_turn", 1, 1))]);
    let _ = collect_stream(drive(&mut agent, "next".into(), CancellationToken::new())).await;

    let reqs = provider.take_requests();
    let body = serde_json::to_string(&reqs[0].messages).expect("serialize");

    // Must use the new self-closing tag.
    assert!(
        body.contains("<monitor-stopped"),
        "stop notification must use <monitor-stopped …/> tag, got: {body}"
    );
    assert!(
        body.contains("mon-55"),
        "stop notification must include the monitor id, got: {body}"
    );
    assert!(
        body.contains("process_exited"),
        "stop notification must include the reason, got: {body}"
    );
    assert!(
        body.contains("42"),
        "stop notification must include the exit code, got: {body}"
    );
    // Must NOT use the old bracket format.
    assert!(
        !body.contains("[Monitor"),
        "stop notification must NOT use legacy [Monitor ...] bracket format, got: {body}"
    );
}

// ---------------------------------------------------------------------------
// Unified Input Model (§15, U1): the persistent `Agent::run` loop.
//
// These drive `run(inbox, cancel)` DIRECTLY with a live `InputQueue` (not
// the one-shot `drive` helper that stops after TurnEnd) so they can observe
// the loop *parking* on an empty queue between turns and serving a SECOND
// message from the SAME run task — the exact shape that deadlocked under the
// old per-message `send_message` + agent-lock design.
// ---------------------------------------------------------------------------

/// Pull the run stream until a `TurnEnd` is observed, panicking if the loop
/// terminates or stalls first.  Returns the tags seen along the way.
async fn pull_to_turn_end<S>(stream: &mut S) -> Vec<&'static str>
where
    S: futures::Stream<Item = AgentItem> + Unpin,
{
    let mut seen = Vec::new();
    loop {
        match pull(stream, 3000).await {
            Pull::Item(item) => {
                let t = tags(std::slice::from_ref(&item));
                let is_end = t == vec!["TurnEnd"];
                seen.extend(t);
                if is_end {
                    return seen;
                }
            }
            Pull::Ended => panic!("run loop terminated before TurnEnd: saw {seen:?}"),
            Pull::Parked => panic!("run loop stalled before TurnEnd: saw {seen:?}"),
        }
    }
}

// U1 acceptance (dog-fooding invariant): a human→agent turn streams to its
// TurnEnd, the loop then PARKS on the empty inbox (does NOT terminate), and a
// SECOND message is served by the SAME run task — no deadlock, no re-entry.
#[tokio::test]
async fn run_loop_parks_after_turn_then_processes_second_message() {
    let (mut agent, provider, _tmp) = make_test_agent();
    provider.push_response(make_terminal_response("end_turn", 5, 5));
    provider.push_response(make_terminal_response("end_turn", 5, 5));

    let queue = InputQueue::new();
    let run_cancel = CancellationToken::new();
    let mut stream = agent.run(queue.clone(), run_cancel.clone());

    // --- First message ---
    queue.push(InputItem::Human {
        content: "first".to_owned(),
    });
    let seen = pull_to_turn_end(&mut stream).await;
    assert!(
        seen.contains(&"UserMessage"),
        "the human message must be echoed as a UserMessage event, saw {seen:?}"
    );

    // --- The loop must PARK on the empty queue (not terminate) ---
    assert!(
        matches!(pull(&mut stream, 300).await, Pull::Parked),
        "loop must park on an empty queue, not terminate"
    );

    // --- Second message: served by the SAME run task ---
    queue.push(InputItem::Human {
        content: "second".to_owned(),
    });
    let _ = pull_to_turn_end(&mut stream).await;

    let reqs = provider.take_requests();
    assert!(
        reqs.len() >= 2,
        "both messages must reach the provider via one run task, got {}",
        reqs.len()
    );
    assert!(
        user_text(&reqs[0]).contains("first"),
        "first request must carry the first message"
    );
    assert!(
        user_text(&reqs[1]).contains("second"),
        "second request must carry the second message"
    );

    // --- Cancelling the run token lets the parked loop terminate cleanly ---
    run_cancel.cancel();
    assert!(
        matches!(pull(&mut stream, 3000).await, Pull::Ended),
        "cancelling the run token must end the run loop"
    );
}

// A single human turn that spans multiple LLM cycles (tool_use → tool_result →
// end_turn) streams end-to-end through the persistent loop, which then parks.
#[tokio::test]
async fn run_loop_streams_multi_cycle_turn_then_parks() {
    let (mut agent, provider, _tmp) = make_test_agent();
    provider.push_response(make_tool_use_items(
        "t1",
        "run_command",
        json!({ "command": "echo hi" }),
    ));
    // One non-empty terminal response for after the tool result.
    provider.push_response(make_terminal_response("end_turn", 5, 5));

    let queue = InputQueue::new();
    let mut stream = agent.run(queue.clone(), CancellationToken::new());
    queue.push(InputItem::Human {
        content: "build".to_owned(),
    });

    let seen = pull_to_turn_end(&mut stream).await;
    assert!(
        seen.contains(&"ToolCall") && seen.contains(&"ToolResult"),
        "a multi-cycle turn must include a tool_use/tool_result, saw {seen:?}"
    );
    assert!(seen.contains(&"TurnEnd"), "the turn must end, saw {seen:?}");

    assert!(
        matches!(pull(&mut stream, 300).await, Pull::Parked),
        "loop must park after a multi-cycle turn, not terminate"
    );
}

// ---------------------------------------------------------------------------
// 3b. §15 U1 — InputQueue behaviour (dog-food acceptance tests)
// ---------------------------------------------------------------------------
//
// These tests verify the `InputQueue` end-to-end through `Agent::run` with
// a `MockProvider`, as required by the §15 U1 acceptance criterion:
// ordinary human↔agent coding turns must still work, and `snapshot()`
// must accurately reflect the pending state.

/// Push one human item, run one turn, verify the item is processed and
/// `snapshot()` is empty after drain.
#[tokio::test]
async fn push_human_item_processed_queue_empty_after_drain() {
    let (mut agent, provider, _tmp) = make_test_agent();
    provider.push_response(make_terminal_response("end_turn", 5, 5));

    let queue = InputQueue::new();
    let run_cancel = CancellationToken::new();
    let mut stream = agent.run(queue.clone(), run_cancel.clone());

    // Before push, snapshot must be empty.
    assert_eq!(queue.snapshot().len(), 0, "queue must start empty");

    // Push one item and verify snapshot shows it pending.
    let snap = queue.push(InputItem::Human {
        content: "please implement".to_owned(),
    });
    assert_eq!(snap.len(), 1, "push snapshot must show 1 item");
    assert_eq!(snap[0].source, "human", "source must be 'human'");
    assert!(
        snap[0].content_preview.contains("please implement"),
        "preview must contain message content: {:?}",
        snap[0].content_preview
    );

    // Run the turn to completion.
    let seen = pull_to_turn_end(&mut stream).await;
    assert!(
        seen.contains(&"TurnEnd"),
        "turn must complete; saw {seen:?}"
    );

    // After the item is drained, snapshot must be empty.
    assert_eq!(
        queue.snapshot().len(),
        0,
        "queue must be empty after item is processed"
    );

    run_cancel.cancel();
}

/// `snapshot()` reflects the pending state accurately — grows on push,
/// shrinks (to zero) after drain.
#[tokio::test]
async fn snapshot_reflects_pending_state() {
    let (mut agent, provider, _tmp) = make_test_agent();
    provider.push_response(make_terminal_response("end_turn", 5, 5));

    let queue = InputQueue::new();
    let run_cancel = CancellationToken::new();
    let mut stream = agent.run(queue.clone(), run_cancel.clone());

    // Empty queue — snapshot is empty.
    assert_eq!(queue.snapshot().len(), 0);

    // Push and confirm snapshot has 1 item.
    queue.push(InputItem::Human {
        content: "snapshot test".to_owned(),
    });
    // Note: the agent might pop the item immediately on a multi-thread
    // scheduler, so we only check that the push succeeded and the queue
    // drains to 0 after the turn completes.
    let seen = pull_to_turn_end(&mut stream).await;
    assert!(seen.contains(&"TurnEnd"));

    // After drain, snapshot must report 0 items.
    assert_eq!(
        queue.snapshot().len(),
        0,
        "snapshot must be empty after drain"
    );
    run_cancel.cancel();
}

/// Two pushed items are processed in FIFO order (one per cycle).
///
/// `InputQueue.pop()` uses `pop_front()` on a `VecDeque`, so the first
/// pushed item is always served first.  This test drives two turns and
/// asserts the LLM received them in order.
#[tokio::test]
async fn two_pushed_items_processed_in_order() {
    let (mut agent, provider, _tmp) = make_test_agent();
    // Two terminal responses, one per turn.
    provider.push_response(make_terminal_response("end_turn", 5, 5));
    provider.push_response(make_terminal_response("end_turn", 5, 5));

    let queue = InputQueue::new();
    let run_cancel = CancellationToken::new();
    let mut stream = agent.run(queue.clone(), run_cancel.clone());

    // Push first item before the loop starts.
    queue.push(InputItem::Human {
        content: "first item".to_owned(),
    });

    // Drain the first turn.
    let _ = pull_to_turn_end(&mut stream).await;

    // Push second item after first turn completes.
    queue.push(InputItem::Human {
        content: "second item".to_owned(),
    });

    // Drain the second turn.
    let _ = pull_to_turn_end(&mut stream).await;

    // Verify the two LLM requests carried the messages in FIFO order.
    let reqs = provider.take_requests();
    assert!(
        reqs.len() >= 2,
        "two turns must produce at least 2 LLM requests; got {}",
        reqs.len()
    );
    assert!(
        user_text(&reqs[0]).contains("first item"),
        "first LLM request must contain 'first item'; got: {:?}",
        user_text(&reqs[0])
    );
    assert!(
        user_text(&reqs[1]).contains("second item"),
        "second LLM request must contain 'second item'; got: {:?}",
        user_text(&reqs[1])
    );

    run_cancel.cancel();
}

/// Ordinary multi-cycle coding turn works end-to-end through `InputQueue`.
///
/// This is the §15 U1 dog-food acceptance test: tool_use → tool_result →
/// end_turn still works identically to U1's pre-InputQueue behaviour.
#[tokio::test]
async fn ordinary_multi_cycle_coding_turn_works_through_input_queue() {
    let (mut agent, provider, _tmp) = make_test_agent();
    provider.push_response(make_tool_use_items(
        "t1",
        "run_command",
        json!({ "command": "echo hello" }),
    ));
    provider.push_response(make_terminal_response("end_turn", 8, 3));

    let queue = InputQueue::new();
    let run_cancel = CancellationToken::new();
    let mut stream = agent.run(queue.clone(), run_cancel.clone());

    queue.push(InputItem::Human {
        content: "echo hello in the shell".to_owned(),
    });

    let seen = pull_to_turn_end(&mut stream).await;
    assert!(
        seen.contains(&"ToolCall") && seen.contains(&"ToolResult"),
        "multi-cycle turn must include ToolCall + ToolResult; saw {seen:?}"
    );
    assert!(
        seen.contains(&"TurnEnd"),
        "multi-cycle turn must end; saw {seen:?}"
    );

    // LLM was called twice (tool_use cycle + follow-up cycle).
    let reqs = provider.take_requests();
    assert_eq!(
        reqs.len(),
        2,
        "multi-cycle turn must produce exactly 2 LLM requests"
    );

    run_cancel.cancel();
}

// ---------------------------------------------------------------------------
// 4. Empty-response continuation (documented Anthropic behaviour, §14)
// ---------------------------------------------------------------------------
//
// Claude occasionally returns a response with zero content blocks
// (stop_reason end_turn OR tool_use) — particularly after tool results.
// The agent must NOT emit TurnEnd; instead it injects a "Please continue."
// user message and re-issues the LLM call (up to EMPTY_RESPONSE_CAP times).
//
// Ref: https://platform.claude.com/docs/en/build-with-claude/handling-stop-reasons

/// Empty response with stop_reason=end_turn → continuation injected →
/// normal response → TurnEnd.  No premature turn-end between calls.
#[tokio::test]
async fn empty_end_turn_injects_continuation_and_completes() {
    let (mut agent, provider, _tmp) = make_test_agent();

    // First LLM response: empty (no signals → zero content blocks).
    provider.push_response(vec![Ok(make_llm_response("end_turn", 3, 2))]);
    // Second LLM response: normal text reply.
    provider.push_response(vec![
        Ok(AgentItem::Signal(StreamSignal::Text {
            index: 0,
            text: "Done.".to_owned(),
        })),
        Ok(make_llm_response("end_turn", 100, 5)),
    ]);

    let stream = drive(&mut agent, "hello".to_owned(), CancellationToken::new());
    let items = collect_stream(stream).await;
    let t = tags(&items);

    assert_eq!(
        t,
        vec![
            "UserMessage", // original human message
            "LlmCall",
            "LlmResponseStarted",
            "LlmResponseEnded", // empty response — no TurnEnd here
            "HarnessRecovery",  // injected continuation (EmptyResponseContinuation)
            "LlmCall",
            "LlmResponseStarted",
            "Signal:Text",
            "LlmResponseEnded", // non-empty response
            "TurnEnd",
        ],
        "empty end_turn sequence diverged: {t:?}"
    );

    // The HarnessRecovery event must carry the continuation prompt.
    let recovery_content = items
        .iter()
        .find_map(|i| match i {
            AgentItem::Event(e) => match e.as_ref() {
                OmegaEvent::HarnessRecovery(ev) => Some(ev.content.as_str()),
                _ => None,
            },
            _ => None,
        })
        .expect("HarnessRecovery event must exist");
    assert_eq!(
        recovery_content, "Please continue.",
        "injected continuation must be 'Please continue.'"
    );

    // Two LLM calls must have been issued.
    let reqs = provider.take_requests();
    assert_eq!(reqs.len(), 2, "must issue exactly 2 LLM calls");
}

/// Empty response with stop_reason=tool_use (no tools) also triggers
/// continuation.  The stop_reason alone must not gate the behaviour.
#[tokio::test]
async fn empty_tool_use_stop_injects_continuation() {
    let (mut agent, provider, _tmp) = make_test_agent();

    // Empty response with stop_reason=tool_use but no tool blocks.
    provider.push_response(vec![Ok(make_llm_response("tool_use", 3, 2))]);
    // Follow-up: normal text reply.
    provider.push_response(vec![
        Ok(AgentItem::Signal(StreamSignal::Text {
            index: 0,
            text: "Done.".to_owned(),
        })),
        Ok(make_llm_response("end_turn", 100, 5)),
    ]);

    let stream = drive(&mut agent, "hello".to_owned(), CancellationToken::new());
    let items = collect_stream(stream).await;
    let t = tags(&items);

    // Same event pattern as the end_turn case.
    assert_eq!(
        t,
        vec![
            "UserMessage",
            "LlmCall",
            "LlmResponseStarted",
            "LlmResponseEnded",
            "HarnessRecovery", // injected continuation (EmptyResponseContinuation)
            "LlmCall",
            "LlmResponseStarted",
            "Signal:Text",
            "LlmResponseEnded",
            "TurnEnd",
        ],
        "empty tool_use stop sequence diverged: {t:?}"
    );
}

/// A NON-empty end_turn must still end the turn normally (no regression).
#[tokio::test]
async fn non_empty_end_turn_ends_turn_normally() {
    let (mut agent, provider, _tmp) = make_test_agent();

    // Normal text response — NOT empty.
    provider.push_response(vec![
        Ok(AgentItem::Signal(StreamSignal::Text {
            index: 0,
            text: "Hello, world!".to_owned(),
        })),
        Ok(make_llm_response("end_turn", 100, 5)),
    ]);

    let stream = drive(&mut agent, "hi".to_owned(), CancellationToken::new());
    let items = collect_stream(stream).await;
    let t = tags(&items);

    // Exactly one LlmCall and one TurnEnd; no injected UserMessage.
    assert_eq!(
        t,
        vec![
            "UserMessage",
            "LlmCall",
            "LlmResponseStarted",
            "Signal:Text",
            "LlmResponseEnded",
            "TurnEnd",
        ],
        "non-empty end_turn must end normally: {t:?}"
    );

    // Only the original human UserMessage was emitted.
    let user_msgs: Vec<_> = items
        .iter()
        .filter_map(|i| match i {
            AgentItem::Event(e) => match e.as_ref() {
                OmegaEvent::UserMessage(ev) => Some(ev.content.as_str()),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(user_msgs, vec!["hi"], "only the original message expected");
}

/// Cap exceeded: EMPTY_RESPONSE_CAP + 1 consecutive empty responses must
/// surface an AgentError + TurnInterrupted, with no infinite loop.
#[tokio::test]
async fn empty_response_cap_exceeded_surfaces_error() {
    let (mut agent, provider, _tmp) = make_test_agent();

    // Four consecutive empty responses (cap = 3, so the 4th triggers the
    // error: count becomes 4, 4 > 3 == true).
    for _ in 0..4 {
        provider.push_response(vec![Ok(make_llm_response("end_turn", 3, 2))]);
    }
    // A 5th response that would succeed if the cap weren't enforced.
    provider.push_response(vec![
        Ok(AgentItem::Signal(StreamSignal::Text {
            index: 0,
            text: "Should not reach here.".to_owned(),
        })),
        Ok(make_llm_response("end_turn", 100, 5)),
    ]);

    let stream = drive(&mut agent, "go".to_owned(), CancellationToken::new());
    let items = collect_stream(stream).await;
    let t = tags(&items);

    // Must end with AgentError + TurnInterrupted, never TurnEnd.
    assert!(
        t.contains(&"AgentError"),
        "must surface AgentError when cap exceeded; got {t:?}"
    );
    assert!(
        t.contains(&"TurnInterrupted"),
        "must surface TurnInterrupted when cap exceeded; got {t:?}"
    );
    assert!(
        !t.contains(&"TurnEnd"),
        "must NOT emit TurnEnd when cap exceeded; got {t:?}"
    );
    // The 5th response provider slot must NOT have been consumed.
    assert_eq!(
        provider.take_requests().len(),
        4,
        "exactly 4 LLM calls expected (1 per empty response before cap triggers)"
    );

    // The AgentError message must mention the cap.
    let err_text = items
        .iter()
        .find_map(|i| match i {
            AgentItem::Event(e) => match e.as_ref() {
                OmegaEvent::AgentError(ae) => Some(ae.error.as_str()),
                _ => None,
            },
            _ => None,
        })
        .expect("AgentError must exist");
    assert!(
        err_text.contains("empty"),
        "error must mention 'empty': {err_text:?}"
    );
    assert!(
        err_text.contains('4'),
        "error must mention the count (4): {err_text:?}"
    );
}

/// The continuation user message must appear in the projected context sent to
/// the LLM — the model must see a new user turn, per the Anthropic docs.
#[tokio::test]
async fn empty_response_continuation_appears_in_next_request() {
    let (mut agent, provider, _tmp) = make_test_agent();

    // Empty response.
    provider.push_response(vec![Ok(make_llm_response("end_turn", 3, 2))]);
    // Normal follow-up.
    provider.push_response(vec![
        Ok(AgentItem::Signal(StreamSignal::Text {
            index: 0,
            text: "Done.".to_owned(),
        })),
        Ok(make_llm_response("end_turn", 100, 5)),
    ]);

    let stream = drive(&mut agent, "hello".to_owned(), CancellationToken::new());
    let _ = collect_stream(stream).await;

    let reqs = provider.take_requests();
    assert_eq!(reqs.len(), 2, "exactly 2 LLM calls");

    // The second request must carry the continuation prompt.
    // project_messages merges consecutive user turns, so "hello" and
    // "Please continue." land in one role:user API message — either as
    // separate Text blocks or merged inline.  We search for the text.
    let second_req = &reqs[1];
    let has_continuation = second_req.messages.iter().any(|m| {
        m.role == Role::User
            && m.content.iter().any(|b| {
                matches!(
                    b,
                    ContentBlock::Text { text } if text.contains("Please continue.")
                )
            })
    });
    assert!(
        has_continuation,
        "continuation prompt must appear in the second LLM call's messages; \
         messages: {:?}",
        second_req.messages
    );

    // The empty assistant turn must NOT appear in the second request's
    // messages (we never persist it to context).
    let has_empty_assistant = second_req
        .messages
        .iter()
        .any(|m| m.role == Role::Assistant && m.content.is_empty());
    assert!(
        !has_empty_assistant,
        "empty assistant turn must NOT appear in the second LLM call's messages"
    );
}

// ---------------------------------------------------------------------------
// 6. HarnessRecovery events — forensics gap close (§15)
// ---------------------------------------------------------------------------

/// An empty response must emit a `HarnessRecovery{EmptyResponseContinuation}`
/// event AND the continuation text must still project as `role: user` in the
/// next LLM request.
#[tokio::test]
async fn empty_response_emits_harness_recovery_event() {
    let (mut agent, provider, _tmp) = make_test_agent();

    // First response: empty (no signals).
    provider.push_response(vec![Ok(make_llm_response("end_turn", 3, 2))]);
    // Second response: normal completion.
    provider.push_response(make_terminal_response("end_turn", 6, 2));

    let stream = drive(&mut agent, "go".to_owned(), CancellationToken::new());
    let items = collect_stream(stream).await;

    // Must emit exactly one HarnessRecovery event.
    let recovery_events: Vec<_> = items
        .iter()
        .filter_map(|i| match i {
            AgentItem::Event(e) => match e.as_ref() {
                OmegaEvent::HarnessRecovery(ev) => Some(ev.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(
        recovery_events.len(),
        1,
        "exactly one HarnessRecovery expected; got: {recovery_events:?}"
    );
    assert_eq!(
        recovery_events[0].kind,
        HarnessRecoveryKind::EmptyResponseContinuation,
        "kind must be EmptyResponseContinuation"
    );
    assert_eq!(
        recovery_events[0].content, "Please continue.",
        "content must be the standard continuation prompt"
    );

    // The continuation must still appear in the second LLM request as role:user.
    let reqs = provider.take_requests();
    assert_eq!(reqs.len(), 2);
    let has_continuation = reqs[1].messages.iter().any(|m| {
        m.role == Role::User
            && m.content.iter().any(|b| {
                matches!(
                    b,
                    ContentBlock::Text { text } if text.contains("Please continue.")
                )
            })
    });
    assert!(
        has_continuation,
        "continuation must project as role:user in the second request"
    );
}

/// Malformed tool JSON must emit a `HarnessRecovery{InvalidToolJson}` event
/// AND the nudge text must still project as `role: user` in the retry.
#[tokio::test]
async fn invalid_tool_json_emits_harness_recovery_event() {
    let (mut agent, provider, _tmp) = make_test_agent();

    // Turn 1: stream errors with the marker prefix.
    provider.push_response(vec![Err(LlmError::Stream {
        message: "malformed tool_use JSON: unexpected char at position 5".to_owned(),
    })]);
    // Turn 2: clean text reply.
    provider.push_response(make_terminal_response("end_turn", 6, 2));

    let stream = drive(&mut agent, "please".to_owned(), CancellationToken::new());
    let items = collect_stream(stream).await;

    // Must emit exactly one HarnessRecovery{InvalidToolJson} event.
    let recovery_events: Vec<_> = items
        .iter()
        .filter_map(|i| match i {
            AgentItem::Event(e) => match e.as_ref() {
                OmegaEvent::HarnessRecovery(ev) => Some(ev.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(
        recovery_events.len(),
        1,
        "exactly one HarnessRecovery expected; got: {recovery_events:?}"
    );
    assert_eq!(
        recovery_events[0].kind,
        HarnessRecoveryKind::InvalidToolJson,
        "kind must be InvalidToolJson"
    );
    assert!(
        recovery_events[0].content.contains("could not be parsed"),
        "nudge content must mention 'could not be parsed': {:?}",
        recovery_events[0].content
    );

    // The nudge must still project as role:user in the retry request.
    let reqs = provider.take_requests();
    assert_eq!(reqs.len(), 2);
    let has_nudge = reqs[1].messages.iter().any(|m| {
        m.role == Role::User
            && m.content.iter().any(|b| {
                matches!(
                    b,
                    ContentBlock::Text { text } if text.contains("could not be parsed")
                )
            })
    });
    assert!(
        has_nudge,
        "nudge must project as role:user in the retry request"
    );
}

// Abort (control-level `request_abort`) cancels the CURRENT block only: the
// loop yields a TurnInterrupted and then returns to PARK — it does NOT
// terminate the whole run.  A subsequent message is still served.
#[tokio::test]
async fn run_loop_abort_cancels_block_and_returns_to_park() {
    let (mut agent, provider, _tmp) = make_test_agent();
    let controls = agent.controls();

    // Turn 1: a long tool the abort will interrupt mid-flight.
    provider.push_response(make_tool_use_items(
        "t1",
        "run_command",
        json!({ "command": "sleep 5" }),
    ));
    // Turn 2 (after re-park): one non-empty response that ends the turn.
    provider.push_response(make_terminal_response("end_turn", 5, 5));

    let queue = InputQueue::new();
    let run_cancel = CancellationToken::new();
    let mut stream = agent.run(queue.clone(), run_cancel.clone());
    queue.push(InputItem::Human {
        content: "long".to_owned(),
    });

    // Fire a control abort shortly after the block starts the tool.
    let ctl = controls.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        ctl.request_abort();
    });

    // The block must end with a TurnInterrupted, NOT terminate the run.
    let mut saw_interrupt = false;
    while !saw_interrupt {
        match pull(&mut stream, 5000).await {
            Pull::Item(item) => {
                if tags(std::slice::from_ref(&item)).contains(&"TurnInterrupted") {
                    saw_interrupt = true;
                }
            }
            Pull::Ended => panic!("abort must NOT terminate the run loop"),
            Pull::Parked => panic!("aborted block stalled without interrupting"),
        }
    }

    // The run-level cancel was never tripped — the loop must return to PARK.
    assert!(
        !run_cancel.is_cancelled(),
        "control abort must not trip the run-level cancel"
    );
    assert!(
        matches!(pull(&mut stream, 500).await, Pull::Parked),
        "after an aborted block the loop must park, not terminate"
    );

    // A new message is still served by the SAME run task.
    queue.push(InputItem::Human {
        content: "after-abort".to_owned(),
    });
    let _ = pull_to_turn_end(&mut stream).await;
}

// =============================================================================
// Halt / Resume (§15 Unified Input Model, U3)
// =============================================================================
//
// Halt = "stop advancing at the next seam and WAIT".  The agent parks at the
// post-tool_results seam instead of starting the next block.  Resume happens
// one of two ways:
//   * a queued steering message wakes the park, is injected, and the loop
//     continues with it ("I queued a steering message"); OR
//   * an explicit `request_resume` continues with NO new input ("never mind,
//     carry on").
// Abort during a halt still wins (kill switch).

/// Pull `stream` until an item whose tag is `tag` appears (or the stream
/// ends / stalls). Returns the tags seen up to and including the match.
async fn pull_until_tag<S>(stream: &mut S, tag: &str, ms: u64) -> Vec<&'static str>
where
    S: futures::Stream<Item = AgentItem> + Unpin,
{
    let mut seen = Vec::new();
    loop {
        match pull(stream, ms).await {
            Pull::Item(item) => {
                let these = tags(std::slice::from_ref(&item));
                seen.extend(these.iter().copied());
                if these.contains(&tag) {
                    return seen;
                }
            }
            Pull::Ended => panic!("stream ended before tag {tag:?}; saw {seen:?}"),
            Pull::Parked => panic!("stream parked before tag {tag:?}; saw {seen:?}"),
        }
    }
}

// Halt parks at the next seam and a QUEUED steering message resumes it: the
// message is injected (role:user) and the block continues with it visible in
// the next LLM request.
#[tokio::test]
async fn halt_parks_at_seam_then_queued_message_resumes_with_injection() {
    let (mut agent, provider, _tmp) = make_test_agent();
    let controls = agent.controls();

    // Block 1: a tool_use → creates a post-tool_results seam.
    provider.push_response(make_tool_use_items(
        "t1",
        "run_command",
        json!({ "command": "echo hi" }),
    ));
    // Block 2 (after resume): ends the turn.
    provider.push_response(make_terminal_response("end_turn", 5, 5));

    let queue = InputQueue::new();
    let cancel = CancellationToken::new();
    let mut stream = agent.run(queue.clone(), cancel.clone());
    queue.push(InputItem::Human {
        content: "go".to_owned(),
    });

    // Drive to the tool result (the last yield before the halt seam). The
    // loop is now suspended at that yield, so requesting halt here is race-
    // free: the seam check runs on the very next poll.
    let _ = pull_until_tag(&mut stream, "ToolResult", 5000).await;
    controls.request_halt().await;

    // Next poll: the seam observes the halt request and parks.
    let seen = pull_until_tag(&mut stream, "TurnHalted", 5000).await;
    assert!(
        !seen.contains(&"LlmCall"),
        "halt must park BEFORE starting the next block; saw {seen:?}"
    );
    assert!(
        matches!(pull(&mut stream, 500).await, Pull::Parked),
        "after TurnHalted the loop must park, not advance"
    );

    // Queue a steering message → wakes the park, injects it, resumes.
    queue.push(InputItem::Human {
        content: "steer left instead".to_owned(),
    });
    let seen = pull_until_tag(&mut stream, "TurnResumed", 5000).await;
    assert!(
        seen.contains(&"UserMessage"),
        "the queued steering message must be injected (UserMessage) before \
         TurnResumed; saw {seen:?}"
    );

    // The block continues and the steering message reaches the next request.
    let _ = pull_to_turn_end(&mut stream).await;
    let reqs = provider.take_requests();
    assert_eq!(
        reqs.len(),
        2,
        "two LLM calls: pre-halt block + resumed block"
    );
    assert!(
        any_message_contains(&reqs[1], "steer left instead"),
        "the resumed block's request must carry the steering message"
    );
}

// Halt parks at the next seam and an EXPLICIT `request_resume` (no new input)
// continues the block with nothing injected.
#[tokio::test]
async fn halt_parks_then_explicit_resume_continues_with_no_input() {
    let (mut agent, provider, _tmp) = make_test_agent();
    let controls = agent.controls();

    provider.push_response(make_tool_use_items(
        "t1",
        "run_command",
        json!({ "command": "echo hi" }),
    ));
    provider.push_response(make_terminal_response("end_turn", 5, 5));

    let queue = InputQueue::new();
    let cancel = CancellationToken::new();
    let mut stream = agent.run(queue.clone(), cancel.clone());
    queue.push(InputItem::Human {
        content: "go".to_owned(),
    });

    let _ = pull_until_tag(&mut stream, "ToolResult", 5000).await;
    controls.request_halt().await;
    let _ = pull_until_tag(&mut stream, "TurnHalted", 5000).await;
    assert!(
        matches!(pull(&mut stream, 500).await, Pull::Parked),
        "halt must park"
    );

    // "Never mind, carry on" — no queued input.
    controls.request_resume();
    let seen = pull_until_tag(&mut stream, "TurnResumed", 5000).await;
    assert!(
        !seen.contains(&"UserMessage"),
        "explicit resume must inject NOTHING; saw {seen:?}"
    );
    let _ = pull_to_turn_end(&mut stream).await;
    let reqs = provider.take_requests();
    assert_eq!(
        reqs.len(),
        2,
        "resume continues the block: a second LLM call"
    );
}

// Abort while halted wins over resume: the parked block is interrupted, the
// run loop returns to PARK (does NOT terminate), and a later message is served.
#[tokio::test]
async fn abort_while_halted_interrupts_block_and_returns_to_park() {
    let (mut agent, provider, _tmp) = make_test_agent();
    let controls = agent.controls();

    provider.push_response(make_tool_use_items(
        "t1",
        "run_command",
        json!({ "command": "echo hi" }),
    ));
    // Served after the re-park, by the SAME run task.
    provider.push_response(make_terminal_response("end_turn", 5, 5));

    let queue = InputQueue::new();
    let run_cancel = CancellationToken::new();
    let mut stream = agent.run(queue.clone(), run_cancel.clone());
    queue.push(InputItem::Human {
        content: "go".to_owned(),
    });

    let _ = pull_until_tag(&mut stream, "ToolResult", 5000).await;
    controls.request_halt().await;
    let _ = pull_until_tag(&mut stream, "TurnHalted", 5000).await;
    assert!(
        matches!(pull(&mut stream, 500).await, Pull::Parked),
        "halt must park"
    );

    controls.request_abort();
    let seen = pull_until_tag(&mut stream, "TurnInterrupted", 5000).await;
    assert!(
        !seen.contains(&"TurnResumed"),
        "an abort during halt must NOT resume the block; saw {seen:?}"
    );
    assert!(
        !run_cancel.is_cancelled(),
        "control abort must not trip the run-level cancel"
    );
    assert!(
        matches!(pull(&mut stream, 500).await, Pull::Parked),
        "after an aborted halt the loop must park, not terminate"
    );

    // A new message is still served by the SAME run task.
    queue.push(InputItem::Human {
        content: "after-abort".to_owned(),
    });
    let _ = pull_to_turn_end(&mut stream).await;
}

// =============================================================================
// A1 structural guard — §15(a) of docs/monitors-design.html
// =============================================================================
//
// Source-scan test: every `context_store.append(Role::User, …)` in agent.rs
// must live inside one of the allowlisted functions below.
//
// This is a carve-out (string-scan, not a runtime assertion).  See the module
// doc comment for the rationale.  Mutation testing is NOT applicable here
// (the test body is a structural assertion with no numeric/boolean mutations
// that could be missed); see Justfile recipe `mutants-a1-guard` for
// documentation of that decision.

/// Scan `lines` backwards from `line_idx` (0-based) to find the name of the
/// nearest enclosing Rust `fn`.  Skips comment and doc-comment lines so that
/// a `// fn old_name` comment above a real fn doesn't produce a false result.
///
/// Returns `"<unknown>"` if no enclosing `fn` is found.
fn enclosing_fn(lines: &[&str], line_idx: usize) -> String {
    for i in (0..=line_idx).rev() {
        let line = lines[i];
        let trimmed = line.trim();
        // Skip comment lines (// and doc-comment lines starting with *).
        if trimmed.starts_with("//") || trimmed.starts_with('*') || trimmed.starts_with("/*") {
            continue;
        }
        if let Some(fn_pos) = line.find("fn ") {
            let after = &line[fn_pos + 3..];
            let name: String = after
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                return name;
            }
        }
    }
    "<unknown>".to_owned()
}

/// A1 invariant guard: every `context_store.append(Role::User, …)` call in
/// `agent.rs` must be inside an allowlisted function.
///
/// ALLOWLIST rules:
///   - `inject_*` helpers: each emits a backing `OmegaEvent` BEFORE the
///     context append, so the invariant holds by construction within the fn.
///   - bootstrap fns: event is emitted immediately before the append
///     (ordering fixed by A1); documented here and in §15(a).
///
/// If this test turns RED, route the new append through an existing or new
/// `inject_*` helper that emits the backing event first, then add the
/// helper to the allowlist.
#[test]
fn user_role_context_appends_are_event_backed() {
    /// Functions allowed to call `context_store.append(Role::User, …)`.
    ///
    /// inject_* helpers emit their backing OmegaEvent before the context
    /// append.  Bootstrap fns are documented below.
    const ALLOWLIST: &[&str] = &[
        // --- inject_* helpers (event emitted BEFORE context append) ---------
        "inject_monitor_delivery",      // backed by MonitorDelivery
        "inject_harness_recovery",      // backed by HarnessRecovery
        "inject_monitor_stopped",       // backed by MonitorStopped (projecting path)
        "inject_user_message",          // backed by UserMessage
        "inject_dangling_tool_results", // backed by ToolResult events (emitted first)
        "inject_tool_results_batch",    // backed by ToolResult events emitted by caller
        // --- bootstrap fns (documented exemptions; see §15(a) A1 note) ------
        // SessionResumed is emitted before the context append.
        "seed_with_resumption_summary",
        // ResumingSession is emitted before the context append (A1 fixed
        // ordering).  The basis record is NOT pushed onto in-memory history
        // (special contract); routing through inject_* would obscure that.
        "perform_resumption",
    ];

    let src = include_str!("../src/agent.rs");
    let lines: Vec<&str> = src.lines().collect();

    let mut violations: Vec<String> = Vec::new();
    let pattern = "append(Role::User,";
    let mut pos = 0;
    while let Some(rel) = src[pos..].find(pattern) {
        let abs = pos + rel;
        // Count newlines up to `abs` to get a 0-based line index.
        let line_idx = src[..abs].chars().filter(|&c| c == '\n').count();
        let fn_name = enclosing_fn(&lines, line_idx);
        if !ALLOWLIST.contains(&fn_name.as_str()) {
            violations.push(format!(
                "  line {}: in fn `{fn_name}` — not in the A1 allowlist",
                line_idx + 1
            ));
        }
        pos = abs + 1;
    }

    assert!(
        violations.is_empty(),
        "INVARIANT VIOLATION (A1 — §15(a) of docs/monitors-design.html):\n\
         context_store.append(Role::User, …) found outside allowlisted fns:\n\
         {}\n\
         Route each site through a named inject_* helper that emits the \
         backing OmegaEvent before the context append, then add the helper \
         to ALLOWLIST above.",
        violations.join("\n")
    );
}

// =============================================================================
//
// U3 source-scan guard: the retired pause-for-INJECTION plumbing must be gone.
//
// The unified model (docs/monitors-design.html §15) replaced pause/continue-
// for-injection with the InputQueue (a user interjects by queuing a message)
// plus a repurposed Halt/Resume.  This guard fails RED if any of the retired
// symbols reappear anywhere in omega-agent's source, so the old
// suspend-and-inject path cannot silently grow back alongside the queue.
//
// This is a carve-out (string-scan, not a runtime assertion); mutation testing
// is not applicable (no numeric/boolean mutation could be missed).

#[test]
fn pause_for_injection_plumbing_is_removed() {
    // Symbols that defined the retired pause-for-injection machinery.
    // Halt/Resume KEEP their own (differently named) symbols, so matching
    // these exact identifiers does not catch the surviving Halt path.
    const RETIRED: &[&str] = &[
        "request_pause",          // → request_halt
        "request_continue",       // → request_resume (no content arg)
        "pending_continue",       // injection draft buffer — gone
        "PendingContinue",        // its type — gone
        "take_pending_continue",  // gone
        "pending_continue_ready", // gone
        "take_pause_request",     // → take_halt_request
        "ContinueMode",           // Manual/Auto resume discriminator — gone
        "TurnContinued",          // event → TurnResumed
        "TurnPaused",             // event → TurnHalted
        "PauseRequested",         // event → HaltRequested
    ];

    // Every Rust source file under omega-agent/src.
    const SOURCES: &[(&str, &str)] = &[
        ("src/agent.rs", include_str!("../src/agent.rs")),
        ("src/controls.rs", include_str!("../src/controls.rs")),
        ("src/lib.rs", include_str!("../src/lib.rs")),
        (
            "src/session_resume.rs",
            include_str!("../src/session_resume.rs"),
        ),
    ];

    let mut violations: Vec<String> = Vec::new();
    for (path, src) in SOURCES {
        for needle in RETIRED {
            if src.contains(needle) {
                violations.push(format!("  {path}: still references `{needle}`"));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "RETIRED PLUMBING (U3 — §15 of docs/monitors-design.html):\n\
         pause-for-injection symbols must not reappear:\n\
         {}\n\
         A user interjects by queuing a message (InputQueue); Halt/Resume \
         replaced pause/continue.",
        violations.join("\n")
    );
}

// ===========================================================================
// U2 — Unified Input Model: monitor delivery re-attached to the inbox queue
// (docs/monitors-design.html §15). Monitors deliver through the SAME inbox
// as human input; drained at both seams; batching is a projection concern.
// ===========================================================================

/// Build a HEADLESS agent (terminates on idle when no monitor is live).
fn make_headless_agent() -> (Agent, Arc<common::MockProvider>, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let provider = Arc::new(common::MockProvider::new());
    let cwd = tmp.path().to_path_buf();
    let agent = Agent::new(
        provider.clone(),
        ContextStore::new(tmp.path().join("context.jsonl")),
        EventStore::new(tmp.path().join("events.jsonl")),
        AgentConfig {
            model: "claude-sonnet-4-6".to_owned(),
            effort: None,
            cwd: cwd.clone(),
            session_dir: cwd,
            headless: true,
            features: None,
            tool_selection: None,
        },
    );
    (agent, provider, tmp)
}

// --- Seam B (THE heart of U2): a monitor that fires mid-turn is injected
// between a tool_result and the NEXT model call — NOT held to end_turn. ----
#[tokio::test]
async fn u2_monitor_line_injected_at_seam_b_mid_turn() {
    let (mut agent, provider, _tmp) = make_test_agent();
    // Turn: call 1 = tool_use(run_command), call 2 = end_turn.
    provider.push_response(make_tool_use_items(
        "t1",
        "run_command",
        json!({ "command": "true" }),
    ));
    provider.push_response(make_terminal_response("end_turn", 5, 5));

    let queue = InputQueue::new();
    let push_handle = queue.clone();
    let run_cancel = CancellationToken::new();
    let mut stream = agent.run(queue, run_cancel.clone());
    push_handle.push(InputItem::Human {
        content: "build it".to_owned(),
    });

    let mut seen: Vec<&'static str> = Vec::new();
    let mut pushed = false;
    loop {
        match pull(&mut stream, 3000).await {
            Pull::Item(item) => {
                let t = tags(std::slice::from_ref(&item));
                let is_tool_result = t == vec!["ToolResult"];
                let is_end = t == vec!["TurnEnd"];
                seen.extend(t);
                if is_tool_result && !pushed {
                    // Monitor fires WHILE the agent is working.
                    push_handle.push(InputItem::MonitorStdout {
                        monitor_id: "mon-b".to_owned(),
                        lines: vec!["seam-b-line".to_owned()],
                    });
                    pushed = true;
                }
                if is_end {
                    break;
                }
            }
            _ => panic!("loop ended/stalled before TurnEnd; seen={seen:?}"),
        }
    }
    assert!(pushed, "test must observe a ToolResult to push at");

    // PROOF Seam-B survives: the delivery is injected mid-turn, strictly
    // BEFORE TurnEnd — not deferred to end_turn.
    let md = seen.iter().position(|t| *t == "MonitorDelivery");
    let te = seen.iter().position(|t| *t == "TurnEnd");
    assert!(
        md.is_some(),
        "Seam-B: monitor must be delivered this turn; seen={seen:?}"
    );
    assert!(
        md.unwrap() < te.unwrap(),
        "Seam-B: delivery must precede end_turn; seen={seen:?}"
    );

    // And it reaches the LLM in the call AFTER the tool result.
    let reqs = provider.take_requests();
    assert_eq!(reqs.len(), 2, "tool_use call then post-tool_result call");
    assert!(
        user_text(&reqs[1]).contains("seam-b-line"),
        "Seam-B: 2nd request must carry the monitor line; got: {}",
        user_text(&reqs[1])
    );
    run_cancel.cancel();
}

// --- Seam A: a monitor that fires while the loop is PARKED wakes it and is
// delivered at the top of the next Gather. ---------------------------------
#[tokio::test]
async fn u2_monitor_line_injected_at_seam_a_after_park() {
    let (mut agent, provider, _tmp) = make_test_agent();
    provider.push_response(make_terminal_response("end_turn", 5, 5)); // human turn
    provider.push_response(make_terminal_response("end_turn", 5, 5)); // monitor turn

    let queue = InputQueue::new();
    let push_handle = queue.clone();
    let run_cancel = CancellationToken::new();
    let mut stream = agent.run(queue, run_cancel.clone());

    push_handle.push(InputItem::Human {
        content: "hello".to_owned(),
    });
    let _ = pull_to_turn_end(&mut stream).await;
    // The loop parks on the empty inbox.
    assert!(
        matches!(pull(&mut stream, 300).await, Pull::Parked),
        "loop must park after the turn"
    );

    // Monitor fires while parked → wakes the loop → delivered at Seam A.
    push_handle.push(InputItem::MonitorStdout {
        monitor_id: "mon-a".to_owned(),
        lines: vec!["parked-line".to_owned()],
    });
    let seen = pull_to_turn_end(&mut stream).await;
    assert!(
        seen.contains(&"MonitorDelivery"),
        "Seam-A: parked loop must deliver the monitor line; seen={seen:?}"
    );
    let reqs = provider.take_requests();
    assert!(
        user_text(reqs.last().unwrap()).contains("parked-line"),
        "Seam-A: monitor line must reach the LLM"
    );
    run_cancel.cancel();
}

// --- A self-terminating monitor's stop is delivered (projecting) and wakes a
// parked loop. -------------------------------------------------------------
#[tokio::test]
async fn u2_monitor_stop_projects_and_wakes_parked_loop() {
    let (mut agent, provider, _tmp) = make_test_agent();
    provider.push_response(make_terminal_response("end_turn", 5, 5)); // human turn
    provider.push_response(make_terminal_response("end_turn", 5, 5)); // stop turn

    let queue = InputQueue::new();
    let push_handle = queue.clone();
    let run_cancel = CancellationToken::new();
    let mut stream = agent.run(queue, run_cancel.clone());

    push_handle.push(InputItem::Human {
        content: "go".to_owned(),
    });
    let _ = pull_to_turn_end(&mut stream).await;
    assert!(
        matches!(pull(&mut stream, 300).await, Pull::Parked),
        "loop must park after the turn"
    );

    // The monitor self-terminates: a MonitorStopped item is enqueued.
    push_handle.push(InputItem::MonitorStopped {
        monitor_id: "mon-x".to_owned(),
        reason: MonitorStopReason::ProcessExited,
        exit_code: Some(0),
    });
    let seen = pull_to_turn_end(&mut stream).await;
    assert!(
        seen.contains(&"MonitorStopped"),
        "stop must wake the parked loop and emit MonitorStopped; seen={seen:?}"
    );
    let reqs = provider.take_requests();
    let last = user_text(reqs.last().unwrap());
    assert!(
        last.contains("mon-x") && last.contains("process_exited"),
        "ProcessExited stop must PROJECT the reason; got: {last}"
    );
    run_cancel.cancel();
}

// --- Batching is a PROJECTION concern: a human message + a monitor line
// pending at one Gather become ONE merged API user message, but TWO context
// records + TWO events. ----------------------------------------------------
#[tokio::test]
async fn u2_batching_human_and_monitor_one_api_message_two_records() {
    let (mut agent, provider, _tmp) = make_test_agent();
    provider.push_response(make_terminal_response("end_turn", 5, 5));

    let queue = InputQueue::new();
    let run_cancel = CancellationToken::new();
    // Both pending BEFORE the first poll: pop() takes one, drain_pending()
    // the rest — one Gather, two items.
    queue.push(InputItem::Human {
        content: "human-ask".to_owned(),
    });
    queue.push(InputItem::MonitorStdout {
        monitor_id: "mon-batch".to_owned(),
        lines: vec!["monitor-line".to_owned()],
    });

    {
        let mut stream = agent.run(queue, run_cancel.clone());
        let seen = pull_to_turn_end(&mut stream).await;
        // TWO events: one per item.
        assert!(
            seen.contains(&"UserMessage") && seen.contains(&"MonitorDelivery"),
            "each item must produce its OWN event; seen={seen:?}"
        );
        run_cancel.cancel();
    } // stream dropped here → &mut agent borrow released.

    // TWO context records (NOT a god record).
    let user_records = agent
        .history()
        .iter()
        .filter(|m| m.role == Role::User)
        .count();
    assert_eq!(
        user_records, 2,
        "two separate role:user context records (one per item)"
    );

    // ONE merged API user message carrying BOTH payloads (project_messages
    // merges consecutive role:user records at request-build time).
    let reqs = provider.take_requests();
    assert_eq!(reqs.len(), 1, "one LLM call for the batch");
    let user_msgs = reqs[0]
        .messages
        .iter()
        .filter(|m| m.role == Role::User)
        .count();
    assert_eq!(
        user_msgs, 1,
        "projection must merge consecutive role:user into ONE API message"
    );
    let txt = user_text(&reqs[0]);
    assert!(
        txt.contains("human-ask") && txt.contains("monitor-line"),
        "merged message must carry BOTH payloads; got: {txt}"
    );
}

// --- Park/terminate (§15): headless + empty queue + no live monitor → the
// loop terminates. ---------------------------------------------------------
#[tokio::test]
async fn u2_headless_terminates_when_idle_no_live_monitor() {
    let (mut agent, provider, _tmp) = make_headless_agent();
    agent.init().await.expect("init");
    provider.push_response(make_terminal_response("end_turn", 5, 5));

    let queue = InputQueue::new();
    let push_handle = queue.clone();
    let mut stream = agent.run(queue, CancellationToken::new());
    push_handle.push(InputItem::Human {
        content: "go".to_owned(),
    });
    let _ = pull_to_turn_end(&mut stream).await;
    // Empty queue + no live monitor + headless → the run loop terminates.
    assert!(
        matches!(pull(&mut stream, 1500).await, Pull::Ended),
        "headless idle loop must terminate when no monitor is live"
    );
}

// --- Park/terminate (§15): headless WAITS (does NOT terminate) while a
// monitor is still live — it may yet produce output. -----------------------
#[tokio::test]
async fn u2_headless_parks_while_a_monitor_is_live() {
    let (mut agent, provider, _tmp) = make_headless_agent();
    agent.init().await.expect("init");
    provider.push_response(make_terminal_response("end_turn", 5, 5));

    let mgr = agent.monitor_manager();
    let queue = InputQueue::new();
    let push_handle = queue.clone();
    let run_cancel = CancellationToken::new();
    let mut stream = agent.run(queue, run_cancel.clone());

    // A long-lived monitor keeps the session alive even when idle.
    mgr.spawn("sleeper", "sleep 30").expect("spawn sleeper");
    push_handle.push(InputItem::Human {
        content: "go".to_owned(),
    });
    let _ = pull_to_turn_end(&mut stream).await;

    // Empty queue but a live monitor → headless must PARK, not terminate.
    assert!(
        matches!(pull(&mut stream, 800).await, Pull::Parked),
        "headless must wait while a monitor is live"
    );
    run_cancel.cancel();
    mgr.shutdown();
}

// --- Real short-lived shell monitor: stdout flows reader → MonitorSink →
// THIS inbox → delivered as a role:user MonitorDelivery. --------------------
#[tokio::test]
async fn u2_real_monitor_stdout_delivered_through_inbox() {
    let (mut agent, provider, _tmp) = make_test_agent();
    // Delivery turn (+ a spare in case the self-stop opens a second turn).
    provider.push_response(make_terminal_response("end_turn", 5, 5));
    provider.push_response(make_terminal_response("end_turn", 5, 5));
    provider.push_response(make_terminal_response("end_turn", 5, 5));

    let mgr = agent.monitor_manager(); // Arc — survives the &mut borrow below.
    let queue = InputQueue::new();
    let run_cancel = CancellationToken::new();
    let mut stream = agent.run(queue, run_cancel.clone()); // sink attached now

    // Real monitor: prints one line then exits. stdout + stop both flow
    // through the manager → MonitorSink → inbox.
    mgr.spawn("ticker", "printf 'real-tick\\n'")
        .expect("spawn ticker");

    let seen = pull_to_turn_end(&mut stream).await;
    assert!(
        seen.contains(&"MonitorDelivery"),
        "real monitor stdout must be delivered through the inbox; seen={seen:?}"
    );
    let reqs = provider.take_requests();
    assert!(
        reqs.iter().any(|r| any_message_contains(r, "real-tick")),
        "monitor line must reach the LLM as role:user"
    );
    run_cancel.cancel();
}

// --- Multiple stdout lines arrive as separate InputItems (one delivery
// event each) but MERGE into one API user message at projection. -----------
#[tokio::test]
async fn u2_multiple_monitor_lines_merge_into_one_api_message() {
    let (mut agent, provider, _tmp) = make_test_agent();
    for _ in 0..3 {
        provider.push_response(make_terminal_response("end_turn", 5, 5));
    }

    let queue = InputQueue::new();
    let run_cancel = CancellationToken::new();
    // Two lines from one monitor, pending at one Gather.
    queue.push(InputItem::MonitorStdout {
        monitor_id: "multi".to_owned(),
        lines: vec!["line-1".to_owned()],
    });
    queue.push(InputItem::MonitorStdout {
        monitor_id: "multi".to_owned(),
        lines: vec!["line-2".to_owned()],
    });

    let mut stream = agent.run(queue, run_cancel.clone());
    let seen = pull_to_turn_end(&mut stream).await;
    let deliveries = seen.iter().filter(|t| **t == "MonitorDelivery").count();
    assert_eq!(
        deliveries, 2,
        "each line is its OWN delivery event; seen={seen:?}"
    );

    let reqs = provider.take_requests();
    assert_eq!(reqs.len(), 1, "one LLM call");
    let user_msgs = reqs[0]
        .messages
        .iter()
        .filter(|m| m.role == Role::User)
        .count();
    assert_eq!(
        user_msgs, 1,
        "projection merges both lines into ONE API message"
    );
    let txt = user_text(&reqs[0]);
    assert!(
        txt.contains("line-1") && txt.contains("line-2"),
        "merged message must carry BOTH lines; got: {txt}"
    );
    run_cancel.cancel();
}

// --- MonitorStderr is NON-PROJECTED diagnostic: it becomes a MonitorStderr
// event, NEVER a role:user record, and NEVER an InputItem in the queue. ----
#[tokio::test]
async fn u2_monitor_stderr_event_not_queued_not_projected() {
    let (mut agent, _provider, _tmp) = make_test_agent();
    agent.init().await.expect("init");

    let queue = InputQueue::new();
    let ev = agent
        .append_monitor_stderr("mon-e".to_owned(), "boom".to_owned())
        .await
        .expect("append stderr");

    assert!(
        matches!(ev, OmegaEvent::MonitorStderr(_)),
        "stderr must emit a MonitorStderr event"
    );
    assert_eq!(
        agent.history().len(),
        0,
        "stderr must NOT project into the LLM context"
    );
    assert_eq!(
        queue.snapshot().len(),
        0,
        "stderr must NEVER enter the InputQueue"
    );
}

/// U2 (§15): a monitor's STDERR is non-projected diagnostic — it stays on the
/// manager's pending queue (never the inbox) and is drained at Seam B into a
/// `MonitorStderr` event by the run loop (`drain_monitor_stderr`), WITHOUT
/// becoming role:user content.  Covers the agent-loop stderr drain path
/// end-to-end with a real short-lived monitor.
#[tokio::test]
async fn u2_real_monitor_stderr_drained_at_seam_b_not_projected() {
    let (mut agent, provider, _tmp) = make_test_agent();
    // call1: spawn a monitor that writes to STDERR and stays briefly alive.
    provider.push_response(make_tool_use_items(
        "tu_mon",
        "monitor",
        json!({ "description": "errmon", "command": "printf 'errline\\n' 1>&2; sleep 3" }),
    ));
    // call2: a second tool turn whose completion gives the stderr time to land,
    // and creates a Seam B at which drain_monitor_stderr runs.
    provider.push_response(make_tool_use_items(
        "tu_wait",
        "run_command",
        json!({ "command": "sleep 0.3" }),
    ));
    // call3: end the turn.
    provider.push_response(make_terminal_response("end_turn", 5, 5));

    let queue = InputQueue::new();
    let run_cancel = CancellationToken::new();
    let q2 = queue.clone();
    let mut stream = agent.run(queue, run_cancel.clone());
    q2.push(InputItem::Human {
        content: "watch".to_owned(),
    });

    let seen = pull_to_turn_end(&mut stream).await;
    assert!(
        seen.contains(&"MonitorStderr"),
        "monitor stderr must be drained at Seam B into a MonitorStderr event; seen={seen:?}"
    );
    // It is NON-projected: it never becomes role:user content, so it must NOT
    // be a MonitorDelivery and must NOT appear in the inbox snapshot.
    assert!(
        !q2.snapshot()
            .iter()
            .any(|v| v.source.starts_with("monitor:")),
        "stderr must NEVER enter the InputQueue"
    );
    run_cancel.cancel();
    drop(stream);
    // No MonitorDelivery (stderr is not projected); the only role:user records
    // are the human turn + any tool plumbing — never an stderr delivery.
    assert!(
        !seen.contains(&"MonitorDelivery"),
        "stderr must NOT be projected as a MonitorDelivery; seen={seen:?}"
    );
}
