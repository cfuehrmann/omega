// ---------------------------------------------------------------------------
// Agent construction, initialisation, and accessor methods.
// ---------------------------------------------------------------------------

use std::collections::HashSet;
use std::sync::Arc;

use omega_core::{ContentBlock, Message, Role};
use omega_store::ContextHash;
use omega_types::events::{ServerStartedEvent, SessionResumedEvent, SessionStartedEvent};
use omega_types::ids::{Origin, SessionId};
use omega_types::{FeatureFlags, OmegaEvent};

use crate::config::max_output_tokens_for_model;
use crate::event_sink::EventBroadcaster;
use crate::system_prompt::{
    SystemBlock, build_system_blocks, discover_instruction_files, join_blocks,
};

use super::Agent;
use super::AgentConfig;
use super::DEFAULT_EFFORT;
use super::ModelEffortHandle;
use super::util::now_iso;

/// Canned preamble injected before the resumption summary in the synthetic
/// user seed message.  Mirrors the literal in `Agent.seedWithResumptionSummary`
/// in `src/agent.ts`.
const SEED_USER_PREAMBLE: &str =
    "The following is context from the previous session to provide continuity:\n\n";

/// Canned acknowledgement used as the synthetic assistant seed message.
/// Mirrors the literal in `Agent.seedWithResumptionSummary` in `src/agent.ts`.
const SEED_ASSISTANT_ACK: &str =
    "Understood. I have reviewed the context from the previous session and am ready to continue.";

impl Agent {
    /// Build a new agent.
    ///
    /// `provider` is typically an [`Arc<RetryingProvider<…>>`] so the
    /// retry / `LlmRetry`-event logic happens transparently.  The agent
    /// itself never retries.
    #[must_use]
    pub fn new(
        provider: Arc<dyn omega_core::Provider>,
        context_store: omega_store::ContextStore,
        event_store: omega_store::EventStore,
        config: AgentConfig,
    ) -> Self {
        let active_model = config.model.clone();
        let active_effort = config
            .effort
            .clone()
            .unwrap_or_else(|| DEFAULT_EFFORT.to_owned());
        // Extract before moving config into Self.  None → default (both off);
        // Some(flags) bypasses from_env() (used by tests and the AI-resume path).
        let features = config.features.unwrap_or_default();
        // Resolve tool selection.  None → the canonical 12 base tools.
        // Some(names) passes through verbatim; validated in `init`.
        let tool_selection = config.tool_selection.clone().unwrap_or_else(|| {
            omega_tools::DEFAULT_TOOL_NAMES
                .iter()
                .map(|s| (*s).to_owned())
                .collect()
        });
        let event_store = Arc::new(event_store);
        let event_sink = Arc::new(crate::event_sink::EventSink::new(Arc::clone(&event_store)));
        let controls = crate::controls::ControlHandle::new(Arc::clone(&event_sink));
        Self {
            provider,
            context_store,
            event_store,
            event_sink,
            controls,
            config,
            active_model: Arc::new(std::sync::Mutex::new(active_model)),
            active_effort: Arc::new(std::sync::Mutex::new(active_effort)),
            system_blocks: Vec::new(),
            system_prompt_paths: Arc::new(HashSet::new()),
            history: Vec::new(),
            context_hashes: Vec::new(),
            features,
            tool_selection,
            python_repl: Arc::new(tokio::sync::Mutex::new(None)),
            monitors: omega_tools::MonitorManager::new(),
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
    pub async fn init(&mut self) -> omega_store::Result<()> {
        // 0. Resolve feature flags.
        //    When config.features is None (new sessions), read from the
        //    process environment.  When Some (tests or AI-resume seeding),
        //    the caller has already supplied the right flags — skip env.
        if self.config.features.is_none() {
            self.features = FeatureFlags::from_env();
        }
        // Validate the tool selection: every name must be in
        // `ALL_TOOL_NAMES`.  Fail loudly at startup so a typo on the
        // wire surfaces to the client / CLI instead of silently
        // shipping a wrong toolset.
        for name in &self.tool_selection {
            if !omega_tools::ALL_TOOL_NAMES.iter().any(|n| n == name) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("unknown tool name in tool_selection: {name}"),
                )
                .into());
            }
        }
        eprintln!(
            "feature flags: subagents={}; tool_selection={}",
            self.features.subagents,
            self.tool_selection.join(","),
        );

        // 1. server_started
        let server_started = OmegaEvent::ServerStarted(ServerStartedEvent { time: now_iso() });
        self.event_store.append(&server_started).await?;

        // 2. Discover AGENTS.md files (global + repo tiers).  Log one
        //    info line per discovered file so the session log records
        //    exactly which instruction sources the agent saw.
        let files = discover_instruction_files(&self.config.cwd);
        if files.is_empty() {
            eprintln!("AGENTS.md: not found in repo");
        } else {
            for f in &files {
                eprintln!("AGENTS.md: loaded {}", f.path.display());
            }
        }

        // 3. Assemble system blocks once for the whole session.
        let max_tokens = max_output_tokens_for_model(&self.active_model());
        self.system_blocks = build_system_blocks(
            &self.config.cwd.to_string_lossy(),
            max_tokens,
            self.config.headless,
            &files,
            &self.tool_selection,
        );
        // Derive the set of canonical on-disk paths for the system-prompt
        // guard in `execute_tool`.  Canonicalisation happens here once so
        // the per-call check is a simple HashSet lookup.
        self.system_prompt_paths = Arc::new(
            self.system_blocks
                .iter()
                .filter_map(|b| b.source_path.as_ref())
                .filter_map(|p| p.canonicalize().ok())
                .collect(),
        );

        // 4. session_started — generate a stable UUID v7 identity for this
        // session (distinct from the directory name, which is a filesystem
        // timestamp slug).  The UUID is stored in the SessionStarted event
        // so the UI and any downstream consumer can reference the session
        // by a well-typed, globally unique id.
        let session_id = SessionId(uuid::Uuid::now_v7());
        let path = self
            .config
            .session_dir
            .strip_prefix(&self.config.cwd)
            .unwrap_or(&self.config.session_dir)
            .to_string_lossy()
            .into_owned();
        // SessionStarted carries the full system prompt as a single
        // string for archival purposes — every block concatenated with
        // a blank line, preserving the order the model saw on the wire.
        let system_prompt = join_blocks(&self.system_blocks);
        let session_started = OmegaEvent::SessionStarted(SessionStartedEvent {
            time: now_iso(),
            session_id,
            path,
            model: self.active_model(),
            effort: self.active_effort(),
            system_prompt,
            omega_commit: crate::OMEGA_GIT_COMMIT.to_owned(),
            // IANA name of the agent host's current TZ (e.g. "Europe/Berlin").
            // The UI uses this to convert every event's UTC `time` into local
            // wall-clock time via `Intl.DateTimeFormat`.  Falling back to `UTC`
            // when detection fails keeps the rendered output well-defined
            // (Intl accepts `UTC` as a valid zone name).
            agent_time_zone: iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".into()),
            // Phase 1: all sessions started by the agent are root sessions
            // (no subagent support yet).
            origin: Origin::Root,
            // Runtime feature flags resolved from env at init time.
            features: self.features,
            // Tool selection resolved in `Agent::new` (defaults to the 12
            // base tools when `config.tool_selection` is `None`).
            tool_selection: self.tool_selection.clone(),
        });
        self.event_store.append(&session_started).await?;
        Ok(())
    }

    /// Runtime feature flags active for this session.
    ///
    /// Populated by [`Agent::init`].  Returns `FeatureFlags::default()` (both
    /// flags off) if called before `init` completes — the flags are not
    /// meaningful until the session has been initialised.
    #[must_use]
    pub fn features(&self) -> FeatureFlags {
        self.features
    }

    /// Borrow the resolved tool selection for this session.
    ///
    /// Populated in [`Agent::new`] from `config.tool_selection` (falling
    /// back to [`omega_tools::DEFAULT_TOOL_NAMES`] when `None`) and
    /// validated in [`Agent::init`].  Useful for tests and for callers
    /// that need to inspect or forward the active selection (e.g. the
    /// session-resume path copying the parent's selection into a
    /// successor session's seed config).
    #[must_use]
    pub fn tool_selection(&self) -> &[String] {
        &self.tool_selection
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
    pub fn controls(&self) -> crate::controls::ControlHandle {
        self.controls.clone()
    }

    /// Switch the active model.  Persists a [`ModelChangedEvent`] and
    /// returns it so callers can fan it out to the UI without a second
    /// load from disk.  Subsequent [`Agent::send_message`] calls send
    /// the new model.
    ///
    /// Mirrors `Agent.setModel` in `src/agent.ts`.
    pub async fn set_model(&mut self, model: String) -> OmegaEvent {
        self.model_effort_handle().set_model(model).await
    }

    /// Lock-free handle for changing model/effort without the agent lock.
    /// See [`ModelEffortHandle`] (§15 Unified Input Model).
    #[must_use]
    pub fn model_effort_handle(&self) -> ModelEffortHandle {
        ModelEffortHandle {
            model: Arc::clone(&self.active_model),
            effort: Arc::clone(&self.active_effort),
            event_sink: Arc::clone(&self.event_sink),
        }
    }

    /// Install the WebSocket broadcaster on the out-of-band [`EventSink`]
    /// (§17, Phase A).  Called once per session by the server after the
    /// agent is built; the broadcaster resolves the *current* `ws_tx` at
    /// emit time (it is replaced on reconnect).  Headless / CLI callers
    /// never call this — the sink then only appends to disk.
    pub fn set_event_broadcaster(&self, broadcaster: Arc<dyn EventBroadcaster>) {
        self.event_sink.set_broadcaster(broadcaster);
    }

    /// Switch the active thinking-effort level.  Persists an
    /// [`EffortChangedEvent`] and returns it.
    ///
    /// Mirrors `Agent.setEffort` in `src/agent.ts`.
    pub async fn set_effort(&mut self, effort: String) -> OmegaEvent {
        self.model_effort_handle().set_effort(effort).await
    }

    /// Currently selected model id.  Reflects the most recent
    /// `set_model` call (or `config.model` if none has happened).
    #[must_use]
    pub fn active_model(&self) -> String {
        self.active_model
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Currently selected thinking-effort level.
    #[must_use]
    pub fn active_effort(&self) -> String {
        self.active_effort
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Hashes of the records in [`Self::history`], in the same order.
    ///
    /// The i-th hash is the [`ContextHash`] of `history[i]` as stored in
    /// `context.jsonl`. These are the foreign-key values that appear in
    /// `events.jsonl` (`LlmCallEvent.context_hashes`,
    /// `LlmResponseEndedEvent.context_hash`).
    ///
    /// Used by the strict-resume path to verify that history is
    /// reconstructed faithfully across a process restart.
    #[must_use]
    pub fn context_hashes(&self) -> &[ContextHash] {
        &self.context_hashes
    }

    /// Pre-seed the in-memory history (used by resumption and tests).
    ///
    /// Callers must keep `history` and `context_hashes` aligned.
    pub fn seed_history(&mut self, history: Vec<Message>, hashes: Vec<ContextHash>) {
        self.history = history;
        self.context_hashes = hashes;
    }

    /// Initialise system blocks for an agent resumed from an existing session.
    ///
    /// Unlike [`Self::init`], does **not** read `AGENTS.md` from disk or write
    /// any events to `events.jsonl`.  Instead, it reconstructs the system
    /// blocks from the `system_prompt` text persisted in `SessionStartedEvent`,
    /// so the resumed agent sees exactly what the original session saw.
    ///
    /// `system_prompt_paths` is left empty: source paths are not persisted,
    /// so the system-prompt guard is disabled for resumed sessions.  This is
    /// acceptable — the guard is a best-effort UX protection, not a security
    /// boundary.
    ///
    /// Used by the AI-resume path to seed a new session with the system
    /// prompt text that was persisted in the original session's
    /// `SessionStartedEvent` (so the resumed session sees exactly what
    /// the original saw, regardless of any changes to instruction files
    /// since the session started).
    pub fn init_for_resume(&mut self, system_prompt: String) {
        // A single synthetic block with no source_path.  join_blocks()
        // on a one-element slice returns the content unchanged.
        self.system_blocks = vec![SystemBlock {
            label: "persisted",
            content: system_prompt,
            source_path: None,
        }];
        self.system_prompt_paths = Arc::new(HashSet::new());
    }

    /// The full system prompt text currently in effect for this session.
    ///
    /// Computed on demand by joining all [`SystemBlock`]s with `\n\n`.
    #[must_use]
    pub fn system_prompt(&self) -> String {
        join_blocks(&self.system_blocks)
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
}
