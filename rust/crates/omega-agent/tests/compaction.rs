//! Phase 1d.1d — agent reaction to server-side compaction.
//!
//! These tests exercise the `OmegaEvent::Compacted` arm in
//! `Agent::send_message`'s drain loop.  They mirror `src/agent.ts:1432–1453`:
//! when the provider emits `Compacted`, the agent must clear its
//! in-memory `history` and `context_hashes` *before* the assistant's
//! `LlmResponse` is processed, so that the post-turn state contains
//! only the compaction-summarised assistant message.
//!
//! The provider stays a `MockProvider`; the SSE-level parsing is
//! covered by the omega-core integration tests.  Here we inject the
//! already-parsed `OmegaEvent::Compacted` directly into the transcript.

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
use omega_protocol::events::CompactedEvent;
use omega_protocol::{OmegaEvent, StreamSignal};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

/// Convenience: build an `AgentItem` carrying a `Compacted` event with
/// the given usage object.  The `time` field is fixed so tests stay
/// deterministic when reading `events.jsonl`.
fn compacted_item(usage: Value) -> AgentItem {
    AgentItem::event(OmegaEvent::Compacted(CompactedEvent {
        time: "2024-06-01T00:00:00.000Z".to_owned(),
        usage,
    }))
}

/// Read every line of `events.jsonl` into a vector of `Value`.
fn read_events_jsonl(path: &std::path::Path) -> Vec<Value> {
    let raw = std::fs::read_to_string(path).expect("events.jsonl readable");
    raw.lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str(l).expect("valid JSON line"))
        .collect()
}

// ---------------------------------------------------------------------------
// Test 1 — clears history and context_hashes
// ---------------------------------------------------------------------------

/// After two prior happy turns and a third turn that emits `Compacted`,
/// the agent's `history` must shrink to exactly one entry — the new
/// post-compaction assistant message.  `context_hashes` must follow
/// suit (one entry).
///
/// Catches mutants that:
///   - delete `self.history.clear()` (history would be 7 entries),
///   - delete `self.context_hashes.clear()` (history shrinks but hashes
///     don't — covered by the second assertion),
///   - swap clear-order with the LlmResponse arm (would also leave
///     stale entries).
#[tokio::test]
async fn compacted_event_clears_history_and_hashes() {
    let (mut agent, provider, tmp) = make_test_agent();

    // Turn 1: plain "ok".
    provider.push_response(vec![
        Ok(AgentItem::Signal(StreamSignal::Text {
            text: "ok1".to_owned(),
        })),
        Ok(make_llm_response("end_turn", Some("ok1"), 100, 1)),
    ]);
    let _ = collect_stream(agent.send_message("first".to_owned(), CancellationToken::new())).await;

    // Turn 2: plain "ok".
    provider.push_response(vec![
        Ok(AgentItem::Signal(StreamSignal::Text {
            text: "ok2".to_owned(),
        })),
        Ok(make_llm_response("end_turn", Some("ok2"), 200, 2)),
    ]);
    let _ = collect_stream(agent.send_message("second".to_owned(), CancellationToken::new())).await;

    // Sanity: 2 turns × 2 messages each = 4 entries before compaction.
    assert_eq!(
        agent.history().len(),
        4,
        "history must hold both prior turns before compaction"
    );

    // Turn 3: provider emits Compacted then LlmResponse with the summary.
    provider.push_response(vec![
        Ok(compacted_item(json!({
            "input_tokens": 80_500,
            "output_tokens": 250
        }))),
        Ok(AgentItem::Signal(StreamSignal::Text {
            text: "summary".to_owned(),
        })),
        Ok(make_llm_response("end_turn", Some("summary"), 80_500, 250)),
    ]);
    let _ = collect_stream(agent.send_message("third".to_owned(), CancellationToken::new())).await;

    // Post-compaction: only the new assistant message survives.
    assert_eq!(
        agent.history().len(),
        1,
        "history must be cleared on Compacted, leaving only the new assistant message"
    );
    assert!(
        matches!(agent.history()[0].role, omega_core::Role::Assistant),
        "the lone surviving entry must be the assistant summary"
    );

    // Indirect check that context_hashes was cleared too: the
    // `LlmCall` of a hypothetical fourth turn would carry only the
    // post-compaction entries.  Drive one more turn to confirm —
    // strengthens this test against a mutant that clears history but
    // not hashes.
    provider.push_response(vec![Ok(make_llm_response(
        "end_turn",
        Some("after"),
        50,
        3,
    ))]);
    let _ = collect_stream(agent.send_message("fourth".to_owned(), CancellationToken::new())).await;

    // After the 4th turn: assistant_summary, user_fourth, assistant_after — 3 entries.
    assert_eq!(
        agent.history().len(),
        3,
        "fourth turn must build atop the cleared history (3 entries: assistant_summary + user_fourth + assistant_after)"
    );

    // Inspect the LlmCall recorded for turn 4 — its context_hashes
    // length is one less than history (it's emitted before the new
    // assistant is appended), so 2.
    let events_path = tmp.path().join("events.jsonl");
    let events = read_events_jsonl(&events_path);
    let last_llm_call = events
        .iter()
        .filter(|v| v["type"] == "llm_call")
        .next_back()
        .expect("a final llm_call event");
    let hashes = last_llm_call["contextHashes"]
        .as_array()
        .expect("contextHashes array");
    assert_eq!(
        hashes.len(),
        2,
        "post-compaction LlmCall must carry only the 2 post-compaction context hashes (assistant_summary + user_fourth), not the 5 pre-compaction ones"
    );
}

// ---------------------------------------------------------------------------
// Test 2 — Compacted is persisted with usage verbatim
// ---------------------------------------------------------------------------

/// The full `usage` JSON — including nested arrays such as `iterations`
/// — must round-trip through `events.jsonl` untouched.  Catches mutants
/// that drop unrecognised usage fields or replace `usage` with a
/// constant.
#[tokio::test]
async fn compacted_event_persisted_with_usage_verbatim() {
    let (mut agent, provider, tmp) = make_test_agent();

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
            text: "ok".to_owned(),
        })),
        Ok(make_llm_response("end_turn", Some("ok"), 500, 50)),
    ]);
    let _ =
        collect_stream(agent.send_message("trigger".to_owned(), CancellationToken::new())).await;

    let events = read_events_jsonl(&tmp.path().join("events.jsonl"));
    let compacted = events
        .iter()
        .find(|v| v["type"] == "compacted")
        .expect("compacted event persisted");
    assert_eq!(
        compacted["usage"], usage,
        "Compacted.usage must be persisted verbatim, including iterations[]"
    );
    assert_eq!(
        compacted["time"], "2024-06-01T00:00:00.000Z",
        "Compacted.time must round-trip to events.jsonl unchanged"
    );
}

// ---------------------------------------------------------------------------
// Test 3 — next turn sends only post-compaction messages
// ---------------------------------------------------------------------------

/// After a compacting turn, the next `LlmRequest` issued by the agent
/// must contain only the post-compaction history (assistant summary +
/// the new user message), not any of the pre-compaction turns.
#[tokio::test]
async fn next_turn_after_compaction_sends_only_post_compaction_messages() {
    let (mut agent, provider, _tmp) = make_test_agent();

    // Two prior plain turns to build up history.
    provider.push_response(vec![Ok(make_llm_response("end_turn", Some("a"), 10, 1))]);
    let _ = collect_stream(agent.send_message("first".to_owned(), CancellationToken::new())).await;

    provider.push_response(vec![Ok(make_llm_response("end_turn", Some("b"), 20, 1))]);
    let _ = collect_stream(agent.send_message("second".to_owned(), CancellationToken::new())).await;

    // Compacting turn.
    provider.push_response(vec![
        Ok(compacted_item(
            json!({"input_tokens": 80_000, "output_tokens": 100}),
        )),
        Ok(make_llm_response("end_turn", Some("summary"), 80_000, 100)),
    ]);
    let _ = collect_stream(agent.send_message("third".to_owned(), CancellationToken::new())).await;

    // Discard everything captured so far — we only care about the next call.
    let _ = provider.take_requests();

    // Fourth turn: provider records the request emitted.
    provider.push_response(vec![Ok(make_llm_response("end_turn", Some("c"), 50, 1))]);
    let _ = collect_stream(agent.send_message("fourth".to_owned(), CancellationToken::new())).await;

    let requests = provider.take_requests();
    assert_eq!(
        requests.len(),
        1,
        "exactly one provider call for the fourth turn"
    );
    let msgs = &requests[0].messages;
    assert_eq!(
        msgs.len(),
        2,
        "fourth turn must send 2 messages: assistant_summary + user_fourth — got {} ({:?})",
        msgs.len(),
        msgs.iter().map(|m| m.role).collect::<Vec<_>>()
    );
    assert!(
        matches!(msgs[0].role, omega_core::Role::Assistant),
        "first message must be the assistant summary"
    );
    assert!(
        matches!(msgs[1].role, omega_core::Role::User),
        "second message must be the new user turn"
    );
    // And the user content must be the new prompt, not "first" or "second".
    let omega_core::ContentBlock::Text { text } = &msgs[1].content[0] else {
        panic!("expected user text block");
    };
    assert_eq!(
        text, "fourth",
        "the surviving user turn must be the new one, not a pre-compaction relic"
    );
}

// ---------------------------------------------------------------------------
// Test 4 — stream order: Compacted before LlmResponse
// ---------------------------------------------------------------------------

/// At the agent layer, the consumer of `send_message` must observe
/// `Compacted` strictly before the `LlmResponse` of the same turn.
/// Catches a mutant that buffers Compacted and emits it after
/// LlmResponse (which would break UI ordering and metrics scoping).
#[tokio::test]
async fn compacted_event_appears_before_llm_response_in_stream() {
    let (mut agent, provider, _tmp) = make_test_agent();

    provider.push_response(vec![
        Ok(compacted_item(
            json!({"input_tokens": 80_500, "output_tokens": 200}),
        )),
        Ok(AgentItem::Signal(StreamSignal::Text {
            text: "summary".to_owned(),
        })),
        Ok(make_llm_response("end_turn", Some("summary"), 80_500, 200)),
    ]);
    let items =
        collect_stream(agent.send_message("trigger".to_owned(), CancellationToken::new())).await;

    let tag_seq = tags(&items);
    let compacted_idx = tag_seq
        .iter()
        .position(|t| *t == "Compacted")
        .expect("Compacted tag present");
    let llm_response_idx = tag_seq
        .iter()
        .position(|t| *t == "LlmResponse")
        .expect("LlmResponse tag present");
    assert!(
        compacted_idx < llm_response_idx,
        "Compacted must appear before LlmResponse in the agent stream — saw {tag_seq:?}"
    );

    // Full expected sequence: UserMessage, LlmCall, Compacted, Signal:Text, LlmResponse, TurnEnd.
    assert_eq!(
        tag_seq,
        vec![
            "UserMessage",
            "LlmCall",
            "Compacted",
            "Signal:Text",
            "LlmResponse",
            "TurnEnd",
        ],
        "agent stream order on a compacting turn diverged from spec"
    );
}

// ---------------------------------------------------------------------------
// Test 5 — control: non-compacting turn leaves history intact
// ---------------------------------------------------------------------------

/// A plain text turn must NOT clear history and must NOT surface
/// Compacted.  Counter-test for the four tests above; catches a mutant
/// that hard-codes `self.history.clear()` unconditionally.
#[tokio::test]
async fn non_compacting_turn_leaves_history_intact() {
    let (mut agent, provider, _tmp) = make_test_agent();

    // Build up a 4-message history.
    provider.push_response(vec![Ok(make_llm_response("end_turn", Some("a"), 10, 1))]);
    let _ = collect_stream(agent.send_message("first".to_owned(), CancellationToken::new())).await;

    provider.push_response(vec![Ok(make_llm_response("end_turn", Some("b"), 20, 1))]);
    let items =
        collect_stream(agent.send_message("second".to_owned(), CancellationToken::new())).await;

    assert_eq!(
        agent.history().len(),
        4,
        "non-compacting turns must accumulate history normally"
    );
    assert!(
        !tags(&items).contains(&"Compacted"),
        "no Compacted event on a plain turn"
    );
}
