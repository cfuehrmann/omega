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

use crate::protocol::{AgentErrorPayload, SessionInfoPayload, TurnState, WsMessage};

// ---------------------------------------------------------------------------
// POD snapshot
// ---------------------------------------------------------------------------

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
    /// Client-local pre-commit flag for the Continue-during-PauseRequested
    /// flow. When `true` and `turn_state` transitions to `Paused`, the
    /// composer auto-fires a `continue` WS message. Cleared on disconnect
    /// and on `ResetDone`. Never sent to the server — purely a UI promise.
    pub pre_committed: bool,
    /// Mirrors [`SessionStore::pending_changes_warning`].  See the
    /// signal docstring for semantics.
    pub pending_changes_warning: Option<crate::protocol::PendingChangesIntent>,
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
    pub transport_errors: RwSignal<Vec<String>>,
    /// See [`SessionState::pre_committed`].
    pub pre_committed: RwSignal<bool>,
    /// `Some(intent)` when the server rejected the most recent
    /// `Reset`/`ResumeSession` because the working tree is dirty and
    /// `allow_dirty` was not set.  Drives the dirty-warning modal: when
    /// `Some`, the modal is open; "Cancel" sets to `None`, "Proceed"
    /// re-issues the original frame with `allow_dirty: true` (then
    /// sets to `None`).
    pub pending_changes_warning: RwSignal<Option<crate::protocol::PendingChangesIntent>>,
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
            transport_errors: RwSignal::new(Vec::new()),
            pre_committed: RwSignal::new(false),
            pending_changes_warning: RwSignal::new(None),
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
                self.events.set(payload.events);
                self.streaming.set(payload.streaming);
                self.streaming_text.set(BTreeMap::new());
                self.streaming_thinking.set(BTreeMap::new());
            }

            WsMessage::ResetDone => {
                self.events.set(Vec::new());
                self.streaming.set(false);
                self.streaming_text.set(BTreeMap::new());
                self.streaming_thinking.set(BTreeMap::new());
                self.turn_state.set(TurnState::Idle);
                self.transport_errors.set(Vec::new());
                // pre_committed is a client-local promise; reset invalidates it.
                self.pre_committed.set(false);
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
            transport_errors: self.transport_errors.get_untracked(),
            pre_committed: self.pre_committed.get_untracked(),
            pending_changes_warning: self.pending_changes_warning.get_untracked(),
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
        OmegaEvent::UserMessage(_) => {
            store.turn_state.set(TurnState::Running);
            store.streaming.set(true);
            store.streaming_text.set(BTreeMap::new());
            store.streaming_thinking.set(BTreeMap::new());
        }
        OmegaEvent::TurnContinued(_) => store.turn_state.set(TurnState::Running),
        OmegaEvent::TurnPaused(_) => store.turn_state.set(TurnState::Paused),
        OmegaEvent::PauseRequested(_) => store.turn_state.set(TurnState::PauseRequested),
        OmegaEvent::TurnEnd(_) | OmegaEvent::TurnInterrupted(_) => {
            store.turn_state.set(TurnState::Idle);
            store.streaming.set(false);
            store.streaming_text.set(BTreeMap::new());
            store.streaming_thinking.set(BTreeMap::new());
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
        }
        // SCHEMA-8 Phase 4b — block events finalise the corresponding
        // streaming accumulator.  Phase 5a refined the per-`String`
        // clear into a per-index drain: blocks complete in start order
        // on the Anthropic wire, so the matching streaming slot is
        // always the lowest-keyed entry in the buffer at the moment
        // the event lands.  `pop_first` is a no-op on an empty buffer
        // (e.g. a block replayed from history that never had live
        // deltas), keeping the arm safe for all replay shapes.
        // `ToolUseBlock` has no streaming buffer at all (input streams
        // via provider deltas that the server doesn't currently
        // forward), so it falls through to the catch-all.
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
        // SCHEMA-8 Phase 4c — response closers drain both streaming
        // accumulators.  Belt-and-braces for the (legal) case of a
        // response that produces zero block events (e.g. an empty
        // tool-only reply): the per-block `TextBlock` / `ThinkingBlock`
        // arms above will usually have drained them already, but any
        // stragglers must be cleared so the next response opens with
        // empty buffers.
        OmegaEvent::LlmResponseEnded(_) | OmegaEvent::LlmResponseDiscarded(_) => {
            store.streaming_text.set(BTreeMap::new());
            store.streaming_thinking.set(BTreeMap::new());
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
        AgentErrorEvent, EffortChangedEvent, LlmResponseDiscardedEvent, LlmResponseEndedEvent,
        LlmResponseStartedEvent, LlmResponseUsage, ModelChangedEvent,
        PauseRequestedEvent, TextBlockEvent, ThinkingBlockEvent, TurnContinuedEvent, TurnEndEvent,
        TurnInterruptedEvent, TurnMetrics, TurnPausedEvent, UserMessageEvent,
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

    #[wasm_bindgen_test]
    fn reset_done_clears_pre_committed() {
        // pre_committed is a client-local UI promise; ResetDone must wipe it
        // so a pre-committed Continue cannot leak into a new session.
        with_owner(|| {
            let s = SessionStore::new();
            s.pre_committed.set(true);
            s.apply(WsMessage::ResetDone);
            assert!(!s.snapshot().pre_committed);
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
    fn pause_requested_event_drives_pause_requested_state() {
        with_owner(|| {
            let s = SessionStore::new();
            s.apply(user_msg("hi"));
            s.apply(WsMessage::PauseRequested(PauseRequestedEvent {
                time: "t".into(),
            }));
            assert_eq!(s.snapshot().turn_state, TurnState::PauseRequested);
        });
    }

    #[wasm_bindgen_test]
    fn turn_paused_event_drives_paused_state() {
        with_owner(|| {
            let s = SessionStore::new();
            s.apply(user_msg("hi"));
            s.apply(WsMessage::TurnPaused(TurnPausedEvent { time: "t".into() }));
            assert_eq!(s.snapshot().turn_state, TurnState::Paused);
        });
    }

    #[wasm_bindgen_test]
    fn turn_continued_event_drives_running_state_after_pause() {
        with_owner(|| {
            let s = SessionStore::new();
            s.apply(user_msg("hi"));
            s.apply(WsMessage::TurnPaused(TurnPausedEvent { time: "t".into() }));
            assert_eq!(s.snapshot().turn_state, TurnState::Paused);
            s.apply(WsMessage::TurnContinued(TurnContinuedEvent {
                time: "t2".into(),
                mode: omega_types::ContinueMode::Manual,
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
                model: "claude-opus-4-7".into(),
            }));
            assert_eq!(s.snapshot().session_info.unwrap().model, "claude-opus-4-7");
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
                model: "claude-opus-4-7".into(),
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
                model: "claude-opus-4-7".into(),
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
