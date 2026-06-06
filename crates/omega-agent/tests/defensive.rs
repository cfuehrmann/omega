#![allow(
    clippy::unnecessary_wraps, // test fixture fns return Result for uniform `?` use in tests
    clippy::double_ended_iterator_last, // .last() is fine in tests
)]

//! SCHEMA-8 Phase 6 defensive tests (T1–T3, item 53).
//!
//! Each test exercises a specific slot-assembly / persistence contract
//! that lives inside `Agent::send_message` and is awkward to verify
//! through the full HTTP/SSE fixture suite.
//!
//! # T1 — Signatures preserved
//! A stream carrying two `ThinkingBlock` signals at *distinct* indices,
//! each with a unique signature, must produce exactly two `Thinking`
//! `ContentBlock`s in `context.jsonl` whose `signature` fields are
//! byte-equal to the corresponding inputs — with no cross-contamination.
//!
//! # T2 — Block order in context.jsonl
//! When the provider emits blocks in the order
//! `thinking(0) → text(1) → thinking(2) → text(3) → tool_use(4)`
//! the assistant record in `context.jsonl` must list its `content`
//! array in that exact order: `[Thinking, Text, Thinking, Text, ToolUse]`.
//!
//! # T3 — Events ↔ context cross-check
//! The `ThinkingBlock`, `TextBlock`, and `ToolUseBlock` entries in
//! `events.jsonl` (in emission order, `partial:false` only) must carry
//! the same content — thinking text, signature, text, tool id/name/input
//! — as the corresponding `ContentBlock`s in `context.jsonl`, and they
//! must appear in the same relative order.

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

use common::{collect_stream, drive, make_llm_response, make_test_agent};
use omega_core::{AgentItem, LlmError};
use omega_types::StreamSignal;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// Shared fixture builders
// ---------------------------------------------------------------------------

/// Signal helpers — return `AgentItem::Signal` wrappers.
fn sig_text(index: usize, text: &str) -> Result<AgentItem, LlmError> {
    Ok(AgentItem::Signal(StreamSignal::Text {
        index,
        text: text.to_owned(),
    }))
}

fn sig_text_complete(index: usize, text: &str) -> Result<AgentItem, LlmError> {
    Ok(AgentItem::Signal(StreamSignal::TextBlockComplete {
        index,
        text: text.to_owned(),
    }))
}

fn sig_thinking(index: usize, text: &str) -> Result<AgentItem, LlmError> {
    Ok(AgentItem::Signal(StreamSignal::Thinking {
        index,
        text: text.to_owned(),
    }))
}

fn sig_thinking_complete(index: usize, signature: &str) -> Result<AgentItem, LlmError> {
    Ok(AgentItem::Signal(StreamSignal::ThinkingBlockComplete {
        index,
        signature: signature.to_owned(),
    }))
}

fn sig_tool_use_complete(
    index: usize,
    id: &str,
    name: &str,
    input: Value,
) -> Result<AgentItem, LlmError> {
    Ok(AgentItem::Signal(StreamSignal::ToolUseBlockComplete {
        index,
        tool_use_id: id.to_owned(),
        name: name.to_owned(),
        input,
    }))
}

// ---------------------------------------------------------------------------
// T1 fixture: two thinking blocks with distinct signatures + one text block
// ---------------------------------------------------------------------------

/// Stream: thinking(0, "sig-alpha") → thinking(1, "sig-beta") → text(2).
///
/// Both thinking blocks use a unique, unmistakable token in their
/// signature so cross-contamination is immediately visible.
fn script_t1() -> Vec<Result<AgentItem, LlmError>> {
    vec![
        sig_thinking(0, "first chain of thought"),
        sig_thinking_complete(0, "SIG-ALPHA-0000000000000000"),
        sig_thinking(1, "second chain of thought"),
        sig_thinking_complete(1, "SIG-BETA-1111111111111111"),
        sig_text(2, "final answer"),
        sig_text_complete(2, "final answer"),
        Ok(make_llm_response("end_turn", 20, 10)),
    ]
}

// ---------------------------------------------------------------------------
// T2 / T3 fixture: interleaved thinking + text + tool_use
// ---------------------------------------------------------------------------

/// Stream: thinking(0) → text(1) → thinking(2) → text(3) → tool_use(4).
///
/// `stop_reason = "end_turn"` so the agent writes the `ToolUse`
/// `ContentBlock` to `context.jsonl` without dispatching the tool.
fn script_t2() -> Vec<Result<AgentItem, LlmError>> {
    vec![
        sig_thinking(0, "think step 1"),
        sig_thinking_complete(0, "SIG-T2-BLOCK0"),
        sig_text(1, "text step 1"),
        sig_text_complete(1, "text step 1"),
        sig_thinking(2, "think step 2"),
        sig_thinking_complete(2, "SIG-T2-BLOCK2"),
        sig_text(3, "text step 3"),
        sig_text_complete(3, "text step 3"),
        sig_tool_use_complete(
            4,
            "toolu_t2_check",
            "run_command",
            json!({ "command": "echo t2" }),
        ),
        Ok(make_llm_response("end_turn", 30, 15)),
    ]
}

// ---------------------------------------------------------------------------
// Parsing helpers for context.jsonl and events.jsonl
// ---------------------------------------------------------------------------

/// Parse every line of a JSONL file into `Vec<Value>`.
fn read_jsonl(path: &std::path::Path) -> Vec<Value> {
    let raw = std::fs::read_to_string(path).expect("read jsonl file");
    raw.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("parse jsonl line"))
        .collect()
}

/// Return the *last* assistant record's `content` array from
/// `context.jsonl`.  Panics if none is found.
fn last_assistant_content(context_path: &std::path::Path) -> Vec<Value> {
    let records = read_jsonl(context_path);
    records
        .into_iter()
        .filter(|r| r["role"] == "assistant")
        .last()
        .expect("no assistant record in context.jsonl")["content"]
        .as_array()
        .expect("content is not an array")
        .clone()
}

/// Collect non-partial block events from `events.jsonl` in emission
/// order.  Only returns records whose `type` is one of the three
/// block-grammar event types *and* `partial == false`.
fn block_events_from_events_jsonl(events_path: &std::path::Path) -> Vec<Value> {
    const BLOCK_TYPES: &[&str] = &["thinking_block", "text_block", "tool_use_block"];
    read_jsonl(events_path)
        .into_iter()
        .filter(|r| {
            let t = r["type"].as_str().unwrap_or("");
            BLOCK_TYPES.contains(&t) && r["partial"] == false
        })
        .collect()
}

// ---------------------------------------------------------------------------
// T1 — signatures preserved
// ---------------------------------------------------------------------------

#[tokio::test]
async fn t1_signatures_preserved() {
    let (mut agent, provider, tmp) = make_test_agent();
    provider.push_response(script_t1());

    let stream = drive(&mut agent, "hi".to_owned(), CancellationToken::new());
    let _items = collect_stream(stream).await;

    let context_path = tmp.path().join("context.jsonl");
    let content = last_assistant_content(&context_path);

    // --- Structural: exactly 3 blocks (Thinking, Thinking, Text) ----------
    assert_eq!(
        content.len(),
        3,
        "T1: expected 3 assistant content blocks, got {}: {content:#?}",
        content.len()
    );

    // --- Block types in expected positions --------------------------------
    assert_eq!(
        content[0]["type"], "thinking",
        "T1: content[0] must be 'thinking'"
    );
    assert_eq!(
        content[1]["type"], "thinking",
        "T1: content[1] must be 'thinking'"
    );
    assert_eq!(content[2]["type"], "text", "T1: content[2] must be 'text'");

    // --- Signatures are present and distinct ------------------------------
    let sig0 = content[0]["signature"]
        .as_str()
        .expect("T1: content[0].signature missing");
    let sig1 = content[1]["signature"]
        .as_str()
        .expect("T1: content[1].signature missing");

    assert_ne!(
        sig0, sig1,
        "T1: signatures must be distinct — found the same value for both"
    );

    // --- Signatures are byte-equal to the inputs --------------------------
    assert_eq!(
        sig0, "SIG-ALPHA-0000000000000000",
        "T1: content[0] signature mismatch"
    );
    assert_eq!(
        sig1, "SIG-BETA-1111111111111111",
        "T1: content[1] signature mismatch"
    );

    // --- Thinking text is also preserved (no cross-contamination) ---------
    assert_eq!(
        content[0]["thinking"], "first chain of thought",
        "T1: content[0] thinking text mismatch"
    );
    assert_eq!(
        content[1]["thinking"], "second chain of thought",
        "T1: content[1] thinking text mismatch"
    );
}

// ---------------------------------------------------------------------------
// T2 — block order in context.jsonl
// ---------------------------------------------------------------------------

#[tokio::test]
async fn t2_block_order_in_context_jsonl() {
    let (mut agent, provider, tmp) = make_test_agent();
    provider.push_response(script_t2());

    let stream = drive(&mut agent, "go".to_owned(), CancellationToken::new());
    let _items = collect_stream(stream).await;

    let context_path = tmp.path().join("context.jsonl");
    let content = last_assistant_content(&context_path);

    // --- Structural: exactly 5 blocks -------------------------------------
    assert_eq!(
        content.len(),
        5,
        "T2: expected 5 content blocks, got {}: {content:#?}",
        content.len()
    );

    // --- Exact emission order: [Thinking, Text, Thinking, Text, ToolUse] --
    let expected_types = ["thinking", "text", "thinking", "text", "tool_use"];
    for (i, expected_type) in expected_types.iter().enumerate() {
        assert_eq!(
            content[i]["type"], *expected_type,
            "T2: content[{i}] type mismatch — expected '{expected_type}', got '{}'",
            content[i]["type"]
        );
    }

    // --- Spot-check content values ----------------------------------------
    assert_eq!(
        content[0]["thinking"], "think step 1",
        "T2: block[0] thinking text"
    );
    assert_eq!(
        content[0]["signature"], "SIG-T2-BLOCK0",
        "T2: block[0] signature"
    );
    assert_eq!(content[1]["text"], "text step 1", "T2: block[1] text");
    assert_eq!(
        content[2]["thinking"], "think step 2",
        "T2: block[2] thinking text"
    );
    assert_eq!(
        content[2]["signature"], "SIG-T2-BLOCK2",
        "T2: block[2] signature"
    );
    assert_eq!(content[3]["text"], "text step 3", "T2: block[3] text");
    assert_eq!(content[4]["id"], "toolu_t2_check", "T2: block[4] tool id");
    assert_eq!(content[4]["name"], "run_command", "T2: block[4] tool name");
    assert_eq!(
        content[4]["input"],
        json!({ "command": "echo t2" }),
        "T2: block[4] tool input"
    );
}

// ---------------------------------------------------------------------------
// T3 — events.jsonl ↔ context.jsonl cross-check
// ---------------------------------------------------------------------------

#[tokio::test]
async fn t3_events_and_context_carry_same_blocks() {
    let (mut agent, provider, tmp) = make_test_agent();
    // Reuse the T2 fixture — same block sequence, same assertions.
    provider.push_response(script_t2());

    let stream = drive(
        &mut agent,
        "cross-check".to_owned(),
        CancellationToken::new(),
    );
    let _items = collect_stream(stream).await;

    let context_path = tmp.path().join("context.jsonl");
    let events_path = tmp.path().join("events.jsonl");

    let ctx_content = last_assistant_content(&context_path);
    let evt_blocks = block_events_from_events_jsonl(&events_path);

    // --- Same number of (non-partial) block events as ContentBlocks -------
    assert_eq!(
        evt_blocks.len(),
        ctx_content.len(),
        "T3: events.jsonl has {} block events but context.jsonl has {} ContentBlocks",
        evt_blocks.len(),
        ctx_content.len()
    );

    // --- Cross-check each pair in order -----------------------------------
    for i in 0..ctx_content.len() {
        let ctx = &ctx_content[i];
        let evt = &evt_blocks[i];
        let ctx_type = ctx["type"].as_str().unwrap_or("?");
        let evt_type = evt["type"].as_str().unwrap_or("?");

        match ctx_type {
            "thinking" => {
                assert_eq!(
                    evt_type, "thinking_block",
                    "T3[{i}]: context type is 'thinking' but event type is '{evt_type}'"
                );
                assert_eq!(
                    evt["thinking"], ctx["thinking"],
                    "T3[{i}]: thinking text mismatch"
                );
                // signature may be null in the event if partial; here partial=false
                assert_eq!(
                    evt["signature"], ctx["signature"],
                    "T3[{i}]: thinking signature mismatch"
                );
            }
            "text" => {
                assert_eq!(
                    evt_type, "text_block",
                    "T3[{i}]: context type is 'text' but event type is '{evt_type}'"
                );
                assert_eq!(evt["text"], ctx["text"], "T3[{i}]: text mismatch");
            }
            "tool_use" => {
                assert_eq!(
                    evt_type, "tool_use_block",
                    "T3[{i}]: context type is 'tool_use' but event type is '{evt_type}'"
                );
                // The event uses `tool_use_id` (LLM-issued, faithfully
                // recorded); the conversation block uses `id` (Anthropic
                // wire-format field name).  Same value, different field
                // names by layer.
                assert_eq!(evt["toolUseId"], ctx["id"], "T3[{i}]: tool_use id mismatch");
                assert_eq!(evt["name"], ctx["name"], "T3[{i}]: tool_use name mismatch");
                assert_eq!(
                    evt["input"], ctx["input"],
                    "T3[{i}]: tool_use input mismatch"
                );
            }
            other => panic!("T3[{i}]: unexpected context block type '{other}'"),
        }
    }
}
