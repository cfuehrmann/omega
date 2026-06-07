//! Shared test scaffolding for the `omega-agent` integration tests.
//!
//! Provides:
//!
//! * [`MockProvider`] — an in-memory [`Provider`] that replays
//!   pre-arranged transcripts (one per LLM call) so each test stays
//!   deterministic and offline.
//! * [`make_test_agent`] — factory that wires a fresh tempdir +
//!   [`ContextStore`] / [`EventStore`] to a freshly built [`Agent`].
//! * Small assertion helpers for inspecting the streamed event sequence.

#![allow(
    dead_code, // helpers used by a subset of tests at any time
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::doc_markdown,
    clippy::too_many_lines,
    clippy::wildcard_enum_match_arm,
    clippy::missing_panics_doc
)]

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_stream::stream;
use futures::StreamExt;
use futures::stream::BoxStream;
use omega_agent::{Agent, AgentConfig, InputItem, InputQueue};
use omega_core::{AgentItem, AgentItemStream, LlmError, LlmRequest, Provider};
use omega_store::{ContextStore, EventStore};
use omega_types::events::{LlmResponseEndedEvent, ToolCallEvent};
use omega_types::{LlmResponseUsage, MonitorDeliveryItem, OmegaEvent, StreamSignal};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// MockProvider
// ---------------------------------------------------------------------------

/// In-memory provider that replays pre-arranged transcripts.
///
/// Each `push_response` call enqueues one LLM-call's worth of items
/// (`Result<AgentItem, LlmError>`).  When the agent calls `stream`, the
/// next transcript in the queue is popped and exposed as a finite
/// `BoxStream`.  An empty queue produces an empty stream — the agent
/// will treat it as "stream ended without LlmResponse" and surface an
/// `AgentError`.
pub struct MockProvider {
    /// One `Vec` per call.  `Mutex` because [`Provider::stream`] takes
    /// `&self` and the agent crate is single-threaded for tests, so a
    /// blocking mutex is fine.
    pub responses: Mutex<VecDeque<Vec<Result<AgentItem, LlmError>>>>,
    /// Captured `LlmRequest`s (one per call) so tests can assert on the
    /// payload the agent sent.
    pub captured_requests: Mutex<Vec<LlmRequest>>,
}

impl MockProvider {
    pub fn new() -> Self {
        Self {
            responses: Mutex::new(VecDeque::new()),
            captured_requests: Mutex::new(Vec::new()),
        }
    }

    /// Enqueue one transcript that will be replayed on the next
    /// `stream()` call.
    pub fn push_response(&self, items: Vec<Result<AgentItem, LlmError>>) {
        self.responses
            .lock()
            .expect("mock responses mutex poisoned")
            .push_back(items);
    }

    /// Drain the captured `LlmRequest`s for assertion.
    pub fn take_requests(&self) -> Vec<LlmRequest> {
        std::mem::take(
            &mut *self
                .captured_requests
                .lock()
                .expect("mock requests mutex poisoned"),
        )
    }
}

impl Provider for MockProvider {
    fn stream(&self, request: LlmRequest) -> AgentItemStream {
        self.captured_requests
            .lock()
            .expect("mock requests mutex poisoned")
            .push(request);
        let items = self
            .responses
            .lock()
            .expect("mock responses mutex poisoned")
            .pop_front()
            .unwrap_or_default();
        let stream: BoxStream<'static, Result<AgentItem, LlmError>> =
            Box::pin(futures::stream::iter(items));
        stream
    }
}

// ---------------------------------------------------------------------------
// Test-agent factory
// ---------------------------------------------------------------------------

/// Wire a fresh `Agent` to an isolated tempdir.
///
/// Returns the agent, the provider handle (so the test can enqueue more
/// transcripts after construction), and the `TempDir` (kept alive until
/// the test ends so the session files remain readable).
pub fn make_test_agent() -> (Agent, Arc<MockProvider>, TempDir) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let context_path = tmp.path().join("context.jsonl");
    let events_path = tmp.path().join("events.jsonl");
    let cwd: PathBuf = tmp.path().to_path_buf();
    let provider = Arc::new(MockProvider::new());

    let agent = Agent::new(
        provider.clone(),
        ContextStore::new(context_path),
        EventStore::new(events_path),
        AgentConfig {
            model: "claude-sonnet-4-6".to_owned(),
            effort: None,
            cwd: cwd.clone(),
            session_dir: cwd,
            headless: false,
            features: None,
            tool_selection: None,
        },
    );
    (agent, provider, tmp)
}

// ---------------------------------------------------------------------------
// Convenience constructors for common AgentItem shapes
// ---------------------------------------------------------------------------

/// Build a transcript for a single LLM call that returns a `tool_use`
/// stop reason: one `ToolCall` event followed by an `LlmResponseEnded` with
/// `stop_reason = "tool_use"`.  The agent will dispatch the named tool
/// and then hit the post-tool_results seam on its next loop iteration.
pub fn make_tool_use_items(
    tool_id: &str,
    tool_name: &str,
    input: serde_json::Value,
) -> Vec<Result<AgentItem, LlmError>> {
    vec![
        Ok(AgentItem::event(OmegaEvent::ToolCall(ToolCallEvent {
            time: "2024-01-01T00:00:00.000Z".to_owned(),
            tool_call_id: tool_id.to_owned(),
            name: tool_name.to_owned(),
            input,
            context_hash: String::new(),
        }))),
        Ok(make_llm_response("tool_use", 10, 5)),
    ]
}

/// Build a default `LlmResponseEndedEvent` with sensible test values.
///
/// `context_hash` is left empty so the agent fills it in (matching real
/// providers).
pub fn make_llm_response(stop_reason: &str, input_tokens: i64, output_tokens: i64) -> AgentItem {
    AgentItem::event(OmegaEvent::LlmResponseEnded(LlmResponseEndedEvent {
        time: "2024-01-01T00:00:00.000Z".to_owned(),
        stop_reason: stop_reason.to_owned(),
        cleared_tool_uses: None,
        cleared_input_tokens: None,
        usage: LlmResponseUsage {
            input_tokens,
            output_tokens,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
            service_tier: None,
            iterations: None,
        },
        context_hash: String::new(),
        response_summary: None,
    }))
}

/// Build a non-empty terminal LLM response: a `Signal:Text` delta followed by
/// a `LlmResponseEnded`.  Use this whenever a test needs a response that
/// actually ends the turn (i.e. the model produced some text content).
///
/// Background: `make_llm_response` alone produces **zero** content blocks —
/// the agent's empty-response guard now intercepts those and injects a
/// continuation rather than emitting `TurnEnd`.  This helper produces the
/// non-empty variant that correctly terminates the agent loop.
pub fn make_terminal_response(
    stop_reason: &str,
    input_tokens: i64,
    output_tokens: i64,
) -> Vec<Result<AgentItem, LlmError>> {
    vec![
        Ok(AgentItem::Signal(StreamSignal::Text {
            index: 0,
            text: "Done.".to_owned(),
        })),
        Ok(make_llm_response(stop_reason, input_tokens, output_tokens)),
    ]
}

// ---------------------------------------------------------------------------
// Stream-collection helpers
// ---------------------------------------------------------------------------

/// Returns `true` when `item` marks the end of a single turn
/// (`TurnEnd` or `TurnInterrupted`).
///
/// `AgentError` is NOT terminal here: it may be followed by
/// `TurnInterrupted` in the same turn (e.g. when the empty-response cap is
/// exceeded), so stopping at `AgentError` would drop the subsequent event.
///
/// Used by [`drive`] to stop the forwarding stream after one turn so the
/// test doesn't hang waiting for the persistent run loop to park.
fn is_turn_terminal(item: &AgentItem) -> bool {
    use omega_types::OmegaEvent;
    matches!(
        item,
        AgentItem::Event(ev)
            if matches!(ev.as_ref(), OmegaEvent::TurnEnd(_) | OmegaEvent::TurnInterrupted(_))
    )
}

/// Drive a single human turn through the persistent [`Agent::run`] loop
/// (§15 Unified Input Model) and return the run stream.
///
/// Pushes one `Human` item into a fresh [`InputQueue`] and forwards the
/// run stream until the first `TurnEnd` / `TurnInterrupted` / `AgentError`
/// (inclusive).  The persistent run loop parks after the turn; this wrapper
/// stops the forwarding stream at the terminal event so tests don't hang,
/// reproducing the one-turn-then-finish shape of the old inbox-close approach.
pub fn drive(
    agent: &mut Agent,
    content: String,
    cancel: CancellationToken,
) -> futures::stream::BoxStream<'_, AgentItem> {
    let queue = InputQueue::new();
    queue.push(InputItem::Human { content });
    let mut inner = agent.run(queue, cancel);
    Box::pin(stream! {
        while let Some(item) = inner.next().await {
            let terminal = is_turn_terminal(&item);
            yield item;
            if terminal {
                break;
            }
        }
    })
}

/// Drive an agent stream to completion and return every item produced.
pub async fn collect_stream<S>(stream: S) -> Vec<AgentItem>
where
    S: futures::Stream<Item = AgentItem> + Unpin,
{
    let mut out = Vec::new();
    let mut s = stream;
    while let Some(item) = s.next().await {
        out.push(item);
    }
    out
}

/// Project [`AgentItem`]s to a list of human-readable tags for snapshot-
/// style assertions.
pub fn tags(items: &[AgentItem]) -> Vec<&'static str> {
    items
        .iter()
        .map(|it| match it {
            AgentItem::Signal(s) => match s {
                omega_types::StreamSignal::Text { .. } => "Signal:Text",
                omega_types::StreamSignal::Thinking { .. } => "Signal:Thinking",
                omega_types::StreamSignal::ThinkingBlockComplete { .. } => {
                    "Signal:ThinkingBlockComplete"
                }
                omega_types::StreamSignal::TextBlockComplete { .. } => "Signal:TextBlockComplete",
                omega_types::StreamSignal::ToolUseBlockComplete { .. } => {
                    "Signal:ToolUseBlockComplete"
                }
                omega_types::StreamSignal::ToolUseBlockStart { .. } => "Signal:ToolUseBlockStart",
                omega_types::StreamSignal::ToolInput { .. } => "Signal:ToolInput",
            },
            AgentItem::Event(boxed) => match boxed.as_ref() {
                OmegaEvent::UserMessage(_) => "UserMessage",
                OmegaEvent::LlmCall(_) => "LlmCall",
                OmegaEvent::ToolCall(_) => "ToolCall",
                OmegaEvent::ToolResult(_) => "ToolResult",
                OmegaEvent::TurnEnd(_) => "TurnEnd",
                OmegaEvent::LlmError(_) => "LlmError",
                OmegaEvent::AgentError(_) => "AgentError",
                OmegaEvent::TurnInterrupted(_) => "TurnInterrupted",
                OmegaEvent::LlmRetry(_) => "LlmRetry",
                OmegaEvent::ResumingSession(_) => "ResumingSession",
                OmegaEvent::SessionResumed(_) => "SessionResumed",
                OmegaEvent::ModelChanged(_) => "ModelChanged",
                OmegaEvent::EffortChanged(_) => "EffortChanged",
                OmegaEvent::PauseRequested(_) => "PauseRequested",
                OmegaEvent::TurnPaused(_) => "TurnPaused",
                OmegaEvent::TurnContinued(_) => "TurnContinued",
                OmegaEvent::LlmResponseStarted(_) => "LlmResponseStarted",
                OmegaEvent::LlmResponseEnded(_) => "LlmResponseEnded",
                OmegaEvent::LlmResponseDiscarded(_) => "LlmResponseDiscarded",
                OmegaEvent::TextBlock(_) => "TextBlock",
                OmegaEvent::ThinkingBlock(_) => "ThinkingBlock",
                OmegaEvent::ToolUseBlock(_) => "ToolUseBlock",
                OmegaEvent::HarnessRecovery(_) => "HarnessRecovery",
                OmegaEvent::MonitorStarted(_) => "MonitorStarted",
                OmegaEvent::MonitorDelivery(_) => "MonitorDelivery",
                OmegaEvent::MonitorStderr(_) => "MonitorStderr",
                OmegaEvent::MonitorStopped(_) => "MonitorStopped",
                _ => "OtherEvent",
            },
        })
        .collect()
}

/// Build a `MonitorDeliveryItem` with a given monitor id and lines.
pub fn make_monitor_item(id: &str, lines: &[&str]) -> MonitorDeliveryItem {
    MonitorDeliveryItem {
        monitor_id: id.to_owned(),
        lines: lines.iter().map(|l| (*l).to_owned()).collect(),
    }
}
