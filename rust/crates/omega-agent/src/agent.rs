//! The Omega agent — the agentic loop core.
//!
//! `Agent` owns:
//!
//! * a [`Provider`](omega_core::Provider) (typically wrapped by
//!   [`RetryingProvider`](omega_core::RetryingProvider)) that performs
//!   LLM calls,
//! * a [`ContextStore`] and an [`EventStore`] for durable session state,
//! * the in-memory `Vec<Message>` history that mirrors `context.jsonl`.
//!
//! Public entry point [`Agent::send_message`] returns a stream of
//! [`AgentItem`]s — text/thinking deltas plus persisted [`OmegaEvent`]s —
//! and drives the agentic loop until either the model produces a final
//! response (no tool calls), an error terminates the turn, or the
//! [`CancellationToken`] is tripped.
//!
//! Mirrors `src/agent.ts::Agent.sendMessage` minus features deferred to
//! later phases (pause/resume/interject, in-agent retries — those now
//! live in [`RetryingProvider`](omega_core::RetryingProvider) — context
//! compaction, tool-result clearing, model-context-window recovery).

use std::collections::HashMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use async_stream::stream;
use chrono::Utc;
use futures::stream::{FuturesUnordered, Stream, StreamExt};
use omega_core::{
    AgentItem, ContentBlock, LlmError, LlmRequest, Message, ModelConfig, Provider, Role,
};
use omega_protocol::StreamSignal;
use omega_protocol::events::{
    AgentErrorEvent, LlmCallEvent, LlmErrorEvent, LlmResponseEvent, ToolCallEvent, ToolResultEvent,
    TurnEndEvent, TurnInterruptedEvent, UserMessageEvent,
};
use omega_protocol::{InterruptReason, OmegaEvent, TurnMetrics};

use omega_store::{ContextHash, ContextStore, EventStore};
use omega_tools::{execute_tool, tool_definitions};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::config::max_output_tokens_for_model;
use crate::error_classify::{is_context_too_long, is_invalid_tool_json};
use crate::system_prompt::build_system_prompt;

const ANTHROPIC_URL: &str = "https://api.anthropic.com/v1/messages";

/// Maximum invalid-tool-JSON nudges per `send_message` call before we
/// give up and end the turn.  Mirrors the TS agent's
/// `feedbackOnExhaustion` cap.
const INVALID_TOOL_JSON_FEEDBACK_CAP: u32 = 2;

const INVALID_TOOL_JSON_NUDGE: &str = "Your previous response could not be parsed — the tool-call JSON had invalid escaping (likely unescaped newlines or quotes in a string argument). Please retry the same tool call, being extra careful with JSON string escaping.";

const DANGLING_TOOL_USE_RESULT: &str =
    "[not executed: previous turn was interrupted before this tool ran]";

// ---------------------------------------------------------------------------
// Agent
// ---------------------------------------------------------------------------

/// Construction-time configuration for [`Agent`].
pub struct AgentConfig {
    /// Model id passed to the provider on every API call.
    pub model: String,
    /// Working directory interpolated into the system prompt.
    pub cwd: PathBuf,
    /// Pre-loaded contents of `<cwd>/.omega/system-prompt-append.md`,
    /// if the file exists.  Pass `None` to skip the append section.
    pub system_prompt_append: Option<String>,
}

/// The agentic loop.
///
/// Held by `omega-server` (one per session) and by tests via the
/// in-memory [`MockProvider`](crate::testing::MockProvider) helper.
pub struct Agent {
    provider: Arc<dyn Provider>,
    context_store: ContextStore,
    event_store: EventStore,
    config: AgentConfig,
    /// In-memory mirror of `context.jsonl`; sent verbatim as the
    /// `messages` array on every API call.
    history: Vec<Message>,
    /// Hashes of `history` records, in insertion order.  Snapshotted
    /// onto every `LlmCall` event so post-mortem inspection can pin
    /// the exact context the model saw.
    context_hashes: Vec<ContextHash>,
}

impl Agent {
    /// Build a new agent.
    ///
    /// `provider` is typically an [`Arc<RetryingProvider<…>>`] so the
    /// retry / `LlmRetry`-event logic happens transparently.  The agent
    /// itself never retries.
    #[must_use]
    pub fn new(
        provider: Arc<dyn Provider>,
        context_store: ContextStore,
        event_store: EventStore,
        config: AgentConfig,
    ) -> Self {
        Self {
            provider,
            context_store,
            event_store,
            config,
            history: Vec::new(),
            context_hashes: Vec::new(),
        }
    }

    /// Pre-seed the in-memory history (used by resumption and tests).
    ///
    /// Callers must keep `history` and `context_hashes` aligned.
    pub fn seed_history(&mut self, history: Vec<Message>, hashes: Vec<ContextHash>) {
        self.history = history;
        self.context_hashes = hashes;
    }

    /// Borrow the in-memory history (read-only — used by tests and
    /// future world-state inspection).
    #[must_use]
    pub fn history(&self) -> &[Message] {
        &self.history
    }

    /// Drive one user turn.  Returns a stream of every event/signal
    /// produced by the agentic loop.
    ///
    /// Cancellation: tripping `cancel` aborts in-flight tool calls and
    /// the LLM stream, then yields a `TurnInterrupted{reason: aborted}`
    /// event before the stream ends.
    #[allow(clippy::too_many_lines)] // single async generator; splitting requires plumbing yields through return types
    pub fn send_message<'a>(
        &'a mut self,
        user_message: String,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AgentItem> + Send + 'a>> {
        Box::pin(stream! {
            // -----------------------------------------------------------------
            // Step 1: dangling tool_use repair.
            //
            // If the previous turn was interrupted between LlmResponse and
            // tool dispatch, the last assistant record contains tool_use blocks
            // with no matching tool_results.  The Anthropic API rejects that
            // shape, so synthesise tool_results=[is_error: true] before letting
            // the new user message land.
            // -----------------------------------------------------------------
            let dangling: Vec<(String, String)> = self
                .history
                .last()
                .filter(|m| m.role == Role::Assistant)
                .map(|m| {
                    m.content
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::ToolUse { id, name, .. } => {
                                Some((id.clone(), name.clone()))
                            }
                            _ => None,
                        })
                        .collect()
                })
                .unwrap_or_default();

            if !dangling.is_empty() {
                let synthetic: Vec<ContentBlock> = dangling
                    .iter()
                    .map(|(id, _)| ContentBlock::ToolResult {
                        tool_use_id: id.clone(),
                        content: DANGLING_TOOL_USE_RESULT.to_owned(),
                        is_error: true,
                    })
                    .collect();
                match self
                    .context_store
                    .append(Role::User, synthetic.clone())
                    .await
                {
                    Ok(hash) => {
                        self.history.push(Message {
                            role: Role::User,
                            content: synthetic,
                        });
                        self.context_hashes.push(hash);
                    }
                    Err(e) => {
                        let ev = OmegaEvent::AgentError(AgentErrorEvent {
                            time: now_iso(),
                            error: format!("context_store append failed: {e}"),
                        });
                        let _ = self.event_store.append(&ev).await;
                        yield AgentItem::event(ev);
                        let ti = OmegaEvent::TurnInterrupted(TurnInterruptedEvent {
                            time: now_iso(),
                            reason: Some(InterruptReason::Error),
                        });
                        let _ = self.event_store.append(&ti).await;
                        yield AgentItem::event(ti);
                        return;
                    }
                }
                for (id, name) in dangling {
                    let ev = OmegaEvent::ToolResult(ToolResultEvent {
                        time: now_iso(),
                        id,
                        name,
                        is_error: true,
                        duration_ms: 0,
                        output: DANGLING_TOOL_USE_RESULT.to_owned(),
                    });
                    let _ = self.event_store.append(&ev).await;
                    yield AgentItem::event(ev);
                }
            }

            // -----------------------------------------------------------------
            // Step 2: append the user message.
            // -----------------------------------------------------------------
            let user_blocks = vec![ContentBlock::Text {
                text: user_message.clone(),
            }];
            match self
                .context_store
                .append(Role::User, user_blocks.clone())
                .await
            {
                Ok(hash) => {
                    self.history.push(Message {
                        role: Role::User,
                        content: user_blocks,
                    });
                    self.context_hashes.push(hash);
                }
                Err(e) => {
                    let ev = OmegaEvent::AgentError(AgentErrorEvent {
                        time: now_iso(),
                        error: format!("context_store append failed: {e}"),
                    });
                    let _ = self.event_store.append(&ev).await;
                    yield AgentItem::event(ev);
                    return;
                }
            }
            let user_ev = OmegaEvent::UserMessage(UserMessageEvent {
                time: now_iso(),
                content: user_message,
            });
            let _ = self.event_store.append(&user_ev).await;
            yield AgentItem::event(user_ev);

            // -----------------------------------------------------------------
            // Step 3: outer agentic loop.
            // -----------------------------------------------------------------
            let mut feedback_attempts: u32 = 0;
            let mut tot_input: i64 = 0;
            let mut tot_output: i64 = 0;
            let mut tot_cache_creation: i64 = 0;
            let mut tot_cache_read: i64 = 0;

            loop {
                if cancel.is_cancelled() {
                    let ev = OmegaEvent::TurnInterrupted(TurnInterruptedEvent {
                        time: now_iso(),
                        reason: Some(InterruptReason::Aborted),
                    });
                    let _ = self.event_store.append(&ev).await;
                    yield AgentItem::event(ev);
                    return;
                }

                let max_tokens = max_output_tokens_for_model(&self.config.model);
                let system = build_system_prompt(
                    &self.config.cwd.to_string_lossy(),
                    max_tokens,
                    self.config.system_prompt_append.as_deref(),
                );
                let request = LlmRequest {
                    model: self.config.model.clone(),
                    messages: self.history.clone(),
                    system: Some(system),
                    tools: tool_definitions(),
                    config: ModelConfig {
                        max_tokens,
                        temperature: None,
                        thinking_budget: None,
                    },
                };

                // --- Emit LlmCall ------------------------------------------
                let request_bytes = serde_json::to_vec(&request)
                    .map_or(0, |v| i64::try_from(v.len()).unwrap_or(i64::MAX));
                let cache_breakpoint_index = if self.context_hashes.is_empty() {
                    None
                } else {
                    i64::try_from(self.context_hashes.len() - 1).ok()
                };
                let call_ev = OmegaEvent::LlmCall(LlmCallEvent {
                    time: now_iso(),
                    url: ANTHROPIC_URL.to_owned(),
                    model: self.config.model.clone(),
                    context_hashes: self
                        .context_hashes
                        .iter()
                        .map(|h| h.as_ref().to_owned())
                        .collect(),
                    cache_breakpoint_index,
                    request_bytes,
                    request_summary: None,
                });
                let _ = self.event_store.append(&call_ev).await;
                yield AgentItem::event(call_ev);

                // --- Drain the provider stream -----------------------------
                let mut provider_stream = self.provider.stream(request);
                let mut text_buf = String::new();
                let mut thinking_buf = String::new();
                let mut tool_uses: Vec<(String, String, Value)> = Vec::new();
                let mut llm_response: Option<LlmResponseEvent> = None;
                let mut stream_error: Option<LlmError> = None;

                while let Some(item) = provider_stream.next().await {
                    if cancel.is_cancelled() {
                        break;
                    }
                    match item {
                        Ok(AgentItem::Signal(sig)) => {
                            match &sig {
                                StreamSignal::Text { text } => text_buf.push_str(text),
                                StreamSignal::Thinking { text } => thinking_buf.push_str(text),
                            }
                            yield AgentItem::Signal(sig);
                        }
                        Ok(AgentItem::Event(boxed)) => {
                            let event = *boxed;
                            match event {
                                OmegaEvent::ToolCall(tc) => {
                                    tool_uses.push((tc.id, tc.name, tc.input));
                                    // Re-emitted later with assistant_hash filled.
                                }
                                OmegaEvent::LlmResponse(lr) => {
                                    llm_response = Some(lr);
                                }
                                OmegaEvent::LlmRetry(retry) => {
                                    // RetryingProvider has just slept and is
                                    // about to re-issue the call; throw away
                                    // any partial assistant content we
                                    // accumulated and forward the event so the
                                    // UI can roll back.
                                    text_buf.clear();
                                    thinking_buf.clear();
                                    let ev = OmegaEvent::LlmRetry(retry);
                                    let _ = self.event_store.append(&ev).await;
                                    yield AgentItem::event(ev);
                                }
                                other => {
                                    // Forward unmodified — provider may emit
                                    // Compacted, etc.
                                    let _ = self.event_store.append(&other).await;
                                    yield AgentItem::event(other);
                                }
                            }
                        }
                        Err(err) => {
                            stream_error = Some(err);
                            break;
                        }
                    }
                }

                // --- Handle abort during streaming -------------------------
                if cancel.is_cancelled() {
                    let ev = OmegaEvent::TurnInterrupted(TurnInterruptedEvent {
                        time: now_iso(),
                        reason: Some(InterruptReason::Aborted),
                    });
                    let _ = self.event_store.append(&ev).await;
                    yield AgentItem::event(ev);
                    return;
                }

                // --- Handle stream error -----------------------------------
                if let Some(err) = stream_error {
                    let llm_err_ev = OmegaEvent::LlmError(LlmErrorEvent {
                        time: now_iso(),
                        url: ANTHROPIC_URL.to_owned(),
                        error: err.to_string(),
                        http_status: err.status(),
                    });
                    let _ = self.event_store.append(&llm_err_ev).await;
                    yield AgentItem::event(llm_err_ev);

                    if is_invalid_tool_json(&err)
                        && feedback_attempts < INVALID_TOOL_JSON_FEEDBACK_CAP
                    {
                        feedback_attempts += 1;
                        let nudge_blocks = vec![ContentBlock::Text {
                            text: INVALID_TOOL_JSON_NUDGE.to_owned(),
                        }];
                        match self
                            .context_store
                            .append(Role::User, nudge_blocks.clone())
                            .await
                        {
                            Ok(hash) => {
                                self.history.push(Message {
                                    role: Role::User,
                                    content: nudge_blocks,
                                });
                                self.context_hashes.push(hash);
                            }
                            Err(e) => {
                                let ev = OmegaEvent::AgentError(AgentErrorEvent {
                                    time: now_iso(),
                                    error: format!("context_store append failed: {e}"),
                                });
                                let _ = self.event_store.append(&ev).await;
                                yield AgentItem::event(ev);
                                return;
                            }
                        }
                        let nudge_ev = OmegaEvent::UserMessage(UserMessageEvent {
                            time: now_iso(),
                            content: INVALID_TOOL_JSON_NUDGE.to_owned(),
                        });
                        let _ = self.event_store.append(&nudge_ev).await;
                        yield AgentItem::event(nudge_ev);
                        continue;
                    }

                    let agent_msg = if is_context_too_long(&err) {
                        "Context too large to send. Start a fresh focused turn.".to_owned()
                    } else if err.is_retryable() {
                        format!("Anthropic API error after retries: {err}")
                    } else {
                        format!("API error: {err}")
                    };
                    let ae = OmegaEvent::AgentError(AgentErrorEvent {
                        time: now_iso(),
                        error: agent_msg,
                    });
                    let _ = self.event_store.append(&ae).await;
                    yield AgentItem::event(ae);
                    let ti = OmegaEvent::TurnInterrupted(TurnInterruptedEvent {
                        time: now_iso(),
                        reason: Some(InterruptReason::Error),
                    });
                    let _ = self.event_store.append(&ti).await;
                    yield AgentItem::event(ti);
                    return;
                }

                // --- Should have an LlmResponse now ------------------------
                let Some(mut lr) = llm_response else {
                    let ae = OmegaEvent::AgentError(AgentErrorEvent {
                        time: now_iso(),
                        error: "Provider stream ended without LlmResponse".to_owned(),
                    });
                    let _ = self.event_store.append(&ae).await;
                    yield AgentItem::event(ae);
                    let ti = OmegaEvent::TurnInterrupted(TurnInterruptedEvent {
                        time: now_iso(),
                        reason: Some(InterruptReason::Error),
                    });
                    let _ = self.event_store.append(&ti).await;
                    yield AgentItem::event(ti);
                    return;
                };

                // --- Build + persist the assistant context record ---------
                let mut assistant_blocks: Vec<ContentBlock> = Vec::new();
                if !thinking_buf.is_empty() {
                    assistant_blocks.push(ContentBlock::Thinking {
                        thinking: std::mem::take(&mut thinking_buf),
                        signature: None,
                    });
                }
                if !text_buf.is_empty() {
                    assistant_blocks.push(ContentBlock::Text {
                        text: std::mem::take(&mut text_buf),
                    });
                }
                for (id, name, input) in &tool_uses {
                    assistant_blocks.push(ContentBlock::ToolUse {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                    });
                }

                let assistant_hash = match self
                    .context_store
                    .append(Role::Assistant, assistant_blocks.clone())
                    .await
                {
                    Ok(h) => h,
                    Err(e) => {
                        let ev = OmegaEvent::AgentError(AgentErrorEvent {
                            time: now_iso(),
                            error: format!("context_store append failed: {e}"),
                        });
                        let _ = self.event_store.append(&ev).await;
                        yield AgentItem::event(ev);
                        return;
                    }
                };
                self.history.push(Message {
                    role: Role::Assistant,
                    content: assistant_blocks,
                });
                self.context_hashes.push(assistant_hash.clone());

                // --- Emit LlmResponse with hash + accumulate metrics ------
                lr.context_hash = assistant_hash.as_ref().to_owned();
                tot_input += lr.usage.input_tokens;
                tot_output += lr.usage.output_tokens;
                tot_cache_creation += lr.usage.cache_creation_input_tokens.unwrap_or(0);
                tot_cache_read += lr.usage.cache_read_input_tokens.unwrap_or(0);
                let stop_reason = lr.stop_reason.clone();
                let response_ev = OmegaEvent::LlmResponse(lr);
                let _ = self.event_store.append(&response_ev).await;
                yield AgentItem::event(response_ev);

                // --- Tool dispatch ----------------------------------------
                if stop_reason == "tool_use" && !tool_uses.is_empty() {
                    // Emit ToolCall events with assistant_hash filled in.
                    for (id, name, input) in &tool_uses {
                        let tc = OmegaEvent::ToolCall(ToolCallEvent {
                            time: now_iso(),
                            id: id.clone(),
                            name: name.clone(),
                            input: input.clone(),
                            context_hash: assistant_hash.as_ref().to_owned(),
                        });
                        let _ = self.event_store.append(&tc).await;
                        yield AgentItem::event(tc);
                    }

                    // Concurrent dispatch — clone (id, name, input, cancel)
                    // into each future so they don't borrow self.
                    let mut futures: FuturesUnordered<_> = tool_uses
                        .iter()
                        .enumerate()
                        .map(|(i, (id, name, input))| {
                            let id = id.clone();
                            let name = name.clone();
                            let input = input.clone();
                            let cancel_clone = cancel.clone();
                            async move {
                                let start = Instant::now();
                                let res =
                                    execute_tool(&name, input, Some(&cancel_clone)).await;
                                let elapsed = start.elapsed();
                                (i, id, name, res, elapsed)
                            }
                        })
                        .collect();

                    // Tool dispatches complete in non-deterministic order;
                    // collect by id, then re-order by the original tool_use
                    // sequence when assembling the user message so the
                    // tool_results land in the same shape the model emitted.
                    let mut by_id: HashMap<String, (String, bool)> = HashMap::new();
                    while let Some((_idx, id, name, res, elapsed)) = futures.next().await {
                        let duration_ms = i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX);
                        let tr = OmegaEvent::ToolResult(ToolResultEvent {
                            time: now_iso(),
                            id: id.clone(),
                            name,
                            is_error: res.is_error,
                            duration_ms,
                            output: res.content.clone(),
                        });
                        let _ = self.event_store.append(&tr).await;
                        yield AgentItem::event(tr);
                        by_id.insert(id, (res.content, res.is_error));
                    }

                    let result_blocks: Vec<ContentBlock> = tool_uses
                        .iter()
                        .map(|(id, _, _)| {
                            // FuturesUnordered always produces one entry per
                            // pushed future, so the lookup cannot miss.
                            // If it ever does, fall back to a synthetic
                            // error result rather than panicking the agent.
                            let (content, is_error) = by_id.remove(id).unwrap_or_else(|| {
                                ("tool dispatch produced no result".to_owned(), true)
                            });
                            ContentBlock::ToolResult {
                                tool_use_id: id.clone(),
                                content,
                                is_error,
                            }
                        })
                        .collect();

                    match self
                        .context_store
                        .append(Role::User, result_blocks.clone())
                        .await
                    {
                        Ok(hash) => {
                            self.history.push(Message {
                                role: Role::User,
                                content: result_blocks,
                            });
                            self.context_hashes.push(hash);
                        }
                        Err(e) => {
                            let ev = OmegaEvent::AgentError(AgentErrorEvent {
                                time: now_iso(),
                                error: format!("context_store append failed: {e}"),
                            });
                            let _ = self.event_store.append(&ev).await;
                            yield AgentItem::event(ev);
                            return;
                        }
                    }
                    continue;
                }

                // --- No tool calls — emit TurnEnd and finish --------------
                let metrics = TurnMetrics {
                    input_tokens: tot_input,
                    output_tokens: tot_output,
                    cache_creation_tokens: if tot_cache_creation > 0 {
                        Some(tot_cache_creation)
                    } else {
                        None
                    },
                    cache_read_tokens: if tot_cache_read > 0 {
                        Some(tot_cache_read)
                    } else {
                        None
                    },
                };
                let te = OmegaEvent::TurnEnd(TurnEndEvent {
                    time: now_iso(),
                    metrics,
                });
                let _ = self.event_store.append(&te).await;
                yield AgentItem::event(te);
                return;
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
