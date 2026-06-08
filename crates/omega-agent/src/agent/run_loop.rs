// ---------------------------------------------------------------------------
// Agent run-loop: `run` (session loop) and `drive_turn` (one coding turn).
// ---------------------------------------------------------------------------

use std::collections::{BTreeMap, HashMap};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use async_stream::stream;
use futures::stream::{FuturesUnordered, Stream, StreamExt};
use omega_core::{AgentItem, ContentBlock, LlmError, LlmRequest, Message, ModelConfig, Role};
use omega_tools::{ToolCtx, execute_tool, tool_definitions};
use omega_types::events::{
    AgentErrorEvent, ContextCompactedEvent, HarnessRecoveryKind, LlmCallEvent, LlmErrorEvent,
    LlmResponseEndedEvent, LlmResponseStartedEvent, TextBlockEvent, ThinkingBlockEvent,
    ToolCallEvent, ToolResultEvent, ToolUseBlockEvent, TurnEndEvent, TurnHaltedEvent,
    TurnInterruptedEvent, TurnResumedEvent,
};
use omega_types::{InterruptReason, OmegaEvent, StreamSignal, TurnMetrics};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::config::{cap_effort_for_model, max_output_tokens_for_model};
use crate::controls::TurnGuard;
use crate::error_classify::{is_context_too_long, is_invalid_tool_json};
use crate::input_queue::{InboxSink, InputQueue};

use super::ANTHROPIC_URL;
use super::Agent;
use super::InputItem;
use super::context::{build_context_management, project_messages};
use super::stream_assembly::{
    BlockSlot, append_text_slot, append_thinking_slot, make_abandonment_closers,
    open_tool_use_slot, seal_text_slot, seal_thinking_slot, seal_tool_use_slot,
};
use super::util::{elide_request, extract_compaction_tokens, now_iso};

/// Maximum invalid-tool-JSON nudges per `send_message` call before we
/// give up and end the turn.  Mirrors the TS agent's
/// `feedbackOnExhaustion` cap.
const INVALID_TOOL_JSON_FEEDBACK_CAP: u32 = 2;

const INVALID_TOOL_JSON_NUDGE: &str = "Your previous response could not be parsed — the tool-call JSON had invalid escaping (likely unescaped newlines or quotes in a string argument). Please retry the same tool call, being extra careful with JSON string escaping.";

/// Continuation prompt injected as a new user message when the model returns
/// a response with zero content blocks.  Follows Anthropic's documented
/// handling:
/// <https://platform.claude.com/docs/en/build-with-claude/handling-stop-reasons>
/// ("Empty responses with `end_turn`")
///
/// The docs advise injecting a NEW user message rather than retrying with
/// the empty assistant turn, because "Claude already decided it's done".
const EMPTY_RESPONSE_CONTINUATION: &str = "Please continue.";

/// Cap on consecutive empty-response continuation injections.  If the model
/// returns more than this many back-to-back empty responses within a single
/// turn, the agent surfaces a [`TurnInterrupted`] error rather than looping
/// forever.
const EMPTY_RESPONSE_CAP: u32 = 3;

impl Agent {
    /// Run the persistent per-session agent loop (§15 Unified Input Model, U2).
    ///
    /// Borrows `&mut self` **once** for the whole session.  Both human input
    /// AND monitor output arrive through `inbox` (monitors via the attached
    /// [`MonitorSink`]); the loop gathers the next batch of [`InputItem`]s,
    /// drives one coding turn via [`Self::drive_turn`] (which also drains the
    /// inbox mid-cycle at Seam B), then parks by awaiting the inbox again.
    /// Parking on an empty inbox is the normal idle state.
    ///
    /// Termination: the loop ends when `cancel` (the run-level / session
    /// teardown token) fires; or, in headless mode, when the inbox is empty
    /// AND no monitor is still live (§15 park/terminate).  Interactive
    /// sessions simply wait.
    ///
    /// Abort: each turn installs its own per-block cancel token (via
    /// [`ControlHandle::reset_for_turn`] inside `drive_turn`, forwarded
    /// from the run-level `cancel`).  An abort cancels only the *current*
    /// turn — after it ends the loop returns to Gather/park rather than
    /// terminating.  Only the run-level `cancel` ends the whole loop.
    pub fn run<'a>(
        &'a mut self,
        inbox: InputQueue,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AgentItem> + Send + 'a>> {
        // §15 U2: attach the monitor-delivery sink so live monitors push
        // their stdout/stop straight onto THIS inbox — the same queue as
        // human input.  From here monitor stdout/stop flows through the
        // unified Gather/seam-drain path.
        //
        // §17 (Phase A): the same sink also carries STDERR — but stderr is
        // NON-PROJECTED, so the sink routes it straight to the agent's
        // `EventSink` (event-log + WS, never the inbox), stamped at the
        // instant the line is read.
        self.monitors.attach_sink(Arc::new(InboxSink::new(
            inbox.clone(),
            Arc::clone(&self.event_sink),
        )));
        Box::pin(stream! {
            loop {
                // ---- Gather (park on empty inbox) -----------------------
                // Parks on `inbox.pop()` for the FIRST item — zero CPU cost
                // while the queue is empty; the run-level `cancel` (session
                // teardown) ends the loop via `tokio::select!`.  A monitor
                // line or self-stop is enqueued as an `InputItem`, so it
                // wakes this park exactly like a human message — no separate
                // roster-changed select needed (§15: the §4 park-select is
                // gone).
                let first = tokio::select! {
                    () = cancel.cancelled() => return,
                    item = inbox.pop() => item,
                };
                // Drain any items that piled up behind the first so a whole
                // Gather lands as one batch (projection merges consecutive
                // role:user records into one API message — §15 batching).
                let mut items = vec![first];
                items.extend(inbox.drain_pending());

                // ---- Process one turn -----------------------------------
                // `drive_turn` installs its own per-turn cancel token
                // (forwarded from the run-level `cancel`) and TurnGuard, so an
                // abort cancels just this turn and the guard drops here.  The
                // inbox is handed in so the mid-cycle Seam-B drain can pull
                // monitor output that fires *during* the turn.
                {
                    let mut turn = self.drive_turn(items, inbox.clone(), cancel.clone());
                    while let Some(item) = turn.next().await {
                        yield item;
                    }
                }

                // ---- Park-vs-terminate (§15) ----------------------------
                // The turn ended (TurnEnd / abort → TurnInterrupted / error).
                // A run-level cancel terminates the session unconditionally.
                if cancel.is_cancelled() {
                    return;
                }
                // Idle == empty inbox.  Headless terminates only when the
                // queue is empty AND no monitor is still live (a live monitor
                // may yet produce output, so we must wait for it).  Interactive
                // simply loops back and parks on `pop()`.
                if self.config.headless
                    && inbox.snapshot().is_empty()
                    && self.monitors.live_count() == 0
                {
                    return;
                }
            }
        })
    }

    /// Drive one coding turn (one Gather→Process block of [`Self::run`]).
    ///
    /// Returns a stream of every event/signal produced by the agentic
    /// loop, ending at `TurnEnd` (clean), `TurnInterrupted` (abort), or an
    /// error.  It does **not** park — parking is `run`'s job.
    ///
    /// Cancellation: tripping `cancel` aborts in-flight tool calls and the
    /// LLM stream, then yields a `TurnInterrupted{reason: aborted}` event
    /// before the stream ends.
    #[allow(clippy::too_many_lines)] // single async generator; splitting requires plumbing yields through return types
    fn drive_turn<'a>(
        &'a mut self,
        items: Vec<InputItem>,
        inbox: InputQueue,
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
            // §17 (Phase A): snapshot model+effort at turn entry.  Every
            // `LlmRequest` issued by THIS turn uses the snapshot, so a
            // mid-turn model/effort change (recorded out-of-band via the
            // sink at click-time) does NOT leak into the in-flight turn — it
            // takes effect on the NEXT `drive_turn`.
            let turn_model = self.active_model();
            let turn_effort = self.active_effort();
            // One shared monitor manager for this turn: cloned into every
            // ToolCtx, drained at the two seams, and selected on while parked.
            let monitors = Arc::clone(&self.monitors);

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
                // Route through inject_dangling_tool_results so every
                // role:user context record has a backing OmegaEvent (A1
                // invariant — §15(a) of docs/monitors-design.html).
                match self.inject_dangling_tool_results(dangling).await {
                    Ok(events) => {
                        for ev in events {
                            yield AgentItem::event(ev);
                        }
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
            }

            // -----------------------------------------------------------------
            // Step 2: inject every gathered inbox item (§15 U2).
            // Each item — human OR monitor — routes through its own eventful
            // inject_* helper, so every role:user context record has exactly
            // one backing OmegaEvent (A1 invariant — §15(a)).  Batching is a
            // PROJECTION concern: `project_messages` merges the consecutive
            // role:user records into a single API message at request-build
            // time, so we do NOT batch into one god context append here.
            // -----------------------------------------------------------------

            for item in items {
                match self.inject_input_item(item).await {
                    Ok(ev) => yield AgentItem::event(ev),
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
            }

            // -----------------------------------------------------------------
            // Step 3: outer agentic loop.
            // -----------------------------------------------------------------
            let mut feedback_attempts: u32 = 0;
            // Consecutive empty-response continuations in this turn.
            // Reset after any non-empty LLM response.
            let mut empty_response_count: u32 = 0;
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

                let active_model = turn_model.clone();
                let active_effort = turn_effort.clone();
                let max_tokens = max_output_tokens_for_model(&active_model);
                let system_blocks: Vec<String> = self
                    .system_blocks
                    .iter()
                    .map(|b| b.content.clone())
                    .collect();
                let request = LlmRequest {
                    model: active_model.clone(),
                    messages: project_messages(&self.history),
                    system: Some(system_blocks),
                    tools: tool_definitions(&self.tool_selection),
                    config: ModelConfig {
                        max_tokens,
                        temperature: None,
                        thinking_budget: None,
                        adaptive_thinking: true,
                        effort: Some(
                            cap_effort_for_model(&active_effort, &active_model)
                                .to_owned(),
                        ),
                    },
                    context_management: Some(build_context_management()),
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
                    model: active_model.clone(),
                    context_hashes: self
                        .context_hashes
                        .iter()
                        .map(|h| h.as_ref().to_owned())
                        .collect(),
                    cache_breakpoint_index,
                    request_bytes,
                    request_summary: Some(elide_request(&request)),
                });
                let _ = self.event_store.append(&call_ev).await;
                yield AgentItem::event(call_ev);

                // --- Drain the provider stream -----------------------------
                let mut provider_stream = self.provider.stream(request);
                // SCHEMA-8 Phase 3 commit 3e: `slots` is now the sole
                // source of truth for text/thinking/tool_use blocks
                // that come via stream signals.  Index-ordered
                // assembly after `LlmResponse` (below) drains them
                // into `assistant_blocks`.
                let mut slots: BTreeMap<usize, BlockSlot> = BTreeMap::new();
                // `tool_uses` still collects legacy `OmegaEvent::ToolCall`
                // emissions from `MockProvider` scripts (goldens.rs,
                // internal.rs).  Removed in Phase 6 fixture refresh.
                let mut tool_uses: Vec<(String, String, Value)> = Vec::new();
                let mut llm_response: Option<LlmResponseEndedEvent> = None;
                let mut stream_error: Option<LlmError> = None;
                // Phase 2.0 (F11): token info extracted when compaction fires.
                let mut compacted_info: Option<(i64, i64, i64)> = None;
                // SCHEMA-8 Phase 3 commit 3b: opener/closer bracketing
                // each provider stream.  Cleared on `LlmRetry` mid-stream
                // so the retried stream gets its own opener; cleared
                // again after `LlmResponseEnded` so the next iteration's
                // first signal triggers a new opener.
                let mut response_started = false;

                while let Some(item) = provider_stream.next().await {
                    if cancel.is_cancelled() {
                        break;
                    }

                    // SCHEMA-8 Phase 3 commit 3b: emit `LlmResponseStarted`
                    // on the first item of a freshly-started provider
                    // stream.  Errors don't open a stream — the closer
                    // for an error path is `LlmResponseDiscarded`
                    // (commit 3d), and we need an opener for that to
                    // close.  So include `Err(_)` too.
                    if !response_started {
                        response_started = true;
                        let started = OmegaEvent::LlmResponseStarted(
                            LlmResponseStartedEvent { time: now_iso() },
                        );
                        let _ = self.event_store.append(&started).await;
                        yield AgentItem::event(started);
                    }

                    match item {
                        Ok(AgentItem::Signal(sig)) => {
                            // Block events to emit after the inner match
                            // completes its accumulator updates.  Used by
                            // the `*BlockComplete` arms below.
                            let mut block_event: Option<OmegaEvent> = None;
                            let forward = match &sig {
                                StreamSignal::Text { index, text } => {
                                    append_text_slot(&mut slots, *index, text);
                                    true
                                }
                                StreamSignal::Thinking { index, text } => {
                                    append_thinking_slot(&mut slots, *index, text);
                                    true
                                }
                                // Forward tool-use start and delta signals to
                                // the UI.  No slot effect here — the slot is
                                // sealed by `ToolUseBlockComplete` below.
                                StreamSignal::ToolUseBlockStart {
                                    index, tool_use_id, name,
                                } => {
                                    // Mint the Omega tool_call_id NOW so
                                    // it's available for any partial
                                    // abandonment event before the
                                    // matching Complete arrives.
                                    open_tool_use_slot(
                                        &mut slots,
                                        *index,
                                        tool_use_id.clone(),
                                        name.clone(),
                                    );
                                    true
                                }
                                StreamSignal::ToolInput { .. } => true,
                                StreamSignal::ThinkingBlockComplete { index, signature } => {
                                    // SCHEMA-8 Phase 3e: read the
                                    // assembled thinking text back from
                                    // the slot after sealing.  Replaces
                                    // the legacy `current_thinking`
                                    // snapshot.
                                    seal_thinking_slot(&mut slots, *index, signature.clone());
                                    let thinking_text = match slots.get(index) {
                                        Some(BlockSlot::Thinking { thinking, .. }) => {
                                            thinking.clone()
                                        }
                                        _ => String::new(),
                                    };
                                    block_event = Some(OmegaEvent::ThinkingBlock(
                                        ThinkingBlockEvent {
                                            time: now_iso(),
                                            thinking: thinking_text,
                                            signature: Some(signature.clone()),
                                            partial: false,
                                        },
                                    ));
                                    false // internal signal, not forwarded to UI
                                }
                                StreamSignal::TextBlockComplete { index, text } => {
                                    seal_text_slot(&mut slots, *index);
                                    block_event = Some(OmegaEvent::TextBlock(
                                        TextBlockEvent {
                                            time: now_iso(),
                                            text: text.clone(),
                                            partial: false,
                                        },
                                    ));
                                    false
                                }
                                StreamSignal::ToolUseBlockComplete {
                                    index, tool_use_id, name, input,
                                } => {
                                    // SCHEMA-8 Phase 3e: signals only
                                    // populate `slots`.  The legacy
                                    // `tool_uses` Vec is now reserved
                                    // for `OmegaEvent::ToolCall`
                                    // emissions (MockProvider scripts);
                                    // both feeds merge into
                                    // `combined_tool_uses` at assembly
                                    // time.
                                    let tool_call_id = seal_tool_use_slot(
                                        &mut slots,
                                        *index,
                                        tool_use_id.clone(),
                                        name.clone(),
                                        input.clone(),
                                    );
                                    block_event = Some(OmegaEvent::ToolUseBlock(
                                        ToolUseBlockEvent {
                                            time: now_iso(),
                                            tool_call_id,
                                            tool_use_id: tool_use_id.clone(),
                                            name: name.clone(),
                                            input: input.clone(),
                                            partial: false,
                                        },
                                    ));
                                    false
                                }
                            };
                            if let Some(be) = block_event {
                                let _ = self.event_store.append(&be).await;
                                yield AgentItem::event(be);
                            }
                            if forward {
                                yield AgentItem::Signal(sig);
                            }
                        }
                        Ok(AgentItem::Event(boxed)) => {
                            let event = *boxed;
                            match event {
                                OmegaEvent::ToolCall(tc) => {
                                    // MockProvider scripts emit ToolCall
                                    // directly without going through
                                    // streaming signals, so there is no
                                    // LLM-issued tool_use_id available.
                                    // The Omega tool_call_id is reused
                                    // as the tool_use_id for these
                                    // mock-only paths so the protocol
                                    // FK in ContentBlock::ToolResult
                                    // still resolves.
                                    tool_uses.push((tc.tool_call_id, tc.name, tc.input));
                                    // Re-emitted later with assistant_hash filled.
                                }
                                OmegaEvent::LlmResponseEnded(lr) => {
                                    // Phase 6.5: detect compaction via
                                    // usage.iterations. When a
                                    // type=="compaction" iteration is present,
                                    // clear history so the next turn starts
                                    // from a fresh baseline.
                                    // Phase 2.0 (F11): extract token info
                                    // for the ContextCompacted event.
                                    if let Some(iters) = lr.usage.iterations.as_ref()
                                        && iters.iter().any(|it| it.iteration_type == "compaction")
                                    {
                                        self.history.clear();
                                        self.context_hashes.clear();
                                        compacted_info =
                                            Some(extract_compaction_tokens(iters));
                                    }
                                    llm_response = Some(lr);
                                }
                                OmegaEvent::LlmRetry(retry) => {
                                    // RetryingProvider has just slept and is
                                    // about to re-issue the call; throw away
                                    // any partial assistant content we
                                    // accumulated and forward the event so the
                                    // UI can roll back.  In Phase 3e the
                                    // slot drain inside
                                    // `make_abandonment_closers` is the
                                    // only state to reset.
                                    // SCHEMA-8 Phase 3 commit 3d: emit
                                    // partial-block events for any
                                    // unsealed slots, then
                                    // `LlmResponseDiscarded`, BEFORE the
                                    // retry event itself.  Replaces the
                                    // implicit fragment tracking that
                                    // retry.rs::track_fragment used to do.
                                    if response_started {
                                        for closer in make_abandonment_closers(
                                            std::mem::take(&mut slots),
                                        ) {
                                            let _ = self.event_store.append(&closer).await;
                                            yield AgentItem::event(closer);
                                        }
                                    }
                                    // SCHEMA-8 Phase 3 commit 3b: the
                                    // retried stream gets its own
                                    // `LlmResponseStarted` opener.
                                    response_started = false;
                                    let ev = OmegaEvent::LlmRetry(retry);
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
                    // SCHEMA-8 Phase 3 commit 3d: closer pair before
                    // the interrupt. `response_started` does not need
                    // resetting because we `return` immediately.
                    if response_started {
                        for closer in make_abandonment_closers(std::mem::take(&mut slots)) {
                            let _ = self.event_store.append(&closer).await;
                            yield AgentItem::event(closer);
                        }
                    }
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
                    // SCHEMA-8 Phase 3 commit 3d: closer pair before
                    // the LlmError event.  After the error path,
                    // control either falls through to the nudge
                    // retry below (whose own next-turn iteration
                    // re-declares response_started=false) or returns
                    // — either way no further read of
                    // `response_started` happens in this scope.
                    if response_started {
                        for closer in make_abandonment_closers(std::mem::take(&mut slots)) {
                            let _ = self.event_store.append(&closer).await;
                            yield AgentItem::event(closer);
                        }
                    }
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
                        match self
                            .inject_harness_recovery(
                                HarnessRecoveryKind::InvalidToolJson,
                                INVALID_TOOL_JSON_NUDGE,
                            )
                            .await
                        {
                            Ok(ev) => {
                                yield AgentItem::event(ev);
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

                // --- Should have an LlmResponseEnded now -------------------
                let Some(mut lr) = llm_response else {
                    let ae = OmegaEvent::AgentError(AgentErrorEvent {
                        time: now_iso(),
                        error: "Provider stream ended without LlmResponseEnded".to_owned(),
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
                // Assemble `assistant_blocks` from `slots` in BTreeMap
                // (= insertion-index) order so interleaved thinking/text
                // blocks land in the API content-block order the model
                // emitted.  Legacy `OmegaEvent::ToolCall` entries (still
                // emitted by MockProvider scripts — removed Phase 6
                // fixture refresh) are appended after the slot blocks.
                // (tool_call_id, tool_use_id, name, input) — the two ids
                // travel together: tool_call_id flows through events,
                // tool_use_id flows into ContentBlock::ToolResult
                // tool_use_id (Anthropic protocol FK).
                let mut combined_tool_uses: Vec<(String, String, String, Value)> = Vec::new();
                let mut assistant_blocks: Vec<ContentBlock> = Vec::new();
                for (_, slot) in std::mem::take(&mut slots) {
                    match slot {
                        BlockSlot::Text { text, .. } if !text.is_empty() => {
                            assistant_blocks.push(ContentBlock::Text { text });
                        }
                        BlockSlot::Thinking {
                            thinking, signature, ..
                        } => {
                            assistant_blocks.push(ContentBlock::Thinking {
                                thinking,
                                signature,
                            });
                        }
                        BlockSlot::ToolUse {
                            tool_call_id, tool_use_id, name, input, ..
                        } => {
                            combined_tool_uses.push((
                                tool_call_id,
                                tool_use_id.clone(),
                                name.clone(),
                                input.clone(),
                            ));
                            assistant_blocks.push(ContentBlock::ToolUse {
                                id: tool_use_id,
                                name,
                                input,
                            });
                        }
                        // Skip empty Text slots (rare — sealed without
                        // any deltas) so they don't bloat the record.
                        BlockSlot::Text { .. } => {}
                    }
                }
                // Legacy MockProvider path: no LLM tool_use_id available;
                // reuse the Omega tool_call_id as the protocol-layer id.
                for (id, name, input) in &tool_uses {
                    combined_tool_uses.push((
                        id.clone(),
                        id.clone(),
                        name.clone(),
                        input.clone(),
                    ));
                    assistant_blocks.push(ContentBlock::ToolUse {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                    });
                }

                // Extract stop_reason before any ownership transfer of lr.
                let stop_reason = lr.stop_reason.clone();

                // ----------------------------------------------------------
                // Empty-response guard (documented Anthropic behaviour):
                // Claude occasionally returns a response with zero content
                // blocks (no text, no thinking, no tool_use), with stop_reason
                // "end_turn" OR "tool_use".  Per:
                // https://platform.claude.com/docs/en/build-with-claude/handling-stop-reasons
                //
                // Detection: `assistant_blocks.is_empty()`, regardless of
                //             stop_reason.
                // Handling:  inject a NEW user continuation message; do NOT
                //            persist the empty assistant turn to context
                //            ("Claude already decided it's done").
                // Decision:  the continuation is emitted as a visible user
                //            turn (option a — simplest, matches the docs
                //            example; design note at §14 in
                //            docs/monitors-design.html).
                // ----------------------------------------------------------
                if assistant_blocks.is_empty() {
                    empty_response_count += 1;

                    // Still accumulate token usage and emit LlmResponseEnded
                    // so the event log records the empty response (forensics).
                    // context_hash is left empty — no assistant record is
                    // persisted.
                    lr.context_hash = String::new();
                    tot_input += lr.usage.input_tokens;
                    tot_output += lr.usage.output_tokens;
                    tot_cache_creation +=
                        lr.usage.cache_creation_input_tokens.unwrap_or(0);
                    tot_cache_read += lr.usage.cache_read_input_tokens.unwrap_or(0);
                    // Phase 2.0 (F11): emit ContextCompacted before
                    // LlmResponseEnded if compaction fired (unlikely with an
                    // empty response, but handled defensively).
                    if let Some((tokens_before, tokens_after, summary_tokens)) =
                        compacted_info.take()
                    {
                        let cc_ev = OmegaEvent::ContextCompacted(ContextCompactedEvent {
                            time: now_iso(),
                            tokens_before,
                            tokens_after,
                            summary_tokens,
                        });
                        let _ = self.event_store.append(&cc_ev).await;
                        yield AgentItem::event(cc_ev);
                    }
                    let ended_ev = OmegaEvent::LlmResponseEnded(lr);
                    let _ = self.event_store.append(&ended_ev).await;
                    yield AgentItem::event(ended_ev);

                    if empty_response_count > EMPTY_RESPONSE_CAP {
                        let ae = OmegaEvent::AgentError(AgentErrorEvent {
                            time: now_iso(),
                            error: format!(
                                "Model returned {empty_response_count} consecutive \
                                 empty responses (cap={EMPTY_RESPONSE_CAP}); ending \
                                 turn to avoid an infinite loop."
                            ),
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

                    // Inject the continuation as a harness-recovery event
                    // (§15 — every role:user record must have a backing event).
                    // The role:user projection ensures the model still sees the
                    // continuation prompt; the HarnessRecovery event in
                    // events.jsonl records *why* it appeared.
                    match self
                        .inject_harness_recovery(
                            HarnessRecoveryKind::EmptyResponseContinuation,
                            EMPTY_RESPONSE_CONTINUATION,
                        )
                        .await
                    {
                        Ok(ev) => {
                            yield AgentItem::event(ev);
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
                // Phase 2.0 (F11): emit ContextCompacted immediately
                // before LlmResponseEnded when compaction was detected.
                if let Some((tokens_before, tokens_after, summary_tokens)) =
                    compacted_info.take()
                {
                    let cc_ev = OmegaEvent::ContextCompacted(ContextCompactedEvent {
                        time: now_iso(),
                        tokens_before,
                        tokens_after,
                        summary_tokens,
                    });
                    let _ = self.event_store.append(&cc_ev).await;
                    yield AgentItem::event(cc_ev);
                }
                let ended_ev = OmegaEvent::LlmResponseEnded(lr);
                let _ = self.event_store.append(&ended_ev).await;
                yield AgentItem::event(ended_ev);

                // --- Tool dispatch ----------------------------------------
                if stop_reason == "tool_use" && !combined_tool_uses.is_empty() {
                    // Emit ToolCall events with assistant_hash filled in.
                    // tool_call_id was minted at ToolUseBlockStart and
                    // already lives in `combined_tool_uses`.
                    for (tool_call_id, _tool_use_id, name, input) in &combined_tool_uses {
                        let tc = OmegaEvent::ToolCall(ToolCallEvent {
                            time: now_iso(),
                            tool_call_id: tool_call_id.clone(),
                            name: name.clone(),
                            input: input.clone(),
                            context_hash: assistant_hash.as_ref().to_owned(),
                        });
                        let _ = self.event_store.append(&tc).await;
                        yield AgentItem::event(tc);
                    }

                    // Concurrent dispatch — clone the call descriptor
                    // into each future so they don't borrow self.
                    let session_cache_dir = self.config.session_dir.join("cache");
                    let system_prompt_paths = Arc::clone(&self.system_prompt_paths);
                    // Capture the tool selection before the async block so
                    // each tool context can own a clone without borrowing
                    // self.
                    let self_tool_selection = self.tool_selection.clone();
                    // Pass the python_repl Arc into the tool context when
                    // `python_repl` is in the selection.  The outer
                    // Option<Arc<...>> is None otherwise so execute_tool
                    // knows the tool is unavailable and can return a clear
                    // error.
                    let python_repl_opt = if self
                        .tool_selection
                        .iter()
                        .any(|n| n == "python_repl")
                    {
                        Some(Arc::clone(&self.python_repl))
                    } else {
                        None
                    };
                    let mut futures: FuturesUnordered<_> = combined_tool_uses
                        .iter()
                        .enumerate()
                        .map(|(i, (tool_call_id, _tool_use_id, name, input))| {
                            let tool_call_id = tool_call_id.clone();
                            let name = name.clone();
                            let input = input.clone();
                            let cancel_clone = cancel.clone();
                            let cache_dir = session_cache_dir.clone();
                            let system_prompt_paths = Arc::clone(&system_prompt_paths);
                            let python_repl = python_repl_opt.clone();
                            let tool_selection = self_tool_selection.clone();
                            let monitors_for_ctx = Arc::clone(&monitors);
                            async move {
                                let start = Instant::now();
                                let ctx = ToolCtx {
                                    cache_dir,
                                    tool_call_id: tool_call_id.clone(),
                                    system_prompt_paths,
                                    python_repl,
                                    tool_selection,
                                    // Phase 2 cutover: every ToolCtx shares the
                                    // agent's single Arc<MonitorManager> so the
                                    // monitor / stop_monitor tools enqueue on the
                                    // same instance the loop drains at its seams.
                                    monitors: Some(monitors_for_ctx),
                                };
                                let res =
                                    execute_tool(&name, input, Some(&cancel_clone), Some(&ctx)).await;
                                let elapsed = start.elapsed();
                                (i, tool_call_id, name, res, elapsed)
                            }
                        })
                        .collect();

                    // Tool dispatches complete in non-deterministic order;
                    // collect keyed by tool_call_id (Omega layer), then
                    // re-order by the original tool_use sequence when
                    // assembling the user message so the tool_results
                    // land in the same shape the model emitted.
                    let mut by_call_id: HashMap<String, (String, bool)> = HashMap::new();
                    while let Some((_idx, tool_call_id, name, res, elapsed)) = futures.next().await {
                        // Emit side-band events (e.g. python_repl bootstrap)
                        // before the ToolResultEvent so the log tells the
                        // full story in chronological order.
                        for ev in &res.extra_events {
                            let _ = self.event_store.append(ev).await;
                            yield AgentItem::event(ev.clone());
                        }
                        let duration_ms = i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX);
                        let tr = OmegaEvent::ToolResult(ToolResultEvent {
                            time: now_iso(),
                            tool_call_id: tool_call_id.clone(),
                            name,
                            is_error: res.is_error,
                            duration_ms,
                            output: res.content.clone(),
                        });
                        let _ = self.event_store.append(&tr).await;
                        yield AgentItem::event(tr);
                        by_call_id.insert(tool_call_id, (res.content, res.is_error));
                    }

                    let result_blocks: Vec<ContentBlock> = combined_tool_uses
                        .iter()
                        .map(|(tool_call_id, tool_use_id, _, _)| {
                            // FuturesUnordered always produces one entry per
                            // pushed future, so the lookup cannot miss.
                            // If it ever does, fall back to a synthetic
                            // error result rather than panicking the agent.
                            let (content, is_error) = by_call_id.remove(tool_call_id).unwrap_or_else(|| {
                                ("tool dispatch produced no result".to_owned(), true)
                            });
                            // tool_use_id (LLM-issued) is what Anthropic
                            // expects on the wire to pair the result
                            // back to the originating tool_use block.
                            ContentBlock::ToolResult {
                                tool_use_id: tool_use_id.clone(),
                                content,
                                is_error,
                            }
                        })
                        .collect();

                    // Route through inject_tool_results_batch; the
                    // individual ToolResult events emitted above are the
                    // backing events for this context record (A1 — §15(a)).
                    if let Err(e) = self.inject_tool_results_batch(result_blocks).await {
                        let ev = OmegaEvent::AgentError(AgentErrorEvent {
                            time: now_iso(),
                            error: format!("context_store append failed: {e}"),
                        });
                        let _ = self.event_store.append(&ev).await;
                        yield AgentItem::event(ev);
                        return;
                    }

                    // -----------------------------------------------------
                    // Halt seam (§15 Unified Input Model, U3).  Fires only
                    // after the current tool batch's results are appended,
                    // so the next LlmCall sees a complete tool_use/
                    // tool_result pair.  Halt = "stop advancing at this seam
                    // and WAIT": instead of continuing the block, park here
                    // until the user resumes — either by queuing a steering
                    // message (woke via inbox.pop, injected, continue) or by
                    // an explicit request_resume (continue with no input) —
                    // or aborts (cancel wins).  This replaces the retired
                    // pause-for-injection machinery; queuing a message is
                    // now the ONLY interjection path (Seam B drain below).
                    // -----------------------------------------------------
                    if self.controls.take_halt_request() {
                        let halted_ev = OmegaEvent::TurnHalted(TurnHaltedEvent {
                            time: now_iso(),
                        });
                        let _ = self.event_store.append(&halted_ev).await;
                        yield AgentItem::event(halted_ev);

                        // Park.  `enter/exit_halt_wait` flips `suspended` so
                        // request_resume / request_abort know to fire a wake.
                        // A resume that beat the park is honoured up-front
                        // (take_resume_request).  The select parks until a
                        // queued steering message, an explicit resume
                        // (notify), or a cancel (abort) arrives.  Dropping
                        // the losing `inbox.pop()` future is safe: the Seam B
                        // drain immediately below picks up anything queued.
                        self.controls.enter_halt_wait();
                        let resumed_item: Option<InputItem> =
                            if self.controls.take_resume_request() {
                                None
                            } else {
                                tokio::select! {
                                    () = self.controls.notify().notified() => None,
                                    () = cancel.cancelled() => None,
                                    item = inbox.pop() => Some(item),
                                }
                            };
                        // Clear a resume flag that arrived via notify so it
                        // cannot leak into a later halt within this turn.
                        let _ = self.controls.take_resume_request();
                        self.controls.exit_halt_wait();

                        // Abort wins over resume if both fired — a click-
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

                        // A queued steering message woke the park → inject it
                        // (its own eventful inject_* call, A1 — §15(a))
                        // before continuing.  Any further items queued behind
                        // it are picked up by the Seam B drain below.
                        if let Some(item) = resumed_item {
                            match self.inject_input_item(item).await {
                                Ok(ev) => yield AgentItem::event(ev),
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
                        }

                        let resumed_ev = OmegaEvent::TurnResumed(TurnResumedEvent {
                            time: now_iso(),
                        });
                        let _ = self.event_store.append(&resumed_ev).await;
                        yield AgentItem::event(resumed_ev);
                    }

                    // ---- Seam B (§3/§15 U2): mid-cycle inbox drain. ------
                    // After each tool_result batch lands, drain EVERYTHING
                    // currently pending in the inbox and inject it before the
                    // next model call.  This is the heart of U2: a monitor
                    // that fires while the agent is working is injected
                    // between a tool_result and the next model call — NOT
                    // held to end_turn.  Each item gets its own eventful
                    // inject_* call (A1 — §15(a)); projection merges the
                    // consecutive role:user records into one API message.
                    let mid_cycle = inbox.drain_pending();
                    let mut seam_b_failed = false;
                    for item in mid_cycle {
                        match self.inject_input_item(item).await {
                            Ok(ev) => yield AgentItem::event(ev),
                            Err(e) => {
                                let ev = OmegaEvent::AgentError(AgentErrorEvent {
                                    time: now_iso(),
                                    error: format!("context_store append failed: {e}"),
                                });
                                let _ = self.event_store.append(&ev).await;
                                yield AgentItem::event(ev);
                                seam_b_failed = true;
                                break;
                            }
                        }
                    }
                    if seam_b_failed {
                        return;
                    }
                    // Monitor stderr is now emitted out-of-band the instant
                    // the reader reads a line (§17, Phase A): it flows through
                    // the [`EventSink`] (event-log + WS), NOT the run-loop
                    // stream — so there is no stderr drain at this seam.
                    empty_response_count = 0; // non-empty response received
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

                // ---- Seam A (§3/§4/§15 U1): the turn ends here.  The former
                // park loop (monitor drain + select over monitor-queue /
                // roster / human-input) was removed in U1.  Parking is now
                // done by `Agent::run`, which awaits the inbox after this
                // stream completes.  Monitor delivery is dark until U2;
                // monitor spawn/reap is unaffected (Drop and
                // shutdown_and_log_monitors still fire at session end).
                return;
            }
        })
    }
}
