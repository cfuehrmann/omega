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
    AgentErrorEvent, EffortChangedEvent, LlmCallEvent, LlmErrorEvent, LlmResponseEvent,
    ModelChangedEvent, ResumingSessionEvent, ServerStartedEvent, SessionResumedEvent,
    SessionStartedEvent, ToolCallEvent, ToolResultEvent, TurnContinuedEvent, TurnEndEvent,
    TurnInterruptedEvent, TurnPausedEvent, UserMessageEvent,
};
use omega_protocol::{ContinueMode, InterruptReason, OmegaEvent, TurnMetrics};

use omega_store::{ContextHash, ContextStore, EventStore};
use omega_tools::{execute_tool, tool_definitions};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::config::max_output_tokens_for_model;
use crate::controls::{ControlHandle, TurnGuard};
use crate::error_classify::{is_context_too_long, is_invalid_tool_json};
use crate::session_resume::{
    RESUMPTION_MAX_TOKENS, RESUMPTION_MODEL, RESUMPTION_SUMMARY_INSTRUCTIONS,
    extract_summary_from_response,
};
use crate::system_prompt::build_system_prompt;

const ANTHROPIC_URL: &str = "https://api.anthropic.com/v1/messages";

/// Maximum invalid-tool-JSON nudges per `send_message` call before we
/// give up and end the turn.  Mirrors the TS agent's
/// `feedbackOnExhaustion` cap.
const INVALID_TOOL_JSON_FEEDBACK_CAP: u32 = 2;

const INVALID_TOOL_JSON_NUDGE: &str = "Your previous response could not be parsed — the tool-call JSON had invalid escaping (likely unescaped newlines or quotes in a string argument). Please retry the same tool call, being extra careful with JSON string escaping.";

const DANGLING_TOOL_USE_RESULT: &str =
    "[not executed: previous turn was interrupted before this tool ran]";

/// Canned preamble injected before the resumption summary in the synthetic
/// user seed message.  Mirrors the literal in `Agent.seedWithResumptionSummary`
/// in `src/agent.ts`.
const SEED_USER_PREAMBLE: &str =
    "The following is context from the previous session to provide continuity:\n\n";

/// Canned acknowledgement used as the synthetic assistant seed message.
/// Mirrors the literal in `Agent.seedWithResumptionSummary` in `src/agent.ts`.
const SEED_ASSISTANT_ACK: &str =
    "Understood. I have reviewed the context from the previous session and am ready to continue.";

/// Default thinking-effort level when none is explicitly set.
///
/// Matches `src/agent.ts` (`activeEffort = "medium"`).
pub const DEFAULT_EFFORT: &str = "medium";

// ---------------------------------------------------------------------------
// Agent
// ---------------------------------------------------------------------------

/// Construction-time configuration for [`Agent`].
pub struct AgentConfig {
    /// Model id passed to the provider on every API call.
    pub model: String,
    /// Initial thinking-effort level.  `None` falls back to
    /// [`DEFAULT_EFFORT`].  Phase 2a wires this through from the
    /// `POST /api/sessions` body and the `reset` client frame.
    pub effort: Option<String>,
    /// Working directory interpolated into the system prompt.
    pub cwd: PathBuf,
    /// Pre-loaded contents of `<cwd>/.omega/system-prompt-append.md`,
    /// if the file exists.  Pass `None` to skip the append section.
    pub system_prompt_append: Option<String>,
    /// Path to the session directory (the parent of `events.jsonl`).
    /// Used by [`Agent::init`] to write the `session_started` event.
    pub session_dir: PathBuf,
}

/// The agentic loop.
///
/// Held by `omega-server` (one per session) and by tests via the
/// in-memory [`MockProvider`](crate::testing::MockProvider) helper.
pub struct Agent {
    provider: Arc<dyn Provider>,
    context_store: ContextStore,
    event_store: Arc<EventStore>,
    /// Pause / continue / abort handle.  Cloned out via
    /// [`Agent::controls`] **before** the caller starts a turn so the
    /// clone can be used to fire control events while `send_message`
    /// holds an exclusive borrow on the agent.
    controls: ControlHandle,
    config: AgentConfig,
    /// Currently selected model id.  Initialised from `config.model`;
    /// mutated by [`Agent::set_model`].  Read on every API call so a
    /// switch takes effect from the next call onward.
    active_model: String,
    /// Currently selected thinking-effort level.  Initialised to
    /// [`DEFAULT_EFFORT`]; mutated by [`Agent::set_effort`].  Phase
    /// 1d.1a stores it but does not yet thread it onto `LlmRequest` —
    /// the request-shape work belongs to `omega-core` and is deferred.
    active_effort: String,
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
        let active_model = config.model.clone();
        let active_effort = config
            .effort
            .clone()
            .unwrap_or_else(|| DEFAULT_EFFORT.to_owned());
        let event_store = Arc::new(event_store);
        let controls = ControlHandle::new(Arc::clone(&event_store));
        Self {
            provider,
            context_store,
            event_store,
            controls,
            config,
            active_model,
            active_effort,
            history: Vec::new(),
            context_hashes: Vec::new(),
        }
    }

    /// Write `server_started` and `session_started` events to `events.jsonl`.
    ///
    /// Must be called once after construction and before any turns.  Mirrors
    /// `Agent.init()` in `src/agent.ts`.
    ///
    /// # Errors
    ///
    /// Returns an error if serialisation or the file write fails.
    pub async fn init(&self) -> omega_store::Result<()> {
        // 1. server_started
        let server_started = OmegaEvent::ServerStarted(ServerStartedEvent { time: now_iso() });
        self.event_store.append(&server_started).await?;

        // 2. session_started
        let session_id = self.config.session_dir.file_name().map_or_else(
            || "unknown".to_owned(),
            |n| n.to_string_lossy().into_owned(),
        );
        let path = self
            .config
            .session_dir
            .strip_prefix(&self.config.cwd)
            .unwrap_or(&self.config.session_dir)
            .to_string_lossy()
            .into_owned();
        let max_tokens = max_output_tokens_for_model(&self.active_model);
        let system_prompt = build_system_prompt(
            &self.config.cwd.to_string_lossy(),
            max_tokens,
            self.config.system_prompt_append.as_deref(),
        );
        let session_started = OmegaEvent::SessionStarted(SessionStartedEvent {
            time: now_iso(),
            session_id,
            path,
            model: self.active_model.clone(),
            effort: self.active_effort.clone(),
            system_prompt,
        });
        self.event_store.append(&session_started).await?;
        Ok(())
    }

    /// Borrow a clone of the pause/continue/abort control handle.
    ///
    /// Callers should obtain the handle **before** invoking
    /// [`Agent::send_message`]; `send_message` exclusively borrows
    /// `&mut self` for the lifetime of its returned stream, so any
    /// `&self` method (including this one) cannot be called
    /// concurrently. The returned handle stays valid across multiple
    /// turns — the underlying turn-cancel token is rotated automatically.
    #[must_use]
    pub fn controls(&self) -> ControlHandle {
        self.controls.clone()
    }

    /// Switch the active model.  Persists a [`ModelChangedEvent`] and
    /// returns it so callers can fan it out to the UI without a second
    /// load from disk.  Subsequent [`Agent::send_message`] calls send
    /// the new model.
    ///
    /// Mirrors `Agent.setModel` in `src/agent.ts`.
    pub async fn set_model(&mut self, model: String) -> OmegaEvent {
        self.active_model = model.clone();
        let ev = OmegaEvent::ModelChanged(ModelChangedEvent {
            time: now_iso(),
            model,
        });
        let _ = self.event_store.append(&ev).await;
        ev
    }

    /// Switch the active thinking-effort level.  Persists an
    /// [`EffortChangedEvent`] and returns it.
    ///
    /// Mirrors `Agent.setEffort` in `src/agent.ts`.
    pub async fn set_effort(&mut self, effort: String) -> OmegaEvent {
        self.active_effort = effort.clone();
        let ev = OmegaEvent::EffortChanged(EffortChangedEvent {
            time: now_iso(),
            effort,
        });
        let _ = self.event_store.append(&ev).await;
        ev
    }

    /// Currently selected model id.  Reflects the most recent
    /// `set_model` call (or `config.model` if none has happened).
    #[must_use]
    pub fn active_model(&self) -> &str {
        &self.active_model
    }

    /// Currently selected thinking-effort level.
    #[must_use]
    pub fn active_effort(&self) -> &str {
        &self.active_effort
    }

    /// Pre-seed the in-memory history (used by resumption and tests).
    ///
    /// Callers must keep `history` and `context_hashes` aligned.
    pub fn seed_history(&mut self, history: Vec<Message>, hashes: Vec<ContextHash>) {
        self.history = history;
        self.context_hashes = hashes;
    }

    /// Seed this session with a summary of a previous session.
    ///
    /// Persists a `SessionResumed` event (carrying the `summary` and the
    /// id of the session it was distilled from), then injects two
    /// synthetic messages into the in-memory history and into
    /// `context.jsonl`:
    ///
    /// 1. a `user` message containing the canned preamble plus the summary
    ///    text — makes the LLM aware of prior context from turn 1; and
    /// 2. an `assistant` message with the canned acknowledgement — keeps
    ///    the conversation in the user/assistant alternation pattern that
    ///    Anthropic expects.
    ///
    /// Returns the persisted `SessionResumed` event so the caller can fan
    /// it out to the UI without re-reading the event log.
    ///
    /// Mirrors `Agent.seedWithResumptionSummary` in `src/agent.ts`.
    ///
    /// # Errors
    ///
    /// Returns [`omega_store::StoreError`] if appending either of the two
    /// synthetic context records fails. The `SessionResumed` event is
    /// emitted before any context-store work, so the caller may still see
    /// it on the wire even when this method errors.
    pub async fn seed_with_resumption_summary(
        &mut self,
        summary: String,
        resumed_from: String,
    ) -> Result<OmegaEvent, omega_store::StoreError> {
        let ev = OmegaEvent::SessionResumed(SessionResumedEvent {
            time: now_iso(),
            resumed_from,
            summary: summary.clone(),
        });
        let _ = self.event_store.append(&ev).await;

        // Synthetic user message: preamble + summary.
        let user_blocks = vec![ContentBlock::Text {
            text: format!("{SEED_USER_PREAMBLE}{summary}"),
        }];
        let user_hash = self
            .context_store
            .append(Role::User, user_blocks.clone())
            .await?;
        self.history.push(Message {
            role: Role::User,
            content: user_blocks,
        });
        self.context_hashes.push(user_hash);

        // Synthetic assistant acknowledgement.
        let assistant_blocks = vec![ContentBlock::Text {
            text: SEED_ASSISTANT_ACK.to_owned(),
        }];
        let assistant_hash = self
            .context_store
            .append(Role::Assistant, assistant_blocks.clone())
            .await?;
        self.history.push(Message {
            role: Role::Assistant,
            content: assistant_blocks,
        });
        self.context_hashes.push(assistant_hash);

        Ok(ev)
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
            // Step 0: turn-entry pause-control reset + cancel forwarder.
            //
            // `reset_for_turn` clears any pause state left from prior turns
            // and installs a fresh turn-scoped cancel token. We forward the
            // external `cancel` parameter into that turn-token via a spawned
            // task so any cancellation source feeds through one merged token.
            // The `TurnGuard` runs on body-drop (normal completion, error,
            // or caller-drops-stream-mid-suspend) and re-clears state +
            // aborts the forwarder.
            // -----------------------------------------------------------------
            let turn_cancel = self.controls.reset_for_turn();
            let forwarder = {
                let external = cancel.clone();
                let turn_for_fwd = turn_cancel.clone();
                tokio::spawn(async move {
                    external.cancelled().await;
                    turn_for_fwd.cancel();
                })
            };
            let _turn_guard = TurnGuard::new(&self.controls, Some(forwarder));
            // Shadow the parameter so every downstream `cancel.is_cancelled()`
            // check and `cancel.clone()` for tool dispatch uses the merged token.
            let cancel = turn_cancel;

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

                let max_tokens = max_output_tokens_for_model(&self.active_model);
                let system = build_system_prompt(
                    &self.config.cwd.to_string_lossy(),
                    max_tokens,
                    self.config.system_prompt_append.as_deref(),
                );
                let request = LlmRequest {
                    model: self.active_model.clone(),
                    messages: self.history.clone(),
                    system: Some(system),
                    tools: tool_definitions(),
                    config: ModelConfig {
                        max_tokens,
                        temperature: None,
                        thinking_budget: None,
                    },
                    context_management: None,
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
                    model: self.active_model.clone(),
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
                                OmegaEvent::Compacted(c) => {
                                    // Server-side compaction fired — discard
                                    // prior history (including the user msg
                                    // that triggered this turn) so the next
                                    // call sends only from this compaction
                                    // block onward.  Mirrors
                                    // src/agent.ts:1432–1453.
                                    self.history.clear();
                                    self.context_hashes.clear();
                                    let ev = OmegaEvent::Compacted(c);
                                    let _ = self.event_store.append(&ev).await;
                                    yield AgentItem::event(ev);
                                }
                                other => {
                                    // Forward unmodified — provider may emit
                                    // future event types we don't yet model.
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

                    // -----------------------------------------------------
                    // Pause seam.  Mirrors src/agent.ts:1765–1832 — fires
                    // only after the current tool batch's results are
                    // appended, so the next LlmCall sees a complete
                    // tool_use/tool_result pair.
                    // -----------------------------------------------------
                    if self.controls.take_pause_request() {
                        // Decide and mark `suspended` BEFORE yielding
                        // TurnPaused.  Any consumer that observes the
                        // TurnPaused event must see `suspended=true` so a
                        // follow-up `request_continue` resolves to
                        // mode=Manual rather than racing the agent.
                        let need_suspend = self.controls.try_enter_suspend();
                        let paused_ev = OmegaEvent::TurnPaused(TurnPausedEvent {
                            time: now_iso(),
                        });
                        let _ = self.event_store.append(&paused_ev).await;
                        yield AgentItem::event(paused_ev);

                        // Suspend loop: wait for Continue/Abort wake or
                        // a cancel.  Skipped entirely when continue
                        // arrived before the seam (need_suspend=false).
                        if need_suspend {
                            // Wait for either a Continue/Abort wake or a
                            // cancel.  Re-check `pending_continue` under
                            // lock at the top of each iteration so a
                            // notify that arrived between create-future
                            // and await is still observed.
                            loop {
                                if self.controls.pending_continue_ready()
                                    || cancel.is_cancelled()
                                {
                                    break;
                                }
                                tokio::select! {
                                    () = self.controls.notify().notified() => {}
                                    () = cancel.cancelled() => {}
                                }
                            }
                            self.controls.exit_suspend();
                        }

                        // Abort wins over Continue if both fired — a click-
                        // race resolves to the kill switch.
                        if cancel.is_cancelled() {
                            let ti = OmegaEvent::TurnInterrupted(
                                TurnInterruptedEvent {
                                    time: now_iso(),
                                    reason: Some(InterruptReason::Aborted),
                                },
                            );
                            let _ = self.event_store.append(&ti).await;
                            yield AgentItem::event(ti);
                            return;
                        }

                        // Take the pending continue (if any) and emit the
                        // optional interjection + TurnContinued.
                        let cont = self.controls.take_pending_continue();
                        let interjection = cont
                            .as_ref()
                            .and_then(|c| c.content.as_ref())
                            .filter(|s| !s.is_empty())
                            .cloned();
                        let mode = cont
                            .map_or(ContinueMode::Auto, |c| c.mode);

                        if let Some(text) = interjection {
                            let blocks = vec![ContentBlock::Text {
                                text: text.clone(),
                            }];
                            match self
                                .context_store
                                .append(Role::User, blocks.clone())
                                .await
                            {
                                Ok(hash) => {
                                    self.history.push(Message {
                                        role: Role::User,
                                        content: blocks,
                                    });
                                    self.context_hashes.push(hash);
                                }
                                Err(e) => {
                                    let ev = OmegaEvent::AgentError(
                                        AgentErrorEvent {
                                            time: now_iso(),
                                            error: format!(
                                                "context_store append failed: {e}"
                                            ),
                                        },
                                    );
                                    let _ = self.event_store.append(&ev).await;
                                    yield AgentItem::event(ev);
                                    return;
                                }
                            }
                            let user_ev = OmegaEvent::UserMessage(
                                UserMessageEvent {
                                    time: now_iso(),
                                    content: text,
                                },
                            );
                            let _ = self.event_store.append(&user_ev).await;
                            yield AgentItem::event(user_ev);
                        }

                        let cont_ev = OmegaEvent::TurnContinued(TurnContinuedEvent {
                            time: now_iso(),
                            mode,
                        });
                        let _ = self.event_store.append(&cont_ev).await;
                        yield AgentItem::event(cont_ev);
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

    /// Run the one-shot summarisation call that distils a previous
    /// session into a continuation summary, then seed this session's
    /// history with that summary.
    ///
    /// Mirrors `Agent.performResumption` in `src/agent.ts`.
    ///
    /// Order of effects (matched verbatim against the TS reference):
    ///
    /// 1. The `basis` is written to `context.jsonl` as a `user` record
    ///    so it is hash-addressable for the upcoming `LlmCall`. Note
    ///    that the basis is **not** pushed onto in-memory `history` —
    ///    it exists only as a context record so the LLM sees it for
    ///    this single call but the seeded session does not carry it
    ///    forward into subsequent turns.
    /// 2. A `ResumingSession` event is emitted (carrying `basis`,
    ///    `resumed_from`, and an optional `name`).
    /// 3. An `LlmCall` event is emitted with `cache_breakpoint_index =
    ///    None` (no caching: this is a one-off prompt) and
    ///    `context_hashes = [user_basis_hash]`.
    /// 4. The provider stream is drained; `Signal` items are forwarded,
    ///    `LlmRetry` events clear partial buffers and are forwarded.
    /// 5. The assembled assistant text is written to `context.jsonl`
    ///    and the `LlmResponse` event is emitted with its
    ///    `context_hash` filled in.
    /// 6. The summary is extracted from the response text via
    ///    [`extract_summary_from_response`] and passed to
    ///    [`Self::seed_with_resumption_summary`], which emits the final
    ///    `SessionResumed` event and seeds the synthetic user/assistant
    ///    message pair into history.
    ///
    /// **Cancellation:** mirrors the TS contract — if the cancel token
    /// fires mid-stream, the method stops cleanly without emitting
    /// `TurnInterrupted` (resumption is not a user turn).
    ///
    /// **Errors:** if the provider yields a terminal `LlmError`, the
    /// stream emits `LlmError` and ends — no `LlmResponse`,
    /// no `SessionResumed`. Callers detect failure by the absence of
    /// `SessionResumed`.
    #[allow(clippy::too_many_lines)] // single async generator; mirrors send_message shape
    pub fn perform_resumption<'a>(
        &'a mut self,
        basis: String,
        resumed_from: String,
        name: Option<String>,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AgentItem> + Send + 'a>> {
        Box::pin(stream! {
            // -----------------------------------------------------------------
            // Step 1: persist the basis as a user context record.
            // (Not pushed onto in-memory history — matches TS.)
            // -----------------------------------------------------------------
            let basis_blocks = vec![ContentBlock::Text {
                text: basis.clone(),
            }];
            let user_hash = match self
                .context_store
                .append(Role::User, basis_blocks)
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

            // -----------------------------------------------------------------
            // Step 2: emit ResumingSession.
            // -----------------------------------------------------------------
            let resuming_ev = OmegaEvent::ResumingSession(ResumingSessionEvent {
                time: now_iso(),
                resumed_from: resumed_from.clone(),
                name: name.clone(),
                basis: basis.clone(),
            });
            let _ = self.event_store.append(&resuming_ev).await;
            yield AgentItem::event(resuming_ev);

            // -----------------------------------------------------------------
            // Step 3: build the resumption request.
            //
            // System prompt = RESUMPTION_SUMMARY_INSTRUCTIONS (verbatim TS).
            // Messages = [{ user, basis }] only — prior history is irrelevant
            //   because the basis already carries the carry-forward context.
            // -----------------------------------------------------------------
            let request = LlmRequest {
                model: RESUMPTION_MODEL.to_owned(),
                messages: vec![Message {
                    role: Role::User,
                    content: vec![ContentBlock::Text { text: basis.clone() }],
                }],
                system: Some(RESUMPTION_SUMMARY_INSTRUCTIONS.to_owned()),
                tools: Vec::new(),
                config: ModelConfig {
                    max_tokens: RESUMPTION_MAX_TOKENS,
                    temperature: None,
                    thinking_budget: None,
                },
                context_management: None,
            };

            // -----------------------------------------------------------------
            // Step 4: emit LlmCall.
            //   context_hashes = [user_basis_hash]
            //   cache_breakpoint_index = None (no caching for one-off call)
            // -----------------------------------------------------------------
            let request_bytes = serde_json::to_vec(&request)
                .map_or(0, |v| i64::try_from(v.len()).unwrap_or(i64::MAX));
            let call_ev = OmegaEvent::LlmCall(LlmCallEvent {
                time: now_iso(),
                url: ANTHROPIC_URL.to_owned(),
                model: RESUMPTION_MODEL.to_owned(),
                context_hashes: vec![user_hash.as_ref().to_owned()],
                cache_breakpoint_index: None,
                request_bytes,
                request_summary: None,
            });
            let _ = self.event_store.append(&call_ev).await;
            yield AgentItem::event(call_ev);

            // -----------------------------------------------------------------
            // Step 5: drain the provider stream.
            // -----------------------------------------------------------------
            let mut provider_stream = self.provider.stream(request);
            let mut text_buf = String::new();
            let mut thinking_buf = String::new();
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
                            OmegaEvent::LlmResponse(lr) => {
                                llm_response = Some(lr);
                            }
                            OmegaEvent::LlmRetry(retry) => {
                                text_buf.clear();
                                thinking_buf.clear();
                                let ev = OmegaEvent::LlmRetry(retry);
                                let _ = self.event_store.append(&ev).await;
                                yield AgentItem::event(ev);
                            }
                            other => {
                                // Resumption is a one-shot summarisation
                                // call without tools — ToolCalls would be a
                                // provider/server bug. Forward unmodified
                                // for traceability.
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

            // -----------------------------------------------------------------
            // Step 6: cancellation — mirror TS, clean stop, no TurnInterrupted.
            // -----------------------------------------------------------------
            if cancel.is_cancelled() {
                return;
            }

            // -----------------------------------------------------------------
            // Step 7: terminal stream error.
            // -----------------------------------------------------------------
            if let Some(err) = stream_error {
                let llm_err_ev = OmegaEvent::LlmError(LlmErrorEvent {
                    time: now_iso(),
                    url: ANTHROPIC_URL.to_owned(),
                    error: err.to_string(),
                    http_status: err.status(),
                });
                let _ = self.event_store.append(&llm_err_ev).await;
                yield AgentItem::event(llm_err_ev);
                return;
            }

            // -----------------------------------------------------------------
            // Step 8: provider must have emitted an LlmResponse.
            // -----------------------------------------------------------------
            let Some(mut lr) = llm_response else {
                let ae = OmegaEvent::AgentError(AgentErrorEvent {
                    time: now_iso(),
                    error: "Provider stream ended without LlmResponse".to_owned(),
                });
                let _ = self.event_store.append(&ae).await;
                yield AgentItem::event(ae);
                return;
            };

            // -----------------------------------------------------------------
            // Step 9: persist the assistant context record.
            // -----------------------------------------------------------------
            let assembled_text = std::mem::take(&mut text_buf);
            let mut assistant_blocks: Vec<ContentBlock> = Vec::new();
            if !thinking_buf.is_empty() {
                assistant_blocks.push(ContentBlock::Thinking {
                    thinking: std::mem::take(&mut thinking_buf),
                    signature: None,
                });
            }
            if !assembled_text.is_empty() {
                assistant_blocks.push(ContentBlock::Text {
                    text: assembled_text.clone(),
                });
            }
            let assistant_hash = match self
                .context_store
                .append(Role::Assistant, assistant_blocks)
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

            // -----------------------------------------------------------------
            // Step 10: emit LlmResponse with hash filled.
            // -----------------------------------------------------------------
            lr.context_hash = assistant_hash.as_ref().to_owned();
            let response_ev = OmegaEvent::LlmResponse(lr);
            let _ = self.event_store.append(&response_ev).await;
            yield AgentItem::event(response_ev);

            // -----------------------------------------------------------------
            // Step 11: extract summary, seed history, emit SessionResumed.
            // -----------------------------------------------------------------
            let summary = extract_summary_from_response(&assembled_text);
            match self
                .seed_with_resumption_summary(summary, resumed_from)
                .await
            {
                Ok(ev) => yield AgentItem::event(ev),
                Err(e) => {
                    let ae = OmegaEvent::AgentError(AgentErrorEvent {
                        time: now_iso(),
                        error: format!("context_store append failed: {e}"),
                    });
                    let _ = self.event_store.append(&ae).await;
                    yield AgentItem::event(ae);
                }
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
