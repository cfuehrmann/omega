//! `SessionStore` — the single reactive source of truth for the Leptos
//! UI's view of the current WebSocket session.
//!
//! Phase 3.1 establishes the data flow without committing to any
//! component shapes:
//!
//! ```text
//!   /ws  ──► WsClient::on_message ──► SessionStore::apply ──► RwSignals
//!                                                                │
//!                                                                ▼
//!                                                    debug-view JSON dump
//! ```
//!
//! Future phases (3.2+) read individual signals out of `SessionStore`
//! via `use_context::<SessionStore>` (`provide_context` happens at the
//! `App` root). Selective subscription Just Works thanks to leptos's
//! per-signal reactivity.
//!
//! ## Snapshot/POD pair
//!
//! [`SessionState`] is a plain serializable struct mirroring the store's
//! signal fields. [`SessionStore::snapshot`] produces one without firing
//! any reactivity (`get_untracked`); the debug view serialises it to
//! pretty JSON. Tests compare snapshots before/after `apply` calls so
//! the assertions stay independent of leptos's reactive plumbing.

use std::collections::BTreeMap;

use leptos::prelude::*;
use omega_types::OmegaEvent;
use serde::Serialize;

use crate::protocol::{
    AgentErrorPayload, InputQueueItem, MonitorRosterEntry, SessionInfoPayload, TurnState, WsMessage,
};

// ---------------------------------------------------------------------------
// POD snapshot
// ---------------------------------------------------------------------------

/// One in-progress tool-use block that has been opened by a
/// [`WsMessage::ToolUseBlockStart`] frame but not yet sealed by a
/// [`OmegaEvent::ToolUseBlock`].
///
/// `partial_json` is the concatenation of every
/// [`WsMessage::ToolInput`] delta received so far.  It is **not**
/// guaranteed to be valid JSON until the block is sealed; render it
/// verbatim during streaming.
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StreamingToolUseSlot {
    pub tool_use_id: String,
    pub name: String,
    pub partial_json: String,
}

/// Plain-data view of the store's current contents. Used for the
/// debug-view JSON dump and as the assertion target in unit tests.
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SessionState {
    pub connected: bool,
    pub session_info: Option<SessionInfoPayload>,
    pub turn_state: TurnState,
    pub streaming: bool,
    pub events: Vec<OmegaEvent>,
    /// Per-block streaming text buffers keyed by Anthropic
    /// `content_block_start.index` (SCHEMA-8 Phase 5a). An entry exists
    /// from the first `text` delta on that index until the matching
    /// `TextBlock` event finalises (and drains) the slot. Multiple
    /// entries can coexist during interleaved-thinking responses.
    pub streaming_text: BTreeMap<usize, String>,
    /// Per-block streaming thinking buffers; same semantics as
    /// [`Self::streaming_text`] but for `thinking` deltas.
    pub streaming_thinking: BTreeMap<usize, String>,
    /// Envelope-level transport errors (not persisted as `OmegaEvent`s).
    pub transport_errors: Vec<String>,
    /// Per-block streaming tool-use buffers; same lifecycle as
    /// [`Self::streaming_text`] but for `tool_use_block_start` /
    /// `tool_input` deltas.  An entry is inserted by
    /// `ToolUseBlockStart`, extended by each `ToolInput`, then drained
    /// by the sealing `ToolUseBlock` event.
    pub streaming_tool_use: BTreeMap<usize, StreamingToolUseSlot>,
    /// Mirrors [`SessionStore::pending_changes_warning`].  See the
    /// signal docstring for semantics.
    pub pending_changes_warning: Option<crate::protocol::PendingChangesIntent>,
    /// IANA time-zone name of the agent host at session start (e.g.
    /// `"Europe/Berlin"`).  Mirrors [`SessionStore::agent_time_zone`];
    /// read by the feed when rendering each event's UTC `time` as a
    /// local wall-clock string via `Intl.DateTimeFormat`.  Defaults to
    /// `"UTC"` until a `SessionStarted` event lands; renders unchanged
    /// from pre-migration behaviour in that case.
    pub agent_time_zone: String,
    /// Latest ephemeral monitor roster snapshot from the server.
    /// Updated by [`WsMessage::MonitorRoster`] frames; never persisted.
    /// See [`SessionStore::roster`].
    pub roster: Vec<MonitorRosterEntry>,
    /// Latest ephemeral input-queue snapshot from the server.
    /// Updated by [`WsMessage::InputQueue`] frames; never persisted.
    /// See [`SessionStore::input_queue`].
    pub input_queue: Vec<InputQueueItem>,
}

// ---------------------------------------------------------------------------
// Reactive store
// ---------------------------------------------------------------------------

/// Reactive container of [`SessionState`] fields, one [`RwSignal`] per
/// field. Cheaply [`Copy`] (each signal is a slotmap handle); pass by
/// value into closures and contexts.
#[derive(Debug, Clone, Copy)]
pub struct SessionStore {
    pub connected: RwSignal<bool>,
    pub session_info: RwSignal<Option<SessionInfoPayload>>,
    pub turn_state: RwSignal<TurnState>,
    pub streaming: RwSignal<bool>,
    pub events: RwSignal<Vec<OmegaEvent>>,
    /// See [`SessionState::streaming_text`].
    pub streaming_text: RwSignal<BTreeMap<usize, String>>,
    /// See [`SessionState::streaming_thinking`].
    pub streaming_thinking: RwSignal<BTreeMap<usize, String>>,
    /// See [`SessionState::streaming_tool_use`].
    pub streaming_tool_use: RwSignal<BTreeMap<usize, StreamingToolUseSlot>>,
    pub transport_errors: RwSignal<Vec<String>>,
    /// `Some(intent)` when the server rejected the most recent
    /// `Reset`/`ResumeSession` because the working tree is dirty and
    /// `allow_dirty` was not set.  Drives the dirty-warning modal: when
    /// `Some`, the modal is open; "Cancel" sets to `None`, "Proceed"
    /// re-issues the original frame with `allow_dirty: true` (then
    /// sets to `None`).
    pub pending_changes_warning: RwSignal<Option<crate::protocol::PendingChangesIntent>>,
    /// IANA time-zone name (e.g. `"Europe/Berlin"`) of the agent host
    /// at the moment the active session was started.  Populated from
    /// the `agentTimeZone` field of the session's `SessionStarted`
    /// event, sourced either via a `History` payload on connect or via
    /// the live forwarded event when a fresh session begins.  The
    /// conversation feed reads this signal and passes it to
    /// `format_time` so every event renders in agent-host-local time,
    /// independently of whatever zone the *browser* happens to be in
    /// or the current DST state.
    ///
    /// Default `"UTC"`; reset to `"UTC"` on `ResetDone` so a freshly
    /// started session doesn't render its first few events in the
    /// previous session's zone before its own `SessionStarted` arrives.
    pub agent_time_zone: RwSignal<String>,
    /// Latest ephemeral monitor roster snapshot from the server.
    /// Replaced on every [`WsMessage::MonitorRoster`] frame; never
    /// persisted to events.  The badge and modal read from this signal.
    pub roster: RwSignal<Vec<MonitorRosterEntry>>,
    /// Latest ephemeral input-queue snapshot from the server.
    /// Replaced on every [`WsMessage::InputQueue`] frame; never
    /// persisted to events.  The queue badge and panel read from this
    /// signal.  U1: human-only; monitor sources join in U2.
    pub input_queue: RwSignal<Vec<InputQueueItem>>,
}

impl Default for SessionStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionStore {
    /// Construct a store with all signals at default values.
    ///
    /// Must be called inside a leptos reactive `Owner` scope (which
    /// `mount_to_body` and tests' `Owner::new()` provide).
    #[must_use]
    pub fn new() -> Self {
        Self {
            connected: RwSignal::new(false),
            session_info: RwSignal::new(None),
            turn_state: RwSignal::new(TurnState::default()),
            streaming: RwSignal::new(false),
            events: RwSignal::new(Vec::new()),
            streaming_text: RwSignal::new(BTreeMap::new()),
            streaming_thinking: RwSignal::new(BTreeMap::new()),
            streaming_tool_use: RwSignal::new(BTreeMap::new()),
            transport_errors: RwSignal::new(Vec::new()),
            pending_changes_warning: RwSignal::new(None),
            agent_time_zone: RwSignal::new("UTC".to_owned()),
            roster: RwSignal::new(Vec::new()),
            input_queue: RwSignal::new(Vec::new()),
        }
    }

    /// Apply one server-emitted [`WsMessage`] to the store. Side
    /// effects: at most one update per touched signal (per-field
    /// reactivity).
    ///
    /// Reducer rules:
    /// - `Ready` → `connected = true`.
    /// - `SessionInfo` → replace cached payload; mirror `turnState`.
    /// - `History` → replace `events`, set `streaming`, clear streaming
    ///   text/thinking (a fresh history batch invalidates partial
    ///   accumulations).
    /// - `ResetDone` → wipe `events`, clear streaming accumulators.
    /// - `AgentError` (envelope) → push `message` onto `transport_errors`.
    /// - `Text` / `Thinking` → append fragment to the slot at `index`
    ///   in the corresponding per-block buffer (Phase 5a).
    /// - `ThinkingBlockComplete` → drop the slot at `index` from the
    ///   thinking buffer (block finished, signature recorded
    ///   server-side).
    /// - `ToolUseBlockStart` → insert a [`StreamingToolUseSlot`] at
    ///   `index` with `tool_use_id`/`name` pre-filled and empty `partial_json`.
    /// - `ToolInput` → append `partial_json` to the slot at `index`.
    /// - Forwarded `OmegaEvent` (incl. `agent_error` event payload) →
    ///   `events.push(ev)` plus turn-state and streaming-accumulator
    ///   side effects keyed off the event type.
    pub fn apply(&self, msg: WsMessage) {
        match msg {
            WsMessage::Ready => self.connected.set(true),

            WsMessage::SessionInfo(payload) => {
                self.turn_state.set(payload.turn_state);
                self.session_info.set(Some(payload));
            }

            WsMessage::History(payload) => {
                // Pick up the session's IANA TZ from the first
                // `SessionStarted` in the history batch, falling back
                // to `"UTC"` if (a) the batch is empty / lacks one or
                // (b) the session was recorded before the field
                // existed (serde default).  Done before `events.set`
                // so any feed render triggered by the events update
                // already sees the correct zone.
                let tz = payload
                    .events
                    .iter()
                    .find_map(|ev| match ev {
                        OmegaEvent::SessionStarted(s) => Some(s.agent_time_zone.clone()),
                        _ => None,
                    })
                    .unwrap_or_else(|| "UTC".to_owned());
                self.agent_time_zone.set(tz);
                self.events.set(payload.events);
                self.streaming.set(payload.streaming);
                self.streaming_text.set(BTreeMap::new());
                self.streaming_thinking.set(BTreeMap::new());
                self.streaming_tool_use.set(BTreeMap::new());
            }

            WsMessage::ResetDone => {
                self.events.set(Vec::new());
                self.streaming.set(false);
                self.streaming_text.set(BTreeMap::new());
                self.streaming_thinking.set(BTreeMap::new());
                self.streaming_tool_use.set(BTreeMap::new());
                self.turn_state.set(TurnState::Idle);
                self.transport_errors.set(Vec::new());
                // The next session's `SessionStarted` will set this
                // again; meanwhile fall back to UTC so the brief
                // window of "no events yet" doesn't carry over stale
                // zone metadata.
                self.agent_time_zone.set("UTC".to_owned());
                // Ephemeral roster is per-session; clear it so the badge
                // disappears immediately on reset before the fresh
                // MonitorRoster snapshot arrives.
                self.roster.set(Vec::new());
                // Ephemeral queue is per-session; clear it immediately on
                // reset so the badge disappears before the fresh snapshot.
                self.input_queue.set(Vec::new());
            }

            WsMessage::AgentError(AgentErrorPayload::Envelope { message }) => {
                self.transport_errors.update(|v| v.push(message));
            }

            WsMessage::SessionDeleted { .. } | WsMessage::SessionRenamed { .. } => {
                // Picker-level side effects belong to 3.2's session-list
                // store. The conversation store is unaffected by other
                // sessions' fates; intentional no-op here.
            }

            WsMessage::PendingChangesWarning { intent } => {
                // Dirty-tree gate fired before the server destroyed the
                // active session.  The conversation store is unchanged;
                // we just record the intent so the dirty modal can
                // open and offer Cancel / Proceed.
                self.pending_changes_warning.set(Some(intent));
            }

            WsMessage::MonitorRoster { monitors } => {
                // Ephemeral roster snapshot from the server.  Replace the
                // stored slice so reactive consumers (badge, modal) see
                // the latest state.  This frame is NEVER passed to
                // `into_omega_event` and NEVER appended to `events`.
                self.roster.set(monitors);
            }

            WsMessage::InputQueue { items } => {
                // Ephemeral queue snapshot from the server.  Replace the
                // stored slice so reactive consumers (queue badge, panel)
                // see the latest pending items.  This frame is NEVER passed
                // to `into_omega_event` and NEVER appended to `events`.
                self.input_queue.set(items);
            }

            WsMessage::Text { index, text } => {
                self.streaming_text.update(|m| {
                    m.entry(index).or_default().push_str(&text);
                });
            }

            WsMessage::Thinking { index, text } => {
                self.streaming_thinking.update(|m| {
                    m.entry(index).or_default().push_str(&text);
                });
            }

            WsMessage::ThinkingBlockComplete { index, .. } => {
                // SCHEMA-8 Phase 5a — drain only the slot this signal
                // refers to.  The subsequent `ThinkingBlock` event
                // (handled in `apply_event_side_effects`) is the
                // authoritative drain; this arm is defensive in case
                // the agent ever emits a `thinking_block_complete`
                // signal without a paired event.
                self.streaming_thinking.update(|m| {
                    m.remove(&index);
                });
            }

            WsMessage::ToolUseBlockStart {
                index,
                tool_use_id,
                name,
            } => {
                // Insert a fresh slot.  If a slot already exists at this
                // index (shouldn't happen on a well-behaved server) we
                // overwrite it so the UI stays consistent.
                self.streaming_tool_use.update(|m| {
                    m.insert(
                        index,
                        StreamingToolUseSlot {
                            tool_use_id,
                            name,
                            partial_json: String::new(),
                        },
                    );
                });
            }

            WsMessage::ToolInput {
                index,
                partial_json,
            } => {
                // Append the delta to the slot opened by
                // `ToolUseBlockStart`.  If no slot exists yet (race or
                // replay) we silently create one with empty id/name so
                // we don't drop data.
                self.streaming_tool_use.update(|m| {
                    m.entry(index)
                        .or_default()
                        .partial_json
                        .push_str(&partial_json);
                });
            }

            // All other variants carry an `OmegaEvent` payload (incl. the
            // `agent_error` *event* shape).
            other => {
                if let Some(event) = other.into_omega_event() {
                    apply_event_side_effects(self, &event);
                    self.events.update(|v| v.push(event));
                }
            }
        }
    }

    /// Untracked snapshot of every field. Used for the debug view's
    /// pretty-printed JSON dump and for tests' assertion target.
    #[must_use]
    pub fn snapshot(&self) -> SessionState {
        SessionState {
            connected: self.connected.get_untracked(),
            session_info: self.session_info.get_untracked(),
            turn_state: self.turn_state.get_untracked(),
            streaming: self.streaming.get_untracked(),
            events: self.events.get_untracked(),
            streaming_text: self.streaming_text.get_untracked(),
            streaming_thinking: self.streaming_thinking.get_untracked(),
            streaming_tool_use: self.streaming_tool_use.get_untracked(),
            transport_errors: self.transport_errors.get_untracked(),
            pending_changes_warning: self.pending_changes_warning.get_untracked(),
            agent_time_zone: self.agent_time_zone.get_untracked(),
            roster: self.roster.get_untracked(),
            input_queue: self.input_queue.get_untracked(),
        }
    }
}

// ---------------------------------------------------------------------------
// Event-driven derived state (turn state + streaming flag + accumulators)
// ---------------------------------------------------------------------------

/// Apply transitions that *event* tags drive: turn-state machine and
/// streaming-accumulator resets, plus session-info mirror updates for
/// `model_changed` / `effort_changed` (the server emits the event but
/// does **not** re-broadcast a fresh `SessionInfo` envelope, so the
/// client must mirror the change locally to keep `session_info.model`
/// and `session_info.effort` honest). Mirrors `next_turn_state_for`
/// in the Rust server. `events.push(ev)` itself is the caller's job.
fn apply_event_side_effects(store: &SessionStore, ev: &OmegaEvent) {
    match ev {
        OmegaEvent::SessionStarted(s) => {
            // Live path: a fresh session emits `SessionStarted` as its
            // first event.  Mirror its `agentTimeZone` into the store
            // signal so subsequent events render in the agent host's
            // local time.  History-replay handles the same job for
            // already-recorded sessions in the `History` arm above.
            store.agent_time_zone.set(s.agent_time_zone.clone());
        }
        OmegaEvent::UserMessage(_) => {
            store.turn_state.set(TurnState::Running);
            store.streaming.set(true);
            store.streaming_text.set(BTreeMap::new());
            store.streaming_thinking.set(BTreeMap::new());
            store.streaming_tool_use.set(BTreeMap::new());
        }
        OmegaEvent::TurnResumed(_) => store.turn_state.set(TurnState::Running),
        OmegaEvent::TurnHalted(_) => store.turn_state.set(TurnState::Halted),
        OmegaEvent::HaltRequested(_) => store.turn_state.set(TurnState::HaltRequested),
        OmegaEvent::TurnEnd(_) | OmegaEvent::TurnInterrupted(_) => {
            store.turn_state.set(TurnState::Idle);
            store.streaming.set(false);
            store.streaming_text.set(BTreeMap::new());
            store.streaming_thinking.set(BTreeMap::new());
            store.streaming_tool_use.set(BTreeMap::new());
        }
        // SCHEMA-8 Phase 4a — `LlmResponseStarted` opens a fresh
        // response container.  The streaming buffers are global (one
        // text + one thinking accumulator across the whole turn) and
        // are cleared by `UserMessage` already at turn-start; clearing
        // again here is defensive against a leftover fragment from a
        // discarded prior response inside the same turn (the agent
        // already emits partial-flagged block events + a
        // `LlmResponseDiscarded` before retrying, which will drain the
        // buffers in Phase 4b/4c, but this belt-and-braces clear makes
        // the invariant explicit at the start of every response).
        OmegaEvent::LlmResponseStarted(_) => {
            store.streaming_text.set(BTreeMap::new());
            store.streaming_thinking.set(BTreeMap::new());
            store.streaming_tool_use.set(BTreeMap::new());
        }
        // SCHEMA-8 Phase 4b — block events finalise the corresponding
        // streaming accumulator.  Phase 5a refined the per-`String`
        // clear into a per-index drain: blocks complete in start order
        // on the Anthropic wire, so the matching streaming slot is
        // always the lowest-keyed entry in the buffer at the moment
        // the event lands.  `pop_first` is a no-op on an empty buffer
        // (e.g. a block replayed from history that never had live
        // deltas), keeping the arm safe for all replay shapes.
        // SCHEMA-8 Phase 5b — the sealing `ToolUseBlock` event drains
        // the lowest-keyed slot in `streaming_tool_use`, mirroring the
        // `TextBlock` / `ThinkingBlock` logic.  `pop_first` is a no-op
        // on an empty buffer for replay safety.
        OmegaEvent::TextBlock(_) => {
            store.streaming_text.update(|m| {
                m.pop_first();
            });
        }
        OmegaEvent::ThinkingBlock(_) => {
            store.streaming_thinking.update(|m| {
                m.pop_first();
            });
        }
        OmegaEvent::ToolUseBlock(_) => {
            store.streaming_tool_use.update(|m| {
                m.pop_first();
            });
        }
        // SCHEMA-8 Phase 4c — response closers drain all streaming
        // accumulators.  Belt-and-braces for the (legal) case of a
        // response that produces zero block events (e.g. an empty
        // tool-only reply): the per-block drain arms above will usually
        // have cleared them already, but any stragglers must be
        // removed so the next response opens with empty buffers.
        OmegaEvent::LlmResponseEnded(_) | OmegaEvent::LlmResponseDiscarded(_) => {
            store.streaming_text.set(BTreeMap::new());
            store.streaming_thinking.set(BTreeMap::new());
            store.streaming_tool_use.set(BTreeMap::new());
        }
        // Mirror server-side model/effort changes into the cached
        // `session_info` so reactive consumers (the composer, the
        // debug view) see the latest value without a fresh
        // `session_info` broadcast. The server's set_model handler
        // refreshes its `info_cache` for *future* broadcasts but does
        // not actively re-emit one; this rule is what keeps the
        // client's projection of session_info accurate. (The 8e2106b
        // SolidJS bug had the same root shape — the UI read from a
        // stale source after model_changed; we read from session_info
        // and update session_info here.)
        OmegaEvent::ModelChanged(e) => {
            store.session_info.update(|si| {
                if let Some(info) = si.as_mut() {
                    info.model.clone_from(&e.model);
                }
            });
        }
        OmegaEvent::EffortChanged(e) => {
            store.session_info.update(|si| {
                if let Some(info) = si.as_mut() {
                    info.effort.clone_from(&e.effort);
                }
            });
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]

    use super::*;
    use leptos::reactive::owner::Owner;
    use omega_types::events::{
        AgentErrorEvent, EffortChangedEvent, HaltRequestedEvent, LlmResponseDiscardedEvent,
        LlmResponseEndedEvent, LlmResponseStartedEvent, LlmResponseUsage, ModelChangedEvent,
        TextBlockEvent, ThinkingBlockEvent, TurnEndEvent, TurnHaltedEvent, TurnInterruptedEvent,
        TurnMetrics, TurnResumedEvent, UserMessageEvent,
    };
    use wasm_bindgen_test::wasm_bindgen_test;

    /// Run `f` inside a fresh leptos `Owner`. Required because
    /// `RwSignal::new` registers with the active owner, and the
    /// wasm-bindgen test harness has no implicit one.
    fn with_owner<F: FnOnce()>(f: F) {
        let owner = Owner::new();
        owner.with(f);
    }

    fn user_msg(content: &str) -> WsMessage {
        WsMessage::UserMessage(UserMessageEvent {
            time: "2024-01-01T00:00:00.000Z".into(),
            content: content.into(),
        })
    }

    fn turn_end() -> WsMessage {
        WsMessage::TurnEnd(TurnEndEvent {
            time: "2024-01-01T00:00:00.000Z".into(),
            metrics: TurnMetrics {
                input_tokens: 1,
                output_tokens: 2,
                cache_creation_tokens: None,
                cache_read_tokens: None,
            },
        })
    }

    fn session_info(turn_state: TurnState) -> WsMessage {
        WsMessage::SessionInfo(SessionInfoPayload {
            dir: "d".into(),
            model: "m".into(),
            effort: "e".into(),
            cwd: "/c".into(),
            turn_state,
            has_pending_changes: false,
            name: None,
        })
    }

    // ---- envelope reducer rules ---------------------------------------------

    #[wasm_bindgen_test]
    fn ready_sets_connected() {
        with_owner(|| {
            let s = SessionStore::new();
            assert!(!s.snapshot().connected);
            s.apply(WsMessage::Ready);
            assert!(s.snapshot().connected);
        });
    }

    #[wasm_bindgen_test]
    fn session_info_replaces_payload_and_mirrors_turn_state() {
        with_owner(|| {
            let s = SessionStore::new();
            s.apply(session_info(TurnState::Running));
            let snap = s.snapshot();
            assert_eq!(snap.turn_state, TurnState::Running);
            assert_eq!(snap.session_info.as_ref().unwrap().model, "m");
        });
    }

    #[wasm_bindgen_test]
    fn history_replaces_events_and_resets_streaming_accumulators() {
        with_owner(|| {
            let s = SessionStore::new();
            // Pre-populate accumulators so the reset is observable.
            s.apply(WsMessage::Text {
                index: 0,
                text: "leftover".into(),
            });
            s.apply(WsMessage::Thinking {
                index: 0,
                text: "x".into(),
            });

            let frame = WsMessage::History(crate::protocol::HistoryPayload {
                events: vec![OmegaEvent::TurnEnd(TurnEndEvent {
                    time: "2024-01-01T00:00:00.000Z".into(),
                    metrics: TurnMetrics {
                        input_tokens: 1,
                        output_tokens: 2,
                        cache_creation_tokens: None,
                        cache_read_tokens: None,
                    },
                })],
                streaming: true,
            });
            s.apply(frame);

            let snap = s.snapshot();
            assert_eq!(snap.events.len(), 1);
            assert!(snap.streaming);
            assert!(snap.streaming_text.is_empty());
            assert!(snap.streaming_thinking.is_empty());
        });
    }

    #[wasm_bindgen_test]
    fn reset_done_wipes_events_and_returns_to_idle() {
        with_owner(|| {
            let s = SessionStore::new();
            s.apply(user_msg("hi"));
            assert!(!s.snapshot().events.is_empty());
            assert_eq!(s.snapshot().turn_state, TurnState::Running);

            s.apply(WsMessage::ResetDone);
            let snap = s.snapshot();
            assert!(snap.events.is_empty());
            assert!(!snap.streaming);
            assert_eq!(snap.turn_state, TurnState::Idle);
        });
    }

    // ---- agent_error envelope vs. event ------------------------------------

    #[wasm_bindgen_test]
    fn envelope_agent_error_appends_to_transport_errors_not_events() {
        with_owner(|| {
            let s = SessionStore::new();
            s.apply(WsMessage::AgentError(AgentErrorPayload::Envelope {
                message: "bad frame".into(),
            }));
            let snap = s.snapshot();
            assert_eq!(snap.transport_errors, vec!["bad frame".to_string()]);
            assert!(snap.events.is_empty());
        });
    }

    #[wasm_bindgen_test]
    fn event_agent_error_appends_to_events_not_transport_errors() {
        with_owner(|| {
            let s = SessionStore::new();
            s.apply(WsMessage::AgentError(AgentErrorPayload::Event(
                AgentErrorEvent {
                    time: "2024-01-01T00:00:00.000Z".into(),
                    error: "oops".into(),
                },
            )));
            let snap = s.snapshot();
            assert_eq!(snap.events.len(), 1);
            assert!(matches!(snap.events[0], OmegaEvent::AgentError(_)));
            assert!(snap.transport_errors.is_empty());
        });
    }

    // ---- streaming accumulators --------------------------------------------

    #[wasm_bindgen_test]
    fn text_signals_concatenate_into_streaming_text() {
        // Per-index buffer (SCHEMA-8 Phase 5a): two fragments on the
        // same index accumulate; an entry on a different index lives
        // in its own slot.
        with_owner(|| {
            let s = SessionStore::new();
            s.apply(WsMessage::Text {
                index: 0,
                text: "Hello".into(),
            });
            s.apply(WsMessage::Text {
                index: 0,
                text: ", world".into(),
            });
            s.apply(WsMessage::Text {
                index: 2,
                text: "other".into(),
            });
            let snap = s.snapshot();
            assert_eq!(
                snap.streaming_text.get(&0).map(String::as_str),
                Some("Hello, world"),
            );
            assert_eq!(
                snap.streaming_text.get(&2).map(String::as_str),
                Some("other"),
            );
        });
    }

    #[wasm_bindgen_test]
    fn thinking_signals_concatenate_into_streaming_thinking() {
        with_owner(|| {
            let s = SessionStore::new();
            s.apply(WsMessage::Thinking {
                index: 1,
                text: "abc".into(),
            });
            s.apply(WsMessage::Thinking {
                index: 1,
                text: "def".into(),
            });
            assert_eq!(
                s.snapshot().streaming_thinking.get(&1).map(String::as_str),
                Some("abcdef"),
            );
        });
    }

    #[wasm_bindgen_test]
    fn thinking_block_complete_clears_thinking_accumulator() {
        // SCHEMA-8 Phase 5a — drains only the matching index slot.
        with_owner(|| {
            let s = SessionStore::new();
            s.apply(WsMessage::Thinking {
                index: 0,
                text: "a".into(),
            });
            s.apply(WsMessage::Thinking {
                index: 1,
                text: "b".into(),
            });
            s.apply(WsMessage::ThinkingBlockComplete {
                index: 0,
                signature: "sig".into(),
            });
            let snap = s.snapshot();
            assert!(snap.streaming_thinking.get(&0).is_none());
            assert_eq!(
                snap.streaming_thinking.get(&1).map(String::as_str),
                Some("b"),
            );
        });
    }

    // ---- turn-state transitions --------------------------------------------

    #[wasm_bindgen_test]
    fn user_message_event_drives_running_state_and_starts_streaming() {
        with_owner(|| {
            let s = SessionStore::new();
            s.apply(user_msg("hi"));
            let snap = s.snapshot();
            assert_eq!(snap.turn_state, TurnState::Running);
            assert!(snap.streaming);
            assert_eq!(snap.events.len(), 1);
        });
    }

    #[wasm_bindgen_test]
    fn turn_end_returns_to_idle_and_clears_streaming() {
        with_owner(|| {
            let s = SessionStore::new();
            s.apply(user_msg("hi"));
            s.apply(WsMessage::Text {
                index: 0,
                text: "partial".into(),
            });
            s.apply(turn_end());
            let snap = s.snapshot();
            assert_eq!(snap.turn_state, TurnState::Idle);
            assert!(!snap.streaming);
            assert!(snap.streaming_text.is_empty());
            assert_eq!(snap.events.len(), 2);
        });
    }

    #[wasm_bindgen_test]
    fn turn_interrupted_returns_to_idle() {
        with_owner(|| {
            let s = SessionStore::new();
            s.apply(user_msg("hi"));
            s.apply(WsMessage::TurnInterrupted(TurnInterruptedEvent {
                time: "t".into(),
                reason: Some(omega_types::InterruptReason::Aborted),
            }));
            let snap = s.snapshot();
            assert_eq!(snap.turn_state, TurnState::Idle);
            assert!(!snap.streaming);
        });
    }

    #[wasm_bindgen_test]
    fn halt_requested_event_drives_halt_requested_state() {
        with_owner(|| {
            let s = SessionStore::new();
            s.apply(user_msg("hi"));
            s.apply(WsMessage::HaltRequested(HaltRequestedEvent {
                time: "t".into(),
            }));
            assert_eq!(s.snapshot().turn_state, TurnState::HaltRequested);
        });
    }

    #[wasm_bindgen_test]
    fn turn_halted_event_drives_halted_state() {
        with_owner(|| {
            let s = SessionStore::new();
            s.apply(user_msg("hi"));
            s.apply(WsMessage::TurnHalted(TurnHaltedEvent { time: "t".into() }));
            assert_eq!(s.snapshot().turn_state, TurnState::Halted);
        });
    }

    #[wasm_bindgen_test]
    fn turn_resumed_event_drives_running_state_after_halt() {
        with_owner(|| {
            let s = SessionStore::new();
            s.apply(user_msg("hi"));
            s.apply(WsMessage::TurnHalted(TurnHaltedEvent { time: "t".into() }));
            assert_eq!(s.snapshot().turn_state, TurnState::Halted);
            s.apply(WsMessage::TurnResumed(TurnResumedEvent {
                time: "t2".into(),
            }));
            assert_eq!(s.snapshot().turn_state, TurnState::Running);
        });
    }

    // ---- SCHEMA-8 Phase 4a: LlmResponseStarted opens a fresh container --

    #[wasm_bindgen_test]
    fn llm_response_started_clears_streaming_accumulators() {
        // The opener event has no visible UI change yet (Phase 4b/4c).
        // What it MUST do at the store level is reset any leftover
        // streaming-buffer content from a discarded prior response
        // within the same turn, so block-event content for the new
        // response can stream into a clean buffer.
        with_owner(|| {
            let s = SessionStore::new();
            s.apply(user_msg("hi"));
            // Simulate a leftover fragment from a previous discarded
            // response (deltas streamed in, the response was
            // abandoned by retry, and the partial-block events have
            // already been emitted but the deltas haven't been
            // cleared on this code path yet).
            s.apply(WsMessage::Text {
                index: 0,
                text: "leftover".into(),
            });
            s.apply(WsMessage::Thinking {
                index: 0,
                text: "stale".into(),
            });
            assert!(!s.snapshot().streaming_text.is_empty());
            assert!(!s.snapshot().streaming_thinking.is_empty());
            s.apply(WsMessage::LlmResponseStarted(LlmResponseStartedEvent {
                time: "t".into(),
            }));
            let snap = s.snapshot();
            assert!(snap.streaming_text.is_empty());
            assert!(snap.streaming_thinking.is_empty());
            // The opener event itself appended to the events list
            // (via the catch-all in `apply`, unchanged).
            assert!(matches!(
                snap.events.last().unwrap(),
                OmegaEvent::LlmResponseStarted(_)
            ));
        });
    }

    // ---- SCHEMA-8 Phase 4b: block events drain matching streaming buffers --

    #[wasm_bindgen_test]
    fn text_block_event_clears_streaming_text_buffer() {
        // When a `TextBlock` event lands the persisted event in `events`
        // owns the content the live `Text` deltas had been accumulating
        // into `streaming_text`.  The matching slot in the per-index
        // buffer is now redundant and must be drained.  SCHEMA-8 Phase
        // 5a refined the drain to per-index via `pop_first` (blocks
        // complete in start order on the Anthropic wire).
        with_owner(|| {
            let s = SessionStore::new();
            s.apply(user_msg("hi"));
            s.apply(WsMessage::Text {
                index: 0,
                text: "hello".into(),
            });
            assert_eq!(
                s.snapshot().streaming_text.get(&0).map(String::as_str),
                Some("hello"),
            );
            s.apply(WsMessage::TextBlock(TextBlockEvent {
                time: "t".into(),
                text: "hello world".into(),
                partial: false,
            }));
            // The streaming buffer is drained; the persisted event has
            // landed in the events list.
            let snap = s.snapshot();
            assert!(snap.streaming_text.is_empty());
            assert!(matches!(
                snap.events.last().unwrap(),
                OmegaEvent::TextBlock(_)
            ));
            // Thinking buffer is untouched (no `ThinkingBlock` arrived).
            assert!(snap.streaming_thinking.is_empty());
        });
    }

    #[wasm_bindgen_test]
    fn text_block_event_drains_lowest_index_only() {
        // SCHEMA-8 Phase 5a — interleaved blocks coexist in the
        // per-index buffer; the `TextBlock` event drains the
        // lowest-keyed slot (the one Anthropic completes first) and
        // leaves higher-index slots intact.
        with_owner(|| {
            let s = SessionStore::new();
            s.apply(user_msg("hi"));
            s.apply(WsMessage::Text {
                index: 0,
                text: "first".into(),
            });
            s.apply(WsMessage::Text {
                index: 2,
                text: "second".into(),
            });
            s.apply(WsMessage::TextBlock(TextBlockEvent {
                time: "t".into(),
                text: "first".into(),
                partial: false,
            }));
            let snap = s.snapshot();
            assert!(
                snap.streaming_text.get(&0).is_none(),
                "index 0 must drain on TextBlock",
            );
            assert_eq!(
                snap.streaming_text.get(&2).map(String::as_str),
                Some("second"),
                "index 2 must remain in-flight",
            );
        });
    }

    #[wasm_bindgen_test]
    fn text_block_partial_also_clears_streaming_text_buffer() {
        // Partial blocks (emitted just before `LlmResponseDiscarded`
        // on mid-stream abandonment) carry the same content the live
        // buffer was holding, so the slot is equally redundant.
        with_owner(|| {
            let s = SessionStore::new();
            s.apply(user_msg("hi"));
            s.apply(WsMessage::Text {
                index: 0,
                text: "abandoned".into(),
            });
            assert_eq!(
                s.snapshot().streaming_text.get(&0).map(String::as_str),
                Some("abandoned"),
            );
            s.apply(WsMessage::TextBlock(TextBlockEvent {
                time: "t".into(),
                text: "abandoned".into(),
                partial: true,
            }));
            assert!(s.snapshot().streaming_text.is_empty());
        });
    }

    #[wasm_bindgen_test]
    fn thinking_block_event_clears_streaming_thinking_buffer() {
        with_owner(|| {
            let s = SessionStore::new();
            s.apply(user_msg("hi"));
            s.apply(WsMessage::Thinking {
                index: 0,
                text: "musing".into(),
            });
            assert_eq!(
                s.snapshot().streaming_thinking.get(&0).map(String::as_str),
                Some("musing"),
            );
            s.apply(WsMessage::ThinkingBlock(ThinkingBlockEvent {
                time: "t".into(),
                thinking: "musing complete".into(),
                signature: Some("sig".into()),
                partial: false,
            }));
            let snap = s.snapshot();
            assert!(snap.streaming_thinking.is_empty());
            assert!(matches!(
                snap.events.last().unwrap(),
                OmegaEvent::ThinkingBlock(_)
            ));
            // Text buffer untouched.
            assert!(snap.streaming_text.is_empty());
        });
    }

    #[wasm_bindgen_test]
    fn thinking_block_partial_also_clears_streaming_thinking_buffer() {
        with_owner(|| {
            let s = SessionStore::new();
            s.apply(user_msg("hi"));
            s.apply(WsMessage::Thinking {
                index: 0,
                text: "abandoned".into(),
            });
            assert_eq!(
                s.snapshot().streaming_thinking.get(&0).map(String::as_str),
                Some("abandoned"),
            );
            s.apply(WsMessage::ThinkingBlock(ThinkingBlockEvent {
                time: "t".into(),
                thinking: "abandoned".into(),
                signature: None,
                partial: true,
            }));
            assert!(s.snapshot().streaming_thinking.is_empty());
        });
    }

    // ---- SCHEMA-8 Phase 5b: tool-use streaming buffer ------------------

    #[wasm_bindgen_test]
    fn tool_use_block_start_inserts_slot() {
        with_owner(|| {
            let s = SessionStore::new();
            s.apply(WsMessage::ToolUseBlockStart {
                index: 1,
                tool_use_id: "tu_abc".into(),
                name: "bash".into(),
            });
            let snap = s.snapshot();
            let slot = snap.streaming_tool_use.get(&1).expect("slot must exist");
            assert_eq!(slot.tool_use_id, "tu_abc");
            assert_eq!(slot.name, "bash");
            assert!(slot.partial_json.is_empty());
        });
    }

    #[wasm_bindgen_test]
    fn tool_input_appends_partial_json() {
        with_owner(|| {
            let s = SessionStore::new();
            s.apply(WsMessage::ToolUseBlockStart {
                index: 1,
                tool_use_id: "tu_abc".into(),
                name: "bash".into(),
            });
            s.apply(WsMessage::ToolInput {
                index: 1,
                partial_json: r#"{"cmd": "ec"#.into(),
            });
            s.apply(WsMessage::ToolInput {
                index: 1,
                partial_json: r#"ho hi}"#.into(),
            });
            let snap = s.snapshot();
            let slot = snap.streaming_tool_use.get(&1).expect("slot must exist");
            assert_eq!(slot.partial_json, r#"{"cmd": "echo hi}"#);
        });
    }

    #[wasm_bindgen_test]
    fn tool_use_block_event_drains_slot() {
        use omega_types::events::ToolUseBlockEvent;
        with_owner(|| {
            let s = SessionStore::new();
            s.apply(WsMessage::ToolUseBlockStart {
                index: 1,
                tool_use_id: "tu_abc".into(),
                name: "bash".into(),
            });
            s.apply(WsMessage::ToolInput {
                index: 1,
                partial_json: r#"{"cmd":"echo"}"#.into(),
            });
            assert!(!s.snapshot().streaming_tool_use.is_empty());
            s.apply(WsMessage::ToolUseBlock(ToolUseBlockEvent {
                time: "t".into(),
                tool_call_id: "tc_abc".into(),
                tool_use_id: "tu_abc".into(),
                name: "bash".into(),
                input: serde_json::json!({"cmd": "echo"}),
                partial: false,
            }));
            assert!(
                s.snapshot().streaming_tool_use.is_empty(),
                "ToolUseBlock event must drain the slot"
            );
        });
    }

    #[wasm_bindgen_test]
    fn llm_response_ended_clears_tool_use_slots() {
        use omega_types::events::ToolUseBlockEvent;
        // Belt-and-braces: if ToolUseBlock events are missing (e.g.
        // a streaming response cut off) LlmResponseEnded must still
        // drain any lingering tool-use slots.
        with_owner(|| {
            let s = SessionStore::new();
            s.apply(WsMessage::ToolUseBlockStart {
                index: 0,
                tool_use_id: "tu_1".into(),
                name: "bash".into(),
            });
            assert!(!s.snapshot().streaming_tool_use.is_empty());
            s.apply(WsMessage::LlmResponseEnded(LlmResponseEndedEvent {
                time: "t".into(),
                stop_reason: "end_turn".into(),
                cleared_tool_uses: None,
                cleared_input_tokens: None,
                usage: LlmResponseUsage {
                    input_tokens: 1,
                    output_tokens: 2,
                    cache_read_input_tokens: None,
                    cache_creation_input_tokens: None,
                    service_tier: None,
                    iterations: None,
                },
                context_hash: "h".into(),
                response_summary: None,
            }));
            assert!(
                s.snapshot().streaming_tool_use.is_empty(),
                "LlmResponseEnded must drain tool-use slots"
            );
            // suppress unused import lint
            let _ = ToolUseBlockEvent {
                time: "t".into(),
                tool_call_id: "".into(),
                tool_use_id: "".into(),
                name: "".into(),
                input: serde_json::Value::Null,
                partial: false,
            };
        });
    }

    // ---- SCHEMA-8 Phase 4c: response-closer side-effects ----------------

    #[wasm_bindgen_test]
    fn llm_response_ended_clears_both_streaming_accumulators() {
        // `LlmResponseEnded` is the closer for `LlmResponseStarted` on a
        // successful response.  The per-block arms above usually drain
        // both buffers by the time this fires; this arm is
        // belt-and-braces for the (legal) case of a response that
        // produces zero block events (e.g. an empty tool-only reply) —
        // any straggling deltas in the global accumulators must be
        // cleared so the next response opens with empty buffers.
        with_owner(|| {
            let s = SessionStore::new();
            s.apply(user_msg("hi"));
            s.apply(WsMessage::Text {
                index: 0,
                text: "leftover text".into(),
            });
            s.apply(WsMessage::Thinking {
                index: 0,
                text: "leftover thinking".into(),
            });
            s.apply(WsMessage::LlmResponseEnded(LlmResponseEndedEvent {
                time: "t".into(),
                stop_reason: "end_turn".into(),
                cleared_tool_uses: None,
                cleared_input_tokens: None,
                usage: LlmResponseUsage {
                    input_tokens: 1,
                    output_tokens: 2,
                    cache_read_input_tokens: None,
                    cache_creation_input_tokens: None,
                    service_tier: None,
                    iterations: None,
                },
                context_hash: "h".into(),
                response_summary: None,
            }));
            let snap = s.snapshot();
            assert!(
                snap.streaming_text.is_empty(),
                "streaming_text not drained on LlmResponseEnded: {:?}",
                snap.streaming_text
            );
            assert!(
                snap.streaming_thinking.is_empty(),
                "streaming_thinking not drained on LlmResponseEnded: {:?}",
                snap.streaming_thinking
            );
            assert!(matches!(
                snap.events.last().unwrap(),
                OmegaEvent::LlmResponseEnded(_)
            ));
        });
    }

    #[wasm_bindgen_test]
    fn llm_response_discarded_clears_both_streaming_accumulators() {
        // `LlmResponseDiscarded` is the closer for an abandoned
        // response.  Same drain contract as `LlmResponseEnded` — the
        // preceding partial `TextBlock` / `ThinkingBlock` events
        // usually do the draining, but a discard with zero blocks is
        // legal and must still settle the buffers.
        with_owner(|| {
            let s = SessionStore::new();
            s.apply(user_msg("hi"));
            s.apply(WsMessage::Text {
                index: 0,
                text: "in flight".into(),
            });
            s.apply(WsMessage::Thinking {
                index: 0,
                text: "in flight".into(),
            });
            s.apply(WsMessage::LlmResponseDiscarded(LlmResponseDiscardedEvent {
                time: "t".into(),
            }));
            let snap = s.snapshot();
            assert!(snap.streaming_text.is_empty());
            assert!(snap.streaming_thinking.is_empty());
            assert!(matches!(
                snap.events.last().unwrap(),
                OmegaEvent::LlmResponseDiscarded(_)
            ));
        });
    }

    // ---- model_changed / effort_changed mirror to session_info -----------

    #[wasm_bindgen_test]
    fn model_changed_event_updates_cached_session_info_model() {
        with_owner(|| {
            let s = SessionStore::new();
            s.apply(session_info(TurnState::Idle));
            assert_eq!(s.snapshot().session_info.unwrap().model, "m");
            s.apply(WsMessage::ModelChanged(ModelChangedEvent {
                time: "t".into(),
                model: "claude-opus-4-8".into(),
            }));
            assert_eq!(s.snapshot().session_info.unwrap().model, "claude-opus-4-8");
        });
    }

    #[wasm_bindgen_test]
    fn effort_changed_event_updates_cached_session_info_effort() {
        with_owner(|| {
            let s = SessionStore::new();
            s.apply(session_info(TurnState::Idle));
            assert_eq!(s.snapshot().session_info.unwrap().effort, "e");
            s.apply(WsMessage::EffortChanged(EffortChangedEvent {
                time: "t".into(),
                effort: "high".into(),
            }));
            assert_eq!(s.snapshot().session_info.unwrap().effort, "high");
        });
    }

    #[wasm_bindgen_test]
    fn model_changed_with_no_session_info_is_a_noop() {
        // Defensive: if session_info hasn't been received yet, the
        // mirror update is a no-op rather than synthesising a partial
        // payload. Catches a mutation that constructs a fresh
        // SessionInfoPayload with default fields.
        with_owner(|| {
            let s = SessionStore::new();
            s.apply(WsMessage::ModelChanged(ModelChangedEvent {
                time: "t".into(),
                model: "claude-opus-4-8".into(),
            }));
            assert!(s.snapshot().session_info.is_none());
        });
    }

    #[wasm_bindgen_test]
    fn effort_changed_with_no_session_info_is_a_noop() {
        with_owner(|| {
            let s = SessionStore::new();
            s.apply(WsMessage::EffortChanged(EffortChangedEvent {
                time: "t".into(),
                effort: "high".into(),
            }));
            assert!(s.snapshot().session_info.is_none());
        });
    }

    #[wasm_bindgen_test]
    fn model_changed_appends_event_to_log() {
        // The model-changed event still ends up in the events log
        // — the mirror is a side effect, not a replacement.
        with_owner(|| {
            let s = SessionStore::new();
            s.apply(session_info(TurnState::Idle));
            s.apply(WsMessage::ModelChanged(ModelChangedEvent {
                time: "t".into(),
                model: "claude-opus-4-8".into(),
            }));
            assert_eq!(s.snapshot().events.len(), 1);
        });
    }

    // ---- end-to-end fixture parse ------------------------------------------

    #[wasm_bindgen_test]
    fn fixture_full_session_replays_into_consistent_state() {
        // Mirrors a realistic frame sequence for a single-turn session,
        // exercising every WsMessage variant the server emits today on a
        // happy path: ready → session_info → history → text → llm_response
        // → turn_end. Every frame must deserialise into the typed enum
        // (no `serde_json::Value` in the parse path).
        let frames: &[&str] = &[
            r#"{"type":"ready"}"#,
            r#"{"type":"session_info","dir":"d","model":"m","effort":"e","cwd":"/c","turnState":"idle","hasPendingChanges":false}"#,
            r#"{"type":"history","events":[]}"#,
            r#"{"type":"user_message","time":"2024-01-01T00:00:00.000Z","content":"hi"}"#,
            r#"{"type":"text","index":0,"text":"part"}"#,
            r#"{"type":"text","index":0,"text":"ial"}"#,
            r#"{"type":"turn_end","time":"2024-01-01T00:00:01.000Z","metrics":{"inputTokens":1,"outputTokens":2}}"#,
        ];
        with_owner(|| {
            let s = SessionStore::new();
            for f in frames {
                let msg: WsMessage = serde_json::from_str(f).unwrap();
                s.apply(msg);
            }
            let snap = s.snapshot();
            assert!(snap.connected);
            assert_eq!(snap.events.len(), 2, "user_message + turn_end persisted");
            assert!(matches!(snap.events[0], OmegaEvent::UserMessage(_)));
            assert!(matches!(snap.events[1], OmegaEvent::TurnEnd(_)));
            assert_eq!(snap.turn_state, TurnState::Idle);
            assert!(!snap.streaming);
            // streaming_text was reset by turn_end.
            assert!(snap.streaming_text.is_empty());
            assert_eq!(snap.session_info.as_ref().unwrap().model, "m");
        });
    }
}
