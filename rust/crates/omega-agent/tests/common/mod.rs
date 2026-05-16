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

use futures::StreamExt;
use futures::stream::BoxStream;
use omega_agent::{Agent, AgentConfig};
use omega_core::{AgentItem, AgentItemStream, LlmError, LlmRequest, Provider};
use omega_store::{ContextStore, EventStore};
use omega_types::events::{LlmResponseEndedEvent, ToolCallEvent};
use omega_types::{LlmResponseUsage, OmegaEvent};
use tempfile::TempDir;

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

// ---------------------------------------------------------------------------
// Stream-collection helpers
// ---------------------------------------------------------------------------

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
                _ => "OtherEvent",
            },
        })
        .collect()
}
