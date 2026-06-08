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

mod context;
mod inject;
mod lifecycle;
mod resume;
mod run_loop;
mod stream_assembly;
mod util;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use omega_core::{Message, Provider};
use omega_types::FeatureFlags;
use omega_types::OmegaEvent;
use omega_types::events::{EffortChangedEvent, ModelChangedEvent, MonitorStopReason};

use omega_store::{ContextHash, ContextStore, EventStore};
use omega_tools::{MonitorManager, PythonRepl};

use crate::controls::ControlHandle;
use crate::event_sink::EventSink;
use crate::system_prompt::SystemBlock;

// Shared constant used by both run_loop.rs and resume.rs.
// Private to the agent module; child modules access it as `super::ANTHROPIC_URL`.
const ANTHROPIC_URL: &str = "https://api.anthropic.com/v1/messages";

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
    /// Working directory.  Used as the discovery root for the repo
    /// `AGENTS.md` (we walk up to the git root from here) and
    /// interpolated into the runtime-context system block.
    pub cwd: PathBuf,
    /// Path to the session directory (the parent of `events.jsonl`).
    /// Used by [`Agent::init`] to write the `session_started` event.
    pub session_dir: PathBuf,
    /// When `true`, output-format rendering guidance and the
    /// interactive-discussion policy are omitted from the core prompt.
    /// Set for headless / benchmark sessions where no human UI is attached.
    pub headless: bool,
    /// Feature flags override.  `None` (the default for new sessions)
    /// means [`Agent::init`] will read flags from environment variables
    /// via [`FeatureFlags::from_env`].  `Some(flags)` bypasses the env
    /// lookup and uses the supplied flags directly — used by tests that
    /// need deterministic, env-independent feature flags, and by the
    /// AI-resume path to seed a new session with specific flags.
    pub features: Option<FeatureFlags>,
    /// Tools enabled for this session.  `None` (the default for new
    /// sessions) → use [`omega_tools::DEFAULT_TOOL_NAMES`] (12 base
    /// tools, no `python_repl`).  `Some(names)` passes through verbatim;
    /// [`Agent::init`] then validates every name against
    /// [`omega_tools::ALL_TOOL_NAMES`].  Used by tests that need a
    /// deterministic toolset, and by the session-resume path to copy the
    /// parent's selection into the seed config.
    pub tool_selection: Option<Vec<String>>,
}

/// The agentic loop.
///
/// Held by `omega-server` (one per session) and by tests via the
/// in-memory [`MockProvider`](crate::testing::MockProvider) helper.
pub struct Agent {
    provider: Arc<dyn Provider>,
    context_store: ContextStore,
    event_store: Arc<EventStore>,
    /// Out-of-band event sink (§17, Phase A): append-to-log + broadcast-to-WS
    /// for events born **outside** a turn (monitor stderr, halt, model/effort
    /// changes).  Shares the same [`Arc<EventStore>`] as the loop's
    /// append-and-yield path; each event source uses exactly one path so
    /// nothing is emitted twice.  The WS half is installed later by the
    /// server via [`Agent::set_event_broadcaster`].
    event_sink: Arc<EventSink>,
    /// Pause / continue / abort handle.  Cloned out via
    /// [`Agent::controls`] **before** the caller starts a turn so the
    /// clone can be used to fire control events while `send_message`
    /// holds an exclusive borrow on the agent.
    controls: ControlHandle,
    config: AgentConfig,
    /// Currently selected model id.  Initialised from `config.model`;
    /// mutated by [`Agent::set_model`].  Read on every API call so a
    /// switch takes effect from the next call onward.
    ///
    /// §15 (Unified Input Model): wrapped in `Arc<Mutex<…>>` so that
    /// [`ModelEffortHandle`] can mutate it without the agent lock — the
    /// persistent [`Agent::run`] task holds that lock for the session's
    /// life, and only *reads* this value when building each `LlmRequest`.
    active_model: Arc<std::sync::Mutex<String>>,
    /// Currently selected thinking-effort level.  Initialised to
    /// [`DEFAULT_EFFORT`]; mutated by [`Agent::set_effort`].
    /// Threaded onto every `LlmRequest` as `config.effort` via
    /// [`cap_effort_for_model`].  Wrapped in `Arc<Mutex<…>>` for the same
    /// lock-free-mutation reason as [`Self::active_model`].
    active_effort: Arc<std::sync::Mutex<String>>,
    /// Cached system-prompt blocks for the active session.  Populated
    /// once in [`Agent::init`] (discovers + reads `AGENTS.md` files
    /// from the global and repo tiers, then assembles core + runtime
    /// + instruction blocks).  Re-used on every API call so disk I/O
    ///   happens at most once per session.
    system_blocks: Vec<SystemBlock>,
    /// Canonical on-disk paths of every file embedded in the system prompt
    /// (i.e. every [`SystemBlock`] whose `source_path` is `Some`).  Built
    /// once in [`Agent::init`] from [`Agent::system_blocks`] immediately
    /// after they are assembled; cloned by `Arc` into every [`ToolCtx`]
    /// so `execute_tool` can block redundant `read_file` calls without
    /// any per-call allocation.
    system_prompt_paths: Arc<HashSet<PathBuf>>,
    /// Runtime feature flags active for this session.
    ///
    /// Populated by [`Agent::init`] from environment variables via
    /// [`FeatureFlags::from_env`].  Recorded in the `SessionStartedEvent`
    /// so forensic analysis can determine which features were active.
    /// Available via [`Agent::features`] for conditional branching.
    features: FeatureFlags,
    /// Tools exposed to the model for this session, in canonical order.
    ///
    /// Resolved in [`Agent::new`] from `config.tool_selection`, falling
    /// back to [`omega_tools::DEFAULT_TOOL_NAMES`] when `None`.  Validated
    /// against [`omega_tools::ALL_TOOL_NAMES`] in [`Agent::init`].  Used
    /// by `tool_definitions`, `build_system_blocks`, and the
    /// `python_repl` gate inside the tool-dispatch loop.
    tool_selection: Vec<String>,
    /// In-memory mirror of `context.jsonl`; sent verbatim as the
    /// `messages` array on every API call.
    history: Vec<Message>,
    /// Hashes of `history` records, in insertion order.  Snapshotted
    /// onto every `LlmCall` event so post-mortem inspection can pin
    /// the exact context the model saw.
    context_hashes: Vec<ContextHash>,
    /// Shared handle to the session's stateful Python REPL subprocess.
    ///
    /// `None` inside the Mutex = subprocess not yet started (lazy startup).
    /// The subprocess is started on the first `python_repl` tool call and
    /// reused for all subsequent calls in the session.
    ///
    /// Only populated in the `ToolCtx` when `python_repl` is in the
    /// tool selection.
    /// Cleaned up (process killed) when the `PythonRepl` is dropped, which
    /// happens when the `Arc` reference count reaches zero — either at
    /// `Agent::drop` or when all outstanding `ToolCtx` handles are dropped.
    python_repl: Arc<tokio::sync::Mutex<Option<PythonRepl>>>,
    /// Per-session async monitor manager (Phase 2).
    ///
    /// One `Arc<MonitorManager>` owned by the agent and cloned into every
    /// [`ToolCtx`] so the `monitor` / `stop_monitor` tools enqueue work and
    /// roster state on the *same* instance the loop drains at its seams.
    /// The manager only enqueues; the loop is the single writer of monitor
    /// events (§4).  Reaped on [`Agent::drop`] so no monitor tree outlives
    /// the session.
    monitors: Arc<MonitorManager>,
}

impl Drop for Agent {
    fn drop(&mut self) {
        // Session-end backstop (§4): reap every monitor tree so no grandchild
        // is orphaned.  Idempotent with an explicit `shutdown_monitors`.
        let _ = self.monitors.shutdown();
    }
}

/// Lock-free handle for changing the active model / effort without
/// acquiring the agent mutex.
///
/// §15 (Unified Input Model): the persistent [`Agent::run`] task owns the
/// agent lock for the session's life, so model/effort changes — which the
/// run loop only *reads* when building each `LlmRequest` — flow through
/// this cheap clonable handle instead of through `agent.lock()`.  The
/// cells are the same `Arc`s the agent reads, so a change takes effect
/// from the next LLM call onward; the event is persisted to the shared
/// event store and returned for fan-out to the UI.
#[derive(Clone)]
pub struct ModelEffortHandle {
    model: Arc<std::sync::Mutex<String>>,
    effort: Arc<std::sync::Mutex<String>>,
    event_sink: Arc<EventSink>,
}

impl ModelEffortHandle {
    /// Currently selected thinking-effort level.
    #[must_use]
    pub fn effort(&self) -> String {
        self.effort
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Switch the active model.  Persists a [`ModelChangedEvent`] and
    /// returns it so callers can fan it out to the UI without a reload.
    pub async fn set_model(&self, model: String) -> OmegaEvent {
        self.model
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone_from(&model);
        // §17: route through the sink so the single click-time creation is
        // committed to disk AND broadcast to the current WS — the caller no
        // longer re-stamps or hand-fans-out.  The event is recorded at
        // click-time even mid-turn; the in-flight turn is unaffected because
        // `drive_turn` snapshots model+effort at entry.
        let ev = OmegaEvent::ModelChanged(ModelChangedEvent {
            time: util::now_iso(),
            model,
        });
        self.event_sink.emit(ev).await
    }

    /// Switch the active thinking-effort level.  Persists an
    /// [`EffortChangedEvent`] and returns it.
    pub async fn set_effort(&self, effort: String) -> OmegaEvent {
        self.effort
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone_from(&effort);
        // §17: route through the sink (see `set_model`).
        let ev = OmegaEvent::EffortChanged(EffortChangedEvent {
            time: util::now_iso(),
            effort,
        });
        self.event_sink.emit(ev).await
    }
}

/// One item of input to the persistent agent loop ([`Agent::run`]).
///
/// §15 Unified Input Model.  Both human input and (since U2) monitor
/// output arrive through the same inbox, so the run loop has a single
/// Gather seam.  Modelled as an enum so growth is purely additive (old
/// readers fail loudly on an unknown variant, per the Contract Authority
/// rule).
///
/// Monitor **stderr** is intentionally absent: it is non-projected
/// diagnostic output that never becomes `role:user` content, so it stays
/// on the [`MonitorManager`]'s pending queue and is drained directly into
/// a `MonitorStderr` event — never an `InputItem`.
#[derive(Debug, Clone)]
pub enum InputItem {
    /// A human coding-turn message — the text the operator typed.
    Human { content: String },
    /// One or more stdout lines from a monitor (§15 U2).  Projected to a
    /// `role:user` `MonitorDelivery` via [`Agent::inject_monitor_delivery`].
    MonitorStdout {
        /// Id of the monitor that produced the lines.
        monitor_id: String,
        /// The stdout lines, oldest first.
        lines: Vec<String>,
    },
    /// A monitor self-terminated (§15 U2).  Projected (for reasons that
    /// project) to a `role:user` `MonitorStopped` via
    /// [`Agent::inject_monitor_stopped`].
    MonitorStopped {
        /// Id of the monitor that stopped.
        monitor_id: String,
        /// Classified outcome (`ProcessExited` / `ProcessCrashed`).
        reason: MonitorStopReason,
        /// Exit code when it exited normally; `None` when killed by a signal.
        exit_code: Option<i32>,
    },
}
