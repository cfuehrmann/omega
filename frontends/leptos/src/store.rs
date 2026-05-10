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
    pub streaming_text: String,
    pub streaming_thinking: String,
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
    pub streaming_text: RwSignal<String>,
    pub streaming_thinking: RwSignal<String>,
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
            streaming_text: RwSignal::new(String::new()),
            streaming_thinking: RwSignal::new(String::new()),
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
    /// - `Text` / `Thinking` → append to the corresponding accumulator.
    /// - `ThinkingBlockComplete` → clear thinking accumulator (block
    ///   finished, signature recorded server-side).
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
                self.streaming_text.set(String::new());
                self.streaming_thinking.set(String::new());
            }

            WsMessage::ResetDone => {
                self.events.set(Vec::new());
                self.streaming.set(false);
                self.streaming_text.set(String::new());
                self.streaming_thinking.set(String::new());
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

            WsMessage::Text { text } => {
                self.streaming_text.update(|s| s.push_str(&text));
            }

            WsMessage::Thinking { text } => {
                self.streaming_thinking.update(|s| s.push_str(&text));
            }

            WsMessage::ThinkingBlockComplete { .. } => {
                self.streaming_thinking.set(String::new());
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
            store.streaming_text.set(String::new());
            store.streaming_thinking.set(String::new());
        }
        OmegaEvent::TurnContinued(_) => store.turn_state.set(TurnState::Running),
        OmegaEvent::TurnPaused(_) => store.turn_state.set(TurnState::Paused),
        OmegaEvent::PauseRequested(_) => store.turn_state.set(TurnState::PauseRequested),
        OmegaEvent::TurnEnd(_) | OmegaEvent::TurnInterrupted(_) => {
            store.turn_state.set(TurnState::Idle);
            store.streaming.set(false);
            store.streaming_text.set(String::new());
            store.streaming_thinking.set(String::new());
        }
        // `LlmResponse` finalises the prior streaming-text accumulator
        // (the model yields it as a `text` block in the response).
        OmegaEvent::LlmResponse(_) => {
            store.streaming_text.set(String::new());
            store.streaming_thinking.set(String::new());
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
    use omega_types::events::{
        AgentErrorEvent, EffortChangedEvent, LlmResponseEvent, LlmResponseUsage,
        ModelChangedEvent, PauseRequestedEvent, TurnContinuedEvent, TurnEndEvent,
        TurnInterruptedEvent, TurnMetrics, TurnPausedEvent, UserMessageEvent,
    };
    use leptos::reactive::owner::Owner;
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
            s.apply(WsMessage::Text { text: "leftover".into() });
            s.apply(WsMessage::Thinking { text: "x".into() });

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
            s.apply(WsMessage::AgentError(AgentErrorPayload::Event(AgentErrorEvent {
                time: "2024-01-01T00:00:00.000Z".into(),
                error: "oops".into(),
            })));
            let snap = s.snapshot();
            assert_eq!(snap.events.len(), 1);
            assert!(matches!(snap.events[0], OmegaEvent::AgentError(_)));
            assert!(snap.transport_errors.is_empty());
        });
    }

    // ---- streaming accumulators --------------------------------------------

    #[wasm_bindgen_test]
    fn text_signals_concatenate_into_streaming_text() {
        with_owner(|| {
            let s = SessionStore::new();
            s.apply(WsMessage::Text { text: "Hello".into() });
            s.apply(WsMessage::Text { text: ", world".into() });
            assert_eq!(s.snapshot().streaming_text, "Hello, world");
        });
    }

    #[wasm_bindgen_test]
    fn thinking_signals_concatenate_into_streaming_thinking() {
        with_owner(|| {
            let s = SessionStore::new();
            s.apply(WsMessage::Thinking { text: "abc".into() });
            s.apply(WsMessage::Thinking { text: "def".into() });
            assert_eq!(s.snapshot().streaming_thinking, "abcdef");
        });
    }

    #[wasm_bindgen_test]
    fn thinking_block_complete_clears_thinking_accumulator() {
        with_owner(|| {
            let s = SessionStore::new();
            s.apply(WsMessage::Thinking { text: "x".into() });
            s.apply(WsMessage::ThinkingBlockComplete { signature: "sig".into() });
            assert!(s.snapshot().streaming_thinking.is_empty());
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
            s.apply(WsMessage::Text { text: "partial".into() });
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

    #[wasm_bindgen_test]
    fn llm_response_event_clears_streaming_accumulators() {
        with_owner(|| {
            let s = SessionStore::new();
            s.apply(user_msg("hi"));
            s.apply(WsMessage::Text { text: "partial".into() });
            s.apply(WsMessage::Thinking { text: "musing".into() });
            assert!(!s.snapshot().streaming_text.is_empty());
            assert!(!s.snapshot().streaming_thinking.is_empty());
            s.apply(WsMessage::LlmResponse(LlmResponseEvent {
                time: "t".into(),
                stop_reason: "end_turn".into(),
                cleared_tool_uses: None,
                cleared_input_tokens: None,
                usage: LlmResponseUsage {
                    input_tokens: 1,
                    output_tokens: 2,
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                    service_tier: None,
                    iterations: None,
                },
                context_hash: "deadbeef".into(),
                text: None,
                thinking: None,
                streaming_start: None,
                response_summary: None,
            }));
            let snap = s.snapshot();
            assert!(snap.streaming_text.is_empty());
            assert!(snap.streaming_thinking.is_empty());
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
            assert_eq!(
                s.snapshot().session_info.unwrap().model,
                "claude-opus-4-7"
            );
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
            r#"{"type":"text","text":"part"}"#,
            r#"{"type":"text","text":"ial"}"#,
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
