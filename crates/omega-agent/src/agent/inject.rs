// ---------------------------------------------------------------------------
// Monitor / input injection methods.
// ---------------------------------------------------------------------------

use std::sync::Arc;

use omega_core::{ContentBlock, Message, Role};
use omega_tools::MonitorManager;
use omega_types::OmegaEvent;
use omega_types::events::{
    HarnessRecoveryEvent, HarnessRecoveryKind, MonitorDeliveryEvent, MonitorDeliveryItem,
    MonitorStartedEvent, MonitorStopReason, MonitorStoppedEvent, ToolResultEvent, UserMessageEvent,
};

use super::Agent;
use super::InputItem;
use super::context::{format_monitor_lines, format_monitor_stopped};
use super::util::{gen_call_id, now_iso};

/// Error text injected for dangling tool-use blocks (no matching result).
const DANGLING_TOOL_USE_RESULT: &str =
    "[not executed: previous turn was interrupted before this tool ran]";

impl Agent {
    // -----------------------------------------------------------------------
    // Phase 0 — Async Monitors: event-log injection + context projection
    // -----------------------------------------------------------------------

    /// Append a `MonitorStarted` event to `events.jsonl`.
    ///
    /// **Not projected into context** (§12 locked decision): the `monitor()`
    /// tool result already informs the agent; this event exists for
    /// log attribution and causality tracing only.
    ///
    /// # Errors
    /// Returns an error if the event store write fails.
    pub async fn append_monitor_started(
        &mut self,
        id: String,
        description: String,
        command: String,
    ) -> omega_store::Result<()> {
        let ev = OmegaEvent::MonitorStarted(MonitorStartedEvent {
            id,
            description,
            command,
            time: now_iso(),
        });
        self.event_store.append(&ev).await
    }

    /// Inject a `MonitorDelivery` into the event log and context.
    ///
    /// Projects to `role: user` in the LLM context.  Consecutive user-role
    /// entries in history are merged into one API message by
    /// [`project_messages`] at the point the LLM request is built.
    ///
    /// Each `MonitorDeliveryItem` in `items` becomes a `Text` content block.
    ///
    /// # Errors
    /// Returns an error if the event store or context store write fails.
    pub async fn inject_monitor_delivery(
        &mut self,
        items: Vec<MonitorDeliveryItem>,
    ) -> omega_store::Result<OmegaEvent> {
        let ev = OmegaEvent::MonitorDelivery(MonitorDeliveryEvent {
            time: now_iso(),
            items: items.clone(),
        });
        self.event_store.append(&ev).await?;

        // Build one Text content block per monitor item.
        let new_blocks: Vec<ContentBlock> = items
            .iter()
            .map(|item| ContentBlock::Text {
                text: format_monitor_lines(&item.monitor_id, &item.lines),
            })
            .collect();

        // Append to context.jsonl and in-memory history as a role:user
        // message.  project_messages() merges consecutive role:user entries
        // when building the LlmRequest so the API always receives a
        // well-formed alternating-role conversation.
        let hash = self
            .context_store
            .append(Role::User, new_blocks.clone())
            .await?;
        self.history.push(Message {
            role: Role::User,
            content: new_blocks,
        });
        self.context_hashes.push(hash);
        Ok(ev)
    }

    /// Inject a harness-authored recovery prompt into the event log and LLM
    /// context as a `role: user` record.
    ///
    /// Called synchronously from the run loop to record and project two
    /// self-repair paths:
    /// - [`HarnessRecoveryKind::EmptyResponseContinuation`] — model returned
    ///   zero content blocks.
    /// - [`HarnessRecoveryKind::InvalidToolJson`] — SSE parser surfaced a
    ///   malformed-JSON error.
    ///
    /// Mirrors `inject_monitor_delivery`; see §15 of
    /// `docs/monitors-design.html` for the invariant this upholds.
    ///
    /// # Errors
    /// Returns an error if the event store or context store write fails.
    pub async fn inject_harness_recovery(
        &mut self,
        kind: HarnessRecoveryKind,
        content: &str,
    ) -> omega_store::Result<OmegaEvent> {
        let ev = OmegaEvent::HarnessRecovery(HarnessRecoveryEvent {
            time: now_iso(),
            kind,
            content: content.to_owned(),
        });
        self.event_store.append(&ev).await?;
        let blocks = vec![ContentBlock::Text {
            text: content.to_owned(),
        }];
        let hash = self
            .context_store
            .append(Role::User, blocks.clone())
            .await?;
        self.history.push(Message {
            role: Role::User,
            content: blocks,
        });
        self.context_hashes.push(hash);
        Ok(ev)
    }

    /// Inject a `MonitorStopped` event into the event log, and optionally
    /// into the LLM context.
    ///
    /// Projection rule (§12 locked decision):
    /// - `StoppedByAgent` → **not projected** (agent already knows).
    /// - `StoppedBySessionEnd` → **not projected** (session ending).
    /// - `StoppedByUser`, `ProcessExited`, `ProcessCrashed` → **projected**
    ///   as `role: user` so the agent learns and does not block waiting on
    ///   a dead monitor.
    ///
    /// # Errors
    /// Returns an error if the event store or context store write fails.
    pub async fn inject_monitor_stopped(
        &mut self,
        id: String,
        reason: MonitorStopReason,
        exit_code: Option<i32>,
    ) -> omega_store::Result<OmegaEvent> {
        let ev = OmegaEvent::MonitorStopped(MonitorStoppedEvent {
            id: id.clone(),
            reason: reason.clone(),
            exit_code,
            time: now_iso(),
        });
        self.event_store.append(&ev).await?;

        if !reason.should_project() {
            return Ok(ev);
        }

        // Project unexpected stop into context as a role:user notification.
        let text = format_monitor_stopped(&id, &reason, exit_code);
        let blocks = vec![ContentBlock::Text { text }];
        let hash = self
            .context_store
            .append(Role::User, blocks.clone())
            .await?;
        self.history.push(Message {
            role: Role::User,
            content: blocks,
        });
        self.context_hashes.push(hash);

        Ok(ev)
    }

    /// Inject a human-authored user message into the event log and LLM
    /// context as a `role:user` record.
    ///
    /// Emits `OmegaEvent::UserMessage` to `events.jsonl` BEFORE appending to
    /// the context store, satisfying the A1 invariant (§15(a) of
    /// `docs/monitors-design.html`) that every user-role context record has
    /// a backing event.
    ///
    /// # Errors
    /// Returns an error if the event store or context store write fails.
    pub async fn inject_user_message(&mut self, content: &str) -> omega_store::Result<OmegaEvent> {
        let ev = OmegaEvent::UserMessage(UserMessageEvent {
            time: now_iso(),
            content: content.to_owned(),
        });
        self.event_store.append(&ev).await?;
        let blocks = vec![ContentBlock::Text {
            text: content.to_owned(),
        }];
        let hash = self
            .context_store
            .append(Role::User, blocks.clone())
            .await?;
        self.history.push(Message {
            role: Role::User,
            content: blocks,
        });
        self.context_hashes.push(hash);
        Ok(ev)
    }

    /// Synthesise `is_error` tool-result records for `tool_use` blocks from a
    /// previous interrupted turn that were never executed.
    ///
    /// Emits one `OmegaEvent::ToolResult` (`is_error=true`) per dangling entry
    /// to `events.jsonl` FIRST (these are the backing events for the single
    /// batch context record per the A1 invariant — §15(a) of
    /// `docs/monitors-design.html`), then appends the batch as one `role:user`
    /// record to the context store and in-memory history.
    ///
    /// `dangling` is a `(tool_use_id, tool_name)` pair list.  Each entry's
    /// `tool_use_id` is placed in the `ContentBlock::ToolResult.tool_use_id`
    /// so the API pairs the synthetic result to the originating `tool_use`
    /// block; a fresh `tool_call_id` is minted for the event because no
    /// surviving `ToolCallEvent` exists to correlate against.
    ///
    /// Returns the emitted events so the caller can yield them on the stream.
    ///
    /// # Errors
    /// Returns an error if any event store or context store write fails.
    pub async fn inject_dangling_tool_results(
        &mut self,
        dangling: Vec<(String, String)>,
    ) -> omega_store::Result<Vec<OmegaEvent>> {
        let synthetic: Vec<ContentBlock> = dangling
            .iter()
            .map(|(id, _)| ContentBlock::ToolResult {
                tool_use_id: id.clone(),
                content: DANGLING_TOOL_USE_RESULT.to_owned(),
                is_error: true,
            })
            .collect();

        // Emit one ToolResult event per dangling tool FIRST — these are the
        // backing events for the batch context record appended below.
        let mut events = Vec::with_capacity(dangling.len());
        for (_id, name) in &dangling {
            let ev = OmegaEvent::ToolResult(ToolResultEvent {
                time: now_iso(),
                tool_call_id: gen_call_id(),
                name: name.clone(),
                is_error: true,
                duration_ms: 0,
                output: DANGLING_TOOL_USE_RESULT.to_owned(),
            });
            self.event_store.append(&ev).await?;
            events.push(ev);
        }

        // Append the batch as a single role:user context record.
        let hash = self
            .context_store
            .append(Role::User, synthetic.clone())
            .await?;
        self.history.push(Message {
            role: Role::User,
            content: synthetic,
        });
        self.context_hashes.push(hash);
        Ok(events)
    }

    /// Append the assembled tool-result content blocks to the context store
    /// as a `role:user` record and update in-memory history.
    ///
    /// **A1 invariant (§15(a) of `docs/monitors-design.html`):** callers MUST
    /// emit the individual `OmegaEvent::ToolResult` events for this batch to
    /// `events.jsonl` BEFORE calling this function.  Those events are the
    /// backing events for this context record.
    ///
    /// # Errors
    /// Returns an error if the context store write fails.
    pub async fn inject_tool_results_batch(
        &mut self,
        result_blocks: Vec<ContentBlock>,
    ) -> omega_store::Result<()> {
        let hash = self
            .context_store
            .append(Role::User, result_blocks.clone())
            .await?;
        self.history.push(Message {
            role: Role::User,
            content: result_blocks,
        });
        self.context_hashes.push(hash);
        Ok(())
    }

    /// Borrow a clone of the session's monitor manager.
    ///
    /// The `Arc` is shared with every [`ToolCtx`]; the UI roster badge /
    /// kill-modal (Phase 3) and tests read and control monitors through it.
    #[must_use]
    pub fn monitor_manager(&self) -> Arc<MonitorManager> {
        Arc::clone(&self.monitors)
    }

    /// Reap every live monitor tree for this session (session-end backstop).
    ///
    /// Kills each monitor's process group so no grandchild outlives the
    /// session, and returns the ids that were live.  Also invoked from
    /// [`Agent::drop`]; calling it explicitly lets the server reap before
    /// the agent is dropped.
    #[must_use]
    pub fn shutdown_monitors(&self) -> Vec<String> {
        self.monitors.shutdown()
    }

    /// Kill every live monitor AND persist a
    /// `MonitorStopped(StoppedBySessionEnd)` event for each one.
    ///
    /// Safe to call after the agent loop has terminated: the loop is the
    /// only other writer, so calling this from a teardown path preserves
    /// the single-writer rule.
    ///
    /// Reuses the `shutdown` CAS so monitors that already reached a
    /// terminal state are not double-logged.  Best-effort: individual
    /// store failures are silently discarded (session is ending anyway).
    ///
    /// Returns the `MonitorStopped` events that were written.
    pub async fn shutdown_and_log_monitors(&mut self) -> Vec<OmegaEvent> {
        let killed = self.monitors.shutdown();
        let mut logged = Vec::with_capacity(killed.len());
        for id in killed {
            // Best-effort: silently discard individual store failures
            // (session is ending anyway).
            if let Ok(ev) = self
                .inject_monitor_stopped(id, MonitorStopReason::StoppedBySessionEnd, None)
                .await
            {
                logged.push(ev);
            }
        }
        logged
    }

    /// Route one gathered [`InputItem`] through its eventful inject_*
    /// helper, returning the single backing `OmegaEvent` to yield (§15 U2).
    ///
    /// This is the unified projection seam: human and monitor input land
    /// the same way, each producing exactly one role:user context record +
    /// one event (A1 invariant — §15(a)).  Stderr is never an `InputItem`,
    /// so it has no arm here — it is drained separately by
    /// [`Self::drain_monitor_stderr`].
    pub(in crate::agent) async fn inject_input_item(
        &mut self,
        item: InputItem,
    ) -> omega_store::Result<OmegaEvent> {
        match item {
            InputItem::Human { content } => self.inject_user_message(&content).await,
            InputItem::MonitorStdout { monitor_id, lines } => {
                self.inject_monitor_delivery(vec![MonitorDeliveryItem { monitor_id, lines }])
                    .await
            }
            InputItem::MonitorStopped {
                monitor_id,
                reason,
                exit_code,
            } => {
                self.inject_monitor_stopped(monitor_id, reason, exit_code)
                    .await
            }
        }
    }
}
