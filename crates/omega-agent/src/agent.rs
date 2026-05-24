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

use std::collections::{BTreeMap, HashMap, HashSet};
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
use omega_types::FeatureFlags;
use omega_types::StreamSignal;
use omega_types::events::{
    AgentErrorEvent, ContextCompactedEvent, EffortChangedEvent, LlmCallEvent, LlmErrorEvent,
    LlmResponseDiscardedEvent, LlmResponseEndedEvent, LlmResponseStartedEvent, ModelChangedEvent,
    ResumingSessionEvent, ServerStartedEvent, SessionResumedEvent, SessionStartedEvent,
    TextBlockEvent, ThinkingBlockEvent, ToolCallEvent, ToolResultEvent, ToolUseBlockEvent,
    TurnContinuedEvent, TurnEndEvent, TurnInterruptedEvent, TurnPausedEvent, UsageIteration,
    UserMessageEvent,
};
use omega_types::ids::{Origin, SessionId};
use omega_types::{ContinueMode, InterruptReason, OmegaEvent, TurnMetrics};

use omega_store::{ContextHash, ContextStore, EventStore};
use omega_tools::{PythonRepl, ToolCtx, execute_tool, tool_definitions};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::config::{cap_effort_for_model, max_output_tokens_for_model};
use crate::controls::{ControlHandle, TurnGuard};
use crate::error_classify::{is_context_too_long, is_invalid_tool_json};
use crate::session_resume::{
    DomainSnapshot, RESUMPTION_EFFORT, RESUMPTION_MAX_TOKENS, RESUMPTION_MODEL,
    RESUMPTION_SUMMARY_INSTRUCTIONS, extract_summary_from_response,
};
use crate::system_prompt::{
    SystemBlock, build_system_blocks, discover_instruction_files, join_blocks,
};

const ANTHROPIC_URL: &str = "https://api.anthropic.com/v1/messages";

// ---------------------------------------------------------------------------
// SCHEMA-8 Phase 3: per-block streaming accumulators
//
// Each [`Provider`] stream is a sequence of indexed `content_block_start` /
// `..._delta` / `content_block_stop` events.  The agent collects each block
// into its own `BlockSlot` keyed by the API's `index`, then assembles the
// assistant message in index order.  This replaces the legacy flat
// accumulators (`text_buf`, `current_thinking`, `completed_thinking_blocks`,
// `tool_uses`) that grouped by kind and reordered interleaved blocks.
//
// Phase 3 staging:
//   * commit 3a (this commit) introduces the slots in parallel with the flat
//     accumulators; the flat path still wins for context.jsonl assembly so
//     all 6 Phase-0 goldens stay byte-equal.
//   * commit 3e drops the flat path and locks the interleaved-thinking
//     golden.
// ---------------------------------------------------------------------------

/// One in-flight assistant content block, keyed by the provider's
/// `content_block_start.index`.  Variants mirror the three
/// `content_block_start` shapes Anthropic emits.
///
/// `sealed` flips to `true` on the matching `*BlockComplete` signal.  An
/// unsealed slot at the moment a stream is abandoned (`LlmRetry` /
/// `LlmError` / `TurnInterrupted` mid-stream) yields a
/// `partial: true` block event in Phase 3 commit 3d.
#[derive(Debug, Clone)]
enum BlockSlot {
    Text {
        text: String,
        sealed: bool,
    },
    Thinking {
        thinking: String,
        signature: Option<String>,
        sealed: bool,
    },
    ToolUse {
        /// Omega-layer identifier (provider-agnostic), minted on
        /// `ToolUseBlockStart` so the same id flows through the
        /// streaming partial-event path, the sealed `ToolUseBlock`
        /// event, and the downstream `ToolCall` / `ToolResult` events.
        tool_call_id: String,
        /// LLM-issued identifier from the provider's `tool_use` block.
        /// Echoed back verbatim in `ContentBlock::ToolResult.tool_use_id`
        /// (protocol layer).
        tool_use_id: String,
        name: String,
        input: Value,
        sealed: bool,
    },
}

/// Append a text delta to slot `idx`, creating an empty `Text` slot if
/// missing.  Logs and ignores type mismatches (defensive — providers
/// shouldn't send a `Text` delta against a non-text slot).
fn append_text_slot(slots: &mut BTreeMap<usize, BlockSlot>, idx: usize, delta: &str) {
    let slot = slots.entry(idx).or_insert_with(|| BlockSlot::Text {
        text: String::new(),
        sealed: false,
    });
    if let BlockSlot::Text { text, .. } = slot {
        text.push_str(delta);
    }
    // Type mismatch: provider sent a `Text` delta against a slot already
    // typed as Thinking/ToolUse.  Drop — a provider bug we can't recover
    // from cleanly here.  Phase 3 commit 3e's index-ordered assembly will
    // surface anything that slips through as a context-record discrepancy
    // detected by goldens.
}

/// Append a thinking delta to slot `idx`, creating an empty `Thinking`
/// slot if missing.
fn append_thinking_slot(slots: &mut BTreeMap<usize, BlockSlot>, idx: usize, delta: &str) {
    let slot = slots.entry(idx).or_insert_with(|| BlockSlot::Thinking {
        thinking: String::new(),
        signature: None,
        sealed: false,
    });
    if let BlockSlot::Thinking { thinking, .. } = slot {
        thinking.push_str(delta);
    }
    // See `append_text_slot` for the type-mismatch rationale.
}

/// Mark a `Text` slot sealed.  Creates an empty `Text` slot if missing
/// (an empty text block is rare but legal — the provider is telling us
/// it's done either way).
fn seal_text_slot(slots: &mut BTreeMap<usize, BlockSlot>, idx: usize) {
    let slot = slots.entry(idx).or_insert_with(|| BlockSlot::Text {
        text: String::new(),
        sealed: false,
    });
    if let BlockSlot::Text { sealed, .. } = slot {
        *sealed = true;
    }
    // See `append_text_slot` for the type-mismatch rationale.
}

/// Mark a `Thinking` slot sealed and record its signature.  Creates an
/// empty `Thinking` slot if missing (rare but legal).
fn seal_thinking_slot(slots: &mut BTreeMap<usize, BlockSlot>, idx: usize, sig: String) {
    let slot = slots.entry(idx).or_insert_with(|| BlockSlot::Thinking {
        thinking: String::new(),
        signature: None,
        sealed: false,
    });
    if let BlockSlot::Thinking {
        signature, sealed, ..
    } = slot
    {
        *signature = Some(sig);
        *sealed = true;
    }
    // See `append_text_slot` for the type-mismatch rationale.
}

/// Open an unsealed `ToolUse` slot at `idx` on `ToolUseBlockStart`,
/// minting a fresh `tool_call_id` so it's available before any input
/// deltas arrive.  Idempotent on retry: a re-`Start` for the same index
/// gets a fresh `tool_call_id` (correct — different attempt).
fn open_tool_use_slot(
    slots: &mut BTreeMap<usize, BlockSlot>,
    idx: usize,
    tool_use_id: String,
    name: String,
) -> String {
    let tool_call_id = gen_call_id();
    slots.insert(
        idx,
        BlockSlot::ToolUse {
            tool_call_id: tool_call_id.clone(),
            tool_use_id,
            name,
            input: Value::Null,
            sealed: false,
        },
    );
    tool_call_id
}

/// Seal a `ToolUse` slot on `ToolUseBlockComplete`, populating `input`.
/// Returns the `tool_call_id` that was minted at open time so the caller
/// can include it in the emitted `ToolUseBlockEvent`.  If the slot is
/// missing (provider bug: Complete without Start), synthesize a fresh
/// `tool_call_id` and insert the slot sealed.
fn seal_tool_use_slot(
    slots: &mut BTreeMap<usize, BlockSlot>,
    idx: usize,
    tool_use_id: String,
    name: String,
    input: Value,
) -> String {
    if let Some(BlockSlot::ToolUse {
        tool_call_id,
        input: i,
        sealed,
        ..
    }) = slots.get_mut(&idx)
    {
        *i = input;
        *sealed = true;
        return tool_call_id.clone();
    }
    let tool_call_id = gen_call_id();
    slots.insert(
        idx,
        BlockSlot::ToolUse {
            tool_call_id: tool_call_id.clone(),
            tool_use_id,
            name,
            input,
            sealed: true,
        },
    );
    tool_call_id
}

/// SCHEMA-8 Phase 3 commit 3d: build the abandonment-closer event
/// sequence for a response stream that was cut short by
/// `LlmRetry` / `LlmError` / `TurnInterrupted` before the provider
/// could surface its terminal `LlmResponse`.
///
/// For each UNSEALED `BlockSlot` left in `slots` (in index order),
/// emit a `partial: true` variant of the corresponding
/// `TextBlock` / `ThinkingBlock` / `ToolUseBlock` event so the
/// consumer has explicit closure for every opened block.  Sealed
/// slots had their final `partial: false` event emitted on their
/// `*BlockComplete` signal and are skipped here to avoid duplicate
/// emission.
///
/// Finally, append the `LlmResponseDiscarded` marker so the
/// consumer knows the response stream was abandoned.  Always
/// emitted when this helper is called, even if `slots` was empty
/// — the caller is expected to gate on `response_started`.
fn make_abandonment_closers(slots: BTreeMap<usize, BlockSlot>) -> Vec<OmegaEvent> {
    let mut events: Vec<OmegaEvent> = slots
        .into_values()
        .filter_map(|slot| match slot {
            BlockSlot::Text {
                text,
                sealed: false,
            } if !text.is_empty() => Some(OmegaEvent::TextBlock(TextBlockEvent {
                time: now_iso(),
                text,
                partial: true,
            })),
            BlockSlot::Thinking {
                thinking,
                signature,
                sealed: false,
            } if !thinking.is_empty() => Some(OmegaEvent::ThinkingBlock(ThinkingBlockEvent {
                time: now_iso(),
                thinking,
                signature,
                partial: true,
            })),
            BlockSlot::ToolUse {
                tool_call_id,
                tool_use_id,
                name,
                input,
                sealed: false,
            } => Some(OmegaEvent::ToolUseBlock(ToolUseBlockEvent {
                time: now_iso(),
                tool_call_id,
                tool_use_id,
                name,
                input,
                partial: true,
            })),
            _ => None,
        })
        .collect();
    events.push(OmegaEvent::LlmResponseDiscarded(
        LlmResponseDiscardedEvent { time: now_iso() },
    ));
    events
}

// ---------------------------------------------------------------------------
// Context-management configuration (BUG-D fix)
//
// Mirrors `config.ts:toolResultClear*` + `autoCompactThreshold` from the
// deleted TypeScript agent (src/agent.ts, pre-3.7 at 8ae104f^).
// ---------------------------------------------------------------------------

/// Input-token threshold that triggers the `clear_tool_uses_20250919` edit.
/// Matches `toolResultClearTrigger = 100_000` in the TS config.
const TOOL_RESULT_CLEAR_TRIGGER: u64 = 100_000;

/// Number of most-recent tool-use rounds to keep after clearing.
/// Matches `toolResultClearKeep = 10` in the TS config.
const TOOL_RESULT_CLEAR_KEEP: u64 = 10;

/// Minimum token savings required before a clearing fires.
/// Matches `toolResultClearAtLeast = 15_000` in the TS config.
const TOOL_RESULT_CLEAR_AT_LEAST: u64 = 15_000;

/// Input-token threshold that triggers full context compaction.
/// Matches `autoCompactThreshold = 750_000` in the TS config.
const AUTO_COMPACT_THRESHOLD: u64 = 750_000;

/// Compaction summary instructions forwarded verbatim to the model.
/// Mirrors `COMPACTION_INSTRUCTIONS` in the deleted `src/config.ts`.
const COMPACTION_INSTRUCTIONS: &str = "You have written a partial transcript for \
the initial task above. Please write a summary of the transcript. The purpose of \
this summary is to provide continuity so you can continue to make progress towards \
solving the task in a future context, where the raw history above may not be \
accessible and will be replaced with this summary.\n\nFor a coding session, focus \
especially on what a developer would need to continue the work:\n\n\
1. **Current state** (snapshot, not narrative): what is true *right now* — \
which files were changed and how they currently stand, what \
constants/config values are currently set to, which plan items are done \
vs. pending.\n\n\
2. **Next step**: the single most important thing to do next, as specifically \
as possible (e.g. exact file, function, test name).\n\n\
3. **Key decisions**: conclusions that should not be re-litigated — design \
choices made, approaches confirmed or rejected, and *why*.\n\n\
4. **Learnings / what not to do**: anything tried that failed and why, so the \
same dead ends are not re-explored.\n\n\
5. **Technical anchors**: specific file paths, function/type/constant names, \
commit hashes, and test names relevant to continuing the work. Prefer \
current values over historical change narratives.\n\n\
You must wrap your summary in a <summary></summary> block.";

/// Build the `context_management` payload sent on every agent turn.
///
/// Three edits in priority order (Anthropic requires this exact ordering
/// when `clear_thinking_20251015` is present):
///
/// 1. `clear_thinking_20251015 keep=all` — keep all thinking blocks to
///    preserve the prompt-cache prefix.  Clearing them (the API default)
///    busts the cache at each clearing point, causing expensive rewrites.
/// 2. `clear_tool_uses_20250919` — when input tokens exceed
///    `TOOL_RESULT_CLEAR_TRIGGER`, discard all but the last
///    `TOOL_RESULT_CLEAR_KEEP` tool-use rounds (server-side only — the
///    local history is unaffected per Anthropic's API docs).
/// 3. `compact_20260112` — full context compaction at
///    `AUTO_COMPACT_THRESHOLD` tokens.
///
/// Mirrors `context_management` in `src/agent.ts:1288–1316` (pre-3.7).
fn build_context_management() -> serde_json::Value {
    serde_json::json!({
        "edits": [
            {
                "type": "clear_thinking_20251015",
                "keep": "all"
            },
            {
                "type": "clear_tool_uses_20250919",
                "trigger": { "type": "input_tokens", "value": TOOL_RESULT_CLEAR_TRIGGER },
                "keep": { "type": "tool_uses", "value": TOOL_RESULT_CLEAR_KEEP },
                "clear_at_least": { "type": "input_tokens", "value": TOOL_RESULT_CLEAR_AT_LEAST },
                "clear_tool_inputs": true
            },
            {
                "type": "compact_20260112",
                "trigger": { "type": "input_tokens", "value": AUTO_COMPACT_THRESHOLD },
                "instructions": COMPACTION_INSTRUCTIONS
            }
        ]
    })
}

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
    /// lookup and uses the supplied flags directly — used by
    /// [`strict_resume`](crate::session_resume::strict_resume) to restore
    /// the exact flags that were active in the original session, and by
    /// tests that need deterministic, env-independent feature flags.
    pub features: Option<FeatureFlags>,
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
    /// [`DEFAULT_EFFORT`]; mutated by [`Agent::set_effort`].
    /// Threaded onto every `LlmRequest` as `config.effort` via
    /// [`cap_effort_for_model`].
    active_effort: String,
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
    /// Only populated in the `ToolCtx` when `features.repl == true`.
    /// Cleaned up (process killed) when the `PythonRepl` is dropped, which
    /// happens when the `Arc` reference count reaches zero — either at
    /// `Agent::drop` or when all outstanding `ToolCtx` handles are dropped.
    python_repl: Arc<tokio::sync::Mutex<Option<PythonRepl>>>,
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
        // Extract before moving config into Self.  None → default (both off);
        // strict_resume passes Some(recovered_flags) to skip from_env().
        let features = config.features.unwrap_or_default();
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
            system_blocks: Vec::new(),
            system_prompt_paths: Arc::new(HashSet::new()),
            history: Vec::new(),
            context_hashes: Vec::new(),
            features,
            python_repl: Arc::new(tokio::sync::Mutex::new(None)),
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
        //    process environment.  When Some (strict-resume or tests),
        //    the caller has already supplied the right flags — skip env.
        if self.config.features.is_none() {
            self.features = FeatureFlags::from_env();
        }
        eprintln!(
            "feature flags: repl={} subagents={}",
            self.features.repl, self.features.subagents
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
        let max_tokens = max_output_tokens_for_model(&self.active_model);
        self.system_blocks = build_system_blocks(
            &self.config.cwd.to_string_lossy(),
            max_tokens,
            self.config.headless,
            &files,
            self.features.repl,
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
            model: self.active_model.clone(),
            effort: self.active_effort.clone(),
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
    /// Called by [`strict_resume`](crate::session_resume::strict_resume).
    pub fn init_for_resume(&mut self, system_prompt: String) {
        // A single synthetic block with no source_path.  join_blocks()
        // on a one-element slice returns the content unchanged, so
        // domain_snapshot().system_prompt round-trips exactly.
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
    /// Used by [`Self::domain_snapshot`] to capture the system-prompt
    /// component of domain state.
    #[must_use]
    pub fn system_prompt(&self) -> String {
        join_blocks(&self.system_blocks)
    }

    /// Snapshot the domain state of this agent.
    ///
    /// **Domain state** contains every field that could be observed by the
    /// LLM (future turns) or by the UI / user (past display).  Fields that
    /// are merely plumbing — connection handles, reconstructed stores,
    /// per-process controls — are excluded.
    ///
    /// The implementation uses an exhaustive `let Self { … } = self;`
    /// destructuring with **no** `..` rest pattern.  This forces every future
    /// field addition to be explicitly classified at compile time — there is
    /// no silent "we're not checking it" bucket.
    ///
    /// See also: `docs/session-design.html#domain-state`.
    #[must_use]
    pub fn domain_snapshot(&self) -> DomainSnapshot {
        let Self {
            active_model,
            active_effort,
            history,
            context_hashes,
            // Content captured as `self.system_prompt()` in the return value
            // below.  Using the getter rather than join_blocks directly means
            // mutations to system_prompt() are observable by mutation testing.
            system_blocks: _,
            // Process-bound; provided by the caller of strict_resume.
            // Does not influence future LLM runs — the provider is a
            // transport layer, not session state.
            provider: _,
            // Reconstructed deterministically from session_dir; the new
            // instance is operationally equivalent.
            context_store: _,
            // Reconstructed deterministically from session_dir; the new
            // instance is operationally equivalent.
            event_store: _,
            // Intra-turn control handles; always start fresh at a resumable
            // boundary.  Future turns begin a new turn lifecycle regardless.
            controls: _,
            // Provided by the caller of strict_resume; part of how this
            // process was started, not of the agent's state.
            //
            // NOTE [oq-cwd-carveout]: config.cwd is a latent carve-out from
            // domain-relevant ⇒ persisted.  If a session is resumed with a
            // different cwd, future tool calls (path resolution, AGENTS.md
            // discovery) would behave differently.  Flagged in
            // docs/session-design.html#oq-cwd-carveout for a future fix.
            config: _,
            // Derived deterministically from system_blocks; the new instance
            // is operationally equivalent.
            system_prompt_paths: _,
            // Features determine which tools are exposed to the LLM on
            // every call — a difference here is observable in future turns.
            // Domain state: restored from SessionStartedEvent on strict resume.
            features,
            // python_repl: NOT in DomainSnapshot.
            // The kernel state is domain-relevant (variables persist
            // across turns), and the code that produced it is persisted
            // (every python_repl tool call is in events.jsonl).  But this
            // MVP does not replay tool calls on resume — strict_resume
            // refuses REPL sessions instead.  See [oq-repl-replay] in
            // docs/session-design.html for the principled future fix.
            python_repl: _,
        } = self;
        DomainSnapshot {
            active_model: active_model.clone(),
            active_effort: active_effort.clone(),
            history: history.clone(),
            context_hashes: context_hashes.clone(),
            // Call system_prompt() rather than join_blocks(system_blocks)
            // directly so that mutations to system_prompt() are visible
            // to the test suite through domain_snapshot().
            system_prompt: self.system_prompt(),
            features: *features,
        }
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
                for (_id, name) in dangling {
                    // Synthetic ToolResult for a tool_use the previous
                    // (interrupted) turn never executed.  No surviving
                    // ToolCallEvent exists to correlate against, so mint
                    // a fresh tool_call_id; consumers will see this
                    // result as unmatched, which is accurate.
                    let ev = OmegaEvent::ToolResult(ToolResultEvent {
                        time: now_iso(),
                        tool_call_id: gen_call_id(),
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
                let system_blocks: Vec<String> = self
                    .system_blocks
                    .iter()
                    .map(|b| b.content.clone())
                    .collect();
                let request = LlmRequest {
                    model: self.active_model.clone(),
                    messages: self.history.clone(),
                    system: Some(system_blocks),
                    tools: tool_definitions(self.features.repl),
                    config: ModelConfig {
                        max_tokens,
                        temperature: None,
                        thinking_budget: None,
                        adaptive_thinking: true,
                        effort: Some(
                            cap_effort_for_model(
                                &self.active_effort,
                                &self.active_model,
                            )
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
                    model: self.active_model.clone(),
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
                    // Pass the python_repl Arc into the tool context when the
                    // REPL feature is enabled.  The outer Option<Arc<...>> is
                    // None when features.repl=false so execute_tool knows the
                    // feature is disabled and can return a clear error.
                    let python_repl_opt = if self.features.repl {
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
                            async move {
                                let start = Instant::now();
                                let ctx = ToolCtx {
                                    cache_dir,
                                    tool_call_id: tool_call_id.clone(),
                                    system_prompt_paths,
                                    python_repl,
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

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Extract compaction token counts from a `usage.iterations` slice.
///
/// Returns `(tokens_before, tokens_after, summary_tokens)` where:
/// - `tokens_before` — `input_tokens` of the `compaction` iteration
///   (old context fed to the summariser; the "before" figure).
/// - `tokens_after`  — `input_tokens` of the `message` iteration
///   (new, compacted baseline; the "after" figure).
/// - `summary_tokens` — `output_tokens` of the `compaction` iteration
///   (tokens produced by the summariser).
///
/// Any missing iteration contributes `0` to the respective field.
fn extract_compaction_tokens(iters: &[UsageIteration]) -> (i64, i64, i64) {
    let compaction = iters.iter().find(|it| it.iteration_type == "compaction");
    let message = iters.iter().find(|it| it.iteration_type == "message");
    (
        compaction.map_or(0, |it| it.input_tokens),
        message.map_or(0, |it| it.input_tokens),
        compaction.map_or(0, |it| it.output_tokens),
    )
}

/// Generate an 8-character lowercase hex string from 4 random bytes.
///
/// Used as the per-tool-call identifier recorded in `events.jsonl` and
/// embedded in tee-log filenames so that the two are bidirectionally
/// cross-referenceable without knowing the LLM provider's ID format.
fn gen_call_id() -> String {
    let bytes: [u8; 4] = rand::random();
    bytes.iter().fold(String::with_capacity(8), |mut s, b| {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Build an elided (non-wall-of-text) summary of an [`LlmRequest`] for
/// the `request_summary` field of [`LlmCallEvent`].
///
/// Mirrors `elideAnthropicRequest` in the TypeScript reference
/// (`src/agent.ts`, commits 50622a9 / 5f1e40a).
///
/// * `system`  → `"[N block(s), X chars, cache_control: ephemeral]"`
///   (the last system block always carries the cache marker)
/// * `tools`   → array of `{name, description: "[N chars]", input_schema:
///               "[elided]"}` with `cache_control: "ephemeral"` on the last
///   entry (matches the wire format produced by `build_wire_tools`)
/// * `messages` → `"[N message(s), X chars, cache_control on msg[N-1]]"`
///   (the last content block of the last message always carries the marker)
/// * Top-level scalar fields (`model`, `max_tokens`, `thinking`, …) are
///   forwarded verbatim.
fn elide_request(req: &LlmRequest) -> Value {
    use serde_json::{Map, json};

    // ---- system ---------------------------------------------------------
    // The last system block always receives `cache_control: ephemeral`
    // (see `build_system_blocks` in omega-core/src/anthropic.rs).
    let system_val = if let Some(sys) = &req.system {
        let blocks = sys.len();
        let chars: usize = sys.iter().map(|b| b.chars().count()).sum();
        let label = if blocks == 1 { "block" } else { "blocks" };
        Value::String(format!(
            "[{blocks} {label}, {chars} chars, cache_control: ephemeral]"
        ))
    } else {
        Value::Null
    };

    // ---- tools ----------------------------------------------------------
    // The last tool definition always receives `cache_control: ephemeral`
    // (see `build_wire_tools` in omega-core/src/anthropic.rs).
    let last_tool_idx = req.tools.len().saturating_sub(1);
    let tools_val: Vec<Value> = req
        .tools
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let desc_chars = t.description.chars().count();
            if i == last_tool_idx {
                json!({
                    "name": t.name,
                    "description": format!("[{desc_chars} chars]"),
                    "input_schema": "[elided]",
                    "cache_control": "ephemeral",
                })
            } else {
                json!({
                    "name": t.name,
                    "description": format!("[{desc_chars} chars]"),
                    "input_schema": "[elided]",
                })
            }
        })
        .collect();

    // ---- messages -------------------------------------------------------
    // The last content block of the last message always receives
    // `cache_control: ephemeral` (see `build_wire_messages` in
    // omega-core/src/anthropic.rs).
    let msg_count = req.messages.len();
    let msg_label = if msg_count == 1 {
        "message"
    } else {
        "messages"
    };
    let msg_chars = serde_json::to_string(&req.messages).map_or(0, |s| s.chars().count());
    let cache_note = if msg_count > 0 {
        format!(", cache_control on msg[{}]", msg_count - 1)
    } else {
        String::new()
    };
    let messages_val = Value::String(format!(
        "[{msg_count} {msg_label}, {msg_chars} chars{cache_note}]"
    ));

    // ---- top-level scalars ----------------------------------------------
    let mut map = Map::new();
    map.insert("model".to_owned(), Value::String(req.model.clone()));
    map.insert(
        "max_tokens".to_owned(),
        Value::Number(req.config.max_tokens.into()),
    );
    if let Some(n) = req
        .config
        .temperature
        .and_then(|t| serde_json::Number::from_f64(f64::from(t)))
    {
        map.insert("temperature".to_owned(), Value::Number(n));
    }
    // thinking: adaptive or budget
    if req.config.adaptive_thinking {
        map.insert("thinking".to_owned(), json!({ "type": "adaptive" }));
    } else if let Some(budget) = req.config.thinking_budget {
        map.insert(
            "thinking".to_owned(),
            json!({ "type": "enabled", "budget_tokens": budget }),
        );
    }
    if let Some(effort) = &req.config.effort {
        map.insert("effort".to_owned(), Value::String(effort.clone()));
    }
    if let Some(cm) = &req.context_management {
        map.insert("context_management".to_owned(), cm.clone());
    }
    // elided compound fields
    map.insert("system".to_owned(), system_val);
    if !tools_val.is_empty() {
        map.insert("tools".to_owned(), Value::Array(tools_val));
    }
    map.insert("messages".to_owned(), messages_val);

    Value::Object(map)
}

#[cfg(test)]
mod gen_call_id_tests {
    //! Inline carve-out tests for [`gen_call_id`].
    //!
    //! Justification for carve-out: `gen_call_id` is a private helper whose
    //! output is embedded in `LlmCallEvent.tool_call_id` and tee-log filenames.
    //! Asserting the exact length/alphabet via the e2e surface (`MockProvider`)
    //! would require parsing event payloads from a full agent run, adding
    //! substantial setup for a property that is far simpler to pin inline.
    //! The uniqueness property also relies on randomness, which the e2e surface
    //! cannot control.

    use super::gen_call_id;

    #[test]
    fn gen_call_id_returns_exactly_8_hex_chars() {
        let id = gen_call_id();
        assert_eq!(id.len(), 8, "expected 8 chars, got {id:?}");
        assert!(
            id.chars().all(|c| c.is_ascii_hexdigit()),
            "non-hex character in {id:?}"
        );
        // lowercase only — `{b:02x}` produces lowercase
        assert_eq!(id, id.to_ascii_lowercase(), "must be lowercase hex: {id:?}");
    }

    #[test]
    fn gen_call_id_successive_calls_differ() {
        // With 4 random bytes per call the probability of collision in two
        // successive calls is 1 / 2^32, which is negligible in CI.
        let a = gen_call_id();
        let b = gen_call_id();
        assert_ne!(
            a, b,
            "two successive gen_call_id() calls produced the same value: {a:?}"
        );
    }
}

#[cfg(test)]
mod elide_request_tests {
    //! Inline carve-out tests for [`elide_request`].
    //!
    //! Justification for carve-out: `elide_request` is a private pure function
    //! whose pluralisation and empty-tools branches are not directly observable
    //! downstream (CLI/server e2e tests don't snapshot
    //! `LlmCall.request_summary`).  These tests pin the branches that survive
    //! `cargo mutants -p omega-agent` otherwise.

    #![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

    use super::elide_request;
    use omega_core::{ContentBlock, LlmRequest, Message, ModelConfig, Role, ToolDefinition};

    fn user_msg(text: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: text.to_owned(),
            }],
        }
    }

    fn make_request(messages: Vec<Message>, tools: Vec<ToolDefinition>) -> LlmRequest {
        LlmRequest {
            model: "claude-sonnet-4-6".to_owned(),
            messages,
            system: Some(vec!["hello".to_owned()]),
            tools,
            config: ModelConfig::default(),
            context_management: None,
        }
    }

    #[test]
    fn singular_message_label() {
        let req = make_request(vec![user_msg("hi")], vec![]);
        let v = elide_request(&req);
        let s = v["messages"].as_str().expect("string");
        assert!(s.starts_with("[1 message,"), "singular: {s}");
        assert!(!s.contains("messages,"), "plural leaked: {s}");
    }

    #[test]
    fn plural_messages_label() {
        let req = make_request(vec![user_msg("a"), user_msg("b")], vec![]);
        let v = elide_request(&req);
        let s = v["messages"].as_str().expect("string");
        assert!(s.starts_with("[2 messages,"), "plural: {s}");
    }

    #[test]
    fn messages_label_includes_cache_control_note() {
        let req = make_request(vec![user_msg("a"), user_msg("b")], vec![]);
        let v = elide_request(&req);
        let s = v["messages"].as_str().expect("string");
        assert!(s.contains("cache_control on msg[1]"), "cache note: {s}");
    }

    #[test]
    fn empty_messages_label_has_no_cache_note() {
        let req = make_request(vec![], vec![]);
        let v = elide_request(&req);
        let s = v["messages"].as_str().expect("string");
        assert!(!s.contains("cache_control"), "unexpected cache note: {s}");
    }

    #[test]
    fn singular_system_block_label() {
        let req = make_request(vec![user_msg("hi")], vec![]);
        let v = elide_request(&req);
        let s = v["system"].as_str().expect("string");
        assert!(s.starts_with("[1 block,"), "singular: {s}");
        assert!(!s.contains("blocks,"), "plural leaked: {s}");
    }

    #[test]
    fn system_label_includes_cache_control() {
        let req = make_request(vec![user_msg("hi")], vec![]);
        let v = elide_request(&req);
        let s = v["system"].as_str().expect("string");
        assert!(s.contains("cache_control: ephemeral"), "cache missing: {s}");
    }

    #[test]
    fn empty_tools_omits_tools_key() {
        let req = make_request(vec![user_msg("hi")], vec![]);
        let v = elide_request(&req);
        assert!(
            v.as_object().expect("object").get("tools").is_none(),
            "empty tools must not produce a `tools` key, got {v:?}"
        );
    }

    #[test]
    fn non_empty_tools_includes_tools_key() {
        let tool = ToolDefinition {
            name: "read_file".to_owned(),
            description: "reads a file".to_owned(),
            input_schema: serde_json::json!({}),
        };
        let req = make_request(vec![user_msg("hi")], vec![tool]);
        let v = elide_request(&req);
        let arr = v["tools"].as_array().expect("tools array");
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["name"], "read_file");
        assert_eq!(arr[0]["description"], "[12 chars]");
    }

    #[test]
    fn last_tool_has_cache_control() {
        let tool_a = ToolDefinition {
            name: "tool_a".to_owned(),
            description: "first".to_owned(),
            input_schema: serde_json::json!({}),
        };
        let tool_b = ToolDefinition {
            name: "tool_b".to_owned(),
            description: "second".to_owned(),
            input_schema: serde_json::json!({}),
        };
        let req = make_request(vec![user_msg("hi")], vec![tool_a, tool_b]);
        let v = elide_request(&req);
        let arr = v["tools"].as_array().expect("tools array");
        assert_eq!(arr.len(), 2);
        assert!(
            arr[0].get("cache_control").is_none(),
            "first tool must not have cache_control"
        );
        assert_eq!(
            arr[1]["cache_control"], "ephemeral",
            "last tool must have cache_control"
        );
    }

    #[test]
    fn single_tool_has_cache_control() {
        let tool = ToolDefinition {
            name: "only".to_owned(),
            description: "sole tool".to_owned(),
            input_schema: serde_json::json!({}),
        };
        let req = make_request(vec![user_msg("hi")], vec![tool]);
        let v = elide_request(&req);
        let arr = v["tools"].as_array().expect("tools array");
        assert_eq!(arr[0]["cache_control"], "ephemeral");
    }
}

#[cfg(test)]
mod abandonment_closer_tests {
    //! Inline tests pinning [`make_abandonment_closers`]'s emission contract
    //! (SCHEMA-8 Phase 3 commit 3d).
    //!
    //! Justification for carve-out: `make_abandonment_closers` is a private
    //! function exercised at mid-stream retry time.  The integration tests in
    //! `tests/internal.rs` exercise only the streaming-loop wiring around this
    //! helper; the per-slot emission decisions (text/thinking/tool-use empty vs.
    //! non-empty, sealed vs. unsealed) are not observable through
    //! `Agent::send_message` / `MockProvider` without constructing specific
    //! slot maps that the real loop cannot easily produce.
    //!
    //! The integration tests in `tests/internal.rs` exercise only the
    //! streaming-loop wiring around this helper, not the per-slot emission
    //! decisions.  Phase 8 (`cargo mutants -p omega-agent`) flagged seven
    //! survivors that escape the integration tests:
    //!
    //! * the `!text.is_empty()` guard (3 mutants: replace-with-true,
    //!   replace-with-false, `delete !`)
    //! * the `!thinking.is_empty()` guard (3 mutants)
    //! * the `BlockSlot::ToolUse { sealed: false, .. }` match arm (1 mutant
    //!   `delete match arm`)
    //!
    //! The first two groups are real gaps: the existing
    //! `script_mid_stream_retry` golden replays mid-stream retry but its
    //! oracle is `context.jsonl` byte-equality, which says nothing about
    //! the `events.jsonl` partial-block emission decisions.  The third
    //! group is defensive code that the current stream loop can never
    //! reach (`insert_tool_use_slot` always seals on insert) but whose
    //! contract is part of Phase 3 commit 3d's spec — we pin it here so
    //! future changes to the seal discipline can't silently drop it.

    #![allow(
        clippy::expect_used,
        clippy::unwrap_used,
        clippy::panic,
        clippy::wildcard_enum_match_arm
    )]

    use super::{BlockSlot, make_abandonment_closers};
    use omega_types::OmegaEvent;
    use serde_json::json;
    use std::collections::BTreeMap;

    fn expect_discarded(ev: &OmegaEvent) {
        match ev {
            OmegaEvent::LlmResponseDiscarded(_) => {}
            other => panic!("expected LlmResponseDiscarded, got {other:?}"),
        }
    }

    #[test]
    fn empty_slot_map_emits_only_discarded_marker() {
        // The closer pair degrades to a single marker when nothing was
        // accumulated before the abandon.
        let events = make_abandonment_closers(BTreeMap::new());
        assert_eq!(events.len(), 1);
        expect_discarded(&events[0]);
    }

    #[test]
    fn unsealed_nonempty_text_slot_emits_partial_text_block() {
        // Catches `agent.rs:215:18 !text.is_empty() -> false` and the
        // matching `delete !` mutation: with either mutation the partial
        // TextBlock disappears and only LlmResponseDiscarded would remain.
        let mut slots = BTreeMap::new();
        slots.insert(
            0,
            BlockSlot::Text {
                text: "hello world".to_owned(),
                sealed: false,
            },
        );
        let events = make_abandonment_closers(slots);
        assert_eq!(events.len(), 2);
        match &events[0] {
            OmegaEvent::TextBlock(t) => {
                assert_eq!(t.text, "hello world");
                assert!(t.partial, "abandonment TextBlock must be partial");
            }
            other => panic!("expected TextBlock, got {other:?}"),
        }
        expect_discarded(&events[1]);
    }

    #[test]
    fn unsealed_empty_text_slot_emits_no_text_block() {
        // Catches `agent.rs:215:18 !text.is_empty() -> true`: with the
        // mutation an empty TextBlock would slip through.
        let mut slots = BTreeMap::new();
        slots.insert(
            0,
            BlockSlot::Text {
                text: String::new(),
                sealed: false,
            },
        );
        let events = make_abandonment_closers(slots);
        assert_eq!(
            events.len(),
            1,
            "empty text slot must not emit a TextBlock event"
        );
        expect_discarded(&events[0]);
    }

    #[test]
    fn sealed_text_slot_is_skipped() {
        // A sealed slot has already had its `partial:false` TextBlock
        // emitted on `TextBlockComplete`; the closer must not re-emit.
        let mut slots = BTreeMap::new();
        slots.insert(
            0,
            BlockSlot::Text {
                text: "complete".to_owned(),
                sealed: true,
            },
        );
        let events = make_abandonment_closers(slots);
        assert_eq!(events.len(), 1);
        expect_discarded(&events[0]);
    }

    #[test]
    fn unsealed_nonempty_thinking_slot_emits_partial_thinking_block() {
        // Catches `agent.rs:224:18 !thinking.is_empty() -> false` and
        // the matching `delete !` mutation.
        let mut slots = BTreeMap::new();
        slots.insert(
            0,
            BlockSlot::Thinking {
                thinking: "deep thought".to_owned(),
                signature: None,
                sealed: false,
            },
        );
        let events = make_abandonment_closers(slots);
        assert_eq!(events.len(), 2);
        match &events[0] {
            OmegaEvent::ThinkingBlock(t) => {
                assert_eq!(t.thinking, "deep thought");
                assert_eq!(t.signature, None);
                assert!(t.partial, "abandonment ThinkingBlock must be partial");
            }
            other => panic!("expected ThinkingBlock, got {other:?}"),
        }
        expect_discarded(&events[1]);
    }

    #[test]
    fn unsealed_thinking_slot_preserves_signature_when_present() {
        // The signature can arrive on `signature_delta` before the
        // model stops streaming; abandonment must forward it untouched.
        let mut slots = BTreeMap::new();
        slots.insert(
            0,
            BlockSlot::Thinking {
                thinking: "half-baked".to_owned(),
                signature: Some("sig-xyz".to_owned()),
                sealed: false,
            },
        );
        let events = make_abandonment_closers(slots);
        match &events[0] {
            OmegaEvent::ThinkingBlock(t) => {
                assert_eq!(t.signature.as_deref(), Some("sig-xyz"));
                assert!(t.partial);
            }
            other => panic!("expected ThinkingBlock, got {other:?}"),
        }
    }

    #[test]
    fn unsealed_empty_thinking_slot_emits_no_thinking_block() {
        // Catches `agent.rs:224:18 !thinking.is_empty() -> true`.
        let mut slots = BTreeMap::new();
        slots.insert(
            0,
            BlockSlot::Thinking {
                thinking: String::new(),
                signature: None,
                sealed: false,
            },
        );
        let events = make_abandonment_closers(slots);
        assert_eq!(
            events.len(),
            1,
            "empty thinking slot must not emit a ThinkingBlock event"
        );
        expect_discarded(&events[0]);
    }

    #[test]
    fn sealed_thinking_slot_is_skipped() {
        let mut slots = BTreeMap::new();
        slots.insert(
            0,
            BlockSlot::Thinking {
                thinking: "complete".to_owned(),
                signature: Some("sig".to_owned()),
                sealed: true,
            },
        );
        let events = make_abandonment_closers(slots);
        assert_eq!(events.len(), 1);
        expect_discarded(&events[0]);
    }

    #[test]
    fn unsealed_tool_use_slot_emits_partial_tool_use_block() {
        // Catches `agent.rs:230:13 delete match arm BlockSlot::ToolUse { sealed: false, .. }`.
        //
        // The current stream loop never produces an unsealed ToolUse
        // slot (`insert_tool_use_slot` always sets `sealed: true`), but
        // SCHEMA-8 Phase 3 commit 3d's contract still covers this case
        // for forward compatibility (e.g. partial `input_json` arriving
        // at abandonment time in a future schema).  Constructing the
        // slot map directly is the only way to exercise the arm.
        let mut slots = BTreeMap::new();
        slots.insert(
            0,
            BlockSlot::ToolUse {
                tool_call_id: "tc-1".to_owned(),
                tool_use_id: "tool-id-1".to_owned(),
                name: "calc".to_owned(),
                input: json!({"x": 1}),
                sealed: false,
            },
        );
        let events = make_abandonment_closers(slots);
        assert_eq!(events.len(), 2);
        match &events[0] {
            OmegaEvent::ToolUseBlock(t) => {
                assert_eq!(t.tool_call_id, "tc-1");
                assert_eq!(t.tool_use_id, "tool-id-1");
                assert_eq!(t.name, "calc");
                assert_eq!(t.input, json!({"x": 1}));
                assert!(t.partial, "abandonment ToolUseBlock must be partial");
            }
            other => panic!("expected ToolUseBlock, got {other:?}"),
        }
        expect_discarded(&events[1]);
    }

    #[test]
    fn sealed_tool_use_slot_is_skipped() {
        // The normal stream path: ToolUseBlockComplete inserts the slot
        // sealed and emits a `partial:false` ToolUseBlock immediately,
        // so abandonment must not re-emit.
        let mut slots = BTreeMap::new();
        slots.insert(
            0,
            BlockSlot::ToolUse {
                tool_call_id: "tc-1".to_owned(),
                tool_use_id: "tool-id-1".to_owned(),
                name: "calc".to_owned(),
                input: json!({}),
                sealed: true,
            },
        );
        let events = make_abandonment_closers(slots);
        assert_eq!(events.len(), 1);
        expect_discarded(&events[0]);
    }

    #[test]
    fn mixed_slots_emit_in_block_index_order() {
        // Phase 2's wire-shape invariant: assistant blocks are persisted
        // in API-declared index order.  The closer pair must respect
        // the same order even when slots are inserted out of order.
        let mut slots = BTreeMap::new();
        slots.insert(
            2,
            BlockSlot::ToolUse {
                tool_call_id: "tc-1".to_owned(),
                tool_use_id: "tu".to_owned(),
                name: "n".to_owned(),
                input: json!({}),
                sealed: false,
            },
        );
        slots.insert(
            0,
            BlockSlot::Text {
                text: "t0".to_owned(),
                sealed: false,
            },
        );
        slots.insert(
            1,
            BlockSlot::Thinking {
                thinking: "th1".to_owned(),
                signature: None,
                sealed: false,
            },
        );
        let events = make_abandonment_closers(slots);
        assert_eq!(events.len(), 4);
        assert!(matches!(events[0], OmegaEvent::TextBlock(_)));
        assert!(matches!(events[1], OmegaEvent::ThinkingBlock(_)));
        assert!(matches!(events[2], OmegaEvent::ToolUseBlock(_)));
        expect_discarded(&events[3]);
    }
}
