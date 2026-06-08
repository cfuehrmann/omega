// ---------------------------------------------------------------------------
// Agent resumption: `perform_resumption` (one-shot summarisation call).
// ---------------------------------------------------------------------------

use std::collections::BTreeMap;
use std::pin::Pin;

use async_stream::stream;
use futures::stream::{Stream, StreamExt};
use omega_core::{AgentItem, ContentBlock, LlmError, LlmRequest, Message, ModelConfig, Role};
use omega_types::events::{
    AgentErrorEvent, ContextCompactedEvent, LlmCallEvent, LlmErrorEvent, LlmResponseEndedEvent,
    LlmResponseStartedEvent, ResumingSessionEvent, TextBlockEvent, ThinkingBlockEvent,
    ToolUseBlockEvent,
};
use omega_types::{OmegaEvent, StreamSignal};
use tokio_util::sync::CancellationToken;

use crate::config::cap_effort_for_model;
use crate::session_resume::{
    RESUMPTION_EFFORT, RESUMPTION_MAX_TOKENS, RESUMPTION_MODEL, RESUMPTION_SUMMARY_INSTRUCTIONS,
    extract_summary_from_response,
};

use super::ANTHROPIC_URL;
use super::Agent;
use super::stream_assembly::{
    BlockSlot, append_text_slot, append_thinking_slot, make_abandonment_closers,
    open_tool_use_slot, seal_text_slot, seal_thinking_slot, seal_tool_use_slot,
};
use super::util::{elide_request, extract_compaction_tokens, now_iso};

impl Agent {
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
            // Step 1: emit ResumingSession (the backing event for the
            // context record below — A1 invariant, §15(a)).
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
            // Step 2: persist the basis as a user context record.
            // (Not pushed onto in-memory history — matches TS.)
            // Backed by the ResumingSession event emitted above.
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
                system: Some(vec![RESUMPTION_SUMMARY_INSTRUCTIONS.to_owned()]),
                tools: Vec::new(),
                config: ModelConfig {
                    max_tokens: RESUMPTION_MAX_TOKENS,
                    temperature: None,
                    thinking_budget: None,
                    adaptive_thinking: true,
                    effort: Some(
                        cap_effort_for_model(RESUMPTION_EFFORT, RESUMPTION_MODEL).to_owned(),
                    ),
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
                request_summary: Some(elide_request(&request)),
            });
            let _ = self.event_store.append(&call_ev).await;
            yield AgentItem::event(call_ev);

            // -----------------------------------------------------------------
            // Step 5: drain the provider stream.
            // -----------------------------------------------------------------
            let mut provider_stream = self.provider.stream(request);
            // SCHEMA-8 Phase 3 commit 3e: `slots` is the sole source of
            // truth for text/thinking blocks; resumption never emits
            // tool_use, but the slot machinery is shared with
            // `send_message` and handles the type uniformly.
            let mut slots: BTreeMap<usize, BlockSlot> = BTreeMap::new();
            let mut llm_response: Option<LlmResponseEndedEvent> = None;
            let mut stream_error: Option<LlmError> = None;
            // Phase 2.0 (F11): token info extracted when compaction fires.
            let mut compacted_info: Option<(i64, i64, i64)> = None;
            // SCHEMA-8 Phase 3 commit 3b: opener/closer bracketing
            // each provider stream (same pattern as `send_message`).
            let mut response_started = false;

            while let Some(item) = provider_stream.next().await {
                if cancel.is_cancelled() {
                    break;
                }
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
                            // the UI.  ToolUseBlockStart mints the
                            // tool_call_id; ToolInput has no slot effect.
                            StreamSignal::ToolUseBlockStart {
                                index, tool_use_id, name,
                            } => {
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
                                seal_thinking_slot(&mut slots, *index, signature.clone());
                                let thinking_text = match slots.get(index) {
                                    Some(BlockSlot::Thinking { thinking, .. }) => thinking.clone(),
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
                                false
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
                            StreamSignal::ToolUseBlockComplete { index, tool_use_id, name, input } => {
                                // Resumption-summary call must not
                                // request tools, but mirror the seal so
                                // any provider misbehaviour is caught by
                                // commit 3d's abandonment closers.
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
                            OmegaEvent::LlmResponseEnded(lr) => {
                                // Phase 6.5: detect compaction via
                                // usage.iterations (same as send_message).
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
                                // SCHEMA-8 Phase 3 commit 3e: slot drain
                                // inside `make_abandonment_closers` is
                                // the only state reset on retry.
                                // SCHEMA-8 Phase 3 commit 3d: emit
                                // partial-block events + LlmResponseDiscarded
                                // closer BEFORE the retry event.
                                if response_started {
                                    for closer in make_abandonment_closers(
                                        std::mem::take(&mut slots),
                                    ) {
                                        let _ = self.event_store.append(&closer).await;
                                        yield AgentItem::event(closer);
                                    }
                                }
                                // SCHEMA-8 Phase 3 commit 3b: retried
                                // stream gets its own opener.
                                response_started = false;
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
                // SCHEMA-8 Phase 3 commit 3d: closer pair before the
                // silent cancel return. perform_resumption deliberately
                // does NOT emit `TurnInterrupted` (clean-stop semantics
                // mirrored from the TS reference), but the closer pair
                // still fires so consumers observe an abandoned
                // response stream.  No `response_started=false`
                // needed — we `return` next.
                if response_started {
                    for closer in make_abandonment_closers(std::mem::take(&mut slots)) {
                        let _ = self.event_store.append(&closer).await;
                        yield AgentItem::event(closer);
                    }
                }
                return;
            }

            // -----------------------------------------------------------------
            // Step 7: terminal stream error.
            // -----------------------------------------------------------------
            if let Some(err) = stream_error {
                // SCHEMA-8 Phase 3 commit 3d: closer pair.  No
                // `response_started=false` needed — we `return` after
                // surfacing LlmError.
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
                return;
            }

            // -----------------------------------------------------------------
            // Step 8: provider must have emitted an LlmResponseEnded.
            // -----------------------------------------------------------------
            let Some(mut lr) = llm_response else {
                let ae = OmegaEvent::AgentError(AgentErrorEvent {
                    time: now_iso(),
                    error: "Provider stream ended without LlmResponseEnded".to_owned(),
                });
                let _ = self.event_store.append(&ae).await;
                yield AgentItem::event(ae);
                return;
            };

            // -----------------------------------------------------------------
            // Step 9: persist the assistant context record.
            // -----------------------------------------------------------------
            // Assemble from `slots` in index order.  Resumption never emits
            // tool_use blocks, so any `ToolUse` slot here is a
            // provider/server bug; we still include it for traceability.
            // `assembled_text` is needed by `extract_summary_from_response`.
            let mut assistant_blocks: Vec<ContentBlock> = Vec::new();
            let mut lr_text_parts: Vec<String> = Vec::new();
            for (_, slot) in std::mem::take(&mut slots) {
                match slot {
                    BlockSlot::Text { text, .. } if !text.is_empty() => {
                        lr_text_parts.push(text.clone());
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
                        tool_use_id, name, input, ..
                    } => {
                        // Wire-format `ContentBlock::ToolUse.id` is the
                        // LLM-issued identifier (Anthropic protocol).
                        assistant_blocks.push(ContentBlock::ToolUse {
                            id: tool_use_id,
                            name,
                            input,
                        });
                    }
                    BlockSlot::Text { .. } => {}
                }
            }
            let assembled_text = lr_text_parts.join("");
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
            // Step 10: emit LlmResponseEnded with context_hash filled.
            //          Phase 2.0 (F11): precede with ContextCompacted if
            //          compaction was detected.
            // -----------------------------------------------------------------
            lr.context_hash = assistant_hash.as_ref().to_owned();
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
