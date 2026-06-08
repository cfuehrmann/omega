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
    AgentErrorEvent, ContextCompactedEvent, EffortChangedEvent, HarnessRecoveryEvent,
    HarnessRecoveryKind, LlmCallEvent, LlmErrorEvent, LlmResponseDiscardedEvent,
    LlmResponseEndedEvent, LlmResponseStartedEvent, ModelChangedEvent, MonitorDeliveryEvent,
    MonitorDeliveryItem, MonitorStartedEvent, MonitorStopReason, MonitorStoppedEvent,
    ResumingSessionEvent, ServerStartedEvent, SessionResumedEvent, SessionStartedEvent,
    TextBlockEvent, ThinkingBlockEvent, ToolCallEvent, ToolResultEvent, ToolUseBlockEvent,
    TurnEndEvent, TurnHaltedEvent, TurnInterruptedEvent, TurnResumedEvent, UsageIteration,
    UserMessageEvent,
};
use omega_types::ids::{Origin, SessionId};
use omega_types::{InterruptReason, OmegaEvent, TurnMetrics};

use omega_store::{ContextHash, ContextStore, EventStore};
use omega_tools::{MonitorManager, PythonRepl, ToolCtx, execute_tool, tool_definitions};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::config::{cap_effort_for_model, max_output_tokens_for_model};
use crate::controls::{ControlHandle, TurnGuard};
use crate::error_classify::{is_context_too_long, is_invalid_tool_json};
use crate::event_sink::{EventBroadcaster, EventSink};
use crate::input_queue::{InboxSink, InputQueue};
use crate::session_resume::{
    RESUMPTION_EFFORT, RESUMPTION_MAX_TOKENS, RESUMPTION_MODEL, RESUMPTION_SUMMARY_INSTRUCTIONS,
    extract_summary_from_response,
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
///
/// `#[mutants::skip]`: This function mutates a slot stored inside a
/// `BTreeMap` that is private to the streaming accumulation loop.  Its
/// observable effect (setting `sealed = true`) is only detectable
/// through the abandonment-closer path, which requires the streaming
/// signal path to be exercised.  The `MockProvider`-based tests bypass
/// real SSE parsing and never emit raw `Signal::TextBlockComplete`
/// events, so the sealed/unsealed distinction is invisible to them.
/// Covered by the CLI / server end-to-end suites instead.
#[mutants::skip]
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
///
/// `#[mutants::skip]` on the body: the returned `tool_call_id` is
/// a generated correlation key used internally; tests do not assert
/// its exact value.  The slot-insertion side-effect is exercised only
/// through the real SSE signal path (`Signal::ToolUseBlockStart`),
/// which `MockProvider` bypasses.  Covered by CLI/server e2e suites.
#[mutants::skip]
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

// ---------------------------------------------------------------------------
// Phase 0 — free helpers
// ---------------------------------------------------------------------------

/// Project `history` into the `messages` array sent to the LLM API.
///
/// Merges consecutive `role: user` entries into a single `Message` by
/// concatenating their content-block lists.  This satisfies the design
/// invariant from the monitors spec (§7):
///
/// > Consecutive user-role events (`UserMessage`, `MonitorDelivery`,
/// > tool-results) project as ONE merged API message.
///
/// `events.jsonl` retains the individual events; only the API view is
/// collapsed.  Role-alternating sequences are emitted unchanged.
pub(crate) fn project_messages(history: &[Message]) -> Vec<Message> {
    let mut result: Vec<Message> = Vec::with_capacity(history.len());
    for msg in history {
        match result.last_mut() {
            Some(last) if last.role == Role::User && msg.role == Role::User => {
                // Both consecutive messages are role:user — merge content.
                last.content.extend(msg.content.clone());
            }
            _ => result.push(msg.clone()),
        }
    }
    result
}

/// Format monitor stdout lines for injection into the LLM context.
///
/// The XML-style tag unambiguously marks the text as automated monitor
/// output — not a human user message — even when it lands in a merged
/// `role:user` turn alongside real human text.
fn format_monitor_lines(monitor_id: &str, lines: &[String]) -> String {
    if lines.is_empty() {
        format!("<monitor id=\"{monitor_id}\">\n</monitor>")
    } else {
        format!(
            "<monitor id=\"{monitor_id}\">\n{}\n</monitor>",
            lines.join("\n")
        )
    }
}

/// Format a `MonitorStopped` notification for injection into the LLM context.
///
/// The self-closing tag keeps the same unambiguous framing as
/// [`format_monitor_lines`] and avoids the weaker `[Monitor …]` bracket
/// syntax that models could mistake for user prose.
fn format_monitor_stopped(id: &str, reason: &MonitorStopReason, exit_code: Option<i32>) -> String {
    let reason_str = match reason {
        MonitorStopReason::StoppedByAgent => "stopped_by_agent",
        MonitorStopReason::StoppedByUser => "stopped_by_user",
        MonitorStopReason::ProcessExited => "process_exited",
        MonitorStopReason::ProcessCrashed => "process_crashed",
        MonitorStopReason::StoppedBySessionEnd => "stopped_by_session_end",
    };
    match exit_code {
        Some(code) => {
            format!("<monitor-stopped id=\"{id}\" reason=\"{reason_str}\" exit-code=\"{code}\"/>")
        }
        None => format!("<monitor-stopped id=\"{id}\" reason=\"{reason_str}\"/>"),
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
            time: now_iso(),
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
            time: now_iso(),
            effort,
        });
        self.event_sink.emit(ev).await
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        // Session-end backstop (§4): reap every monitor tree so no grandchild
        // is orphaned.  Idempotent with an explicit `shutdown_monitors`.
        let _ = self.monitors.shutdown();
    }
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
        let event_sink = Arc::new(EventSink::new(Arc::clone(&event_store)));
        let controls = ControlHandle::new(Arc::clone(&event_sink));
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
            monitors: MonitorManager::new(),
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
    async fn inject_input_item(&mut self, item: InputItem) -> omega_store::Result<OmegaEvent> {
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

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// `#[mutants::skip]`: timestamp value (not format) is not asserted by
/// any in-process test — the format is verified indirectly in events.jsonl
/// assertion tests, but the mutation survivors produce wrong *values*,
/// not wrong formats.  CLI/server e2e suites verify real timestamps.
#[mutants::skip]
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

#[cfg(test)]
mod format_monitor_tests {
    //! Inline carve-out tests for [`format_monitor_lines`] and
    //! [`format_monitor_stopped`].
    //!
    //! Justification for carve-out: these are private pure functions whose
    //! exact output format is the load-bearing nudging mechanism (the
    //! `<monitor …>` / `<monitor-stopped …/>` tags).  The integration tests in
    //! `tests/internal.rs` verify that the text reaches the LLM API, but
    //! cannot easily pin the exact wrapper format without inspecting the raw
    //! string — which is done here so a mutation to the tag shape is caught
    //! immediately rather than via a text-contains search.

    use super::*;
    use omega_types::events::MonitorStopReason;

    // ── format_monitor_lines ──────────────────────────────────────────────

    /// Non-empty lines: output must be wrapped in `<monitor id="…">…</monitor>`.
    #[test]
    fn format_monitor_lines_wraps_in_monitor_tag() {
        let out = format_monitor_lines("mon-1", &["line A".to_owned(), "line B".to_owned()]);
        assert!(
            out.starts_with("<monitor id=\"mon-1\">"),
            "must start with opening monitor tag, got: {out}"
        );
        assert!(
            out.ends_with("</monitor>"),
            "must end with closing monitor tag, got: {out}"
        );
        assert!(
            out.contains("line A"),
            "must contain first line, got: {out}"
        );
        assert!(
            out.contains("line B"),
            "must contain second line, got: {out}"
        );
    }

    /// Monitor id must appear in the opening tag attribute.
    #[test]
    fn format_monitor_lines_id_in_tag_attribute() {
        let out = format_monitor_lines("abc-42", &["x".to_owned()]);
        assert!(
            out.contains("id=\"abc-42\""),
            "monitor id must appear as an attribute in the opening tag, got: {out}"
        );
    }

    /// Empty lines: output must still use `<monitor …>` tags, not brackets.
    #[test]
    fn format_monitor_lines_empty_no_bracket_marker() {
        let out = format_monitor_lines("mon-2", &[]);
        assert!(
            out.starts_with("<monitor id=\"mon-2\">"),
            "empty delivery must still use opening monitor tag, got: {out}"
        );
        assert!(
            out.ends_with("</monitor>"),
            "empty delivery must still end with closing monitor tag, got: {out}"
        );
        assert!(
            !out.contains("[Monitor"),
            "output must NOT use legacy bracket markers, got: {out}"
        );
    }

    // ── format_monitor_stopped ───────────────────────────────────────────

    /// With exit code: must emit `<monitor-stopped id="…" reason="…" exit-code="…"/>`.
    #[test]
    fn format_monitor_stopped_with_exit_code() {
        let out = format_monitor_stopped("mon-3", &MonitorStopReason::ProcessExited, Some(0));
        assert!(
            out.starts_with("<monitor-stopped"),
            "must use self-closing monitor-stopped tag, got: {out}"
        );
        assert!(out.ends_with("/>"), "must be self-closing, got: {out}");
        assert!(
            out.contains("id=\"mon-3\""),
            "must contain id attribute, got: {out}"
        );
        assert!(
            out.contains("reason=\"process_exited\""),
            "must contain reason attribute, got: {out}"
        );
        assert!(
            out.contains("exit-code=\"0\""),
            "must contain exit-code attribute, got: {out}"
        );
        assert!(
            !out.contains("[Monitor"),
            "must NOT use legacy bracket markers, got: {out}"
        );
    }

    /// Without exit code: `exit-code` attribute must be absent.
    #[test]
    fn format_monitor_stopped_without_exit_code() {
        let out = format_monitor_stopped("mon-4", &MonitorStopReason::ProcessCrashed, None);
        assert!(
            out.starts_with("<monitor-stopped"),
            "must use monitor-stopped tag, got: {out}"
        );
        assert!(out.ends_with("/>"), "must be self-closing, got: {out}");
        assert!(
            !out.contains("exit-code"),
            "exit-code attribute must be absent when None, got: {out}"
        );
    }

    /// Reason enum variants map to the correct kebab-case strings.
    #[test]
    fn format_monitor_stopped_reason_variants() {
        let cases = [
            (MonitorStopReason::StoppedByAgent, "stopped_by_agent"),
            (MonitorStopReason::StoppedByUser, "stopped_by_user"),
            (MonitorStopReason::ProcessExited, "process_exited"),
            (MonitorStopReason::ProcessCrashed, "process_crashed"),
            (
                MonitorStopReason::StoppedBySessionEnd,
                "stopped_by_session_end",
            ),
        ];
        for (reason, expected) in &cases {
            let out = format_monitor_stopped("id", reason, None);
            assert!(
                out.contains(&format!("reason=\"{expected}\"")),
                "reason {expected} must appear in output, got: {out}"
            );
        }
    }
}
