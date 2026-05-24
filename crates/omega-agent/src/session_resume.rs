//! Session-resumption helpers (pure functions over event lists).
//!
//! Mirrors `src/session-resume.ts` for the parts that don't touch the
//! agent or the LLM. Phase 1d.1a ports [`extract_last_model_and_effort`];
//! Phase 1d.1b adds [`extract_resumption_basis`],
//! [`extract_summary_from_response`].
//! Phase 1d.1c adds the [`RESUMPTION_SUMMARY_INSTRUCTIONS`] system prompt
//! and the [`RESUMPTION_MODEL`] / [`RESUMPTION_EFFORT`] defaults consumed
//! by [`Agent::perform_resumption`](crate::Agent::perform_resumption).
//!
//! **Phase 2.1–2.4 — Strict resume** adds:
//! - [`is_resumable_boundary`] / [`find_last_resumable_boundary`]: the
//!   authoritative "awaiting user" predicate.
//! - [`strict_resume`]: reconstruct a live [`Agent`] from an existing
//!   session directory, folding `events.jsonl` up to the last resumable
//!   boundary and loading `context.jsonl` for history.
//! - [`StrictResumeError`]: structured errors for the strict-resume path.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use omega_core::{Message, Provider};
use omega_store::{ContextHash, ContextRecord, ContextStore, EventStore, StoreError};
use omega_types::FeatureFlags;
use omega_types::ids::LoggedEvent;
use omega_types::{OmegaEvent, events::InterruptReason};

use crate::agent::{Agent, AgentConfig, DEFAULT_EFFORT};

// ---------------------------------------------------------------------------
// DomainSnapshot — typed compile-time-enforced classification (Phase 2 follow-up)
// ---------------------------------------------------------------------------

/// A snapshot of the **domain state** of an [`Agent`] at a point in time.
///
/// Contains exactly the fields that are domain state — those whose values could
/// be observed by either of the two observers:
///
/// - The **LLM** (future turns unfold the same way).
/// - The **UI / user** (the past displays the same way).
///
/// Everything else is plumbing: the resumed process needs some technical
/// instance, but its contents need not match the predecessor's.
///
/// ## Compile-time classification contract
///
/// [`Agent::domain_snapshot`] builds a `DomainSnapshot` through an exhaustive
/// `let Self { field1, field2, _: field3, … } = self;` destructuring with
/// **no** `..` rest pattern.  This means the compiler forces every future
/// field addition to be explicitly classified as domain state (bound by name)
/// or plumbing (bound to `_`).  Silent "we're not checking it" situations are
/// impossible at the type level.
///
/// ## Domain-state mutation principle
///
/// If a domain field is ever mutated mid-session, a corresponding domain event
/// must record the change — see `ModelChanged` / `EffortChanged` for the
/// pattern.  Until a mutation mechanism exists for a field, it is restored
/// from the first `SessionStartedEvent` on strict resume.
///
/// See also: `docs/session-design.html#domain-state`.
#[derive(Debug, Clone, PartialEq)]
pub struct DomainSnapshot {
    /// Currently selected model id.  Affects every future LLM call.
    pub active_model: String,
    /// Currently selected thinking-effort level.  Affects every future LLM call.
    pub active_effort: String,
    /// In-memory message history sent on every LLM call.  Observable by the LLM.
    pub history: Vec<Message>,
    /// Hashes of the history records, in insertion order.
    /// Snapshotted onto every `LlmCall` event; observable via the event log.
    pub context_hashes: Vec<ContextHash>,
    /// Full system prompt text (all blocks joined with `\n\n`).
    /// Observable by the LLM on every call; persisted in `SessionStartedEvent`.
    pub system_prompt: String,
    /// Runtime feature flags controlling which tools are exposed to the LLM.
    /// A difference in flags changes future tool availability — domain state.
    /// Restored from `SessionStartedEvent.features` on strict resume.
    pub features: FeatureFlags,
}

// ---------------------------------------------------------------------------
// Resumption-call configuration (Phase 1d.1c)
// ---------------------------------------------------------------------------

/// System prompt used by [`Agent::perform_resumption`](crate::Agent::perform_resumption).
///
/// Mirrors `RESUMPTION_SUMMARY_INSTRUCTIONS` in `src/session-resume.ts` —
/// kept verbatim so summary quality matches the TS implementation.
pub const RESUMPTION_SUMMARY_INSTRUCTIONS: &str = "\
Summarise the coding session history below so it can be continued in a new session.

Produce a concise summary (1000\u{2013}2000 words) covering exactly what a developer needs to continue the work seamlessly:

1. **Current state** (snapshot, not narrative): which files were changed and how they currently stand, what constants/config values are set to, which plan items are done vs. pending.

2. **Next step**: the single most important thing to do next, as specifically as possible (exact file, function, or test name).

3. **Key decisions**: conclusions that should not be re-litigated \u{2014} design choices made, approaches confirmed or rejected, and why.

4. **Learnings / what not to do**: anything tried that failed and why, so the same dead ends are not re-explored.

5. **Technical anchors**: specific file paths, function/type/constant names, commit hashes, and test names relevant to continuing the work.

You must wrap your summary in a <summary></summary> block.";

/// Model used for the resumption summarisation call.
///
/// Mirrors `config.resumptionModel` in `src/config.ts`. Sonnet 4.6 is the
/// right balance of speed/cost/quality for what is fundamentally a
/// reading-comprehension task.
pub const RESUMPTION_MODEL: &str = "claude-sonnet-4-6";

/// Thinking-effort level for the resumption summarisation call.
///
/// Mirrors `config.resumptionEffort` in `src/config.ts`. `\"low\"` is
/// intentional \u2014 summarisation does not need extended reasoning.
///
/// Threaded onto the resumption `LlmRequest` as `config.effort` via
/// [`cap_effort_for_model`](crate::config::cap_effort_for_model).
pub const RESUMPTION_EFFORT: &str = "low";

/// Maximum output tokens for the resumption summarisation call.
///
/// Mirrors the `max_tokens: 4096` literal in `src/agent.ts::performResumption`.
pub const RESUMPTION_MAX_TOKENS: u32 = 4096;

// ---------------------------------------------------------------------------
// Private helpers — basis extraction
// ---------------------------------------------------------------------------

/// Return the first non-empty line of `s`, truncated to 120 chars and trimmed.
///
/// Mirrors `firstMeaningfulLine` in `src/session-resume.ts`.
fn first_meaningful_line(s: &str) -> String {
    let line = s.lines().find(|l| !l.trim().is_empty()).unwrap_or(s);
    let truncated: String = line.chars().take(120).collect();
    truncated.trim().to_owned()
}

/// Return the primary display argument for a named tool call.
///
/// Extracts the single most important argument for display purposes —
/// the path, pattern, command, URL, etc. — without duplicating full
/// input JSON. Mirrors `primaryToolArg` in `src/tools.schema.ts`.
fn primary_tool_arg(name: &str, input: &serde_json::Value) -> String {
    if input.is_null() {
        return "(none)".to_owned();
    }
    match name {
        "read_file" | "write_file" | "edit_file" | "list_files" => input
            .get("path")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_owned(),
        "find_files" => input
            .get("pattern")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_owned(),
        "run_command" | "run_background" => input
            .get("command")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_owned(),
        "grep_files" => {
            let pattern = input
                .get("pattern")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let path = input
                .get("path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            format!("{pattern} @ {path}")
        }
        "fetch_url" => input
            .get("url")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_owned(),
        "web_search" => input
            .get("query")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_owned(),
        "wait_for_output" => input
            .get("logFile")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_owned(),
        "write_stdin" => input
            .get("text")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_owned(),
        _ => serde_json::to_string(input).unwrap_or_default(),
    }
}

/// A group of events forming a single agent turn.
struct Turn {
    events: Vec<OmegaEvent>,
}

/// Group a flat event list into turns.
///
/// A turn opens when a `user_message` arrives outside any open turn.
/// It closes when `turn_end` or `turn_interrupted` is encountered.
/// A `user_message` that arrives while a turn is already open is an
/// interjection and stays inside the current turn so `project_turn` can
/// render it with the `User (mid-turn):` prefix if appropriate.
/// Events that arrive outside any turn are silently dropped.
///
/// Mirrors `groupIntoTurns` in `src/session-resume.ts`.
fn group_into_turns(events: &[OmegaEvent]) -> Vec<Turn> {
    let mut turns: Vec<Turn> = Vec::new();
    let mut in_turn = false;

    for event in events {
        match event {
            OmegaEvent::UserMessage(_) => {
                if in_turn {
                    // Mid-turn interjection — keep in the current turn.
                    if let Some(turn) = turns.last_mut() {
                        turn.events.push(event.clone());
                    }
                } else {
                    turns.push(Turn {
                        events: vec![event.clone()],
                    });
                    in_turn = true;
                }
            }
            _ => {
                if in_turn {
                    let is_end = matches!(
                        event,
                        OmegaEvent::TurnEnd(_) | OmegaEvent::TurnInterrupted(_)
                    );
                    if let Some(turn) = turns.last_mut() {
                        turn.events.push(event.clone());
                    }
                    if is_end {
                        in_turn = false;
                    }
                }
                // Events outside any turn are dropped.
            }
        }
    }

    turns
}

/// Project a single turn into a markdown string.
///
/// Tool calls are paired with their results by ID. A `user_message`
/// inside a `turn_paused` / `turn_continued` window renders with
/// the `User (mid-turn):` prefix to preserve its semantics in the summary.
///
/// Mirrors `projectTurn` in `src/session-resume.ts`.
fn project_turn(turn: &Turn, index: usize) -> String {
    let mut lines: Vec<String> = vec![format!("### Turn {index}")];

    // tool_call id → (name, input)
    let mut tool_calls: HashMap<String, (String, serde_json::Value)> = HashMap::new();
    let mut in_paused_window = false;
    // Accumulate TextBlock text; flush on LlmResponseEnded (or at end of the
    // loop for interrupted turns that never received LlmResponseEnded).
    let mut pending_text: Vec<String> = Vec::new();

    for event in &turn.events {
        match event {
            OmegaEvent::UserMessage(e) => {
                if in_paused_window {
                    lines.push(format!("\nUser (mid-turn): {}", e.content.trim()));
                } else {
                    lines.push(format!("\nUser: {}", e.content.trim()));
                }
            }
            OmegaEvent::TurnPaused(_) => {
                in_paused_window = true;
            }
            OmegaEvent::TurnContinued(_) => {
                in_paused_window = false;
            }
            OmegaEvent::TextBlock(e) => {
                pending_text.push(e.text.clone());
            }
            OmegaEvent::LlmResponseEnded(_) => {
                // Flush unconditionally: if pending_text is empty,
                // join("").trim() is "" and the inner guard suppresses
                // the push. Removing the outer is_empty() guard
                // eliminates the equivalent-mutant problem for that guard
                // (cargo-mutants § session_resume.rs survivors, guard line).
                let joined = pending_text.join("");
                pending_text.clear();
                let text = joined.trim();
                if !text.is_empty() {
                    lines.push(format!("\nAgent: {text}"));
                }
            }
            OmegaEvent::ToolCall(e) => {
                tool_calls.insert(e.tool_call_id.clone(), (e.name.clone(), e.input.clone()));
            }
            OmegaEvent::ToolResult(e) => {
                let arg = tool_calls
                    .get(&e.tool_call_id)
                    .map(|(name, input)| primary_tool_arg(name, input))
                    .unwrap_or_default();
                let arg_part = if arg.is_empty() {
                    String::new()
                } else {
                    format!(" {arg}")
                };
                let result = if e.is_error {
                    format!("error \u{2014} {}", first_meaningful_line(&e.output))
                } else {
                    "ok".to_owned()
                };
                lines.push(format!("\n  {}{} \u{2192} {}", e.name, arg_part, result));
            }
            OmegaEvent::AgentError(e) => {
                lines.push(format!("\nError: {}", e.error));
            }
            OmegaEvent::TurnInterrupted(e) => {
                if matches!(e.reason, Some(InterruptReason::Error)) {
                    lines.push("\n[Turn interrupted due to error]".to_owned());
                }
            }
            _ => {}
        }
    }

    // Flush any text not followed by LlmResponseEnded (e.g. interrupted turns).
    // Outer guard removed: join("").trim() on an empty vec is "", which the
    // inner guard already suppresses. Removing the outer guard eliminates the
    // equivalent-mutant problem (cargo-mutants § session_resume.rs survivors,
    // flush guard line).
    {
        let joined = pending_text.join("");
        let text = joined.trim();
        if !text.is_empty() {
            lines.push(format!("\nAgent: {text}"));
        }
    }

    lines.join("\n")
}

// ---------------------------------------------------------------------------
// Private helper — slice-start calculation
// ---------------------------------------------------------------------------

/// Returns the index of the first event to include in the relevant slice —
/// the position immediately *after* `resumed_idx`.
///
/// `#[mutants::skip]`: the `i + 1 → i * 1` mutation is behaviourally
/// equivalent. `session_resumed` events are transparent to
/// [`group_into_turns`]: they fall into the `_ => {}` drop branch outside
/// any open turn, so whether the relevant slice starts at `i` (the
/// `session_resumed` itself) or at `i + 1` (the next event), the rendered
/// turns are identical.
#[mutants::skip]
fn slice_start_after(resumed_idx: Option<usize>) -> usize {
    resumed_idx.map_or(0, |i| i + 1)
}

// ---------------------------------------------------------------------------
// Private helper — XML-block extraction
// ---------------------------------------------------------------------------

/// Extract the inner text of the first `<tag>…</tag>` block in `text`,
/// trimmed. Returns `None` if either tag boundary is absent.
fn extract_block(text: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&close)?;
    Some(text[start..start + end].trim().to_owned())
}

// ---------------------------------------------------------------------------
// Public: last model/effort extraction (Phase 1d.1a)
// ---------------------------------------------------------------------------

/// Scan a session's event list and return the last explicitly set model
/// and effort values.
///
/// Returns `None` for either value when no corresponding change event is
/// found, meaning the default should be used. This is a pure function —
/// no I/O. Left-to-right scan so the *latest* change wins.
///
/// Mirrors `extractLastModelAndEffort` in `src/session-resume.ts`.
#[must_use]
pub fn extract_last_model_and_effort(events: &[OmegaEvent]) -> (Option<String>, Option<String>) {
    let mut model: Option<String> = None;
    let mut effort: Option<String> = None;
    for event in events {
        match event {
            OmegaEvent::ModelChanged(ev) => model = Some(ev.model.clone()),
            OmegaEvent::EffortChanged(ev) => effort = Some(ev.effort.clone()),
            _ => {}
        }
    }
    (model, effort)
}

// ---------------------------------------------------------------------------
// Public: basis extraction (Phase 1d.1b)
// ---------------------------------------------------------------------------

/// Extract the basis text from a session's event list.
///
/// The basis is a markdown-formatted string structured for LLM readability:
///
/// ```text
/// ## Carried-forward context   (only if a prior session_resumed event exists)
/// <prior summary>
///
/// ## Session events
///
/// ### Turn 1
/// User: ...
/// Agent: ...
///   tool arg → ok
/// ```
///
/// This is a pure function — no I/O, no LLM calls.
///
/// Mirrors `extractResumptionBasis` in `src/session-resume.ts`.
#[must_use]
pub fn extract_resumption_basis(events: &[OmegaEvent]) -> String {
    let mut parts: Vec<String> = Vec::new();

    // Find the LAST session_resumed event; its summary is the carry-forward
    // context from all prior sessions. Events before it are not re-processed.
    let resumed_idx = events
        .iter()
        .rposition(|e| matches!(e, OmegaEvent::SessionResumed(_)));

    if let Some(idx) = resumed_idx
        && let OmegaEvent::SessionResumed(e) = &events[idx]
    {
        let summary = e.summary.trim();
        if !summary.is_empty() {
            parts.push(format!("## Carried-forward context\n\n{summary}"));
        }
    }

    // Only process events AFTER the last session_resumed (or all if none).
    let start = slice_start_after(resumed_idx);
    let relevant = &events[start..];
    let turns = group_into_turns(relevant);

    if !turns.is_empty() {
        let turn_strs: Vec<String> = turns
            .iter()
            .enumerate()
            .map(|(i, t)| project_turn(t, i + 1))
            .collect();
        parts.push(format!("## Session events\n\n{}", turn_strs.join("\n\n")));
    }

    if parts.is_empty() {
        return "(empty session — no turns recorded)".to_owned();
    }

    parts.join("\n\n")
}

// ---------------------------------------------------------------------------
// Public: summary extraction from LLM response (Phase 1d.1b)
// ---------------------------------------------------------------------------

/// Extract the summary from an LLM response.
///
/// Parses the `<summary>…</summary>` block if present; falls back to the
/// full response text (trimmed) when the block is absent.
///
/// Mirrors `extractSummaryFromResponse` in `src/session-resume.ts`.
#[must_use]
pub fn extract_summary_from_response(response_text: &str) -> String {
    extract_block(response_text, "summary").unwrap_or_else(|| response_text.trim().to_owned())
}

// ---------------------------------------------------------------------------
// Phase 2.1 — Resumable boundary predicate and locator
// ---------------------------------------------------------------------------

/// Returns `true` for the two event variants that mark the
/// **"awaiting user" boundary** in an Omega session.
///
/// ## Awaiting-user boundary definition
///
/// The agent is "awaiting user" — and therefore at a safe
/// **strict-resume** point — when the most recent terminal turn event
/// is one of:
///
/// - **[`TurnEnd`]** — the model completed the turn cleanly
///   (`stop_reason == "end_turn"`).  The definitive safe resume point;
///   `history` and `context.jsonl` are fully consistent with no
///   dangling references.
///
/// - **[`TurnInterrupted`]** — the turn ended via user abort
///   (`reason: Aborted`) or an internal error (`reason: Error`).
///   Still a resumable boundary: the partial turn is silently discarded
///   on resume, and `send_message`’s **dangling-tool-use repair** (Step 1)
///   handles any incomplete assistant record left in `context.jsonl`.
///
/// Both variants close the intra-turn lifecycle.  Neither can appear
/// mid-turn; each appears at most once at a turn’s very end.
///
/// This function is the **single authoritative place** that defines
/// “awaiting user” for the resume path.  All resume logic must go through
/// it rather than pattern-matching on `TurnEnd`/`TurnInterrupted` directly.
///
/// [`TurnEnd`]: omega_types::OmegaEvent::TurnEnd
/// [`TurnInterrupted`]: omega_types::OmegaEvent::TurnInterrupted
#[must_use]
pub fn is_resumable_boundary(event: &OmegaEvent) -> bool {
    matches!(
        event,
        OmegaEvent::TurnEnd(_) | OmegaEvent::TurnInterrupted(_)
    )
}

/// Return the index of the last "awaiting user" boundary event in
/// `events`, or `None` if no such event exists.
///
/// Scans right-to-left (O(n), one pass) so a single call finds the
/// latest boundary without revisiting earlier ones.
///
/// A `None` result means the session has never completed or interrupted
/// a turn — strict resume is not possible, and the caller must return or
/// propagate [`StrictResumeError::NoResumableBoundary`].
#[must_use]
pub fn find_last_resumable_boundary(events: &[OmegaEvent]) -> Option<usize> {
    events.iter().rposition(is_resumable_boundary)
}

// ---------------------------------------------------------------------------
// Phase 2.2 — State-folding helpers (private)
// ---------------------------------------------------------------------------

/// Compute the ordered list of context-record hashes that are
/// **"live"** at the last resumable boundary.
///
/// `events` **must** be pre-trimmed to `events[..=boundary]`; callers
/// are responsible for slicing before invoking this function.
///
/// ## Algorithm
///
/// 1. Scan left-to-right, tracking:
///    - `last_call_hashes` — the `context_hashes` field of the most
///      recent `LlmCallEvent`.  This list is the *ordered* snapshot of
///      every context record the model received on that call: all prior
///      turns’ user messages, assistant replies, and tool-result batches.
///    - `last_response_hash` — the `context_hash` of the most recent
///      `LlmResponseEndedEvent`: the assistant record written after that
///      call returned.
/// 2. When `ContextCompacted` is encountered, clear `last_call_hashes`.
///    The agent cleared `history` in response to compaction, so the
///    next `LlmResponseEndedEvent.context_hash` is the sole
///    post-compaction record.
/// 3. Return `last_call_hashes + [last_response_hash]`.  This list is
///    exactly the set of context records loaded into `history` and
///    `context_hashes` at the resumable boundary.
///
/// ## Edge cases
///
/// | Situation | Behaviour |
/// |---|---|
/// | No `LlmResponseEnded` in events | Returns `[]` (empty history) |
/// | `TurnInterrupted` before any `LlmCall` | Returns `[]` |
/// | `TurnInterrupted` after tool-results but before next `LlmCall` | Returns hashes up to last assistant; dangling repair fires on next `send_message` |
/// | Multiple `ContextCompacted` events | Each clears `last_call_hashes`; last compaction wins |
fn compute_valid_context_hashes(events: &[OmegaEvent]) -> Vec<String> {
    let mut last_call_hashes: Vec<String> = Vec::new();
    let mut last_response_hash: Option<String> = None;

    for event in events {
        match event {
            OmegaEvent::LlmCall(e) => {
                last_call_hashes.clone_from(&e.context_hashes);
            }
            OmegaEvent::ContextCompacted(_) => {
                // Server-side compaction fired.  The agent cleared
                // `history` and `context_hashes`; the very next
                // `LlmResponseEndedEvent` carries the post-compaction
                // baseline hash (a single record containing only the
                // compacted summary).
                //
                // Clearing `last_call_hashes` here ensures the result
                // `[last_call_hashes] + [response_hash]` contains only
                // the compacted record, not the pre-compaction hashes
                // that appeared in the `LlmCall` immediately before
                // compaction was detected.
                last_call_hashes.clear();
            }
            OmegaEvent::LlmResponseEnded(e) => {
                last_response_hash = Some(e.context_hash.clone());
            }
            _ => {}
        }
    }

    let Some(response_hash) = last_response_hash else {
        // No completed LLM response → empty history.
        return Vec::new();
    };

    let mut ordered = last_call_hashes;
    ordered.push(response_hash);
    ordered
}

/// Fold all model- and effort-bearing events into a `(model, effort)` pair.
///
/// Priority (highest wins):
/// 1. Last `ModelChangedEvent.model` / `EffortChangedEvent.effort`
///    — explicit overrides beat the session baseline.
/// 2. `SessionStartedEvent.model` / `.effort` — the baseline at
///    session start (the first occurrence wins as the baseline).
/// 3. Hard-coded defaults — only reached when `events.jsonl` contains
///    no `SessionStarted` event (should not happen in production, but
///    possible in tests that do not call `Agent::init`).
fn fold_model_and_effort(events: &[OmegaEvent]) -> (String, String) {
    let mut model_started: Option<String> = None;
    let mut effort_started: Option<String> = None;
    let mut model_changed: Option<String> = None;
    let mut effort_changed: Option<String> = None;

    for event in events {
        match event {
            OmegaEvent::SessionStarted(e) => {
                // First `SessionStarted` wins as the baseline; subsequent
                // ones are ignored (a session has exactly one).
                if model_started.is_none() {
                    model_started = Some(e.model.clone());
                }
                if effort_started.is_none() {
                    effort_started = Some(e.effort.clone());
                }
            }
            OmegaEvent::ModelChanged(e) => {
                model_changed = Some(e.model.clone());
            }
            OmegaEvent::EffortChanged(e) => {
                effort_changed = Some(e.effort.clone());
            }
            _ => {}
        }
    }

    let model = model_changed
        .or(model_started)
        .unwrap_or_else(|| RESUMPTION_MODEL.to_owned());
    let effort = effort_changed
        .or(effort_started)
        .unwrap_or_else(|| DEFAULT_EFFORT.to_owned());

    (model, effort)
}

// ---------------------------------------------------------------------------
// Phase 2.3 — StrictResumeError
// ---------------------------------------------------------------------------

/// Error returned by [`strict_resume`].
#[derive(Debug)]
pub enum StrictResumeError {
    /// `events.jsonl` could not be read (I/O error).
    Io(std::io::Error),

    /// `events.jsonl` contains no `SessionStartedEvent` with a non-empty
    /// `system_prompt`.
    ///
    /// Per the **schema-evolution-loud** rule in `AGENTS.md`, strict resume
    /// fails loudly rather than silently falling back to reading `AGENTS.md`
    /// from disk — that fallback would mask the missing persisted data and
    /// produce a session whose LLM-visible state differs from the original.
    MissingSystemPrompt,

    /// `events.jsonl` contains no `SessionStartedEvent` from which to recover
    /// the feature flags active at session start.
    ///
    /// Per the **schema-evolution-loud** rule in `AGENTS.md`, strict resume
    /// fails loudly rather than silently falling back to
    /// [`FeatureFlags::default`] — that fallback would mask the missing
    /// persisted data and could produce a session with a different tool set
    /// from the original.
    MissingFeatures,

    /// A non-blank line in `events.jsonl` could not be parsed as a
    /// [`LoggedEvent`].
    ///
    /// Per the **schema-evolution-loud** rule in `AGENTS.md`,
    /// type-system rejection is the intended guard against schema drift
    /// — do **not** add `#[serde(default)]` attributes to make this
    /// disappear.  Old log files that reference removed fields or
    /// renamed variants must fail loudly so that the root cause
    /// (schema mismatch) is visible.  A deserialization error on an
    /// old log is diagnostic signal; silently skipping it is a latent
    /// bug.
    MalformedEvent {
        /// 1-based line number in `events.jsonl`.
        line: usize,
        /// Deserialisation error from `serde_json`.
        reason: String,
    },

    /// `events.jsonl` contains no `TurnEnd` or `TurnInterrupted` events;
    /// strict resume requires at least one completed or interrupted turn.
    ///
    /// A new session that has never had a turn completed cannot be
    /// strictly resumed — there is no safe "awaiting user" boundary to
    /// fold up to.
    NoResumableBoundary,

    /// `context.jsonl` could not be read or parsed.
    ContextStore(StoreError),

    /// A context hash referenced in `events.jsonl` was not found in
    /// `context.jsonl`.  Indicates file corruption or an events/context
    /// mismatch.
    MissingContextRecord {
        /// The hash that was referenced but not found.
        hash: String,
    },

    /// The session was started with `OMEGA_FEATURE_REPL=1`.
    ///
    /// The REPL MVP does not support strict resume: the Python interpreter
    /// state (defined variables, imported modules) is not persisted and
    /// cannot be reconstructed from `events.jsonl` alone.
    ///
    /// **Remediation:** start a new session with `OMEGA_FEATURE_REPL=1`.
    /// See `[oq-repl-replay]` in `docs/session-design.html` for the
    /// principled future fix.
    ReplResumeUnsupported {
        /// The session directory that contains the REPL session.
        session_dir: PathBuf,
    },
}

impl fmt::Display for StrictResumeError {
    // Presentation-only; the exact wording of error messages is not
    // semantically significant and cannot be meaningfully mutation-tested
    // (any string survives because callers only check the variant, not the
    // message).  Mutation coverage is therefore intentionally waived here.
    #[mutants::skip]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "events.jsonl I/O error: {e}"),
            Self::MalformedEvent { line, reason } => {
                write!(f, "events.jsonl line {line}: {reason}")
            }
            Self::NoResumableBoundary => write!(
                f,
                "no TurnEnd or TurnInterrupted found in events.jsonl — \
                 cannot determine a resumable boundary"
            ),
            Self::MissingSystemPrompt => write!(
                f,
                "SessionStartedEvent.system_prompt is absent or empty in \
                 events.jsonl — cannot reconstruct the system prompt for \
                 strict resume (schema-evolution-loud: no silent fallback)"
            ),
            Self::MissingFeatures => write!(
                f,
                "no SessionStartedEvent found in events.jsonl — cannot \
                 recover feature flags for strict resume \
                 (schema-evolution-loud: no silent fallback)"
            ),
            Self::ContextStore(e) => write!(f, "context.jsonl store error: {e}"),
            Self::MissingContextRecord { hash } => write!(
                f,
                "context record {hash} referenced in events.jsonl not found \
                 in context.jsonl"
            ),
            Self::ReplResumeUnsupported { session_dir } => write!(
                f,
                "session {} was started with OMEGA_FEATURE_REPL=1 — \
                 strict resume is not supported for REPL sessions (the Python \
                 interpreter state cannot be reconstructed from events.jsonl). \
                 Start a new session with OMEGA_FEATURE_REPL=1 instead. \
                 See [oq-repl-replay] in docs/session-design.html for the \
                 principled future fix.",
                session_dir.display()
            ),
        }
    }
}

impl std::error::Error for StrictResumeError {}

// ---------------------------------------------------------------------------
// Phase 2.2 — Strict event reader (private)
// ---------------------------------------------------------------------------

/// Read and strictly parse all events from `path` (`events.jsonl`).
///
/// Unlike [`EventStore::read_all`] (which reads raw `serde_json::Value`s
/// and silently skips malformed lines), this function fails loudly on any
/// non-blank line that does not deserialise as a [`LoggedEvent`].
///
/// This implements the **schema-evolution-loud** rule from `AGENTS.md`:
/// a deserialization error on a stale log file is diagnostic signal —
/// it tells you exactly which line uses a removed field or renamed
/// variant.  Silently skipping it would mask the schema mismatch.
///
/// Blank or whitespace-only lines are silently skipped (they appear
/// only as formatting artefacts and carry no semantic content).
///
/// Returns an empty `Vec` when the file does not exist (a session
/// directory that has never had any events written).
async fn read_events_strict(path: &Path) -> Result<Vec<OmegaEvent>, StrictResumeError> {
    let text = match tokio::fs::read_to_string(path).await {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(StrictResumeError::Io(e)),
    };

    let mut events = Vec::new();
    for (zero_idx, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let logged: LoggedEvent =
            serde_json::from_str(trimmed).map_err(|e| StrictResumeError::MalformedEvent {
                line: zero_idx + 1,
                reason: e.to_string(),
            })?;
        events.push(logged.event);
    }
    Ok(events)
}

// ---------------------------------------------------------------------------
// Private helper — system prompt extraction
// ---------------------------------------------------------------------------

/// Extract the system prompt from the first `SessionStartedEvent` in `events`
/// that has a non-empty `system_prompt` field.
///
/// Returns `None` when no such event exists (e.g. in test logs that never
/// called `Agent::init`).  [`strict_resume`] converts `None` to
/// [`StrictResumeError::MissingSystemPrompt`] — no silent fallback.
fn fold_system_prompt(events: &[OmegaEvent]) -> Option<String> {
    for event in events {
        if let OmegaEvent::SessionStarted(e) = event
            && !e.system_prompt.is_empty()
        {
            return Some(e.system_prompt.clone());
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Private helper — feature flags extraction
// ---------------------------------------------------------------------------

/// Extract the feature flags from the first `SessionStartedEvent` in `events`.
///
/// Returns `None` when no `SessionStartedEvent` is present at all.
/// [`strict_resume`] converts `None` to
/// [`StrictResumeError::MissingFeatures`] — no silent fallback to default
/// flags, because a wrong tool set is observable by the LLM.
///
/// Unlike [`fold_system_prompt`], this function accepts a `SessionStartedEvent`
/// with the default (both-off) flags as valid — the absent-vs-default
/// distinction is meaningful only for `system_prompt` (empty = missing).
fn fold_features(events: &[OmegaEvent]) -> Option<FeatureFlags> {
    for event in events {
        if let OmegaEvent::SessionStarted(e) = event {
            return Some(e.features);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Phase 2.4 — strict_resume entry point
// ---------------------------------------------------------------------------

/// Reconstruct a live [`Agent`] from an existing session directory.
///
/// ## What “strict resume” means
///
/// Strict resume replays `events.jsonl` and `context.jsonl` of a
/// completed session to produce an `Agent` whose observable state is
/// equivalent to what it was at the session’s last **"awaiting user"**
/// boundary ([`TurnEnd`] or [`TurnInterrupted`]).  Any events after
/// that boundary — from an abandoned partial turn, e.g., a process
/// crash mid-turn — are **silently discarded**.  The next call to
/// [`Agent::send_message`] on the returned agent picks up exactly where
/// the last complete turn left off.
///
/// ## Domain state vs. plumbing
///
/// The authoritative field-by-field classification lives in
/// [`DomainSnapshot`] and in [`Agent::domain_snapshot`]'s exhaustive
/// destructuring.  In brief: `active_model`, `active_effort`, `history`,
/// `context_hashes`, and `system_prompt` are domain state (observable
/// by the LLM or user); everything else is plumbing (reconstructed from
/// files or provided fresh by the caller).
///
/// `system_prompt` is reconstructed from
/// `SessionStartedEvent.system_prompt` — **not** re-read from `AGENTS.md`
/// on disk — so the resumed session sees exactly what the original
/// session saw, regardless of any changes to instruction files since
/// the session started.
///
/// ## Errors
///
/// See [`StrictResumeError`] variants for the complete list.
///
/// [`TurnEnd`]: omega_types::OmegaEvent::TurnEnd
/// [`TurnInterrupted`]: omega_types::OmegaEvent::TurnInterrupted
pub async fn strict_resume(
    session_dir: PathBuf,
    cwd: PathBuf,
    provider: Arc<dyn Provider>,
    headless: bool,
) -> Result<Agent, StrictResumeError> {
    // --- 1. Read events.jsonl with strict typing -------------------------
    let events = read_events_strict(&session_dir.join("events.jsonl")).await?;

    // --- 1b. REPL sessions cannot be strictly resumed -------------------
    //
    // Check features immediately after reading events (before the boundary
    // check) so that `ReplResumeUnsupported` has priority over
    // `NoResumableBoundary`.  The REPL incompatibility is the more
    // fundamental reason the resume cannot proceed; surfacing it first
    // gives the user actionable remediation (start a new session) rather
    // than the more opaque "no resumable boundary" message.
    //
    // `fold_features` may return `None` if no `SessionStartedEvent` is
    // found — in that case we fall through and let the later
    // `MissingFeatures` or `NoResumableBoundary` error fire as appropriate.
    if let Some(f) = fold_features(&events)
        && f.repl
    {
        return Err(StrictResumeError::ReplResumeUnsupported {
            session_dir: session_dir.clone(),
        });
    }

    // --- 2. Find last resumable boundary --------------------------------
    let boundary =
        find_last_resumable_boundary(&events).ok_or(StrictResumeError::NoResumableBoundary)?;

    // --- 3. Fold state from events[..=boundary] -------------------------
    let valid_events = &events[..=boundary];
    let (model, effort) = fold_model_and_effort(valid_events);
    let context_hash_strings = compute_valid_context_hashes(valid_events);

    // --- 4. Load context.jsonl and build history ------------------------
    let context_path = session_dir.join("context.jsonl");
    let context_store = ContextStore::new(context_path);
    let all_records = context_store
        .read_all()
        .await
        .map_err(StrictResumeError::ContextStore)?;

    // Build a lookup map: hash string → `ContextRecord`.
    // Two records with the same `(role, content)` share a hash (HASH-1),
    // but the payload is identical so either is fine.
    let record_map: HashMap<String, ContextRecord> = all_records
        .into_iter()
        .map(|r| (r.hash.as_ref().to_owned(), r))
        .collect();

    let mut history: Vec<Message> = Vec::with_capacity(context_hash_strings.len());
    let mut hashes = Vec::with_capacity(context_hash_strings.len());
    for hash_str in &context_hash_strings {
        let record =
            record_map
                .get(hash_str)
                .ok_or_else(|| StrictResumeError::MissingContextRecord {
                    hash: hash_str.clone(),
                })?;
        history.push(Message {
            role: record.role,
            content: record.content.clone(),
        });
        hashes.push(record.hash.clone());
    }

    // --- 5. Build Agent -------------------------------------------------
    //
    // Also recover feature flags before constructing, so they are passed
    // via AgentConfig and preserved as domain state.
    let features = fold_features(valid_events).ok_or(StrictResumeError::MissingFeatures)?;

    // REPL sessions cannot be strictly resumed: the Python interpreter
    // state (defined variables, imported modules) is not persisted and
    // cannot be reconstructed from events.jsonl alone.
    // See [oq-repl-replay] in docs/session-design.html for the future fix.
    if features.repl {
        return Err(StrictResumeError::ReplResumeUnsupported {
            session_dir: session_dir.clone(),
        });
    }

    let event_store = EventStore::new(session_dir.join("events.jsonl"));
    let config = AgentConfig {
        model: model.clone(),
        effort: Some(effort),
        cwd,
        session_dir,
        headless,
        // Restore the exact feature flags from the original session so the
        // resumed agent exposes the same tool set to the LLM as before.
        // Bypasses from_env() — features are locked in at session-start time.
        features: Some(features),
    };
    let mut agent = Agent::new(provider, context_store, event_store, config);

    // --- 6. Reconstruct system prompt from persisted SessionStartedEvent ---
    //
    // Use `SessionStartedEvent.system_prompt` as the sole source of truth,
    // NOT AGENTS.md on disk.  This guarantees the resumed agent sees exactly
    // what the original session saw, regardless of any changes to instruction
    // files since the session started.
    //
    // Failing loudly when the field is absent or empty implements the
    // schema-evolution-loud rule: a missing system_prompt is diagnostic
    // signal that the event log is incomplete or stale, not a condition
    // to paper over with a silent disk fallback.
    let system_prompt =
        fold_system_prompt(valid_events).ok_or(StrictResumeError::MissingSystemPrompt)?;
    agent.init_for_resume(system_prompt);

    // --- 7. Seed history ------------------------------------------------
    agent.seed_history(history, hashes);

    Ok(agent)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]

    use omega_types::{
        ContinueMode, FeatureFlags, OmegaEvent,
        events::{
            AgentErrorEvent, ContextCompactedEvent, EffortChangedEvent, InterruptReason,
            LlmCallEvent, LlmResponseEndedEvent, LlmResponseUsage, ModelChangedEvent,
            SessionResumedEvent, SessionStartedEvent, TextBlockEvent, ToolCallEvent,
            ToolResultEvent, TurnContinuedEvent, TurnEndEvent, TurnInterruptedEvent, TurnMetrics,
            TurnPausedEvent, UserMessageEvent,
        },
        ids::{Origin, SessionId},
    };

    use super::*;

    // -----------------------------------------------------------------------
    // Additional constructors for Phase 2 tests
    // -----------------------------------------------------------------------

    fn session_started(model: &str, effort: &str) -> OmegaEvent {
        OmegaEvent::SessionStarted(SessionStartedEvent {
            time: t(),
            // Using nil UUID — stable and unit-test-adequate.
            session_id: SessionId(uuid::Uuid::nil()),
            path: String::new(),
            model: model.to_owned(),
            effort: effort.to_owned(),
            system_prompt: String::new(),
            omega_commit: "unknown".to_owned(),
            agent_time_zone: "UTC".to_owned(),
            origin: Origin::Root,
            features: FeatureFlags::default(),
        })
    }

    /// Build an `LlmCall` event carrying `context_hashes` (String aliases).
    fn llm_call(context_hashes: Vec<&str>) -> OmegaEvent {
        OmegaEvent::LlmCall(LlmCallEvent {
            time: t(),
            url: "https://api.anthropic.com/v1/messages".to_owned(),
            model: "claude-sonnet-4-6".to_owned(),
            context_hashes: context_hashes.into_iter().map(str::to_owned).collect(),
            cache_breakpoint_index: None,
            request_bytes: 0,
            request_summary: None,
        })
    }

    /// Build an `LlmResponseEnded` event with the given `context_hash`.
    fn llm_response_with_hash(context_hash: &str) -> OmegaEvent {
        OmegaEvent::LlmResponseEnded(LlmResponseEndedEvent {
            time: t(),
            stop_reason: "end_turn".to_owned(),
            cleared_tool_uses: None,
            cleared_input_tokens: None,
            usage: LlmResponseUsage {
                input_tokens: 10,
                output_tokens: 5,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
                service_tier: None,
                iterations: None,
            },
            context_hash: context_hash.to_owned(),
            response_summary: None,
        })
    }

    /// Build a `ContextCompacted` event.
    fn context_compacted() -> OmegaEvent {
        OmegaEvent::ContextCompacted(ContextCompactedEvent {
            time: t(),
            tokens_before: 80_000,
            tokens_after: 500,
            summary_tokens: 300,
        })
    }

    // -----------------------------------------------------------------------
    // Phase 2.1 — is_resumable_boundary
    // -----------------------------------------------------------------------

    #[test]
    fn is_resumable_boundary_true_for_turn_end() {
        assert!(is_resumable_boundary(&turn_end()));
    }

    #[test]
    fn is_resumable_boundary_true_for_turn_interrupted() {
        assert!(is_resumable_boundary(&turn_interrupted(None)));
        assert!(is_resumable_boundary(&turn_interrupted(Some(
            InterruptReason::Aborted
        ))));
        assert!(is_resumable_boundary(&turn_interrupted(Some(
            InterruptReason::Error
        ))));
    }

    #[test]
    fn is_resumable_boundary_false_for_non_boundary_events() {
        // A representative sample of events that must NOT be resumable
        // boundaries. Catches mutations that return `true` for everything.
        assert!(!is_resumable_boundary(&user_msg("hi")));
        assert!(!is_resumable_boundary(&model_changed("claude-opus-4-7")));
        assert!(!is_resumable_boundary(&effort_changed("high")));
        assert!(!is_resumable_boundary(&text_block("hello")));
        assert!(!is_resumable_boundary(&llm_response_ended()));
        assert!(!is_resumable_boundary(&context_compacted()));
        assert!(!is_resumable_boundary(&tool_call(
            "c1",
            "read_file",
            serde_json::json!({"path": "x"})
        )));
        assert!(!is_resumable_boundary(&tool_result(
            "c1",
            "read_file",
            false,
            "ok"
        )));
    }

    // -----------------------------------------------------------------------
    // Phase 2.1 — find_last_resumable_boundary
    // -----------------------------------------------------------------------

    #[test]
    fn find_last_resumable_boundary_empty_is_none() {
        assert!(find_last_resumable_boundary(&[]).is_none());
    }

    #[test]
    fn find_last_resumable_boundary_no_boundary_is_none() {
        let events = vec![user_msg("hi"), text_block("hello"), llm_response_ended()];
        assert!(find_last_resumable_boundary(&events).is_none());
    }

    #[test]
    fn find_last_resumable_boundary_single_turn_end() {
        let events = vec![user_msg("hi"), llm_response_ended(), turn_end()];
        assert_eq!(find_last_resumable_boundary(&events), Some(2));
    }

    #[test]
    fn find_last_resumable_boundary_returns_last_not_first() {
        // Two turn_end events; must return the index of the LAST one.
        let events = vec![
            user_msg("a"),
            turn_end(), // index 1
            user_msg("b"),
            turn_end(), // index 3 ← must be returned
        ];
        assert_eq!(find_last_resumable_boundary(&events), Some(3));
    }

    #[test]
    fn find_last_resumable_boundary_turn_interrupted_counts() {
        let events = vec![
            user_msg("a"),
            turn_end(), // index 1
            user_msg("b"),
            turn_interrupted(None), // index 3 ← last boundary
        ];
        assert_eq!(find_last_resumable_boundary(&events), Some(3));
    }

    #[test]
    fn find_last_resumable_boundary_events_after_boundary_ignored() {
        // Events after the last turn_end (a partial turn) must not affect
        // the boundary index.
        let events = vec![
            user_msg("a"),
            turn_end(),            // index 1 ← last boundary
            user_msg("abandoned"), // index 2 — partial turn, ignored
        ];
        assert_eq!(find_last_resumable_boundary(&events), Some(1));
    }

    // -----------------------------------------------------------------------
    // Phase 2.2 — compute_valid_context_hashes
    // -----------------------------------------------------------------------

    #[test]
    fn compute_context_hashes_empty_events_returns_empty() {
        assert_eq!(compute_valid_context_hashes(&[]), Vec::<String>::new());
    }

    #[test]
    fn compute_context_hashes_no_llm_response_returns_empty() {
        // LlmCall without a corresponding LlmResponseEnded → empty history.
        let events = vec![user_msg("hi"), llm_call(vec!["h_u1"])];
        assert_eq!(compute_valid_context_hashes(&events), Vec::<String>::new());
    }

    #[test]
    fn compute_context_hashes_single_turn() {
        // One turn: LlmCall with user hash, then assistant response.
        let events = vec![
            llm_call(vec!["h_u1"]),
            llm_response_with_hash("h_a1"),
            turn_end(),
        ];
        assert_eq!(
            compute_valid_context_hashes(&events),
            vec!["h_u1".to_owned(), "h_a1".to_owned()]
        );
    }

    #[test]
    fn compute_context_hashes_two_turns_accumulates() {
        // Turn 1: user + assistant.  Turn 2: user + assistant.
        // The second LlmCall carries all four hashes; result appends the
        // second assistant.
        let events = vec![
            llm_call(vec!["h_u1"]),
            llm_response_with_hash("h_a1"),
            turn_end(),
            llm_call(vec!["h_u1", "h_a1", "h_u2"]),
            llm_response_with_hash("h_a2"),
            turn_end(),
        ];
        assert_eq!(
            compute_valid_context_hashes(&events),
            vec![
                "h_u1".to_owned(),
                "h_a1".to_owned(),
                "h_u2".to_owned(),
                "h_a2".to_owned(),
            ]
        );
    }

    #[test]
    fn compute_context_hashes_compaction_resets_call_hashes() {
        // Compaction turn:
        //   LlmCall carries [h_u1, h_a1, h_u2, h_a2, h_u3]
        //   ContextCompacted fires → clear last_call_hashes
        //   LlmResponseEnded(h_ac)           ← post-compaction baseline
        // Result must be [h_ac] only (not the pre-compaction hashes).
        let events = vec![
            llm_call(vec!["h_u1", "h_a1", "h_u2", "h_a2", "h_u3"]),
            context_compacted(),
            llm_response_with_hash("h_ac"),
            turn_end(),
        ];
        assert_eq!(
            compute_valid_context_hashes(&events),
            vec!["h_ac".to_owned()]
        );
    }

    #[test]
    fn compute_context_hashes_turn_after_compaction() {
        // After compaction (h_ac baseline), one more turn:
        //   LlmCall([h_ac, h_u4])  → response h_a4  → TurnEnd
        // Result = [h_ac, h_u4, h_a4].
        let events = vec![
            // Compaction turn
            llm_call(vec!["h_u1", "h_a1", "h_u2"]),
            context_compacted(),
            llm_response_with_hash("h_ac"),
            turn_end(),
            // Normal turn after compaction
            llm_call(vec!["h_ac", "h_u4"]),
            llm_response_with_hash("h_a4"),
            turn_end(),
        ];
        assert_eq!(
            compute_valid_context_hashes(&events),
            vec!["h_ac".to_owned(), "h_u4".to_owned(), "h_a4".to_owned()]
        );
    }

    #[test]
    fn compute_context_hashes_multiple_tool_rounds() {
        // Turn with tool use:
        //   LlmCall #1([u1]) → h_a1_tool  (tool_use)
        //   Tool results appended as user → not directly in events
        //   LlmCall #2([u1, a1, tr1]) → h_a2
        // Last LlmCall has all hashes → result = [u1, a1, tr1, a2].
        let events = vec![
            llm_call(vec!["h_u1"]),
            llm_response_with_hash("h_a1"), // tool_use response
            llm_call(vec!["h_u1", "h_a1", "h_tr1"]), // second call with tool results
            llm_response_with_hash("h_a2"),
            turn_end(),
        ];
        assert_eq!(
            compute_valid_context_hashes(&events),
            vec![
                "h_u1".to_owned(),
                "h_a1".to_owned(),
                "h_tr1".to_owned(),
                "h_a2".to_owned(),
            ]
        );
    }

    // -----------------------------------------------------------------------
    // fold_features
    // -----------------------------------------------------------------------

    #[test]
    fn fold_features_no_events_returns_none() {
        assert_eq!(fold_features(&[]), None);
    }

    #[test]
    fn fold_features_no_session_started_returns_none() {
        // Only non-SessionStarted events — no SessionStartedEvent present.
        let events = vec![user_msg("hi"), model_changed("claude-opus-4-7"), turn_end()];
        assert_eq!(fold_features(&events), None);
    }

    #[test]
    fn fold_features_returns_default_flags_when_both_off() {
        // SessionStartedEvent with default (both-off) flags — valid, returns Some(default).
        let events = vec![session_started("claude-sonnet-4-6", "medium")];
        assert_eq!(fold_features(&events), Some(FeatureFlags::default()));
    }

    #[test]
    fn fold_features_repl_on() {
        let events = vec![OmegaEvent::SessionStarted(SessionStartedEvent {
            time: t(),
            session_id: SessionId(uuid::Uuid::nil()),
            path: String::new(),
            model: "claude-sonnet-4-6".to_owned(),
            effort: "medium".to_owned(),
            system_prompt: String::new(),
            omega_commit: "unknown".to_owned(),
            agent_time_zone: "UTC".to_owned(),
            origin: Origin::Root,
            features: FeatureFlags {
                repl: true,
                subagents: false,
            },
        })];
        assert_eq!(
            fold_features(&events),
            Some(FeatureFlags {
                repl: true,
                subagents: false
            })
        );
    }

    #[test]
    fn fold_features_both_on() {
        let events = vec![OmegaEvent::SessionStarted(SessionStartedEvent {
            time: t(),
            session_id: SessionId(uuid::Uuid::nil()),
            path: String::new(),
            model: "claude-sonnet-4-6".to_owned(),
            effort: "medium".to_owned(),
            system_prompt: String::new(),
            omega_commit: "unknown".to_owned(),
            agent_time_zone: "UTC".to_owned(),
            origin: Origin::Root,
            features: FeatureFlags {
                repl: true,
                subagents: true,
            },
        })];
        assert_eq!(
            fold_features(&events),
            Some(FeatureFlags {
                repl: true,
                subagents: true
            })
        );
    }

    #[test]
    fn fold_features_uses_first_session_started() {
        // Two SessionStartedEvents (unusual but defensive) — first one wins.
        let first = OmegaEvent::SessionStarted(SessionStartedEvent {
            time: t(),
            session_id: SessionId(uuid::Uuid::nil()),
            path: String::new(),
            model: "claude-sonnet-4-6".to_owned(),
            effort: "medium".to_owned(),
            system_prompt: String::new(),
            omega_commit: "unknown".to_owned(),
            agent_time_zone: "UTC".to_owned(),
            origin: Origin::Root,
            features: FeatureFlags {
                repl: true,
                subagents: false,
            },
        });
        let second = OmegaEvent::SessionStarted(SessionStartedEvent {
            time: t(),
            session_id: SessionId(uuid::Uuid::nil()),
            path: String::new(),
            model: "claude-sonnet-4-6".to_owned(),
            effort: "medium".to_owned(),
            system_prompt: String::new(),
            omega_commit: "unknown".to_owned(),
            agent_time_zone: "UTC".to_owned(),
            origin: Origin::Root,
            features: FeatureFlags {
                repl: false,
                subagents: true,
            },
        });
        let events = vec![first, second];
        // First SessionStartedEvent wins.
        assert_eq!(
            fold_features(&events),
            Some(FeatureFlags {
                repl: true,
                subagents: false
            })
        );
    }

    // -----------------------------------------------------------------------
    // Phase 2.2 — fold_model_and_effort
    // -----------------------------------------------------------------------

    #[test]
    fn fold_model_effort_empty_uses_defaults() {
        let (model, effort) = fold_model_and_effort(&[]);
        assert_eq!(model, RESUMPTION_MODEL);
        assert_eq!(effort, DEFAULT_EFFORT);
    }

    #[test]
    fn fold_model_effort_session_started_provides_baseline() {
        let events = vec![session_started("claude-sonnet-4-6", "medium")];
        let (model, effort) = fold_model_and_effort(&events);
        assert_eq!(model, "claude-sonnet-4-6");
        assert_eq!(effort, "medium");
    }

    #[test]
    fn fold_model_effort_model_changed_overrides_started() {
        let events = vec![
            session_started("claude-sonnet-4-6", "medium"),
            model_changed("claude-opus-4-7"),
        ];
        let (model, effort) = fold_model_and_effort(&events);
        assert_eq!(model, "claude-opus-4-7");
        assert_eq!(effort, "medium"); // unchanged
    }

    #[test]
    fn fold_model_effort_effort_changed_overrides_started() {
        let events = vec![
            session_started("claude-sonnet-4-6", "medium"),
            effort_changed("high"),
        ];
        let (model, effort) = fold_model_and_effort(&events);
        assert_eq!(model, "claude-sonnet-4-6"); // unchanged
        assert_eq!(effort, "high");
    }

    #[test]
    fn fold_model_effort_last_model_changed_wins() {
        let events = vec![
            session_started("claude-sonnet-4-6", "medium"),
            model_changed("claude-opus-4-7"),
            model_changed("claude-sonnet-4-6"), // reverts to sonnet
        ];
        let (model, _) = fold_model_and_effort(&events);
        assert_eq!(model, "claude-sonnet-4-6");
    }

    #[test]
    fn fold_model_effort_only_model_changed_no_started() {
        // No SessionStarted event — only ModelChanged.  ModelChanged must win.
        let events = vec![model_changed("claude-opus-4-7")];
        let (model, effort) = fold_model_and_effort(&events);
        assert_eq!(model, "claude-opus-4-7");
        assert_eq!(effort, DEFAULT_EFFORT); // default
    }

    #[test]
    fn fold_model_effort_session_started_after_model_changed_does_not_override() {
        // SessionStarted appearing AFTER ModelChanged must NOT clobber the
        // prior ModelChanged (it only sets the `_started` baseline, which
        // loses to `_changed`).
        let events = vec![
            model_changed("claude-opus-4-7"),
            session_started("claude-sonnet-4-6", "low"),
        ];
        let (model, effort) = fold_model_and_effort(&events);
        // ModelChanged wins over SessionStarted for model.
        assert_eq!(model, "claude-opus-4-7");
        // SessionStarted provides the effort baseline (no EffortChanged).
        assert_eq!(effort, "low");
    }

    // -----------------------------------------------------------------------
    // Shared event constructors
    // -----------------------------------------------------------------------

    fn t() -> String {
        "2024-01-01T00:00:00.000Z".to_owned()
    }

    fn user_msg(content: &str) -> OmegaEvent {
        OmegaEvent::UserMessage(UserMessageEvent {
            time: t(),
            content: content.to_owned(),
        })
    }

    fn model_changed(model: &str) -> OmegaEvent {
        OmegaEvent::ModelChanged(ModelChangedEvent {
            time: t(),
            model: model.to_owned(),
        })
    }

    fn effort_changed(effort: &str) -> OmegaEvent {
        OmegaEvent::EffortChanged(EffortChangedEvent {
            time: t(),
            effort: effort.to_owned(),
        })
    }

    fn text_block(text: &str) -> OmegaEvent {
        OmegaEvent::TextBlock(TextBlockEvent {
            time: t(),
            text: text.to_owned(),
            partial: false,
        })
    }

    fn llm_response_ended() -> OmegaEvent {
        OmegaEvent::LlmResponseEnded(LlmResponseEndedEvent {
            time: t(),
            stop_reason: "end_turn".to_owned(),
            cleared_tool_uses: None,
            cleared_input_tokens: None,
            usage: LlmResponseUsage {
                input_tokens: 10,
                output_tokens: 5,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
                service_tier: None,
                iterations: None,
            },
            context_hash: "aabbcc".to_owned(),
            response_summary: None,
        })
    }

    fn tool_call(id: &str, name: &str, input: serde_json::Value) -> OmegaEvent {
        OmegaEvent::ToolCall(ToolCallEvent {
            time: t(),
            tool_call_id: id.to_owned(),
            name: name.to_owned(),
            input,
            context_hash: "aabbcc".to_owned(),
        })
    }

    fn tool_result(id: &str, name: &str, is_error: bool, output: &str) -> OmegaEvent {
        OmegaEvent::ToolResult(ToolResultEvent {
            time: t(),
            tool_call_id: id.to_owned(),
            name: name.to_owned(),
            is_error,
            duration_ms: 10,
            output: output.to_owned(),
        })
    }

    fn turn_end() -> OmegaEvent {
        OmegaEvent::TurnEnd(TurnEndEvent {
            time: t(),
            metrics: TurnMetrics {
                input_tokens: 0,
                output_tokens: 0,
                cache_creation_tokens: None,
                cache_read_tokens: None,
            },
        })
    }

    fn turn_interrupted(reason: Option<InterruptReason>) -> OmegaEvent {
        OmegaEvent::TurnInterrupted(TurnInterruptedEvent { time: t(), reason })
    }

    fn session_resumed(summary: &str) -> OmegaEvent {
        OmegaEvent::SessionResumed(SessionResumedEvent {
            time: t(),
            resumed_from: "20240101_000000".to_owned(),
            summary: summary.to_owned(),
        })
    }

    fn agent_error(error: &str) -> OmegaEvent {
        OmegaEvent::AgentError(AgentErrorEvent {
            time: t(),
            error: error.to_owned(),
        })
    }

    fn turn_paused() -> OmegaEvent {
        OmegaEvent::TurnPaused(TurnPausedEvent { time: t() })
    }

    fn turn_continued() -> OmegaEvent {
        OmegaEvent::TurnContinued(TurnContinuedEvent {
            time: t(),
            mode: ContinueMode::Manual,
        })
    }

    // -----------------------------------------------------------------------
    // extract_last_model_and_effort (Phase 1d.1a — retained)
    // -----------------------------------------------------------------------

    #[test]
    fn empty_event_list_returns_none_for_both() {
        let (m, e) = extract_last_model_and_effort(&[]);
        assert_eq!(m, None);
        assert_eq!(e, None);
    }

    #[test]
    fn returns_none_when_no_change_events_present() {
        let evs = vec![user_msg("hi"), user_msg("there")];
        let (m, e) = extract_last_model_and_effort(&evs);
        assert_eq!(m, None);
        assert_eq!(e, None);
    }

    #[test]
    fn returns_last_model_when_multiple_changes() {
        let evs = vec![
            model_changed("claude-sonnet-4-6"),
            user_msg("between"),
            model_changed("claude-opus-4-7"),
        ];
        let (m, e) = extract_last_model_and_effort(&evs);
        assert_eq!(m.as_deref(), Some("claude-opus-4-7"));
        assert_eq!(e, None);
    }

    #[test]
    fn returns_last_effort_when_multiple_changes() {
        let evs = vec![
            effort_changed("low"),
            effort_changed("medium"),
            effort_changed("high"),
        ];
        let (m, e) = extract_last_model_and_effort(&evs);
        assert_eq!(m, None);
        assert_eq!(e.as_deref(), Some("high"));
    }

    #[test]
    fn model_and_effort_are_independent_keys() {
        let evs = vec![
            model_changed("claude-opus-4-6"),
            effort_changed("xhigh"),
            user_msg("noise"),
        ];
        let (m, e) = extract_last_model_and_effort(&evs);
        assert_eq!(m.as_deref(), Some("claude-opus-4-6"));
        assert_eq!(e.as_deref(), Some("xhigh"));
    }

    #[test]
    fn later_event_overrides_earlier_for_same_key() {
        let evs = vec![
            model_changed("first"),
            model_changed("second"),
            model_changed("third"),
        ];
        let (m, _) = extract_last_model_and_effort(&evs);
        assert_eq!(m.as_deref(), Some("third"));
    }

    #[test]
    fn unrelated_event_types_are_ignored() {
        let evs = vec![user_msg("hello"), user_msg("world")];
        let (m, e) = extract_last_model_and_effort(&evs);
        assert_eq!(m, None);
        assert_eq!(e, None);
    }

    // -----------------------------------------------------------------------
    // first_meaningful_line (private helper — tested via basis extraction
    // AND directly through #[cfg(test)] visibility)
    // -----------------------------------------------------------------------

    #[test]
    fn fml_returns_first_non_blank_line() {
        assert_eq!(first_meaningful_line("\nfoo\nbar"), "foo");
    }

    #[test]
    fn fml_skips_whitespace_only_lines() {
        assert_eq!(first_meaningful_line("   \n  hello  \nother"), "hello");
    }

    #[test]
    fn fml_truncates_at_120_chars() {
        let long: String = "x".repeat(150);
        let result = first_meaningful_line(&long);
        assert_eq!(result.len(), 120);
    }

    #[test]
    fn fml_exactly_120_chars_not_truncated() {
        let exactly: String = "x".repeat(120);
        let result = first_meaningful_line(&exactly);
        assert_eq!(result.len(), 120);
    }

    #[test]
    fn fml_119_chars_not_truncated() {
        // Boundary: one below 120 must be returned in full.
        let s: String = "y".repeat(119);
        assert_eq!(first_meaningful_line(&s).len(), 119);
    }

    #[test]
    fn fml_trims_surrounding_whitespace() {
        assert_eq!(first_meaningful_line("  hello  "), "hello");
    }

    #[test]
    fn fml_all_blank_lines_returns_empty() {
        // When every line is blank, unwrap_or(s) fallback fires; trim → "".
        assert_eq!(first_meaningful_line("   \n   "), "");
    }

    // -----------------------------------------------------------------------
    // primary_tool_arg (private helper)
    // -----------------------------------------------------------------------

    #[test]
    fn pta_null_input_returns_none_string() {
        assert_eq!(
            primary_tool_arg("read_file", &serde_json::Value::Null),
            "(none)"
        );
    }

    #[test]
    fn pta_read_write_edit_list_return_path() {
        let inp = serde_json::json!({"path": "src/main.rs", "content": "hi"});
        assert_eq!(primary_tool_arg("read_file", &inp), "src/main.rs");
        assert_eq!(primary_tool_arg("write_file", &inp), "src/main.rs");
        assert_eq!(primary_tool_arg("edit_file", &inp), "src/main.rs");
        assert_eq!(primary_tool_arg("list_files", &inp), "src/main.rs");
    }

    #[test]
    fn pta_find_files_returns_pattern() {
        let inp = serde_json::json!({"pattern": "*.rs", "path": "src/"});
        // Must return pattern, NOT path
        assert_eq!(primary_tool_arg("find_files", &inp), "*.rs");
    }

    #[test]
    fn pta_run_command_and_background_return_command() {
        let inp = serde_json::json!({"command": "cargo test"});
        assert_eq!(primary_tool_arg("run_command", &inp), "cargo test");
        assert_eq!(primary_tool_arg("run_background", &inp), "cargo test");
    }

    #[test]
    fn pta_grep_files_returns_pattern_at_path() {
        let inp = serde_json::json!({"pattern": "TODO", "path": "src/"});
        assert_eq!(primary_tool_arg("grep_files", &inp), "TODO @ src/");
    }

    #[test]
    fn pta_grep_files_includes_both_parts() {
        // Verify pattern AND path are both present (kills mutations that drop
        // either side of the " @ " join).
        let inp = serde_json::json!({"pattern": "fn main", "path": "rust/"});
        let result = primary_tool_arg("grep_files", &inp);
        assert!(result.contains("fn main"), "pattern absent: {result}");
        assert!(result.contains("rust/"), "path absent: {result}");
        assert!(result.contains(" @ "), "separator absent: {result}");
    }

    #[test]
    fn pta_fetch_url_returns_url() {
        let inp = serde_json::json!({"url": "https://example.com", "postprocess": "head -5"});
        assert_eq!(primary_tool_arg("fetch_url", &inp), "https://example.com");
    }

    #[test]
    fn pta_web_search_returns_query() {
        let inp = serde_json::json!({"query": "rust async"});
        assert_eq!(primary_tool_arg("web_search", &inp), "rust async");
    }

    #[test]
    fn pta_wait_for_output_returns_log_file() {
        let inp = serde_json::json!({"logFile": "/tmp/bg.log", "timeoutMs": 5000});
        assert_eq!(primary_tool_arg("wait_for_output", &inp), "/tmp/bg.log");
    }

    #[test]
    fn pta_write_stdin_returns_text() {
        let inp = serde_json::json!({"pid": 123, "text": "yes\n"});
        assert_eq!(primary_tool_arg("write_stdin", &inp), "yes\n");
    }

    #[test]
    fn pta_unknown_tool_falls_back_to_json() {
        let inp = serde_json::json!({"x": 1});
        assert_eq!(primary_tool_arg("mystery_tool", &inp), r#"{"x":1}"#);
    }

    // -----------------------------------------------------------------------
    // extract_block (private helper)
    // -----------------------------------------------------------------------

    #[test]
    fn extract_block_returns_inner_content_without_tags() {
        let text = "before<foo>inner content</foo>after";
        let result = extract_block(text, "foo").unwrap();
        // Must not contain the opening tag.
        assert!(
            !result.contains("<foo>"),
            "tag leaked into result: {result}"
        );
        assert_eq!(result, "inner content");
    }

    #[test]
    fn extract_block_trims_content() {
        let result = extract_block("<foo>  hello  </foo>", "foo").unwrap();
        assert_eq!(result, "hello");
    }

    #[test]
    fn extract_block_multiline_content() {
        let text = "<foo>\nline one\nline two\n</foo>";
        let result = extract_block(text, "foo").unwrap();
        assert_eq!(result, "line one\nline two");
    }

    #[test]
    fn extract_block_no_open_tag_returns_none() {
        assert_eq!(extract_block("no tags here", "foo"), None);
    }

    #[test]
    fn extract_block_no_close_tag_returns_none() {
        assert_eq!(extract_block("<foo>unclosed", "foo"), None);
    }

    // -----------------------------------------------------------------------
    // extract_summary_from_response
    // -----------------------------------------------------------------------

    #[test]
    fn summary_extracts_block_when_present() {
        let text = "preamble\n<summary>This is the summary.</summary>\npostamble";
        assert_eq!(extract_summary_from_response(text), "This is the summary.");
    }

    #[test]
    fn summary_falls_back_to_trimmed_full_text_when_absent() {
        let text = "  no tags here  ";
        assert_eq!(extract_summary_from_response(text), "no tags here");
    }

    #[test]
    fn summary_trims_whitespace_inside_block() {
        let text = "<summary>\n  padded content  \n</summary>";
        assert_eq!(extract_summary_from_response(text), "padded content");
    }

    #[test]
    fn summary_captures_multiline_block() {
        let text = "<summary>\nLine A\nLine B\n</summary>";
        assert_eq!(extract_summary_from_response(text), "Line A\nLine B");
    }

    #[test]
    fn summary_fallback_trims_leading_trailing_whitespace() {
        // When there is NO <summary> tag, the FULL response text is trimmed.
        assert_eq!(
            extract_summary_from_response("\n  plain response  \n"),
            "plain response"
        );
    }

    // -----------------------------------------------------------------------
    // extract_resumption_basis — empty / no-turn cases
    // -----------------------------------------------------------------------

    #[test]
    fn basis_empty_event_list_returns_placeholder() {
        assert_eq!(
            extract_resumption_basis(&[]),
            "(empty session — no turns recorded)"
        );
    }

    #[test]
    fn basis_events_outside_turn_returns_placeholder() {
        // TextBlock + LlmResponseEnded before any user_message — no turns formed.
        let evs = vec![text_block("hi"), llm_response_ended()];
        assert_eq!(
            extract_resumption_basis(&evs),
            "(empty session — no turns recorded)"
        );
    }

    // -----------------------------------------------------------------------
    // extract_resumption_basis — user message rendering
    // -----------------------------------------------------------------------

    #[test]
    fn basis_user_message_renders_with_user_prefix() {
        let evs = vec![user_msg("hello agent"), turn_end()];
        let result = extract_resumption_basis(&evs);
        assert!(result.contains("User: hello agent"), "got: {result}");
    }

    #[test]
    fn basis_user_message_content_is_trimmed() {
        let evs = vec![user_msg("  hello  "), turn_end()];
        let result = extract_resumption_basis(&evs);
        assert!(result.contains("User: hello"), "got: {result}");
        assert!(!result.contains("User:   hello"), "got: {result}");
    }

    // -----------------------------------------------------------------------
    // extract_resumption_basis — LLM response rendering
    // -----------------------------------------------------------------------

    #[test]
    fn basis_llm_response_renders_with_agent_prefix() {
        let evs = vec![
            user_msg("q"),
            text_block("answer"),
            llm_response_ended(),
            turn_end(),
        ];
        let result = extract_resumption_basis(&evs);
        assert!(result.contains("Agent: answer"), "got: {result}");
    }

    #[test]
    fn basis_llm_response_without_text_emits_nothing() {
        // No TextBlock before LlmResponseEnded — nothing emitted.
        let evs = vec![user_msg("q"), llm_response_ended(), turn_end()];
        let result = extract_resumption_basis(&evs);
        assert!(!result.contains("Agent:"), "got: {result}");
    }

    // -----------------------------------------------------------------------
    // extract_resumption_basis — tool call/result pairing
    // -----------------------------------------------------------------------

    #[test]
    fn basis_tool_call_paired_with_ok_result() {
        let evs = vec![
            user_msg("do it"),
            tool_call(
                "id1",
                "read_file",
                serde_json::json!({"path": "src/main.rs"}),
            ),
            tool_result("id1", "read_file", false, "fn main() {}"),
            turn_end(),
        ];
        let result = extract_resumption_basis(&evs);
        // Name, arg, and ok marker all present.
        assert!(result.contains("read_file"), "got: {result}");
        assert!(result.contains("src/main.rs"), "got: {result}");
        assert!(result.contains("→ ok"), "got: {result}");
    }

    #[test]
    fn basis_tool_call_paired_with_error_result_shows_first_line() {
        let evs = vec![
            user_msg("do it"),
            tool_call("id1", "run_command", serde_json::json!({"command": "make"})),
            tool_result(
                "id1",
                "run_command",
                true,
                "error: not found\nmore detail\n",
            ),
            turn_end(),
        ];
        let result = extract_resumption_basis(&evs);
        assert!(
            result.contains("→ error \u{2014} error: not found"),
            "got: {result}"
        );
        // Second line must NOT appear.
        assert!(!result.contains("more detail"), "got: {result}");
    }

    #[test]
    fn basis_tool_result_without_matching_call_shows_no_arg() {
        // No preceding tool_call with matching id → arg is empty → no space before →.
        let evs = vec![
            user_msg("go"),
            tool_result("orphan", "web_search", false, "results"),
            turn_end(),
        ];
        let result = extract_resumption_basis(&evs);
        // Should contain "  web_search → ok" with exactly one space before →.
        assert!(result.contains("  web_search \u{2192} ok"), "got: {result}");
    }

    #[test]
    fn basis_tool_result_with_arg_has_space_before_arrow() {
        // Verifies that arg_part = " arg" (leading space) produces the right
        // output and kills the `if arg.is_empty()` inversion mutation.
        let evs = vec![
            user_msg("search"),
            tool_call(
                "id1",
                "web_search",
                serde_json::json!({"query": "rust async"}),
            ),
            tool_result("id1", "web_search", false, "ok output"),
            turn_end(),
        ];
        let result = extract_resumption_basis(&evs);
        assert!(
            result.contains("  web_search rust async \u{2192} ok"),
            "got: {result}"
        );
    }

    // -----------------------------------------------------------------------
    // extract_resumption_basis — agent error
    // -----------------------------------------------------------------------

    #[test]
    fn basis_agent_error_renders() {
        let evs = vec![user_msg("q"), agent_error("context too long"), turn_end()];
        let result = extract_resumption_basis(&evs);
        assert!(result.contains("Error: context too long"), "got: {result}");
    }

    // -----------------------------------------------------------------------
    // extract_resumption_basis — turn_interrupted
    // -----------------------------------------------------------------------

    #[test]
    fn basis_turn_interrupted_error_renders_bracketed_text() {
        let evs = vec![
            user_msg("q"),
            turn_interrupted(Some(InterruptReason::Error)),
        ];
        let result = extract_resumption_basis(&evs);
        assert!(
            result.contains("[Turn interrupted due to error]"),
            "got: {result}"
        );
    }

    #[test]
    fn basis_turn_interrupted_aborted_drops_line() {
        // Aborted interruptions are not surfaced in the basis.
        let evs = vec![
            user_msg("q"),
            turn_interrupted(Some(InterruptReason::Aborted)),
        ];
        let result = extract_resumption_basis(&evs);
        assert!(
            !result.contains("[Turn interrupted"),
            "aborted interruption must be dropped: {result}"
        );
    }

    #[test]
    fn basis_turn_interrupted_no_reason_drops_line() {
        // reason=None is treated the same as Aborted — not surfaced.
        let evs = vec![user_msg("q"), turn_interrupted(None)];
        let result = extract_resumption_basis(&evs);
        assert!(!result.contains("[Turn interrupted"), "got: {result}");
    }

    // -----------------------------------------------------------------------
    // extract_resumption_basis — pause/continue interjection window
    // -----------------------------------------------------------------------

    #[test]
    fn basis_user_message_in_paused_window_has_mid_turn_prefix() {
        let evs = vec![
            user_msg("initial"),
            turn_paused(),
            user_msg("interjection while paused"),
            turn_continued(),
            turn_end(),
        ];
        let result = extract_resumption_basis(&evs);
        assert!(
            result.contains("User (mid-turn): interjection while paused"),
            "got: {result}"
        );
    }

    #[test]
    fn basis_user_message_after_continued_has_plain_prefix() {
        // After turn_continued the window closes — next user message is plain.
        let evs = vec![
            user_msg("initial"),
            turn_paused(),
            turn_continued(),
            user_msg("after continue"),
            turn_end(),
        ];
        let result = extract_resumption_basis(&evs);
        assert!(result.contains("\nUser: after continue"), "got: {result}");
        assert!(
            !result.contains("mid-turn"),
            "should not have mid-turn label: {result}"
        );
    }

    #[test]
    fn basis_turn_paused_sets_window_and_continued_clears_it() {
        // Two interjections: one while paused (mid-turn), one before pause (plain).
        // Also verifies that TurnContinued kills the window.
        let evs = vec![
            user_msg("first"),
            turn_paused(),
            user_msg("paused-1"),
            user_msg("paused-2"),
            turn_continued(),
            user_msg("resumed"),
            turn_end(),
        ];
        let result = extract_resumption_basis(&evs);
        assert!(
            result.contains("User (mid-turn): paused-1"),
            "got: {result}"
        );
        assert!(
            result.contains("User (mid-turn): paused-2"),
            "got: {result}"
        );
        assert!(result.contains("\nUser: resumed"), "got: {result}");
    }

    // -----------------------------------------------------------------------
    // extract_resumption_basis — turn grouping
    // -----------------------------------------------------------------------

    #[test]
    fn basis_multiple_turns_numbered_correctly() {
        let evs = vec![
            user_msg("turn one"),
            turn_end(),
            user_msg("turn two"),
            turn_end(),
        ];
        let result = extract_resumption_basis(&evs);
        assert!(result.contains("### Turn 1"), "got: {result}");
        assert!(result.contains("### Turn 2"), "got: {result}");
    }

    #[test]
    fn basis_events_outside_turn_are_dropped() {
        // TextBlock + LlmResponseEnded before first user_message is outside any turn.
        let evs = vec![
            text_block("stray"),
            llm_response_ended(),
            user_msg("real user"),
            turn_end(),
        ];
        let result = extract_resumption_basis(&evs);
        assert!(!result.contains("stray"), "stray event leaked: {result}");
        assert!(result.contains("User: real user"), "got: {result}");
    }

    #[test]
    fn basis_second_user_message_inside_open_turn_is_interjection() {
        // Second UserMessage while in_turn=true stays in the same turn
        // (no new turn opened) and gets plain "User:" prefix (not mid-turn,
        // because there was no pause event).
        let evs = vec![user_msg("first"), user_msg("interjection"), turn_end()];
        let result = extract_resumption_basis(&evs);
        // Only one turn heading.
        let turn_count = result.matches("### Turn").count();
        assert_eq!(turn_count, 1, "expected 1 turn, got: {result}");
        // Both messages appear.
        assert!(result.contains("User: first"), "got: {result}");
        assert!(result.contains("User: interjection"), "got: {result}");
    }

    #[test]
    fn basis_turn_end_closes_turn_so_next_user_starts_new_turn() {
        // Verifies in_turn=false after TurnEnd.
        let evs = vec![user_msg("a"), turn_end(), user_msg("b"), turn_end()];
        let result = extract_resumption_basis(&evs);
        assert_eq!(
            result.matches("### Turn").count(),
            2,
            "expected 2 turns: {result}"
        );
    }

    #[test]
    fn basis_turn_interrupted_closes_turn_so_next_user_starts_new_turn() {
        // Verifies in_turn=false after TurnInterrupted.
        let evs = vec![
            user_msg("a"),
            turn_interrupted(Some(InterruptReason::Error)),
            user_msg("b"),
            turn_end(),
        ];
        let result = extract_resumption_basis(&evs);
        assert_eq!(
            result.matches("### Turn").count(),
            2,
            "expected 2 turns: {result}"
        );
    }

    // -----------------------------------------------------------------------
    // extract_resumption_basis — session_resumed carry-forward
    // -----------------------------------------------------------------------

    #[test]
    fn basis_no_session_resumed_uses_all_events() {
        let evs = vec![user_msg("q"), turn_end()];
        let result = extract_resumption_basis(&evs);
        assert!(result.contains("## Session events"), "got: {result}");
        assert!(
            !result.contains("## Carried-forward context"),
            "got: {result}"
        );
    }

    #[test]
    fn basis_session_resumed_summary_appears_in_carry_forward() {
        let evs = vec![
            session_resumed("prior work summary"),
            user_msg("q"),
            turn_end(),
        ];
        let result = extract_resumption_basis(&evs);
        assert!(
            result.contains("## Carried-forward context"),
            "got: {result}"
        );
        assert!(result.contains("prior work summary"), "got: {result}");
    }

    #[test]
    fn basis_session_resumed_event_itself_not_in_relevant_events() {
        // The session_resumed event must be excluded from the events that
        // feed into group_into_turns (start = idx + 1).
        let evs = vec![session_resumed("summary"), user_msg("after"), turn_end()];
        let result = extract_resumption_basis(&evs);
        // session_resumed should appear as carry-forward, not as a turn event.
        // Verify there's exactly one "## Carried-forward context" and the
        // session event's data doesn't appear as a turn.
        let cfc_count = result.matches("## Carried-forward context").count();
        assert_eq!(cfc_count, 1, "got: {result}");
    }

    #[test]
    fn basis_last_session_resumed_wins_when_multiple() {
        // rposition must be used — only the LAST session_resumed summary is
        // carried forward; events before it (including the first resumed event)
        // are dropped.
        let evs = vec![
            session_resumed("first summary"),
            user_msg("ignored turn"),
            turn_end(),
            session_resumed("second summary"),
            user_msg("live turn"),
            turn_end(),
        ];
        let result = extract_resumption_basis(&evs);
        assert!(
            result.contains("second summary"),
            "last summary should appear: {result}"
        );
        assert!(
            !result.contains("first summary"),
            "first summary must not appear: {result}"
        );
        // The turn between the two session_resumed events is also dropped.
        assert!(!result.contains("ignored turn"), "got: {result}");
    }

    #[test]
    fn basis_session_resumed_with_empty_summary_skips_carry_forward() {
        let evs = vec![
            session_resumed("   "), // blank after trim
            user_msg("q"),
            turn_end(),
        ];
        let result = extract_resumption_basis(&evs);
        assert!(
            !result.contains("## Carried-forward context"),
            "empty summary must not produce carry-forward section: {result}"
        );
    }

    // -----------------------------------------------------------------------
    // extract_resumption_basis — structure / section headers
    // -----------------------------------------------------------------------

    #[test]
    fn basis_has_session_events_section_when_turns_exist() {
        let evs = vec![user_msg("q"), turn_end()];
        let result = extract_resumption_basis(&evs);
        assert!(result.starts_with("## Session events"), "got: {result}");
    }

    #[test]
    fn basis_carry_forward_before_session_events_when_both_present() {
        let evs = vec![session_resumed("summary"), user_msg("q"), turn_end()];
        let result = extract_resumption_basis(&evs);
        let cfc_pos = result.find("## Carried-forward context").unwrap();
        let se_pos = result.find("## Session events").unwrap();
        assert!(
            cfc_pos < se_pos,
            "carry-forward must come before session events"
        );
    }

    #[test]
    fn basis_only_carry_forward_when_no_turns_after_resumed() {
        // session_resumed is the last event; no events after it form turns.
        let evs = vec![session_resumed("only context")];
        let result = extract_resumption_basis(&evs);
        assert!(
            result.contains("## Carried-forward context"),
            "got: {result}"
        );
        assert!(!result.contains("## Session events"), "got: {result}");
    }

    #[test]
    fn basis_first_meaningful_line_limits_error_output_in_turn() {
        // Verifies that first_meaningful_line is applied to error output
        // (120-char truncation) within a real basis extraction call.
        let long_error: String = "e".repeat(150);
        let evs = vec![
            user_msg("run"),
            tool_call("id1", "run_command", serde_json::json!({"command": "make"})),
            tool_result("id1", "run_command", true, &long_error),
            turn_end(),
        ];
        let result = extract_resumption_basis(&evs);
        // The error part after "error — " should be exactly 120 'e's.
        let marker = "error \u{2014} ";
        let err_start = result.find(marker).unwrap() + marker.len();
        let err_tail = &result[err_start..];
        // Find end of the error text (next newline or end of string).
        let err_text = err_tail.split_once('\n').map_or(err_tail, |(a, _)| a);
        assert_eq!(
            err_text.len(),
            120,
            "error text should be truncated to 120: {result}"
        );
    }

    // -----------------------------------------------------------------------
    // FU-1 — post-loop flush inner guard: whitespace-only TextBlock must not
    //         emit a stray "\nAgent: " line.
    //
    // Targets the `session_resume.rs:270:12 delete !` mutant identified in
    // rust/SCHEMA-8-MUTANTS.md § "omega-agent non-focal survivors".  Without
    // the `!text.is_empty()` inner guard in the post-loop flush block, the
    // mutant changes `if !text.is_empty()` → `if text.is_empty()`, which
    // causes `format!("\nAgent: ")` (no body) to be pushed when
    // `pending_text` contains only whitespace.
    // -----------------------------------------------------------------------

    #[test]
    fn project_turn_whitespace_only_text_block_no_agent_line_emitted() {
        // A turn whose TextBlock carries only whitespace, followed by no
        // LlmResponseEnded (interrupted turn).  The post-loop flush path fires;
        // `joined.trim()` == "" so the Agent line must be suppressed.
        let evs = vec![
            user_msg("q"),
            text_block("   \n  \t  "), // all whitespace — trims to ""
            // No LlmResponseEnded → in-loop flush is skipped.
            // TurnInterrupted(None) closes the turn without adding an Agent line.
            turn_interrupted(None),
        ];
        let result = extract_resumption_basis(&evs);
        assert!(
            !result.contains("\nAgent: "),
            "whitespace-only TextBlock must not emit a stray Agent line: {result:?}"
        );
    }

    // -----------------------------------------------------------------------
    // project_turn: in-loop LlmResponseEnded guard (session_resume.rs guard
    // line) — kills the `false` and `delete !` mutations.
    //
    // With the `false` or `delete !` mutation the in-loop arm is never taken,
    // so `pending_text` is never cleared.  When a second TextBlock follows a
    // first LlmResponseEnded in the same turn (tool-call round), the mutant
    // concatenates both chunks instead of emitting two separate Agent lines.
    // The outer guard has been removed in favour of relying on the inner
    // `!text.is_empty()` guard; the tests below target that inner guard.
    // -----------------------------------------------------------------------

    #[test]
    fn project_turn_llm_response_ended_without_text_no_agent_line() {
        // LlmResponseEnded arrives with no preceding TextBlock events.
        // The inner `!text.is_empty()` guard must suppress the push.
        // Kills the `replace !text.is_empty() with true` mutation in the
        // in-loop flush block (would produce a spurious empty "Agent: " line).
        let evs = vec![user_msg("q"), llm_response_ended(), turn_end()];
        let result = extract_resumption_basis(&evs);
        assert!(
            !result.contains("Agent:"),
            "LlmResponseEnded without text must not produce an Agent line: {result:?}"
        );
    }

    #[test]
    fn project_turn_llm_response_ended_with_text_emits_agent_line() {
        // LlmResponseEnded arrives after TextBlock events.
        // The inner `!text.is_empty()` guard must allow the push.
        // Kills the `replace !text.is_empty() with false` / `delete !`
        // mutations in the in-loop flush block.
        let evs = vec![
            user_msg("q"),
            text_block("hello world"),
            llm_response_ended(),
            turn_end(),
        ];
        let result = extract_resumption_basis(&evs);
        assert!(
            result.contains("Agent: hello world"),
            "TextBlock before LlmResponseEnded must produce an Agent line: {result:?}"
        );
    }

    #[test]
    fn project_turn_interrupted_text_appears_via_flush() {
        // Turn ends with TextBlock events but no LlmResponseEnded.
        // The post-loop flush must emit the accumulated text.
        // Kills the `replace !text.is_empty() with false` / `delete !`
        // mutations in the post-loop flush block.
        let evs = vec![
            user_msg("q"),
            text_block("partial response"),
            turn_interrupted(None),
        ];
        let result = extract_resumption_basis(&evs);
        assert!(
            result.contains("Agent: partial response"),
            "interrupted turn with text must flush via post-loop path: {result:?}"
        );
    }

    #[test]
    fn project_turn_empty_sequence_no_blank_agent_line() {
        // Empty event sequence (no TextBlock at all, no LlmResponseEnded).
        // The post-loop flush fires with empty pending_text;
        // join("").trim() == "" so no Agent line must appear.
        // Kills the `replace !text.is_empty() with true` mutation in the
        // post-loop flush block (would produce a spurious empty "Agent: " line).
        let evs: Vec<OmegaEvent> = vec![user_msg("q"), turn_end()];
        let result = extract_resumption_basis(&evs);
        assert!(
            !result.contains("Agent:"),
            "empty sequence must not produce a blank Agent line: {result:?}"
        );
    }

    // -----------------------------------------------------------------------
    // project_turn: delete-entire-arm mutant
    //
    // When the `OmegaEvent::LlmResponseEnded` match arm is deleted entirely,
    // `pending_text` is never cleared between rounds.  A turn with two
    // separate LLM responses (text → LlmResponseEnded → text → LlmResponseEnded)
    // then concatenates both chunks instead of emitting two separate "Agent:"
    // lines.  This test catches that survivor.
    // -----------------------------------------------------------------------

    #[test]
    fn project_turn_two_llm_responses_produce_separate_agent_lines() {
        // A multi-round turn: first LLM response produces "first", then a
        // second LLM response (e.g. after a tool call) produces "second".
        // The LlmResponseEnded arm must flush AND clear pending_text between
        // rounds so the two texts appear on separate Agent lines.
        //
        // Kills the `delete match arm OmegaEvent::LlmResponseEnded(_)` mutant:
        // without the arm, pending_text accumulates across both rounds and the
        // post-loop flush produces a single "Agent: firstsecond" line instead of
        // two separate lines.
        let evs = vec![
            user_msg("q"),
            text_block("first"),
            llm_response_ended(), // round 1 done — must flush and clear
            text_block("second"),
            llm_response_ended(), // round 2 done — flush and clear
            turn_end(),
        ];
        let result = extract_resumption_basis(&evs);
        assert!(
            result.contains("Agent: first"),
            "first Agent line must appear separately: {result:?}"
        );
        assert!(
            result.contains("Agent: second"),
            "second Agent line must appear separately: {result:?}"
        );
        assert!(
            !result.contains("firstsecond"),
            "two responses must not be concatenated: {result:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Phase 2.2 — read_events_strict tests
    //
    // These cover the two branches in the NotFound guard and the +1 line
    // number calculation that cargo-mutants marked as survivors.
    // -----------------------------------------------------------------------

    /// Missing file is not an error — returns an empty event list.
    ///
    /// Kills the `replace guard with false` mutant: with `false`, a missing
    /// file would propagate as `Err(StrictResumeError::Io(...))` instead of
    /// `Ok(vec![])`.
    #[tokio::test]
    async fn read_events_strict_missing_file_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let result = read_events_strict(&dir.path().join("events.jsonl")).await;
        assert!(
            result.is_ok(),
            "missing file should not be an error: {result:?}"
        );
        assert!(result.unwrap().is_empty());
    }

    /// An existing but unreadable path (permission denied) propagates as
    /// `Err(StrictResumeError::Io)`, not silently treated as empty.
    ///
    /// Kills the `replace guard with true` mutant: with `true`, every I/O
    /// error (including permission-denied) would silently return `Ok(vec![])`,
    /// masking real read failures.
    #[tokio::test]
    async fn read_events_strict_io_error_propagates() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        // Create the file so it exists, then make it unreadable.
        std::fs::write(&path, b"").unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o000);
        std::fs::set_permissions(&path, perms).unwrap();

        let result = read_events_strict(&path).await;
        // Restore permissions so tempdir cleanup doesn't fail.
        let mut perms2 = std::fs::metadata(&path).unwrap().permissions();
        perms2.set_mode(0o644);
        std::fs::set_permissions(&path, perms2).unwrap();

        assert!(
            matches!(result, Err(StrictResumeError::Io(_))),
            "permission-denied should be Io error, got {result:?}"
        );
    }

    /// A malformed non-blank line causes `MalformedEvent` with a correct
    /// 1-based line number.
    ///
    /// Kills the `replace + with -` and `replace + with *` mutants on
    /// `zero_idx + 1`: with `-`, line 2 would be reported as 1; with `*`,
    /// it would be 0.
    #[tokio::test]
    async fn read_events_strict_malformed_line_has_correct_line_number() {
        use omega_types::ids::LoggedEvent;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");

        // First line: a valid ServerStarted event.
        let valid = serde_json::to_string(&LoggedEvent {
            event_id: None,
            event: OmegaEvent::ServerStarted(omega_types::events::ServerStartedEvent {
                time: "2024-01-01T00:00:00.000Z".to_owned(),
            }),
        })
        .unwrap();
        // Second line: invalid JSON / unknown variant.
        let invalid = r#"{"type":"UNKNOWN_VARIANT","time":"t"}"#;
        tokio::fs::write(&path, format!("{valid}\n{invalid}\n"))
            .await
            .unwrap();

        let result = read_events_strict(&path).await;
        match result {
            Err(StrictResumeError::MalformedEvent { line, .. }) => {
                assert_eq!(line, 2, "second line is line 2 (1-based)");
            }
            other => panic!("expected MalformedEvent, got {other:?}"),
        }
    }
}
