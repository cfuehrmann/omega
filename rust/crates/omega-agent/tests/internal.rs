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
//!    so the next turn starts from a fresh baseline.  Injecting this via the
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
use omega_store::content_hash;
use omega_types::events::{LlmResponseEndedEvent, LlmResponseUsage, UsageIteration};
use omega_types::{OmegaEvent, StreamSignal};
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
    let user_hash = content_hash(&user_msg.role, &user_msg.content);
    let assistant_hash = content_hash(&assistant_msg.role, &assistant_msg.content);
    agent.seed_history(
        vec![user_msg, assistant_msg],
        vec![user_hash, assistant_hash],
    );

    // Provider just returns a clean reply for the resumed turn.
    provider.push_response(vec![Ok(make_llm_response("end_turn", 3, 1))]);

    let stream = agent.send_message("continue".to_owned(), CancellationToken::new());
    let items = collect_stream(stream).await;

    let t = tags(&items);
    assert_eq!(
        t,
        vec![
            "ToolResult",
            "UserMessage",
            "LlmCall",
            "LlmResponseStarted",
            "LlmResponseEnded",
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
    let _ = collect_stream(agent.send_message("first".to_owned(), CancellationToken::new())).await;

    // Turn 2.
    provider.push_response(vec![
        Ok(AgentItem::Signal(StreamSignal::Text {
            index: 0,
            text: "ok2".to_owned(),
        })),
        Ok(make_llm_response("end_turn", 200, 2)),
    ]);
    let _ = collect_stream(agent.send_message("second".to_owned(), CancellationToken::new())).await;
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
    let _ = collect_stream(agent.send_message("third".to_owned(), CancellationToken::new())).await;

    // History collapsed to the lone post-compaction assistant message.
    assert_eq!(agent.history().len(), 1);
    assert!(matches!(agent.history()[0].role, Role::Assistant));

    // Turn 4 must build on the cleared history; its LlmCall must carry
    // only the 2 post-compaction context hashes.
    provider.push_response(vec![Ok(make_llm_response("end_turn", 50, 3))]);
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
    provider.push_response(vec![Ok(make_llm_response("end_turn", 6, 2))]);

    let stream = agent.send_message("please".to_owned(), CancellationToken::new());
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
            "UserMessage",
            "LlmCall",
            "LlmResponseStarted",
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
            id: id.to_owned(),
            name: "run_command".to_owned(),
            input: serde_json::json!({ "command": format!("echo turn{turn_num}") }),
            context_hash: String::new(),
            call_id: None,
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
        // Post-tool final turn.
        provider.push_response(vec![Ok(make_llm_response("end_turn", (i * 100) as i64, 3))]);
    }

    for i in 1..=8_usize {
        let stream = agent.send_message(format!("turn {i}"), CancellationToken::new());
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
        provider.push_response(vec![Ok(make_llm_response("end_turn", (i * 50) as i64, 2))]);
    }

    let mut request_bytes_seq: Vec<i64> = Vec::new();
    for i in 1..=6_usize {
        let stream = agent.send_message(format!("turn {i}"), CancellationToken::new());
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
    let _ = agent.set_model("claude-opus-4-7".to_owned()).await;
    assert_eq!(agent.active_model(), "claude-opus-4-7");
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

    provider.push_response(vec![Ok(response_with_cache)]);

    let stream = agent.send_message("hello".to_owned(), CancellationToken::new());
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
