//! Deterministic mock LLM provider for the Playwright real-server suite.
//!
//! Routing logic mirrors the historical `e2e/fixtures/real-server.ts`
//! one-to-one:
//!
//! * The first user-message text in the conversation history selects the
//!   scenario (`MULTI_TOOL_TEST`, `LONG_STREAM_TEST`, …).
//! * The number of assistant messages already in the history selects which
//!   step within that scenario to return.
//! * A system prompt that starts with `"Summarise the coding session"`
//!   means this is a resumption call — return a well-formed
//!   `<summary>…</summary><description>…</description>`.
//!
//! Every call also gets recorded into [`CallHistory`] so the control HTTP
//! API can expose it to tests.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::stream::{self, BoxStream, StreamExt};
use omega_core::{AgentItem, AgentItemStream, ContentBlock, LlmError, LlmRequest, Provider, Role};
use omega_protocol::events::{LlmResponseEvent, ToolCallEvent};
use omega_protocol::{LlmResponseUsage, OmegaEvent, StreamSignal};
use serde::Serialize;
use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// Captured call history (shared with the control server)
// ---------------------------------------------------------------------------

/// One captured LLM call, projected into the shape the JS tests expect.
#[derive(Debug, Clone, Serialize)]
pub struct CapturedCall {
    /// `"task"` for normal turns, `"resumption"` for the synthesised summary
    /// call when a session is resumed.
    #[serde(rename = "systemKind")]
    pub system_kind: &'static str,
    /// Wall-clock millis since the unix epoch — same field name as the JS
    /// fixture exposed (`Date.now()`).
    pub at: u128,
    /// One entry per message in the request history; `content` is a string
    /// (JSON-stringified content blocks for assistant / tool messages, or
    /// the plain text for user-text messages).
    pub messages: Vec<CapturedMessage>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CapturedMessage {
    pub role: String,
    pub content: String,
}

/// Shared, lock-protected history of every captured call.  Cloneable
/// `Arc` handle.
#[derive(Clone, Default)]
pub struct CallHistory {
    inner: Arc<Mutex<Vec<CapturedCall>>>,
}

impl CallHistory {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&self, call: CapturedCall) {
        if let Ok(mut g) = self.inner.lock() {
            g.push(call);
        }
    }

    pub fn snapshot(&self) -> Vec<CapturedCall> {
        self.inner.lock().map(|g| g.clone()).unwrap_or_default()
    }

    pub fn reset(&self) {
        if let Ok(mut g) = self.inner.lock() {
            g.clear();
        }
    }
}

// ---------------------------------------------------------------------------
// MockProvider
// ---------------------------------------------------------------------------

pub struct MockProvider {
    history: CallHistory,
}

impl MockProvider {
    #[must_use]
    pub fn new(history: CallHistory) -> Self {
        Self { history }
    }
}

impl Provider for MockProvider {
    fn stream(&self, request: LlmRequest) -> AgentItemStream {
        // 1. Capture the call for the control API.
        self.history.push(project_call(&request));

        // 2. Pick the script.
        let is_resumption = request
            .system
            .as_deref()
            .is_some_and(|s| s.starts_with("Summarise the coding session"));

        if is_resumption {
            return text_stream(
                "<summary>Resumed session summary.</summary>\n<description>Resumed work.</description>",
            );
        }

        let trigger = first_user_text(&request);
        let nth = nth_call_in_turn(&request);

        // --- abort_sleep_test (legacy) -------------------------------------
        if trigger.contains("abort_sleep_test") {
            return tool_use_stream(
                "toolu_sleep_abort",
                "run_command",
                json!({ "command": "sleep 10" }),
            );
        }

        // --- MULTI_TOOL_TEST -----------------------------------------------
        if trigger.contains("MULTI_TOOL_TEST") {
            if nth < 3 {
                return tool_use_stream(
                    &format!("toolu_mt_{}", nth + 1),
                    "run_command",
                    json!({ "command": "sleep 0.6" }),
                );
            }
            return text_stream("done multi");
        }

        // --- CONCURRENT_TOOLS_TEST -----------------------------------------
        if trigger.contains("CONCURRENT_TOOLS_TEST") {
            if nth == 0 {
                return concurrent_tools_stream(&[
                    (
                        "toolu_ct_fast",
                        "run_command",
                        json!({ "command": "sleep 0.1" }),
                    ),
                    (
                        "toolu_ct_slow",
                        "run_command",
                        json!({ "command": "sleep 1.5" }),
                    ),
                ]);
            }
            return text_stream("done concurrent");
        }

        // --- LONG_STREAM_TEST ----------------------------------------------
        if trigger.contains("LONG_STREAM_TEST") {
            return slow_text_stream(
                "This is a deliberately long streaming response emitted in chunks. done stream",
                8,
                Duration::from_millis(100),
            );
        }

        // --- TWO_PAUSES_TEST -----------------------------------------------
        if trigger.contains("TWO_PAUSES_TEST") {
            if nth < 4 {
                return tool_use_stream(
                    &format!("toolu_tp_{}", nth + 1),
                    "run_command",
                    json!({ "command": "sleep 0.6" }),
                );
            }
            return text_stream("done two pauses");
        }

        // --- RESUME_BASIS_TEST ---------------------------------------------
        if trigger.contains("RESUME_BASIS_TEST") {
            if nth == 0 {
                return tool_use_stream(
                    "toolu_rb_1",
                    "run_command",
                    json!({ "command": "sleep 0.3" }),
                );
            }
            return text_stream("done basis");
        }

        // --- Default: simple "pong" ----------------------------------------
        text_stream("pong")
    }
}

// ---------------------------------------------------------------------------
// Routing helpers
// ---------------------------------------------------------------------------

/// Returns the joined text content of the first user message — the trigger
/// that selected the scenario.  Mirrors `firstUserText` in the JS fixture.
fn first_user_text(req: &LlmRequest) -> String {
    let Some(first) = req.messages.iter().find(|m| m.role == Role::User) else {
        return String::new();
    };
    let mut buf = String::new();
    for block in &first.content {
        if let ContentBlock::Text { text } = block {
            if !buf.is_empty() {
                buf.push(' ');
            }
            buf.push_str(text);
        }
    }
    buf
}

/// Number of assistant messages already in the history — selects which step
/// of the chosen scenario to return.
fn nth_call_in_turn(req: &LlmRequest) -> usize {
    req.messages
        .iter()
        .filter(|m| m.role == Role::Assistant)
        .count()
}

// ---------------------------------------------------------------------------
// Stream constructors
// ---------------------------------------------------------------------------

fn now_iso() -> String {
    // Timestamps in mock events do not need to be unique; use a fixed value
    // so test-side snapshots are not noisy.  The agent overwrites
    // `time` on persisted events anyway via its own clock for the events it
    // synthesises, but for the events we feed in the agent passes them
    // through as-is.  A constant is therefore the least surprising choice.
    "2024-01-01T00:00:00.000Z".to_owned()
}

fn usage() -> LlmResponseUsage {
    LlmResponseUsage {
        input_tokens: 10,
        output_tokens: 5,
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
        service_tier: None,
    }
}

fn llm_response(stop_reason: &str, text: Option<&str>) -> AgentItem {
    AgentItem::event(OmegaEvent::LlmResponse(LlmResponseEvent {
        time: now_iso(),
        stop_reason: stop_reason.to_owned(),
        cleared_tool_uses: None,
        cleared_input_tokens: None,
        usage: usage(),
        context_hash: String::new(),
        text: text.map(str::to_owned),
        thinking: None,
        streaming_start: None,
        response_summary: None,
    }))
}

/// `[Signal::Text(text), llm_response("end_turn", Some(text))]`.
fn text_stream(text: &str) -> AgentItemStream {
    let items: Vec<Result<AgentItem, LlmError>> = vec![
        Ok(AgentItem::Signal(StreamSignal::Text {
            text: text.to_owned(),
        })),
        Ok(llm_response("end_turn", Some(text))),
    ];
    Box::pin(stream::iter(items))
}

/// Slow streaming variant: emits `chunks` text fragments separated by
/// `delay`, then a single `LlmResponse(end_turn)` carrying the full text.
///
/// Implementation note: every yielded item is preceded by `tokio::time::sleep(delay)`
/// — including the trailing `LlmResponse`.  That extra `delay` at the end
/// is harmless for the only consumer (`LONG_STREAM_TEST`), which only cares
/// that the stream takes long enough for the UI to register pause / interject
/// activity while text is still arriving.
fn slow_text_stream(text: &str, chunks: usize, delay: Duration) -> AgentItemStream {
    let chunks = chunks.max(1);
    let chunk_size = text.len().div_ceil(chunks);
    let mut items: Vec<Result<AgentItem, LlmError>> = text
        .as_bytes()
        .chunks(chunk_size)
        .map(|c| {
            Ok(AgentItem::Signal(StreamSignal::Text {
                text: String::from_utf8_lossy(c).into_owned(),
            }))
        })
        .collect();
    items.push(Ok(llm_response("end_turn", Some(text))));

    let base = stream::iter(items);
    let slow = base.then(move |x| async move {
        tokio::time::sleep(delay).await;
        x
    });
    Box::pin(slow) as BoxStream<'static, Result<AgentItem, LlmError>>
}

/// `[ToolCall(id, name, input), llm_response("tool_use", None)]`.
fn tool_use_stream(id: &str, name: &str, input: Value) -> AgentItemStream {
    let items: Vec<Result<AgentItem, LlmError>> = vec![
        Ok(AgentItem::event(OmegaEvent::ToolCall(ToolCallEvent {
            time: now_iso(),
            id: id.to_owned(),
            name: name.to_owned(),
            input,
            context_hash: String::new(),
        }))),
        Ok(llm_response("tool_use", None)),
    ];
    Box::pin(stream::iter(items))
}

/// Multiple `tool_use` blocks in a single LLM response → the agent dispatches
/// them concurrently.
fn concurrent_tools_stream(tools: &[(&str, &str, Value)]) -> AgentItemStream {
    let mut items: Vec<Result<AgentItem, LlmError>> = Vec::with_capacity(tools.len() + 1);
    for (id, name, input) in tools {
        items.push(Ok(AgentItem::event(OmegaEvent::ToolCall(ToolCallEvent {
            time: now_iso(),
            id: (*id).to_owned(),
            name: (*name).to_owned(),
            input: input.clone(),
            context_hash: String::new(),
        }))));
    }
    items.push(Ok(llm_response("tool_use", None)));
    Box::pin(stream::iter(items))
}

// ---------------------------------------------------------------------------
// Capture projection
// ---------------------------------------------------------------------------

/// Project an [`LlmRequest`] into the shape the JS tests expected from the
/// control API.  Mirrors the projection block in the historical fixture.
fn project_call(req: &LlmRequest) -> CapturedCall {
    let system_kind = if req
        .system
        .as_deref()
        .is_some_and(|s| s.starts_with("Summarise the coding session"))
    {
        "resumption"
    } else {
        "task"
    };

    let messages = req
        .messages
        .iter()
        .map(|m| {
            let role = match m.role {
                Role::User => "user",
                Role::Assistant => "assistant",
            }
            .to_owned();
            // Single-block plain-text user messages → expose as a plain
            // string so substring assertions are convenient.  Anything
            // else (multiple blocks, tool_use, tool_result, …) → JSON-
            // stringify the whole content array.
            let content = match m.content.as_slice() {
                [ContentBlock::Text { text }] => text.clone(),
                _ => serde_json::to_string(&m.content).unwrap_or_default(),
            };
            CapturedMessage { role, content }
        })
        .collect();

    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());

    CapturedCall {
        system_kind,
        at,
        messages,
    }
}
