//! Phase 0 of SCHEMA-8 — defensive byte-equal goldens for `context.jsonl`.
//!
//! Each fixture in this file:
//!
//! 1. wires a fresh in-memory [`MockProvider`] (see `tests/common/mod.rs`)
//!    to a tempdir-backed [`Agent`],
//! 2. enqueues one or more deterministic transcripts onto the provider,
//! 3. drives a single `send_message` turn to completion,
//! 4. reads the resulting `context.jsonl`,
//! 5. scrubs the wall-clock `time` field in every context record (the only
//!    non-deterministic value remaining after HASH-1 made `ContextHash`
//!    deterministic), and
//! 6. byte-compares the scrubbed output against a checked-in golden.
//!
//! # Why
//!
//! SCHEMA-8 reshapes the streaming-response side of the protocol, but the
//! resulting `context.jsonl` (the conversation state) must remain byte-equal
//! for every non-interleaved fixture. These goldens lock that property in
//! before any code change so we can refactor with confidence.
//!
//! # Scope and the parser/agent split
//!
//! These goldens drive the agent via direct `AgentItem` injection through
//! `MockProvider` — they bypass the Anthropic SSE parser and
//! `RetryingProvider`. That is intentional:
//!
//! * The **agent's** persistence semantics (which deltas land in
//!   `context.jsonl`, in what order, and which are discarded by retry
//!   /compaction) are exactly what SCHEMA-8 might accidentally change —
//!   so this layer is what these fixtures lock.
//! * The **parser's** emission shape (what `AgentItem`s come out of a
//!   given Anthropic SSE byte-stream) is locked separately by the
//!   parser-level tests added in Phase 2 of SCHEMA-8 (`omega-core`).
//! * **Integration** of both is covered by the existing e2e suites
//!   (`omega-cli/tests/cli.rs`, `omega-server/tests/ws*.rs`,
//!   `omega-e2e`) driving the real binaries against the SSE-shaped fake
//!   in `omega-test-fixtures`.
//!
//! Server-side compaction and mid-stream retry can't be reproduced via
//! the SSE fake at all (compaction's frame format is undocumented; retry
//! requires multi-attempt provider scripting + clock control), which is
//! why `omega-agent/tests/internal.rs` already uses `MockProvider` for
//! exactly those flows. The split here is consistent with that prior
//! decision.
//!
//! # Updating goldens
//!
//! Run with `OMEGA_GOLDEN_UPDATE=1` to overwrite the golden files in place:
//!
//! ```bash
//! OMEGA_GOLDEN_UPDATE=1 cargo test -p omega-agent --test goldens
//! ```
//!
//! Goldens were originally captured against the develop tip immediately
//! before SCHEMA-8 began. Any post-Phase-0 update should be challenged in
//! review — drift in a non-interleaved fixture is the regression
//! SCHEMA-8 must avoid.
//!
//! # Time scrubbing
//!
//! Every `"time":"<ISO-8601>"` value is replaced with `"time":"<scrubbed>"`
//! before comparison. The plan permits either freezing the clock or
//! scrubbing; scrubbing is simpler and keeps production code untouched.
//! Apply uniformly across all fixtures.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::doc_markdown,
    clippy::too_many_lines,
    clippy::missing_panics_doc
)]

mod common;

use std::fs;
use std::path::PathBuf;

use common::{collect_stream, make_llm_response, make_test_agent};
use omega_core::{AgentItem, LlmError};
use omega_types::events::{CompactedEvent, LlmRetryEvent, ToolCallEvent};
use omega_types::{OmegaEvent, StreamSignal};
use serde_json::json;
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn goldens_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/goldens")
}

/// Replace every `"time":"<value>"` with `"time":"<scrubbed>"`.
///
/// Hand-rolled scanner; avoids pulling `regex` as a dev-dep just for this.
/// Matches the literal field name `"time"` followed by `":"` and a quoted
/// value with no embedded escapes (ISO-8601 timestamps never contain them).
fn scrub_time(input: &str) -> String {
    const KEY: &str = "\"time\":\"";
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(idx) = rest.find(KEY) {
        out.push_str(&rest[..idx]);
        out.push_str(KEY);
        out.push_str("<scrubbed>");
        let after_key = &rest[idx + KEY.len()..];
        // Find the closing quote of the timestamp value.
        if let Some(end) = after_key.find('"') {
            // Skip the timestamp value; resume after the closing quote.
            rest = &after_key[end..];
        } else {
            // Malformed — preserve the rest verbatim and stop.
            rest = after_key;
            break;
        }
    }
    out.push_str(rest);
    out
}

/// Run a fixture: build agent, push provider scripts, send user message,
/// drain stream, scrub time fields in context.jsonl, and either compare
/// against `<fixture>/context.jsonl` or write it (if `OMEGA_GOLDEN_UPDATE`
/// is set).
async fn run_fixture(
    name: &str,
    user_message: &str,
    scripts: Vec<Vec<Result<AgentItem, LlmError>>>,
) {
    let (mut agent, provider, tmp) = make_test_agent();
    for s in scripts {
        provider.push_response(s);
    }
    let stream = agent.send_message(user_message.to_owned(), CancellationToken::new());
    let _ = collect_stream(stream).await;

    let raw = fs::read_to_string(tmp.path().join("context.jsonl")).expect("context.jsonl");
    let scrubbed = scrub_time(&raw);

    let golden_path = goldens_dir().join(name).join("context.jsonl");
    if std::env::var("OMEGA_GOLDEN_UPDATE").is_ok() {
        fs::create_dir_all(golden_path.parent().expect("parent")).expect("mkdir golden");
        fs::write(&golden_path, &scrubbed).expect("write golden");
        return;
    }
    let expected = match fs::read_to_string(&golden_path) {
        Ok(s) => s,
        Err(e) => panic!(
            "missing golden at {} ({e}) — run `OMEGA_GOLDEN_UPDATE=1 cargo test -p omega-agent --test goldens` to capture it",
            golden_path.display()
        ),
    };
    if scrubbed != expected {
        // Write the actual output next to the golden for diffing, then fail.
        let actual_path = golden_path.with_extension("jsonl.actual");
        let _ = fs::write(&actual_path, &scrubbed);
        panic!(
            "fixture {name} drifted from golden {}\n  diff with: diff -u {} {}\n  full actual written to: {}",
            golden_path.display(),
            golden_path.display(),
            actual_path.display(),
            actual_path.display(),
        );
    }
}

// ---------------------------------------------------------------------------
// Sanity check for the scrubber itself
// ---------------------------------------------------------------------------

#[test]
fn scrub_time_replaces_iso_timestamps() {
    let input = r#"{"time":"2024-01-15T12:00:00.123Z","x":1}"#;
    assert_eq!(
        scrub_time(input),
        r#"{"time":"<scrubbed>","x":1}"#,
        "scrub should replace the timestamp value"
    );
}

#[test]
fn scrub_time_passes_through_non_time_fields() {
    let input = r#"{"name":"foo","time":"2024-01-15T12:00:00.123Z"}"#;
    assert_eq!(scrub_time(input), r#"{"name":"foo","time":"<scrubbed>"}"#);
}

#[test]
fn scrub_time_handles_multiple_lines() {
    let input = "{\"time\":\"a\"}\n{\"time\":\"b\"}\n";
    assert_eq!(
        scrub_time(input),
        "{\"time\":\"<scrubbed>\"}\n{\"time\":\"<scrubbed>\"}\n"
    );
}

#[test]
fn scrub_time_passes_through_no_match() {
    let input = "{\"x\":1}\n";
    assert_eq!(scrub_time(input), input);
}

// ---------------------------------------------------------------------------
// Fixture: simple_turn — one user message, one text-only assistant reply.
// ---------------------------------------------------------------------------

fn script_simple_turn() -> Vec<Result<AgentItem, LlmError>> {
    vec![
        Ok(AgentItem::Signal(StreamSignal::Text {
            text: "Hello, world!".to_owned(),
        })),
        Ok(make_llm_response("end_turn", Some("Hello, world!"), 7, 4)),
    ]
}

#[tokio::test]
async fn fixture_simple_turn() {
    run_fixture("simple_turn", "say hi", vec![script_simple_turn()]).await;
}

// ---------------------------------------------------------------------------
// Fixture: thinking_blocks — assistant emits two thinking blocks in a row,
// then a single text block, no tools.  Non-interleaved on purpose: this
// fixture must stay byte-equal across SCHEMA-8.  The genuinely interleaved
// case (`thinking → text → thinking`) is captured separately as the
// Phase-3 `interleaved_thinking` fixture, where the persisted order is
// expected to change once block ordering is preserved.
// ---------------------------------------------------------------------------

fn script_thinking_blocks() -> Vec<Result<AgentItem, LlmError>> {
    vec![
        // First thinking block + signature.
        Ok(AgentItem::Signal(StreamSignal::Thinking {
            text: "First, let me consider…".to_owned(),
        })),
        Ok(AgentItem::Signal(StreamSignal::ThinkingBlockComplete {
            signature: "sig-thinking-1".to_owned(),
        })),
        // Second thinking block immediately after the first.
        Ok(AgentItem::Signal(StreamSignal::Thinking {
            text: "Wait — let me double-check.".to_owned(),
        })),
        Ok(AgentItem::Signal(StreamSignal::ThinkingBlockComplete {
            signature: "sig-thinking-2".to_owned(),
        })),
        // Single text block — comes after both thinking blocks.
        Ok(AgentItem::Signal(StreamSignal::Text {
            text: "Here is the answer: 42.".to_owned(),
        })),
        Ok(make_llm_response(
            "end_turn",
            Some("Here is the answer: 42."),
            12,
            8,
        )),
    ]
}

#[tokio::test]
async fn fixture_thinking_blocks() {
    run_fixture(
        "thinking_blocks",
        "what is the answer?",
        vec![script_thinking_blocks()],
    )
    .await;
}

// ---------------------------------------------------------------------------
// Fixture: parallel_tool_calls — one assistant turn issues two tool calls
// (`read_file` + `list_files`), the agent dispatches both, results are
// appended, then a final text response closes the turn.
// ---------------------------------------------------------------------------

fn script_parallel_tool_calls_call1() -> Vec<Result<AgentItem, LlmError>> {
    vec![
        Ok(AgentItem::Signal(StreamSignal::Text {
            text: "Let me look around.".to_owned(),
        })),
        // Two tool_use events emitted by the provider mid-stream.
        Ok(AgentItem::event(OmegaEvent::ToolCall(ToolCallEvent {
            time: "2024-01-01T00:00:00.000Z".to_owned(),
            id: "tu_1".to_owned(),
            name: "list_files".to_owned(),
            input: json!({ "path": "." }),
            context_hash: String::new(),
        }))),
        Ok(AgentItem::event(OmegaEvent::ToolCall(ToolCallEvent {
            time: "2024-01-01T00:00:00.000Z".to_owned(),
            id: "tu_2".to_owned(),
            name: "list_files".to_owned(),
            input: json!({ "path": "src" }),
            context_hash: String::new(),
        }))),
        Ok(make_llm_response("tool_use", None, 15, 6)),
    ]
}

fn script_parallel_tool_calls_call2() -> Vec<Result<AgentItem, LlmError>> {
    vec![
        Ok(AgentItem::Signal(StreamSignal::Text {
            text: "Done.".to_owned(),
        })),
        Ok(make_llm_response("end_turn", Some("Done."), 18, 3)),
    ]
}

#[tokio::test]
async fn fixture_parallel_tool_calls() {
    run_fixture(
        "parallel_tool_calls",
        "what files are here?",
        vec![
            script_parallel_tool_calls_call1(),
            script_parallel_tool_calls_call2(),
        ],
    )
    .await;
}

// ---------------------------------------------------------------------------
// Fixture: multi_thinking_tools — interleaves multiple thinking blocks
// with tool calls across two LLM calls, then a final text answer.
// ---------------------------------------------------------------------------

fn script_multi_thinking_tools_call1() -> Vec<Result<AgentItem, LlmError>> {
    vec![
        Ok(AgentItem::Signal(StreamSignal::Thinking {
            text: "Plan: list, then read.".to_owned(),
        })),
        Ok(AgentItem::Signal(StreamSignal::ThinkingBlockComplete {
            signature: "sig-plan".to_owned(),
        })),
        Ok(AgentItem::Signal(StreamSignal::Text {
            text: "Looking at the workspace.".to_owned(),
        })),
        Ok(AgentItem::event(OmegaEvent::ToolCall(ToolCallEvent {
            time: "2024-01-01T00:00:00.000Z".to_owned(),
            id: "tu_a".to_owned(),
            name: "list_files".to_owned(),
            input: json!({ "path": "." }),
            context_hash: String::new(),
        }))),
        Ok(make_llm_response("tool_use", None, 9, 5)),
    ]
}

fn script_multi_thinking_tools_call2() -> Vec<Result<AgentItem, LlmError>> {
    vec![
        Ok(AgentItem::Signal(StreamSignal::Thinking {
            text: "Now I will pick a file.".to_owned(),
        })),
        Ok(AgentItem::Signal(StreamSignal::ThinkingBlockComplete {
            signature: "sig-pick".to_owned(),
        })),
        Ok(AgentItem::event(OmegaEvent::ToolCall(ToolCallEvent {
            time: "2024-01-01T00:00:00.000Z".to_owned(),
            id: "tu_b".to_owned(),
            name: "read_file".to_owned(),
            input: json!({ "path": "README.md" }),
            context_hash: String::new(),
        }))),
        Ok(make_llm_response("tool_use", None, 11, 4)),
    ]
}

fn script_multi_thinking_tools_call3() -> Vec<Result<AgentItem, LlmError>> {
    vec![
        Ok(AgentItem::Signal(StreamSignal::Text {
            text: "All done.".to_owned(),
        })),
        Ok(make_llm_response("end_turn", Some("All done."), 14, 3)),
    ]
}

#[tokio::test]
async fn fixture_multi_thinking_tools() {
    run_fixture(
        "multi_thinking_tools",
        "explore and summarise",
        vec![
            script_multi_thinking_tools_call1(),
            script_multi_thinking_tools_call2(),
            script_multi_thinking_tools_call3(),
        ],
    )
    .await;
}

// ---------------------------------------------------------------------------
// Fixture: mid_stream_retry — partial deltas arrive, then the underlying
// `RetryingProvider` emits an `LlmRetry` event mid-stream (after sleeping).
// The agent must drop the partial buffers; the persisted assistant message
// then reflects only the post-retry deltas.  Pinning the persisted shape
// here protects the buffer-clearing behaviour during the SCHEMA-8 refactor.
// ---------------------------------------------------------------------------

fn script_mid_stream_retry() -> Vec<Result<AgentItem, LlmError>> {
    vec![
        // Partial pre-retry content the agent must throw away.
        Ok(AgentItem::Signal(StreamSignal::Text {
            text: "Partial answer that will be retried…".to_owned(),
        })),
        Ok(AgentItem::Signal(StreamSignal::Thinking {
            text: "Half-baked thought".to_owned(),
        })),
        // RetryingProvider has slept and is about to re-issue.
        Ok(AgentItem::event(OmegaEvent::LlmRetry(LlmRetryEvent {
            time: "2024-01-01T00:00:00.000Z".to_owned(),
            attempt: 1,
            http_status: Some(529),
            wait_ms: 1000,
            error: "overloaded_error".to_owned(),
            retry_at: None,
            error_body: None,
            thinking_fragment: Some("Half-baked thought".to_owned()),
            text_fragment: Some("Partial answer that will be retried…".to_owned()),
            reason: None,
        }))),
        // Post-retry content — this is what gets persisted.
        Ok(AgentItem::Signal(StreamSignal::Text {
            text: "Final answer.".to_owned(),
        })),
        Ok(make_llm_response("end_turn", Some("Final answer."), 11, 4)),
    ]
}

#[tokio::test]
async fn fixture_mid_stream_retry() {
    run_fixture(
        "mid_stream_retry",
        "please retry",
        vec![script_mid_stream_retry()],
    )
    .await;
}

// ---------------------------------------------------------------------------
// Fixture: compaction — the provider emits a server-side `Compacted` event
// mid-stream.  The agent clears in-memory history but `context.jsonl` is
// append-only: the user message stays, then the new post-compaction
// assistant message is appended.
//
// `usage` carries an opaque API payload kept verbatim; we use a small
// canned object so the golden stays readable.
// ---------------------------------------------------------------------------

fn script_compaction() -> Vec<Result<AgentItem, LlmError>> {
    vec![
        // Some pre-compaction text the agent will discard.
        Ok(AgentItem::Signal(StreamSignal::Text {
            text: "About to be compacted…".to_owned(),
        })),
        // Server-side compaction fires.
        Ok(AgentItem::event(OmegaEvent::Compacted(CompactedEvent {
            time: "2024-01-01T00:00:00.000Z".to_owned(),
            usage: json!({
                "input_tokens": 8000,
                "output_tokens": 250,
                "compaction": { "saved_tokens": 12345 }
            }),
        }))),
        // Post-compaction content — this is what gets persisted as
        // the new assistant message.
        Ok(AgentItem::Signal(StreamSignal::Text {
            text: "Picking up after compaction.".to_owned(),
        })),
        Ok(make_llm_response(
            "end_turn",
            Some("Picking up after compaction."),
            42,
            6,
        )),
    ]
}

#[tokio::test]
async fn fixture_compaction() {
    run_fixture("compaction", "please compact", vec![script_compaction()]).await;
}
