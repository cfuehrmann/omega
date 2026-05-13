//! Pure helpers for the conversation feed (Phase 3.3).
//!
//! Four concerns, all pure / DOM-free / mutation-tested:
//!
//! 1. [`EventKind`] projection: route an [`OmegaEvent`] variant to one
//!    of six visual families (user / assistant / tool_call / tool_result
//!    / status / error). The big match still lives inside the
//!    `<EventBlock/>` component (it needs typed field access per
//!    variant), but the family-class decision is carved out here so
//!    `cargo mutants` can lock it down without DOM infrastructure —
//!    same role `is_active` / `apply_renamed` played for 3.2.
//!
//! 2. [`should_autoscroll`]: predicate that decides whether the feed
//!    should snap to the tail on new content. Pure boolean function
//!    over the three scroll-geometry numbers + a threshold. The DOM
//!    half (reading `scrollTop` / `clientHeight` / `scrollHeight` and
//!    calling `scrollIntoView()`) lives in `feed.rs` and is treated as
//!    a JS-interop edge — same pattern as 3.1's `ws.rs::WsClient::send`.
//!
//! 3. [`truncate_for_preview`]: tool_result inline-preview helper.
//!    Returns `Some(<truncated_with_marker>)` when the input exceeds
//!    `max_chars`, `None` otherwise. Matches the SolidJS UI's
//!    `truncate(s, maxChars=3000)` (see `src/web/client/App.tsx:305`).
//!
//! 4. [`assign_tool_corr`]: walk an event slice and assign 1-based
//!    correlation integers to tool-call / tool-result pairs within each
//!    `LlmCall` group. The integers are shown as superscripts in the
//!    feed so the user can visually pair calls with their results.

use std::collections::HashMap;

use omega_types::OmegaEvent;

// ---------------------------------------------------------------------------
// Visual-family projection
// ---------------------------------------------------------------------------

/// Visual family of an [`OmegaEvent`]. Drives the CSS class of the
/// rendered block; multiple `OmegaEvent` variants share a kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    /// User-submitted message.
    User,
    /// Assistant turn output (text response from the LLM).
    Assistant,
    /// Tool invocation by the agent.
    ToolCall,
    /// Successful tool execution result.
    ToolResult,
    /// Lifecycle / status event (server_started, model_changed, etc).
    Status,
    /// Failure-mode event (errored tool result, LLM/transport/agent
    /// error, interrupted turn).
    Error,
}

/// Project an [`OmegaEvent`] to its [`EventKind`].
///
/// `ToolResult` splits by `is_error`: an errored result lands in
/// [`EventKind::Error`], a successful one in [`EventKind::ToolResult`].
/// All explicitly-error variants (`AgentError`, `LlmError`,
/// `TransportError`, `TurnInterrupted`) collapse into
/// [`EventKind::Error`]. Everything else is [`EventKind::Status`].
#[must_use]
pub fn kind_for(event: &OmegaEvent) -> EventKind {
    match event {
        OmegaEvent::UserMessage(_) => EventKind::User,
        OmegaEvent::ToolCall(_) => EventKind::ToolCall,
        OmegaEvent::ToolResult(r) => {
            if r.is_error {
                EventKind::Error
            } else {
                EventKind::ToolResult
            }
        }
        OmegaEvent::AgentError(_)
        | OmegaEvent::LlmError(_)
        | OmegaEvent::TransportError(_)
        | OmegaEvent::TurnInterrupted(_) => EventKind::Error,
        // SCHEMA-8 additive variants. Block events surface as Assistant
        // content; lifecycle markers are Status. Phase 4 will give them
        // proper rendering / coalescing.
        OmegaEvent::TextBlock(_)
        | OmegaEvent::ThinkingBlock(_)
        | OmegaEvent::ToolUseBlock(_) => EventKind::Assistant,
        OmegaEvent::SessionStarted(_)
        | OmegaEvent::ServerStarted(_)
        | OmegaEvent::ServerStopped(_)
        | OmegaEvent::LlmCall(_)
        | OmegaEvent::TurnEnd(_)
        | OmegaEvent::LlmRetry(_)
        | OmegaEvent::ModelChanged(_)
        | OmegaEvent::EffortChanged(_)
        | OmegaEvent::ResumingSession(_)
        | OmegaEvent::SessionResumed(_)
        | OmegaEvent::PauseRequested(_)
        | OmegaEvent::TurnPaused(_)
        | OmegaEvent::TurnContinued(_)
        | OmegaEvent::LlmResponseStarted(_)
        | OmegaEvent::LlmResponseEnded(_)
        | OmegaEvent::LlmResponseDiscarded(_) => EventKind::Status,
    }
}

/// CSS class string for a kind. Static `&'static str` so leptos's
/// view! can splat it into `class=` directly.
#[must_use]
pub fn css_class_for(kind: EventKind) -> &'static str {
    match kind {
        EventKind::User => "block block-user",
        EventKind::Assistant => "block block-assistant",
        EventKind::ToolCall => "block block-tool-call",
        EventKind::ToolResult => "block block-tool-result",
        EventKind::Status => "block block-status",
        EventKind::Error => "block block-error",
    }
}

/// Stable string tag for a kind, used as a `data-event-kind` attribute
/// (one stable selector for Playwright + dev-tools).
#[must_use]
pub fn kind_tag(kind: EventKind) -> &'static str {
    match kind {
        EventKind::User => "user",
        EventKind::Assistant => "assistant",
        EventKind::ToolCall => "tool_call",
        EventKind::ToolResult => "tool_result",
        EventKind::Status => "status",
        EventKind::Error => "error",
    }
}

/// `OmegaEvent` discriminator string — the `"type"` field's wire
/// projection. Used as a `data-event-type` attribute so Playwright
/// specs can target each specific event variant.
#[must_use]
pub fn event_type_tag(event: &OmegaEvent) -> &'static str {
    match event {
        OmegaEvent::SessionStarted(_) => "session_started",
        OmegaEvent::ServerStarted(_) => "server_started",
        OmegaEvent::ServerStopped(_) => "server_stopped",
        OmegaEvent::UserMessage(_) => "user_message",
        OmegaEvent::LlmCall(_) => "llm_call",
        OmegaEvent::ToolCall(_) => "tool_call",
        OmegaEvent::ToolResult(_) => "tool_result",
        OmegaEvent::TurnEnd(_) => "turn_end",
        OmegaEvent::LlmError(_) => "llm_error",
        OmegaEvent::AgentError(_) => "agent_error",
        OmegaEvent::TurnInterrupted(_) => "turn_interrupted",
        OmegaEvent::LlmRetry(_) => "llm_retry",
        OmegaEvent::ModelChanged(_) => "model_changed",
        OmegaEvent::EffortChanged(_) => "effort_changed",
        OmegaEvent::TransportError(_) => "transport_error",
        OmegaEvent::ResumingSession(_) => "resuming_session",
        OmegaEvent::SessionResumed(_) => "session_resumed",
        OmegaEvent::PauseRequested(_) => "pause_requested",
        OmegaEvent::TurnPaused(_) => "turn_paused",
        OmegaEvent::TurnContinued(_) => "turn_continued",
        // SCHEMA-8 additive variants — Phase 1b stubs.
        OmegaEvent::LlmResponseStarted(_) => "llm_response_started",
        OmegaEvent::LlmResponseEnded(_) => "llm_response_ended",
        OmegaEvent::LlmResponseDiscarded(_) => "llm_response_discarded",
        OmegaEvent::TextBlock(_) => "text_block",
        OmegaEvent::ThinkingBlock(_) => "thinking_block",
        OmegaEvent::ToolUseBlock(_) => "tool_use_block",
    }
}

// ---------------------------------------------------------------------------
// Event label (single source of truth)
// ---------------------------------------------------------------------------
//
// One canonical label per event variant.  Both the big-block
// `<span class="block-label">` and the lower-left status chip read from
// here so the two displays cannot diverge.  Most labels are static
// `&'static str`; `ToolUseBlock` borrows the dynamic tool name.
//
// Companion to `event_type_tag` (machine tag for CSS selectors) and
// `kind_for` (visual-family projection).  Keep the three in sync when
// adding a new variant.

pub const LABEL_USER_MESSAGE: &str = "user_message";
pub const LABEL_LLM_CALL: &str = "LLM call";
pub const LABEL_TOOL_CALL: &str = "tool call";
pub const LABEL_TOOL_RESULT: &str = "tool result";
pub const LABEL_TURN_END: &str = "turn_end";
pub const LABEL_LLM_ERROR: &str = "llm_error";
pub const LABEL_AGENT_ERROR: &str = "agent_error";
pub const LABEL_TRANSPORT_ERROR: &str = "transport_error";
pub const LABEL_TURN_INTERRUPTED: &str = "turn_interrupted";
pub const LABEL_SESSION_STARTED: &str = "session_started";
pub const LABEL_SERVER_STARTED: &str = "server_started";
pub const LABEL_SERVER_STOPPED: &str = "server_stopped";
pub const LABEL_LLM_RETRY: &str = "llm_retry";
pub const LABEL_MODEL_CHANGED: &str = "model_changed";
pub const LABEL_EFFORT_CHANGED: &str = "effort_changed";
pub const LABEL_RESUMING_SESSION: &str = "resuming_session";
pub const LABEL_SESSION_RESUMED: &str = "session_resumed";
pub const LABEL_PAUSE_REQUESTED: &str = "pause_requested";
pub const LABEL_TURN_PAUSED: &str = "turn_paused";
pub const LABEL_TURN_CONTINUED: &str = "turn_continued";
pub const LABEL_LLM_RESPONSE_STARTED: &str = "LLM response start";
pub const LABEL_LLM_RESPONSE_ENDED: &str = "LLM response end";
pub const LABEL_ASSISTANT: &str = "assistant";
pub const LABEL_THINKING: &str = "thinking";

/// Canonical human label for an event.  Used by the big-block
/// `<span class="block-label">` and the status chip alike.
///
/// Returns `&e.name` for `ToolUseBlock` (dynamic — the actual tool
/// name); every other arm returns a static string constant defined
/// in this module.
#[must_use]
pub fn event_label(event: &OmegaEvent) -> &str {
    match event {
        OmegaEvent::UserMessage(_) => LABEL_USER_MESSAGE,
        OmegaEvent::LlmCall(_) => LABEL_LLM_CALL,
        OmegaEvent::ToolCall(_) => LABEL_TOOL_CALL,
        OmegaEvent::ToolResult(_) => LABEL_TOOL_RESULT,
        OmegaEvent::TurnEnd(_) => LABEL_TURN_END,
        OmegaEvent::LlmError(_) => LABEL_LLM_ERROR,
        OmegaEvent::AgentError(_) => LABEL_AGENT_ERROR,
        OmegaEvent::TransportError(_) => LABEL_TRANSPORT_ERROR,
        OmegaEvent::TurnInterrupted(_) => LABEL_TURN_INTERRUPTED,
        OmegaEvent::SessionStarted(_) => LABEL_SESSION_STARTED,
        OmegaEvent::ServerStarted(_) => LABEL_SERVER_STARTED,
        OmegaEvent::ServerStopped(_) => LABEL_SERVER_STOPPED,
        OmegaEvent::LlmRetry(_) => LABEL_LLM_RETRY,
        OmegaEvent::ModelChanged(_) => LABEL_MODEL_CHANGED,
        OmegaEvent::EffortChanged(_) => LABEL_EFFORT_CHANGED,
        OmegaEvent::ResumingSession(_) => LABEL_RESUMING_SESSION,
        OmegaEvent::SessionResumed(_) => LABEL_SESSION_RESUMED,
        OmegaEvent::PauseRequested(_) => LABEL_PAUSE_REQUESTED,
        OmegaEvent::TurnPaused(_) => LABEL_TURN_PAUSED,
        OmegaEvent::TurnContinued(_) => LABEL_TURN_CONTINUED,
        OmegaEvent::LlmResponseStarted(_) => LABEL_LLM_RESPONSE_STARTED,
        OmegaEvent::LlmResponseEnded(_) => LABEL_LLM_RESPONSE_ENDED,
        OmegaEvent::LlmResponseDiscarded(_) => LABEL_ASSISTANT,
        OmegaEvent::TextBlock(_) => LABEL_ASSISTANT,
        OmegaEvent::ThinkingBlock(_) => LABEL_THINKING,
        OmegaEvent::ToolUseBlock(e) => &e.name,
    }
}

// ---------------------------------------------------------------------------
// Current-status projection (status chip)
// ---------------------------------------------------------------------------

/// Project the running-session state to a `(label, event_type_tag)`
/// pair for the status chip.
///
/// Highest-priority match wins:
///
/// 1. A live thinking buffer  → (`"thinking"`, `"thinking_block"`)
/// 2. A live tool-use buffer  → (`<tool name>`, `"tool_use_block"`)
/// 3. A live text buffer      → (`"assistant"`, `"text_block"`)
/// 4. Otherwise               → label/tag of `events.last()`
///
/// Returns `None` only when `events` is empty and no streaming buffer
/// is live — i.e. nothing has happened yet.  Callers (the status
/// chip) fall back to a generic label in that case.
///
/// The pair is returned by value (`String` + `&'static str`) so the
/// chip's `data-event-type` attribute and label string can be set
/// from one snapshot of the reactive inputs without re-borrowing.
#[must_use]
pub fn current_status_label(
    events: &[OmegaEvent],
    streaming_text_active: bool,
    streaming_thinking_active: bool,
    streaming_tool_use_last_name: Option<&str>,
) -> Option<(String, &'static str)> {
    if streaming_thinking_active {
        Some((LABEL_THINKING.to_owned(), "thinking_block"))
    } else if let Some(name) = streaming_tool_use_last_name {
        Some((name.to_owned(), "tool_use_block"))
    } else if streaming_text_active {
        Some((LABEL_ASSISTANT.to_owned(), "text_block"))
    } else {
        events
            .last()
            .map(|ev| (event_label(ev).to_owned(), event_type_tag(ev)))
    }
}

// ---------------------------------------------------------------------------
// Auto-scroll predicate
// ---------------------------------------------------------------------------

/// True iff the feed should auto-scroll on the next content change.
///
/// Inputs are the three scroll-geometry numbers from the feed
/// container (in CSS pixels) plus a permissive bottom threshold. The
/// predicate returns true when the visible region's bottom edge sits
/// within `threshold` pixels of the scrollable content's bottom edge —
/// i.e. the user is still effectively "at the tail". Scrolling up
/// even a little (past the threshold) flips the predicate to false,
/// pinning the scroll position.
///
/// The conventional threshold is ≈ 40 px (handles browser sub-pixel
/// rounding + a bit of grace for the user's own micro-scrolls).
#[must_use]
pub fn should_autoscroll(
    scroll_top: f64,
    client_height: f64,
    scroll_height: f64,
    threshold: f64,
) -> bool {
    scroll_top + client_height + threshold >= scroll_height
}

// ---------------------------------------------------------------------------
// Tool-result preview truncation
// ---------------------------------------------------------------------------

/// Truncate `s` to `max_chars` Unicode scalars for inline preview.
///
/// Returns `None` if `s` already fits, leaving the caller to render
/// `s` verbatim. Returns `Some(<truncated_with_marker>)` otherwise —
/// the marker line `\n… [{total} chars total — showing first
/// {max_chars}]` mirrors the SolidJS UI exactly so visual parity holds
/// across the two bundles during the 3.0–3.6 co-existence window.
///
/// Char-count is on `chars()`, not `len()`, so multi-byte sequences
/// don't get cut mid-codepoint and counting matches what the user
/// perceives as "characters" rather than UTF-8 bytes.
#[must_use]
pub fn truncate_for_preview(s: &str, max_chars: usize) -> Option<String> {
    let total = s.chars().count();
    if total <= max_chars {
        return None;
    }
    let prefix: String = s.chars().take(max_chars).collect();
    Some(format!(
        "{prefix}\n\u{2026} [{total} chars total — showing first {max_chars}]"
    ))
}

// ---------------------------------------------------------------------------
// Line-count truncation (TODO-C tool previews)
// ---------------------------------------------------------------------------

/// Truncate `s` to at most `max_lines` newline-delimited lines for
/// inline preview.
///
/// Returns `None` if `s` has `max_lines` or fewer lines (no truncation
/// needed — the caller can render `s` verbatim). Returns
/// `Some(first_n_lines)` — without a trailing `\n` — when there is
/// content beyond line `max_lines`.
///
/// Used by `ToolCallBlock` and `ToolResultBlock` (TODO-C) to produce
/// 2-line inline previews that redirect to a `TextModal` for the full
/// output.
#[must_use]
pub fn truncate_to_lines(s: &str, max_lines: usize) -> Option<String> {
    if max_lines == 0 {
        return if s.is_empty() { None } else { Some(String::new()) };
    }
    let mut count = 0usize;
    for (i, c) in s.char_indices() {
        if c == '\n' {
            count += 1;
            if count >= max_lines && i + 1 < s.len() {
                return Some(s[..i].to_owned());
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Virtual-line count (thinking block toggle gate)
// ---------------------------------------------------------------------------

/// Count how many display lines `text` would occupy on a notional
/// `width`-character terminal, treating every hard newline as a line
/// break and every run of `width` characters as an additional wrapped
/// line.  Empty lines count as one line each.
///
/// This is used to decide whether the thinking-block more/less toggle
/// should be shown: a single very long line wraps just as visually
/// imposingly as many short lines, and raw `lines().count()` misses it.
#[must_use]
pub fn virtual_line_count(text: &str, width: usize) -> usize {
    text.lines()
        .map(|l| {
            let n = l.chars().count();
            if n == 0 { 1 } else { n.div_ceil(width) }
        })
        .sum::<usize>()
        .max(if text.is_empty() { 0 } else { 1 })
}

// ---------------------------------------------------------------------------
// Byte + line combined preview (tool call / tool result blocks)
// ---------------------------------------------------------------------------

/// Truncate `s` to at most `max_lines` newline-delimited lines *and*
/// at most `max_bytes` UTF-8 bytes for the inline tool preview.
///
/// The line limit is applied first; the byte cap is then applied to
/// whatever that yielded. Returns `None` when neither limit binds
/// (the caller renders `s` verbatim). Returns `Some(truncated)` when
/// at least one limit binds.
///
/// Byte truncation always cuts at a UTF-8 character boundary so the
/// output is always valid UTF-8. No trailing ellipsis is appended —
/// clicking the block to open the modal signals that more content exists.
#[must_use]
pub fn truncate_preview(s: &str, max_lines: usize, max_bytes: usize) -> Option<String> {
    let line_cut = truncate_to_lines(s, max_lines);
    let candidate: &str = line_cut.as_deref().unwrap_or(s);
    if candidate.len() <= max_bytes {
        // Byte limit is not binding — return the (possibly None) line result.
        return line_cut;
    }
    // Byte limit is binding. Walk down from max_bytes to find a char boundary.
    let mut boundary = max_bytes;
    while boundary > 0 && !candidate.is_char_boundary(boundary) {
        boundary -= 1;
    }
    Some(candidate[..boundary].to_owned())
}

// ---------------------------------------------------------------------------
// Tool-call / tool-result correlation integers
// ---------------------------------------------------------------------------

/// Assign 1-based correlation integers to tool-call and tool-result events.
///
/// Returns a parallel `Vec<Option<usize>>` the same length as `events`:
///
/// * `LlmCall` events reset the per-call counter to zero and clear the id
///   map, so numbering restarts at 1 for each LLM round-trip.
/// * `ToolCall` events in a group with **two or more** calls get `Some(n)`
///   where *n* is the 1-based ordinal within the current `LlmCall` group.
///   Groups with only a single call get `None` — there is nothing to pair,
///   so the number would add visual noise without helping the user.
/// * `ToolResult` events get `Some(n)` matching the `ToolCall` with the
///   same `id`, subject to the same single-call suppression.
/// * `ToolUseBlock` events (SCHEMA-8 Phase 5e) get `Some(n)` matching the
///   `ToolCall` with the same `id` (the provider's `tool_use_id` flows
///   through both events), subject to the same single-call suppression.
///   This lets the operator visually pair the tool_use block emitted on
///   the response side with the tool_call dispatch and its result.
/// * All other events get `None`.
///
/// `ToolUseBlock` typically appears *before* its sibling `ToolCall` in the
/// event stream (tool_use blocks land during streaming; the dispatch is
/// emitted after `LlmResponseEnded`). The algorithm processes each
/// `LlmCall`-delimited group in two phases:
///   1. Walk the group forward to number `ToolCall` events and build an
///      id→corr map.
///   2. Walk the group again to fill `ToolUseBlock` and `ToolResult`
///      corrs from the map.
///
/// Designed to be called once per reactive frame in `ConversationFeed`
/// so that numbers stay consistent across the entire displayed feed.
#[must_use]
pub fn assign_tool_corr(events: &[OmegaEvent]) -> Vec<Option<usize>> {
    let mut result = vec![None; events.len()];
    let n = events.len();
    let mut group_start = 0usize;
    let mut i = 0usize;

    while i <= n {
        let at_boundary = i == n || matches!(events[i], OmegaEvent::LlmCall(_));
        if at_boundary {
            // Phase A — number ToolCall events in this group, build id→corr map.
            let mut counter = 0usize;
            let mut id_map: HashMap<String, usize> = HashMap::new();
            for j in group_start..i {
                if let OmegaEvent::ToolCall(e) = &events[j] {
                    counter += 1;
                    id_map.insert(e.id.clone(), counter);
                    result[j] = Some(counter);
                }
            }
            // Phase B — lookup ToolUseBlock + ToolResult corrs by id.
            for j in group_start..i {
                match &events[j] {
                    OmegaEvent::ToolUseBlock(e) => {
                        result[j] = id_map.get(&e.id).copied();
                    }
                    OmegaEvent::ToolResult(e) => {
                        result[j] = id_map.get(&e.id).copied();
                    }
                    _ => {}
                }
            }
            // Phase C — suppress corrs in groups with ≤1 tool call.
            if counter <= 1 {
                for slot in &mut result[group_start..i] {
                    *slot = None;
                }
            }
            group_start = i + 1;
        }
        i += 1;
    }

    result
}

/// SCHEMA-8 Phase 5g — number of `partial: true` sibling blocks
/// preceding each `LlmResponseDiscarded` event in the same response.
///
/// Returns a vector aligned to `events`: every index is `None` except
/// for `LlmResponseDiscarded` indices, which carry `Some(count)` where
/// `count` is how many `partial:true` `TextBlock` / `ThinkingBlock` /
/// `ToolUseBlock` events appear between the most recent
/// `LlmResponseStarted` (or the start of the events vec) and this
/// `LlmResponseDiscarded`.  Used by the discarded-response renderer to
/// surface an `N partial blocks` count next to the closer.
///
/// Counting resets at each `LlmResponseStarted` so that a session
/// containing multiple rounds attributes each abandonment to its own
/// batch of partials.  `LlmResponseEnded` resets too — a discarded
/// response is never preceded by a clean Ended in the same round.
#[must_use]
pub fn assign_partial_counts(events: &[OmegaEvent]) -> Vec<Option<usize>> {
    let mut result = vec![None; events.len()];
    let mut partial_count = 0usize;

    for (i, event) in events.iter().enumerate() {
        match event {
            OmegaEvent::LlmResponseStarted(_) | OmegaEvent::LlmResponseEnded(_) => {
                partial_count = 0;
            }
            OmegaEvent::TextBlock(e) if e.partial => {
                partial_count += 1;
            }
            OmegaEvent::ThinkingBlock(e) if e.partial => {
                partial_count += 1;
            }
            OmegaEvent::ToolUseBlock(e) if e.partial => {
                partial_count += 1;
            }
            OmegaEvent::LlmResponseDiscarded(_) => {
                result[i] = Some(partial_count);
                partial_count = 0;
            }
            _ => {}
        }
    }

    result
}

// ---------------------------------------------------------------------------
// Tool-call preview
// ---------------------------------------------------------------------------

/// Build a human-readable one/two-line summary of a tool invocation.
///
/// Instead of showing the raw JSON input object, this function extracts the
/// most important parameters for each known tool and formats them for quick
/// scanning. The result is NOT yet truncated — callers should apply
/// [`truncate_preview`] with `(2, 300)` limits.
///
/// For unknown tool names the function falls back to a JSON pretty-print of
/// the full `input` value so nothing is ever hidden without the modal.
///
/// # Parameter conventions
/// * String fields are extracted as-is; missing optional fields are omitted.
/// * Timeout / duration values are shown as `timeout=Ns` / `timeout=Nms`.
/// * File-glob and path-type filters are shown in `[…]` brackets.
#[must_use]
pub fn tool_call_preview(name: &str, input: &serde_json::Value) -> String {
    /// Pull a string field, returning `""` when missing or not a string.
    fn s<'a>(v: &'a serde_json::Value, key: &str) -> &'a str {
        v.get(key).and_then(|x| x.as_str()).unwrap_or("")
    }
    /// Pull an unsigned-integer field.
    fn u(v: &serde_json::Value, key: &str) -> Option<u64> {
        v.get(key).and_then(|x| x.as_u64())
    }
    /// Pull a boolean field.
    fn b(v: &serde_json::Value, key: &str) -> bool {
        v.get(key).and_then(|x| x.as_bool()).unwrap_or(false)
    }

    match name {
        // ── shell execution ────────────────────────────────────────────────
        "run_command" => {
            let cmd = s(input, "command");
            let timeout = u(input, "timeout");
            match timeout {
                Some(t) => format!("{cmd}  [timeout: {t}s]"),
                None => cmd.to_owned(),
            }
        }

        "run_background" => {
            let cmd = s(input, "command");
            let cwd = s(input, "cwd");
            if cwd.is_empty() {
                cmd.to_owned()
            } else {
                format!("{cmd}  [@ {cwd}]")
            }
        }

        // ── background process I/O ─────────────────────────────────────────
        "wait_for_output" => {
            let pid = u(input, "pid");
            let timeout_ms = u(input, "timeoutMs");
            let pattern = s(input, "pattern");
            let log_file = s(input, "logFile");
            let mut parts: Vec<String> = Vec::new();
            if let Some(p) = pid {
                parts.push(format!("pid={p}"));
            }
            if let Some(t) = timeout_ms {
                parts.push(format!("timeout={t}ms"));
            }
            if !pattern.is_empty() {
                parts.push(format!("pattern={pattern:?}"));
            }
            if !log_file.is_empty() {
                parts.push(log_file.to_owned());
            }
            parts.join("  ")
        }

        "write_stdin" => {
            let pid = u(input, "pid");
            let text = s(input, "text");
            let end_stdin = b(input, "end_stdin");
            let prefix = pid.map_or_else(String::new, |p| format!("pid={p}  "));
            if end_stdin {
                format!("{prefix}{text}  [EOF]")
            } else {
                format!("{prefix}{text}")
            }
        }

        // ── file system reads ──────────────────────────────────────────────
        "read_file" => {
            let path = s(input, "path");
            let offset = u(input, "offset");
            let limit = u(input, "limit");
            match (offset, limit) {
                (None, None) => path.to_owned(),
                (Some(o), Some(l)) => format!("{path}  [offset={o}, limit={l}]"),
                (Some(o), None) => format!("{path}  [offset={o}]"),
                (None, Some(l)) => format!("{path}  [limit={l}]"),
            }
        }

        "list_files" => {
            let path = s(input, "path");
            if b(input, "recursive") {
                format!("{path}  [recursive]")
            } else {
                path.to_owned()
            }
        }

        // ── file system writes ─────────────────────────────────────────────
        "write_file" => {
            // Content can be huge — only show the destination path.
            s(input, "path").to_owned()
        }

        "edit_file" => {
            let path = s(input, "path");
            let n = input
                .get("replacements")
                .and_then(|v| v.as_array())
                .map_or(0, |a| a.len());
            let plural = if n == 1 { "" } else { "s" };
            format!("{path}  ({n} replacement{plural})")
        }

        // ── search ─────────────────────────────────────────────────────────
        "grep_files" => {
            let pattern = s(input, "pattern");
            let path = s(input, "path");
            let file_glob = s(input, "file_glob");
            if file_glob.is_empty() {
                format!("{pattern:?}  in {path}")
            } else {
                format!("{pattern:?}  in {path}  [{file_glob}]")
            }
        }

        "find_files" => {
            let pattern = s(input, "pattern");
            let path = s(input, "path");
            let type_filter = s(input, "type");
            if type_filter.is_empty() {
                format!("{pattern}  in {path}")
            } else {
                format!("{pattern}  in {path}  [type={type_filter}]")
            }
        }

        // ── web / network ──────────────────────────────────────────────────
        "web_search" => s(input, "query").to_owned(),

        "fetch_url" => {
            let url = s(input, "url");
            let postprocess = s(input, "postprocess");
            if postprocess.is_empty() {
                url.to_owned()
            } else {
                format!("{url}  [{postprocess}]")
            }
        }

        // ── fallback: pretty-print full JSON ───────────────────────────────
        _ => serde_json::to_string_pretty(input).unwrap_or_else(|_| "{}".to_owned()),
    }
}

// ---------------------------------------------------------------------------
// Time formatting
// ---------------------------------------------------------------------------

/// Format an ISO-8601 timestamp (always UTC, ending in `Z`) as a
/// compact `HH:MM:SS.mmm` wall-clock string in the agent host's local
/// time zone.
///
/// `tz` must be an IANA zone name (e.g. `"Europe/Berlin"`, `"UTC"`),
/// typically sourced from `SessionStore::agent_time_zone`, which in
/// turn was captured from the session's `SessionStarted.agentTimeZone`
/// field at recording time.  Passing `"UTC"` reproduces the
/// pre-migration UI behaviour (the `Z`-suffix slice of the input).
///
/// On `wasm32` this delegates to `Intl.DateTimeFormat`, which has the
/// IANA database baked into every browser — so winter sessions viewed
/// in summer still render with the correct (winter) offset.  On host
/// targets (SSR snapshot tests) we don't have Intl available and just
/// slice the UTC string; this is safe because snapshot fixtures pin a
/// fixed-string `time` and don't exercise the TZ conversion path.
///
/// Falls back to the raw input string when:
/// - the input is shorter than 12 chars (malformed timestamp), or
/// - `Intl.DateTimeFormat` fails (e.g. an unknown zone name); a hand-
///   edited or otherwise corrupt session shouldn't crash the feed.
#[must_use]
pub fn format_time(iso: &str, tz: &str) -> String {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(out) = format_time_intl(iso, tz) {
            return out;
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = tz;
    }
    iso.get(11..23).unwrap_or(iso).to_owned()
}

#[cfg(target_arch = "wasm32")]
fn format_time_intl(iso: &str, tz: &str) -> Option<String> {
    use js_sys::{Array, Date, Intl, Object, Reflect};
    use wasm_bindgen::JsValue;

    let date = Date::new(&JsValue::from_str(iso));
    // `new Date(invalid)` yields a Date whose `getTime()` is NaN; Intl
    // would then format `"Invalid Date"`.  Guard explicitly so the
    // caller's fallback path runs instead.
    if date.get_time().is_nan() {
        return None;
    }

    let opts = Object::new();
    Reflect::set(&opts, &"timeZone".into(), &JsValue::from_str(tz)).ok()?;
    Reflect::set(&opts, &"hour12".into(), &JsValue::FALSE).ok()?;
    Reflect::set(&opts, &"hour".into(), &JsValue::from_str("2-digit")).ok()?;
    Reflect::set(&opts, &"minute".into(), &JsValue::from_str("2-digit")).ok()?;
    Reflect::set(&opts, &"second".into(), &JsValue::from_str("2-digit")).ok()?;
    Reflect::set(&opts, &"fractionalSecondDigits".into(), &JsValue::from(3_u32)).ok()?;

    // `en-GB` gives a 24-hour clock with `:` separators and a `.` for
    // the fractional-second separator, matching the `HH:MM:SS.mmm`
    // shape the slice fallback produces.
    let locales = Array::new();
    locales.push(&JsValue::from_str("en-GB"));

    // `Intl.DateTimeFormat(locales, options)` throws on an invalid
    // zone name (e.g. someone hand-edited `agentTimeZone: "Mars"`);
    // catch and surface the fallback rather than panicking the feed.
    let dtf = Intl::DateTimeFormat::new(&locales, &opts);
    let formatter = dtf.format();
    let result = formatter.call1(&JsValue::NULL, &date.into()).ok()?;
    result.as_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic, clippy::float_cmp)]

    use omega_types::events::{
        AgentErrorEvent, EffortChangedEvent, LlmCallEvent, LlmErrorEvent,
        LlmResponseDiscardedEvent, LlmResponseEndedEvent, LlmResponseStartedEvent, LlmResponseUsage,
        LlmRetryEvent, ModelChangedEvent, PauseRequestedEvent,
        ResumingSessionEvent, ServerStartedEvent, ServerStopOutcome, ServerStoppedEvent,
        SessionResumedEvent, SessionStartedEvent, ToolCallEvent, ToolResultEvent,
        ToolUseBlockEvent, TextBlockEvent, ThinkingBlockEvent, TransportErrorEvent,
        TurnContinuedEvent, TurnEndEvent,
        TurnInterruptedEvent, TurnMetrics, TurnPausedEvent, UserMessageEvent,
    };
    use omega_types::{ContinueMode, InterruptReason, OmegaEvent};
    use serde_json::json;
    use wasm_bindgen_test::wasm_bindgen_test;

    use super::*;

    fn t() -> String {
        "2024-01-01T00:00:00.000Z".into()
    }

    fn user() -> OmegaEvent {
        OmegaEvent::UserMessage(UserMessageEvent {
            time: t(),
            content: "hi".into(),
        })
    }

    fn llm_response_ended() -> OmegaEvent {
        OmegaEvent::LlmResponseEnded(LlmResponseEndedEvent {
            time: t(),
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
            response_summary: None,
        })
    }

    fn tool_call() -> OmegaEvent {
        OmegaEvent::ToolCall(ToolCallEvent {
            time: t(),
            id: "id".into(),
            name: "run_command".into(),
            input: json!({ "command": "echo hi" }),
            context_hash: "deadbeef".into(),
        })
    }

    fn tool_result(is_error: bool) -> OmegaEvent {
        OmegaEvent::ToolResult(ToolResultEvent {
            time: t(),
            id: "id".into(),
            name: "run_command".into(),
            is_error,
            duration_ms: 1,
            output: "ok".into(),
        })
    }

    // ---- kind_for: one test per OmegaEvent variant -------------------------
    //
    // 22 variants, plus the ToolResult split → 23 distinct `kind_for`
    // calls. Each test catches the deletion mutation of the variant's
    // arm (which would route through a different arm or the wildcard).

    #[wasm_bindgen_test]
    fn kind_user_message_is_user() {
        assert_eq!(kind_for(&user()), EventKind::User);
    }

    #[wasm_bindgen_test]
    fn kind_llm_response_ended_is_status() {
        assert_eq!(kind_for(&llm_response_ended()), EventKind::Status);
    }

    #[wasm_bindgen_test]
    fn kind_tool_call_is_tool_call() {
        assert_eq!(kind_for(&tool_call()), EventKind::ToolCall);
    }

    #[wasm_bindgen_test]
    fn kind_tool_result_success_is_tool_result() {
        assert_eq!(kind_for(&tool_result(false)), EventKind::ToolResult);
    }

    #[wasm_bindgen_test]
    fn kind_tool_result_failure_is_error() {
        assert_eq!(kind_for(&tool_result(true)), EventKind::Error);
    }

    #[wasm_bindgen_test]
    fn kind_agent_error_is_error() {
        let ev = OmegaEvent::AgentError(AgentErrorEvent {
            time: t(),
            error: "boom".into(),
        });
        assert_eq!(kind_for(&ev), EventKind::Error);
    }

    #[wasm_bindgen_test]
    fn kind_llm_error_is_error() {
        let ev = OmegaEvent::LlmError(LlmErrorEvent {
            time: t(),
            url: "u".into(),
            error: "e".into(),
            http_status: Some(400),
        });
        assert_eq!(kind_for(&ev), EventKind::Error);
    }

    #[wasm_bindgen_test]
    fn kind_transport_error_is_error() {
        let ev = OmegaEvent::TransportError(TransportErrorEvent {
            time: t(),
            error: "x".into(),
            context: None,
        });
        assert_eq!(kind_for(&ev), EventKind::Error);
    }

    #[wasm_bindgen_test]
    fn kind_turn_interrupted_is_error() {
        let ev = OmegaEvent::TurnInterrupted(TurnInterruptedEvent {
            time: t(),
            reason: Some(InterruptReason::Aborted),
        });
        assert_eq!(kind_for(&ev), EventKind::Error);
    }

    #[wasm_bindgen_test]
    fn kind_session_started_is_status() {
        let ev = OmegaEvent::SessionStarted(SessionStartedEvent {
            time: t(),
            session_id: "s".into(),
            path: ".".into(),
            model: "m".into(),
            effort: "e".into(),
            system_prompt: "p".into(),
            omega_commit: "u".into(),
            agent_time_zone: "UTC".into(),
        });
        assert_eq!(kind_for(&ev), EventKind::Status);
    }

    #[wasm_bindgen_test]
    fn kind_server_started_is_status() {
        let ev = OmegaEvent::ServerStarted(ServerStartedEvent { time: t() });
        assert_eq!(kind_for(&ev), EventKind::Status);
    }

    #[wasm_bindgen_test]
    fn kind_server_stopped_is_status() {
        let ev = OmegaEvent::ServerStopped(ServerStoppedEvent {
            time: t(),
            outcome: ServerStopOutcome::Clean,
            reason: None,
        });
        assert_eq!(kind_for(&ev), EventKind::Status);
    }

    #[wasm_bindgen_test]
    fn kind_llm_call_is_status() {
        let ev = OmegaEvent::LlmCall(LlmCallEvent {
            time: t(),
            url: "u".into(),
            model: "m".into(),
            context_hashes: vec![],
            cache_breakpoint_index: None,
            request_bytes: 0,
            request_summary: None,
        });
        assert_eq!(kind_for(&ev), EventKind::Status);
    }

    #[wasm_bindgen_test]
    fn kind_turn_end_is_status() {
        let ev = OmegaEvent::TurnEnd(TurnEndEvent {
            time: t(),
            metrics: TurnMetrics {
                input_tokens: 1,
                output_tokens: 2,
                cache_creation_tokens: None,
                cache_read_tokens: None,
            },
        });
        assert_eq!(kind_for(&ev), EventKind::Status);
    }

    #[wasm_bindgen_test]
    fn kind_llm_retry_is_status() {
        let ev = OmegaEvent::LlmRetry(LlmRetryEvent {
            time: t(),
            attempt: 1,
            http_status: Some(500),
            wait_ms: 100,
            error: "e".into(),
            retry_at: None,
            error_body: None,
            reason: None,
        });
        assert_eq!(kind_for(&ev), EventKind::Status);
    }

    #[wasm_bindgen_test]
    fn kind_model_changed_is_status() {
        let ev = OmegaEvent::ModelChanged(ModelChangedEvent {
            time: t(),
            model: "m".into(),
        });
        assert_eq!(kind_for(&ev), EventKind::Status);
    }

    #[wasm_bindgen_test]
    fn kind_effort_changed_is_status() {
        let ev = OmegaEvent::EffortChanged(EffortChangedEvent {
            time: t(),
            effort: "e".into(),
        });
        assert_eq!(kind_for(&ev), EventKind::Status);
    }

    #[wasm_bindgen_test]
    fn kind_resuming_session_is_status() {
        let ev = OmegaEvent::ResumingSession(ResumingSessionEvent {
            time: t(),
            resumed_from: "x".into(),
            name: None,
            basis: "b".into(),
        });
        assert_eq!(kind_for(&ev), EventKind::Status);
    }

    #[wasm_bindgen_test]
    fn kind_session_resumed_is_status() {
        let ev = OmegaEvent::SessionResumed(SessionResumedEvent {
            time: t(),
            resumed_from: "x".into(),
            summary: "s".into(),
        });
        assert_eq!(kind_for(&ev), EventKind::Status);
    }

    #[wasm_bindgen_test]
    fn kind_pause_requested_is_status() {
        let ev = OmegaEvent::PauseRequested(PauseRequestedEvent { time: t() });
        assert_eq!(kind_for(&ev), EventKind::Status);
    }

    #[wasm_bindgen_test]
    fn kind_turn_paused_is_status() {
        let ev = OmegaEvent::TurnPaused(TurnPausedEvent { time: t() });
        assert_eq!(kind_for(&ev), EventKind::Status);
    }

    #[wasm_bindgen_test]
    fn kind_turn_continued_is_status() {
        let ev = OmegaEvent::TurnContinued(TurnContinuedEvent {
            time: t(),
            mode: ContinueMode::Manual,
        });
        assert_eq!(kind_for(&ev), EventKind::Status);
    }

    // ---- css_class_for ------------------------------------------------------

    #[wasm_bindgen_test]
    fn css_class_per_kind_is_distinct_and_namespaced() {
        // Every kind must produce a string starting with "block " and a
        // family-specific suffix. Catches "swap two arms" mutations.
        assert_eq!(css_class_for(EventKind::User), "block block-user");
        assert_eq!(css_class_for(EventKind::Assistant), "block block-assistant");
        assert_eq!(css_class_for(EventKind::ToolCall), "block block-tool-call");
        assert_eq!(
            css_class_for(EventKind::ToolResult),
            "block block-tool-result"
        );
        assert_eq!(css_class_for(EventKind::Status), "block block-status");
        assert_eq!(css_class_for(EventKind::Error), "block block-error");
    }

    #[wasm_bindgen_test]
    fn css_class_values_are_pairwise_unique() {
        // Locks down the "every arm returns the same string" mutation.
        let all = [
            EventKind::User,
            EventKind::Assistant,
            EventKind::ToolCall,
            EventKind::ToolResult,
            EventKind::Status,
            EventKind::Error,
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(
                    css_class_for(*a),
                    css_class_for(*b),
                    "kinds {a:?} and {b:?} share a CSS class"
                );
            }
        }
    }

    // ---- kind_tag -----------------------------------------------------------

    #[wasm_bindgen_test]
    fn kind_tag_per_kind_is_snake_case_lowercase() {
        assert_eq!(kind_tag(EventKind::User), "user");
        assert_eq!(kind_tag(EventKind::Assistant), "assistant");
        assert_eq!(kind_tag(EventKind::ToolCall), "tool_call");
        assert_eq!(kind_tag(EventKind::ToolResult), "tool_result");
        assert_eq!(kind_tag(EventKind::Status), "status");
        assert_eq!(kind_tag(EventKind::Error), "error");
    }

    #[wasm_bindgen_test]
    fn kind_tag_values_are_pairwise_unique() {
        let all = [
            EventKind::User,
            EventKind::Assistant,
            EventKind::ToolCall,
            EventKind::ToolResult,
            EventKind::Status,
            EventKind::Error,
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(
                    kind_tag(*a),
                    kind_tag(*b),
                    "kinds {a:?} and {b:?} share a tag"
                );
            }
        }
    }

    // ---- event_type_tag -----------------------------------------------------

    #[wasm_bindgen_test]
    fn event_type_tag_matches_serde_discriminator_for_each_variant() {
        // The tag we render as `data-event-type` must match the wire
        // discriminator (`#[serde(tag = "type", rename_all =
        // "snake_case")]` on OmegaEvent). If a future field-name change
        // breaks this, Playwright specs would silently miss the block.
        let cases: &[(OmegaEvent, &str)] = &[
            (user(), "user_message"),
            (llm_response_ended(), "llm_response_ended"),
            (tool_call(), "tool_call"),
            (tool_result(false), "tool_result"),
            (
                OmegaEvent::AgentError(AgentErrorEvent {
                    time: t(),
                    error: "x".into(),
                }),
                "agent_error",
            ),
            (
                OmegaEvent::TurnEnd(TurnEndEvent {
                    time: t(),
                    metrics: TurnMetrics {
                        input_tokens: 0,
                        output_tokens: 0,
                        cache_creation_tokens: None,
                        cache_read_tokens: None,
                    },
                }),
                "turn_end",
            ),
        ];
        for (ev, tag) in cases {
            assert_eq!(event_type_tag(ev), *tag, "mismatch for {tag}");
        }
    }

    // ---- event_label --------------------------------------------------------

    #[wasm_bindgen_test]
    fn event_label_user_message_is_snake_case_literal() {
        assert_eq!(event_label(&user()), "user_message");
    }

    #[wasm_bindgen_test]
    fn event_label_tool_call_is_human_form() {
        assert_eq!(event_label(&tool_call()), "tool call");
    }

    #[wasm_bindgen_test]
    fn event_label_tool_result_is_human_form() {
        assert_eq!(event_label(&tool_result(false)), "tool result");
    }

    #[wasm_bindgen_test]
    fn event_label_tool_use_block_returns_dynamic_name() {
        // The chip should show the actual tool name during a tool_use_block,
        // not a static label — distinguishes "run_command" from "read_file".
        let ev = OmegaEvent::ToolUseBlock(ToolUseBlockEvent {
            time: t(),
            id: "id".into(),
            name: "read_file".into(),
            input: json!({}),
            partial: false,
        });
        assert_eq!(event_label(&ev), "read_file");
    }

    #[wasm_bindgen_test]
    fn event_label_thinking_block_is_short_form() {
        let ev = OmegaEvent::ThinkingBlock(ThinkingBlockEvent {
            time: t(),
            thinking: "".into(),
            signature: Some("sig".into()),
            partial: false,
        });
        assert_eq!(event_label(&ev), "thinking");
    }

    #[wasm_bindgen_test]
    fn event_label_text_block_is_assistant() {
        let ev = OmegaEvent::TextBlock(TextBlockEvent {
            time: t(),
            text: "hello".into(),
            partial: false,
        });
        assert_eq!(event_label(&ev), "assistant");
    }

    // ---- current_status_label -----------------------------------------------

    #[wasm_bindgen_test]
    fn current_status_thinking_buffer_wins_over_last_event() {
        // Even though `events.last()` is a tool_call, an active thinking
        // buffer means we are mid-thinking-stream — chip shows that.
        let evs = [tool_call()];
        let got = current_status_label(&evs, false, true, None);
        assert_eq!(got, Some(("thinking".into(), "thinking_block")));
    }

    #[wasm_bindgen_test]
    fn current_status_tool_use_buffer_shows_tool_name() {
        let evs = [user()];
        let got = current_status_label(&evs, false, false, Some("grep_files"));
        assert_eq!(got, Some(("grep_files".into(), "tool_use_block")));
    }

    #[wasm_bindgen_test]
    fn current_status_text_buffer_is_assistant() {
        let evs = [user()];
        let got = current_status_label(&evs, true, false, None);
        assert_eq!(got, Some(("assistant".into(), "text_block")));
    }

    #[wasm_bindgen_test]
    fn current_status_falls_back_to_last_event() {
        let evs = [user(), tool_call()];
        let got = current_status_label(&evs, false, false, None);
        assert_eq!(got, Some(("tool call".into(), "tool_call")));
    }

    #[wasm_bindgen_test]
    fn current_status_thinking_priority_over_tool_use() {
        // Both buffers active (shouldn't happen but is well-defined):
        // thinking wins per the documented priority order.
        let evs: [OmegaEvent; 0] = [];
        let got = current_status_label(&evs, true, true, Some("x"));
        assert_eq!(got, Some(("thinking".into(), "thinking_block")));
    }

    #[wasm_bindgen_test]
    fn current_status_empty_inputs_is_none() {
        let evs: [OmegaEvent; 0] = [];
        assert_eq!(current_status_label(&evs, false, false, None), None);
    }

    // ---- should_autoscroll: boundary semantics for cargo-mutants ----------

    #[wasm_bindgen_test]
    fn autoscroll_true_when_clearly_at_bottom() {
        // visible bottom (900 + 100 = 1000) == content bottom (1000)
        // even with zero threshold → true.
        assert!(should_autoscroll(900.0, 100.0, 1000.0, 0.0));
    }

    #[wasm_bindgen_test]
    fn autoscroll_false_when_clearly_scrolled_up() {
        // Way up top: 0 + 100 + 40 = 140 < 1000 → false.
        assert!(!should_autoscroll(0.0, 100.0, 1000.0, 40.0));
    }

    #[wasm_bindgen_test]
    fn autoscroll_at_exact_threshold_boundary_is_true() {
        // Equal-case: catches `>=` → `>` (would be false) and
        // `>=` → `!=` (would be false). This is the load-bearing
        // boundary test.
        // 900 + 100 + 40 = 1040 == 1040 → true under `>=`.
        assert!(should_autoscroll(900.0, 100.0, 1040.0, 40.0));
    }

    #[wasm_bindgen_test]
    fn autoscroll_one_pixel_past_threshold_is_false() {
        // 900 + 100 + 40 = 1040 < 1041 → false.
        // Catches `>=` → `<=` (would be true).
        assert!(!should_autoscroll(900.0, 100.0, 1041.0, 40.0));
    }

    #[wasm_bindgen_test]
    fn autoscroll_threshold_lifts_borderline_case_to_true() {
        // Without threshold: 100 + 20 + 0 = 120 < 130 → false.
        // With threshold 10: 100 + 20 + 10 = 130 == 130 → true.
        // Catches dropping the threshold from the sum.
        assert!(!should_autoscroll(100.0, 20.0, 130.0, 0.0));
        assert!(should_autoscroll(100.0, 20.0, 130.0, 10.0));
    }

    #[wasm_bindgen_test]
    fn autoscroll_scroll_top_contributes_to_sum() {
        // scroll_top=0 → false; scroll_top=900 → true (everything
        // else equal). Catches `+ scroll_top` being mutated away.
        assert!(!should_autoscroll(0.0, 100.0, 1000.0, 0.0));
        assert!(should_autoscroll(900.0, 100.0, 1000.0, 0.0));
    }

    #[wasm_bindgen_test]
    fn autoscroll_client_height_contributes_to_sum() {
        // client_height=0 → false; client_height=100 → true (everything
        // else equal). Catches `+ client_height` being mutated away.
        assert!(!should_autoscroll(900.0, 0.0, 1000.0, 0.0));
        assert!(should_autoscroll(900.0, 100.0, 1000.0, 0.0));
    }

    #[wasm_bindgen_test]
    fn autoscroll_returns_true_when_visible_overshoots_content() {
        // scroll_top + client_height > scroll_height (rare but
        // possible during layout transitions). Must remain true.
        assert!(should_autoscroll(950.0, 100.0, 1000.0, 0.0));
    }

    // ---- truncate_for_preview ----------------------------------------------

    #[wasm_bindgen_test]
    fn truncate_returns_none_below_limit() {
        assert_eq!(truncate_for_preview("abc", 10), None);
    }

    #[wasm_bindgen_test]
    fn truncate_returns_none_at_exact_limit() {
        // Boundary: chars().count() == max_chars → None.
        // Catches `<=` → `<` (would truncate the equal case).
        assert_eq!(truncate_for_preview("abc", 3), None);
    }

    #[wasm_bindgen_test]
    fn truncate_returns_some_one_char_above_limit() {
        // Boundary: chars().count() == max_chars + 1 → Some.
        // Catches `<=` → `>=` (would never truncate).
        let r = truncate_for_preview("abcd", 3).expect("must truncate");
        assert!(r.starts_with("abc"), "kept first 3 chars: {r}");
        assert!(r.contains("4 chars total"), "marker has total: {r}");
        assert!(r.contains("first 3"), "marker has prefix size: {r}");
    }

    #[wasm_bindgen_test]
    fn truncate_keeps_exactly_max_chars_of_prefix() {
        let s = "x".repeat(100);
        let r = truncate_for_preview(&s, 10).expect("must truncate");
        // First 10 chars preserved verbatim.
        let prefix: String = r.chars().take(10).collect();
        assert_eq!(prefix, "xxxxxxxxxx");
        // Marker appended on a new line so it's visually distinct in
        // the rendered <pre>.
        assert!(r.contains("\n\u{2026} "), "newline + ellipsis present");
    }

    #[wasm_bindgen_test]
    fn truncate_marker_reports_correct_total_and_first_n() {
        let s = "x".repeat(7654);
        let r = truncate_for_preview(&s, 1234).expect("must truncate");
        assert!(r.contains("7654 chars total"), "{r}");
        assert!(r.contains("first 1234"), "{r}");
    }

    #[wasm_bindgen_test]
    fn truncate_handles_multibyte_chars_without_panic() {
        // Five Greek letters → 5 chars / 10 bytes. max_chars=10 fits.
        let s = "αβγδε";
        assert_eq!(truncate_for_preview(s, 10), None);
        // max_chars=3 truncates to first 3 codepoints (αβγ).
        let r = truncate_for_preview(s, 3).expect("must truncate");
        let kept: String = r.chars().take(3).collect();
        assert_eq!(kept, "αβγ");
        assert!(r.contains("5 chars total"));
    }

    #[wasm_bindgen_test]
    fn truncate_with_zero_max_truncates_to_empty_prefix() {
        // Edge case: max_chars=0 always truncates non-empty input
        // to an empty prefix + marker. Catches a `take(max_chars)` →
        // `take(0)` mutation that would otherwise be observable only
        // here.
        let r = truncate_for_preview("ab", 0).expect("must truncate");
        // Marker but no prefix chars — the format's first '\n' is the
        // separator.
        assert!(r.starts_with("\n\u{2026}"), "starts with marker: {r:?}");
        assert!(r.contains("2 chars total"));
    }

    // ---- truncate_to_lines ------------------------------------------------

    #[wasm_bindgen_test]
    #[test]
    fn truncate_to_lines_short_input_returns_none() {
        assert_eq!(truncate_to_lines("one", 2), None);
    }

    #[wasm_bindgen_test]
    #[test]
    fn truncate_to_lines_exactly_max_lines_returns_none() {
        // "one\ntwo" has exactly 2 lines — no truncation needed.
        assert_eq!(truncate_to_lines("one\ntwo", 2), None);
    }

    #[wasm_bindgen_test]
    #[test]
    fn truncate_to_lines_extra_line_returns_some() {
        assert_eq!(
            truncate_to_lines("one\ntwo\nthree", 2),
            Some("one\ntwo".into()),
        );
    }

    #[wasm_bindgen_test]
    #[test]
    fn truncate_to_lines_preserves_prefix_exactly() {
        // Catches off-by-one on the slice boundary.
        let result = truncate_to_lines("aaa\nbbb\nccc", 2).unwrap();
        assert_eq!(result, "aaa\nbbb");
    }

    #[wasm_bindgen_test]
    #[test]
    fn truncate_to_lines_trailing_newline_at_boundary_is_none() {
        // "one\ntwo\n" — the third "line" is empty (no real content).
        // The newline at position 7 is i+1 == s.len() == 8, so the
        // i+1 < s.len() guard correctly returns None (no truncation).
        assert_eq!(truncate_to_lines("one\ntwo\n", 2), None);
    }

    #[wasm_bindgen_test]
    #[test]
    fn truncate_to_lines_max_zero_empty_returns_none() {
        assert_eq!(truncate_to_lines("", 0), None);
    }

    #[wasm_bindgen_test]
    #[test]
    fn truncate_to_lines_max_zero_nonempty_returns_some_empty() {
        assert_eq!(truncate_to_lines("hello", 0), Some(String::new()));
    }

    // ---- virtual_line_count --------------------------------------------

    #[wasm_bindgen_test]
    #[test]
    fn virtual_line_count_empty_is_zero() {
        assert_eq!(virtual_line_count("", 80), 0);
    }

    #[wasm_bindgen_test]
    #[test]
    fn virtual_line_count_short_single_line() {
        // 10 chars, width 80 → 1 virtual line.
        assert_eq!(virtual_line_count("hello world", 80), 1);
    }

    #[wasm_bindgen_test]
    #[test]
    fn virtual_line_count_exact_width_is_one_line() {
        let s = "x".repeat(80);
        assert_eq!(virtual_line_count(&s, 80), 1);
    }

    #[wasm_bindgen_test]
    #[test]
    fn virtual_line_count_one_over_width_wraps_to_two() {
        let s = "x".repeat(81);
        assert_eq!(virtual_line_count(&s, 80), 2);
    }

    #[wasm_bindgen_test]
    #[test]
    fn virtual_line_count_multiple_hard_lines() {
        // 3 short hard lines → 3 virtual lines.
        assert_eq!(virtual_line_count("one\ntwo\nthree", 80), 3);
    }

    #[wasm_bindgen_test]
    #[test]
    fn virtual_line_count_long_line_plus_hard_lines() {
        // 160-char line (2 virtual) + 2 short lines → 4 virtual lines.
        let long = "x".repeat(160);
        let s = format!("{long}\nfoo\nbar");
        assert_eq!(virtual_line_count(&s, 80), 4);
    }

    #[wasm_bindgen_test]
    #[test]
    fn virtual_line_count_empty_hard_line_counts_as_one() {
        // "a\n\nb" → line "a" (1) + empty line (1) + line "b" (1) = 3.
        assert_eq!(virtual_line_count("a\n\nb", 80), 3);
    }

    // ---- toggle-gate boundary (virtual_line_count > 4) -------------------
    //
    // These tests name the _semantic_ meaning of the count: the more/less
    // toggle button appears iff `virtual_line_count(text, 80) > 4`.
    // We cover both the hard-newline path and the single-long-line wrap path,
    // because the whole point of `virtual_line_count` over `lines().count()`
    // is that a long line counts as multiple visual lines.

    #[wasm_bindgen_test]
    #[test]
    fn toggle_gate_four_hard_lines_does_not_trigger() {
        // 4 short hard lines → virtual_line_count = 4 → NOT > 4 → no toggle.
        let s = "line one\nline two\nline three\nline four";
        assert_eq!(virtual_line_count(s, 80), 4);
        assert!(!(virtual_line_count(s, 80) > 4), "4 lines should not show the toggle");
    }

    #[wasm_bindgen_test]
    #[test]
    fn toggle_gate_five_hard_lines_triggers() {
        // 5 short hard lines → virtual_line_count = 5 → > 4 → toggle shown.
        let s = "line one\nline two\nline three\nline four\nline five";
        assert_eq!(virtual_line_count(s, 80), 5);
        assert!(virtual_line_count(s, 80) > 4, "5 lines should show the toggle");
    }

    #[wasm_bindgen_test]
    #[test]
    fn toggle_gate_single_320_char_line_does_not_trigger() {
        // 320 chars → div_ceil(320, 80) = 4 virtual lines → NOT > 4 → no toggle.
        let s = "x".repeat(320);
        assert_eq!(virtual_line_count(&s, 80), 4);
        assert!(!(virtual_line_count(&s, 80) > 4), "320-char line should not show the toggle");
    }

    #[wasm_bindgen_test]
    #[test]
    fn toggle_gate_single_321_char_line_triggers() {
        // 321 chars → div_ceil(321, 80) = 5 virtual lines → > 4 → toggle shown.
        let s = "x".repeat(321);
        assert_eq!(virtual_line_count(&s, 80), 5);
        assert!(virtual_line_count(&s, 80) > 4, "321-char line should show the toggle");
    }

    // ---- truncate_preview -------------------------------------------------

    #[wasm_bindgen_test]
    #[test]
    fn truncate_preview_no_limit_binds_returns_none() {
        // 2 lines, 10 bytes — well within both limits.
        assert_eq!(truncate_preview("abc", 2, 200), None);
    }

    #[wasm_bindgen_test]
    #[test]
    fn truncate_preview_line_limit_binds_byte_does_not() {
        // 3 lines, each short — line limit fires, byte doesn't.
        let r = truncate_preview("one\ntwo\nthree", 2, 200);
        assert_eq!(r, Some("one\ntwo".into()));
    }

    #[wasm_bindgen_test]
    #[test]
    fn truncate_preview_byte_limit_binds_line_does_not() {
        // 1 long line, no newline — byte limit fires.
        let s = "x".repeat(300);
        let r = truncate_preview(&s, 2, 200).expect("must truncate");
        assert_eq!(r.len(), 200);
        assert!(r.chars().all(|c| c == 'x'));
    }

    #[wasm_bindgen_test]
    #[test]
    fn truncate_preview_byte_cuts_at_char_boundary_for_multibyte() {
        // 'α' = 2 bytes. 5 × 'α' = 10 bytes. max_bytes=9 → cuts before the 5th α.
        let s = "α".repeat(5); // 10 bytes
        let r = truncate_preview(&s, 10, 9).expect("must truncate");
        // Must be valid UTF-8 and exactly 4 α chars (8 bytes).
        assert_eq!(r, "α".repeat(4));
        assert_eq!(r.len(), 8);
    }

    #[wasm_bindgen_test]
    #[test]
    fn truncate_preview_byte_limit_applied_after_line_limit() {
        // 3 long lines — line limit fires first (giving "xxx…\nyyy…"),
        // then byte limit fires on that result.
        let s = format!("{0}\n{1}\n{2}", "x".repeat(50), "y".repeat(50), "z".repeat(50));
        // max_lines=2 gives "xxx…\nyyy…" (101 bytes including \n).
        // max_bytes=20 then cuts that to the first 20 bytes.
        let r = truncate_preview(&s, 2, 20).expect("must truncate");
        assert_eq!(r.len(), 20);
        assert!(r.starts_with(&"x".repeat(20)));
    }

    #[wasm_bindgen_test]
    #[test]
    fn truncate_preview_exactly_at_byte_limit_returns_none() {
        // s fits in exactly 10 bytes (10 ASCII chars, 1 line) → None.
        let s = "x".repeat(10);
        assert_eq!(truncate_preview(&s, 2, 10), None);
    }

    // ---- assign_tool_corr ------------------------------------------------

    fn make_llm_call() -> OmegaEvent {
        OmegaEvent::LlmCall(LlmCallEvent {
            time: "2024-01-01T00:00:00.000Z".into(),
            url: "u".into(),
            model: "m".into(),
            context_hashes: vec![],
            cache_breakpoint_index: None,
            request_bytes: 0,
            request_summary: None,
        })
    }

    fn make_tool_call(id: &str) -> OmegaEvent {
        OmegaEvent::ToolCall(ToolCallEvent {
            time: "2024-01-01T00:00:00.000Z".into(),
            id: id.into(),
            name: "run_command".into(),
            input: serde_json::json!({}),
            context_hash: "deadbeef".into(),
        })
    }

    fn make_tool_result(id: &str) -> OmegaEvent {
        OmegaEvent::ToolResult(ToolResultEvent {
            time: "2024-01-01T00:00:00.000Z".into(),
            id: id.into(),
            name: "run_command".into(),
            is_error: false,
            duration_ms: 1,
            output: "ok".into(),
        })
    }

    fn make_tool_use_block(id: &str) -> OmegaEvent {
        OmegaEvent::ToolUseBlock(ToolUseBlockEvent {
            time: "2024-01-01T00:00:00.000Z".into(),
            id: id.into(),
            name: "run_command".into(),
            input: serde_json::json!({}),
            partial: false,
        })
    }

    fn make_user() -> OmegaEvent {
        OmegaEvent::UserMessage(UserMessageEvent {
            time: "2024-01-01T00:00:00.000Z".into(),
            content: "hi".into(),
        })
    }

    #[wasm_bindgen_test]
    #[test]
    fn assign_tool_corr_empty_events_returns_empty() {
        assert_eq!(assign_tool_corr(&[]), vec![]);
    }

    #[wasm_bindgen_test]
    #[test]
    fn assign_tool_corr_non_tool_events_get_none() {
        let events = vec![make_user(), make_llm_call()];
        assert_eq!(assign_tool_corr(&events), vec![None, None]);
    }

    #[wasm_bindgen_test]
    #[test]
    fn assign_tool_corr_single_tool_call_suppressed() {
        // A group with exactly one tool call gets None — no pair to highlight.
        let events = vec![make_tool_call("id1")];
        assert_eq!(assign_tool_corr(&events), vec![None]);
    }

    #[wasm_bindgen_test]
    #[test]
    fn assign_tool_corr_result_matches_call_by_id() {
        // Two calls in the group — both numbered, results matched by id.
        let events = vec![
            make_tool_call("id1"),
            make_tool_call("id2"),
            make_tool_result("id1"),
        ];
        assert_eq!(
            assign_tool_corr(&events),
            vec![Some(1), Some(2), Some(1)],
        );
    }

    #[wasm_bindgen_test]
    #[test]
    fn assign_tool_corr_single_call_result_both_suppressed() {
        // One call + its result: both suppressed (single-call group).
        let events = vec![make_tool_call("id1"), make_tool_result("id1")];
        assert_eq!(assign_tool_corr(&events), vec![None, None]);
    }

    #[wasm_bindgen_test]
    #[test]
    fn assign_tool_corr_two_calls_numbered_sequentially() {
        // Two calls in the same group: both numbered.
        let events = vec![make_tool_call("id1"), make_tool_call("id2")];
        assert_eq!(assign_tool_corr(&events), vec![Some(1), Some(2)]);
    }

    #[wasm_bindgen_test]
    #[test]
    fn assign_tool_corr_two_calls_two_results_matched() {
        let events = vec![
            make_tool_call("a"),
            make_tool_call("b"),
            make_tool_result("b"),
            make_tool_result("a"),
        ];
        assert_eq!(
            assign_tool_corr(&events),
            vec![Some(1), Some(2), Some(2), Some(1)],
        );
    }

    #[wasm_bindgen_test]
    #[test]
    fn assign_tool_corr_llm_call_resets_counter() {
        let events = vec![
            make_tool_call("a"),
            make_tool_call("b"),
            make_llm_call(),
            make_tool_call("c"),
        ];
        // Group 1 (before LlmCall): 2 calls → numbered.
        // Group 2 (after LlmCall): 1 call → suppressed.
        assert_eq!(
            assign_tool_corr(&events),
            vec![Some(1), Some(2), None, None],
        );
    }

    #[wasm_bindgen_test]
    #[test]
    fn assign_tool_corr_result_before_matching_call_gets_none() {
        // ToolResult arrives before the ToolCall with the same id (shouldn't
        // happen in practice, but must not panic and should return None).
        let events = vec![make_tool_result("unknown")];
        assert_eq!(assign_tool_corr(&events), vec![None]);
    }

    #[wasm_bindgen_test]
    #[test]
    fn assign_tool_corr_llm_call_clears_id_map() {
        // After an LlmCall, a ToolResult with an id from the previous group
        // should NOT match (map was cleared). Also: the single call in group 1
        // is suppressed, and the orphan result in group 2 is also None.
        let events = vec![
            make_tool_call("id1"),
            make_llm_call(),
            make_tool_result("id1"), // id1 was in the previous group
        ];
        assert_eq!(
            assign_tool_corr(&events),
            vec![None, None, None],
        );
    }

    // ---- ToolUseBlock correlation (SCHEMA-8 Phase 5e) -----------------

    #[wasm_bindgen_test]
    #[test]
    fn assign_tool_corr_tool_use_block_pairs_with_tool_call() {
        // ToolUseBlock arrives BEFORE its sibling ToolCall in the stream
        // (tool_use blocks land during streaming; the dispatch is emitted
        // after LlmResponseEnded). The algorithm still pairs them by id.
        let events = vec![
            make_tool_use_block("id1"),
            make_tool_use_block("id2"),
            // (LlmResponseEnded would normally go here, but it doesn't
            // affect correlation — only LlmCall is a group boundary.)
            make_tool_call("id1"),
            make_tool_call("id2"),
            make_tool_result("id1"),
            make_tool_result("id2"),
        ];
        assert_eq!(
            assign_tool_corr(&events),
            vec![Some(1), Some(2), Some(1), Some(2), Some(1), Some(2)],
        );
    }

    #[wasm_bindgen_test]
    #[test]
    fn assign_tool_corr_tool_use_block_single_call_suppressed() {
        // Single-call group: the single ToolUseBlock + ToolCall + ToolResult
        // triple all get None (nothing to pair within the group).
        let events = vec![
            make_tool_use_block("id1"),
            make_tool_call("id1"),
            make_tool_result("id1"),
        ];
        assert_eq!(assign_tool_corr(&events), vec![None, None, None]);
    }

    #[wasm_bindgen_test]
    #[test]
    fn assign_tool_corr_tool_use_block_orphan_gets_none() {
        // ToolUseBlock without a matching ToolCall (e.g. response was
        // discarded mid-flight before the agent dispatched the tool).
        // Counter stays at 0, suppression rule clears everything.
        let events = vec![
            make_tool_use_block("id1"),
            make_tool_use_block("id2"),
        ];
        assert_eq!(assign_tool_corr(&events), vec![None, None]);
    }

    #[wasm_bindgen_test]
    #[test]
    fn assign_tool_corr_tool_use_block_partial_match_still_numbers_rest() {
        // Two ToolUseBlocks but only one ToolCall (the other was never
        // dispatched). Counter reaches 2? No — only ToolCall increments
        // the counter. So counter = 1, suppression kicks in, everything
        // becomes None.
        let events = vec![
            make_tool_use_block("id1"),
            make_tool_use_block("id2"),
            make_tool_call("id1"),
        ];
        assert_eq!(assign_tool_corr(&events), vec![None, None, None]);
    }

    #[wasm_bindgen_test]
    #[test]
    fn assign_tool_corr_tool_use_block_two_calls_one_use_block_numbered() {
        // 2 ToolCalls (counter=2, suppression skipped), 1 ToolUseBlock that
        // matches the second ToolCall by id. The matched ToolUseBlock gets
        // Some(2); the orphan ToolCall stays Some(1).
        let events = vec![
            make_tool_use_block("id2"),
            make_tool_call("id1"),
            make_tool_call("id2"),
        ];
        assert_eq!(
            assign_tool_corr(&events),
            vec![Some(2), Some(1), Some(2)],
        );
    }

    // ---- assign_partial_counts (SCHEMA-8 Phase 5g) --------------------

    fn make_llm_response_started() -> OmegaEvent {
        OmegaEvent::LlmResponseStarted(LlmResponseStartedEvent {
            time: "2024-01-01T00:00:00.000Z".into(),
        })
    }

    fn make_llm_response_discarded() -> OmegaEvent {
        OmegaEvent::LlmResponseDiscarded(LlmResponseDiscardedEvent {
            time: "2024-01-01T00:00:00.000Z".into(),
        })
    }

    fn make_text_block(partial: bool) -> OmegaEvent {
        OmegaEvent::TextBlock(TextBlockEvent {
            time: "2024-01-01T00:00:00.000Z".into(),
            text: "hi".into(),
            partial,
        })
    }

    fn make_thinking_block(partial: bool) -> OmegaEvent {
        OmegaEvent::ThinkingBlock(ThinkingBlockEvent {
            time: "2024-01-01T00:00:00.000Z".into(),
            thinking: "hmm".into(),
            signature: if partial { None } else { Some("sig".into()) },
            partial,
        })
    }

    fn make_partial_tool_use_block(id: &str) -> OmegaEvent {
        OmegaEvent::ToolUseBlock(ToolUseBlockEvent {
            time: "2024-01-01T00:00:00.000Z".into(),
            id: id.into(),
            name: "run_command".into(),
            input: serde_json::json!({}),
            partial: true,
        })
    }

    #[wasm_bindgen_test]
    #[test]
    fn assign_partial_counts_empty_returns_empty() {
        assert_eq!(assign_partial_counts(&[]), vec![]);
    }

    #[wasm_bindgen_test]
    #[test]
    fn assign_partial_counts_no_discarded_all_none() {
        // No LlmResponseDiscarded → every slot is None even if partials exist.
        let events = vec![
            make_llm_response_started(),
            make_text_block(true),
            make_text_block(false),
        ];
        assert_eq!(assign_partial_counts(&events), vec![None, None, None]);
    }

    #[wasm_bindgen_test]
    #[test]
    fn assign_partial_counts_counts_mixed_partial_siblings() {
        // Started, partial Text, partial Thinking, partial ToolUse, Discarded
        // → result at Discarded index = Some(3); all other slots None.
        let events = vec![
            make_llm_response_started(),
            make_text_block(true),
            make_thinking_block(true),
            make_partial_tool_use_block("id1"),
            make_llm_response_discarded(),
        ];
        assert_eq!(
            assign_partial_counts(&events),
            vec![None, None, None, None, Some(3)],
        );
    }

    #[wasm_bindgen_test]
    #[test]
    fn assign_partial_counts_resets_between_responses() {
        // Response 1: Started, partial Text, Ended  (no Discarded → no count).
        // Response 2: Started, 2 partial blocks, Discarded → Some(2).
        // Counter must reset on Started (and on Ended) so response 2's count
        // doesn't include response 1's partial.
        let events = vec![
            make_llm_response_started(),
            make_text_block(true),
            OmegaEvent::LlmResponseEnded(
                omega_types::events::LlmResponseEndedEvent {
                    time: "2024-01-01T00:00:00.000Z".into(),
                    stop_reason: "end_turn".into(),
                    cleared_tool_uses: None,
                    cleared_input_tokens: None,
                    usage: LlmResponseUsage {
                        input_tokens: 0,
                        output_tokens: 0,
                        cache_creation_input_tokens: None,
                        cache_read_input_tokens: None,
                        service_tier: None,
                        iterations: None,
                    },
                    context_hash: "deadbeef".into(),
                    response_summary: None,
                },
            ),
            make_llm_response_started(),
            make_text_block(true),
            make_thinking_block(true),
            make_llm_response_discarded(),
        ];
        let result = assign_partial_counts(&events);
        assert_eq!(result[6], Some(2));
        for (i, slot) in result.iter().enumerate() {
            if i != 6 {
                assert_eq!(*slot, None, "slot {i} should be None");
            }
        }
    }

    #[wasm_bindgen_test]
    #[test]
    fn assign_partial_counts_partial_false_blocks_not_counted() {
        // Non-partial blocks (partial=false) MUST NOT be counted as discarded.
        let events = vec![
            make_llm_response_started(),
            make_text_block(false),
            make_text_block(false),
            make_thinking_block(false),
            make_llm_response_discarded(),
        ];
        assert_eq!(
            assign_partial_counts(&events),
            vec![None, None, None, None, Some(0)],
        );
    }

    #[wasm_bindgen_test]
    #[test]
    fn assign_partial_counts_discarded_with_no_partials_is_zero() {
        // Discarded immediately after Started (no content streamed) → Some(0).
        // Operator can tell "network blip before any block" from
        // "discarded after N partials".
        let events = vec![
            make_llm_response_started(),
            make_llm_response_discarded(),
        ];
        assert_eq!(assign_partial_counts(&events), vec![None, Some(0)]);
    }

    #[wasm_bindgen_test]
    #[test]
    fn assign_partial_counts_two_discards_each_counted_separately() {
        // Two abandoned responses in the same session: each Discarded reports
        // only its own preceding partials.
        let events = vec![
            make_llm_response_started(),
            make_text_block(true),
            make_llm_response_discarded(),
            make_llm_response_started(),
            make_text_block(true),
            make_thinking_block(true),
            make_partial_tool_use_block("id1"),
            make_llm_response_discarded(),
        ];
        let result = assign_partial_counts(&events);
        assert_eq!(result[2], Some(1));
        assert_eq!(result[7], Some(3));
    }

    #[wasm_bindgen_test]
    #[test]
    fn preview_run_command_no_timeout() {
        let input = serde_json::json!({ "command": "echo hi" });
        assert_eq!(tool_call_preview("run_command", &input), "echo hi");
    }

    #[wasm_bindgen_test]
    #[test]
    fn preview_run_command_with_timeout() {
        let input = serde_json::json!({ "command": "sleep 999", "timeout": 300 });
        assert_eq!(
            tool_call_preview("run_command", &input),
            "sleep 999  [timeout: 300s]",
        );
    }

    #[wasm_bindgen_test]
    #[test]
    fn preview_run_background_no_cwd() {
        let input = serde_json::json!({ "command": "cargo watch" });
        assert_eq!(tool_call_preview("run_background", &input), "cargo watch");
    }

    #[wasm_bindgen_test]
    #[test]
    fn preview_run_background_with_cwd() {
        let input = serde_json::json!({ "command": "npm start", "cwd": "frontend" });
        assert_eq!(
            tool_call_preview("run_background", &input),
            "npm start  [@ frontend]",
        );
    }

    #[wasm_bindgen_test]
    #[test]
    fn preview_wait_for_output_all_fields() {
        let input = serde_json::json!({
            "pid": 42,
            "logFile": "/tmp/out.log",
            "timeoutMs": 5000,
            "pattern": "ready"
        });
        assert_eq!(
            tool_call_preview("wait_for_output", &input),
            r#"pid=42  timeout=5000ms  pattern="ready"  /tmp/out.log"#,
        );
    }

    #[wasm_bindgen_test]
    #[test]
    fn preview_wait_for_output_no_pattern() {
        let input = serde_json::json!({
            "pid": 7,
            "logFile": "/tmp/x.log",
            "timeoutMs": 2000
        });
        assert_eq!(
            tool_call_preview("wait_for_output", &input),
            "pid=7  timeout=2000ms  /tmp/x.log",
        );
    }

    #[wasm_bindgen_test]
    #[test]
    fn preview_write_stdin_basic() {
        let input = serde_json::json!({ "pid": 12, "text": "yes\n" });
        assert_eq!(
            tool_call_preview("write_stdin", &input),
            "pid=12  yes\n",
        );
    }

    #[wasm_bindgen_test]
    #[test]
    fn preview_write_stdin_eof() {
        let input = serde_json::json!({ "pid": 12, "text": "data", "end_stdin": true });
        assert_eq!(
            tool_call_preview("write_stdin", &input),
            "pid=12  data  [EOF]",
        );
    }

    #[wasm_bindgen_test]
    #[test]
    fn preview_read_file_path_only() {
        let input = serde_json::json!({ "path": "src/main.rs" });
        assert_eq!(tool_call_preview("read_file", &input), "src/main.rs");
    }

    #[wasm_bindgen_test]
    #[test]
    fn preview_read_file_with_offset_and_limit() {
        let input = serde_json::json!({ "path": "a.txt", "offset": 10, "limit": 50 });
        assert_eq!(
            tool_call_preview("read_file", &input),
            "a.txt  [offset=10, limit=50]",
        );
    }

    #[wasm_bindgen_test]
    #[test]
    fn preview_read_file_with_offset_only() {
        let input = serde_json::json!({ "path": "b.txt", "offset": 5 });
        assert_eq!(tool_call_preview("read_file", &input), "b.txt  [offset=5]");
    }

    #[wasm_bindgen_test]
    #[test]
    fn preview_read_file_with_limit_only() {
        let input = serde_json::json!({ "path": "c.txt", "limit": 30 });
        assert_eq!(tool_call_preview("read_file", &input), "c.txt  [limit=30]");
    }

    #[wasm_bindgen_test]
    #[test]
    fn preview_list_files_not_recursive() {
        let input = serde_json::json!({ "path": "." });
        assert_eq!(tool_call_preview("list_files", &input), ".");
    }

    #[wasm_bindgen_test]
    #[test]
    fn preview_list_files_recursive() {
        let input = serde_json::json!({ "path": "src", "recursive": true });
        assert_eq!(
            tool_call_preview("list_files", &input),
            "src  [recursive]",
        );
    }

    #[wasm_bindgen_test]
    #[test]
    fn preview_write_file_shows_only_path() {
        // Content field is intentionally suppressed (can be huge).
        let input = serde_json::json!({ "path": "out.txt", "content": "hello world" });
        assert_eq!(tool_call_preview("write_file", &input), "out.txt");
    }

    #[wasm_bindgen_test]
    #[test]
    fn preview_edit_file_one_replacement() {
        let input = serde_json::json!({
            "path": "foo.rs",
            "replacements": [{"old_text": "a", "new_text": "b"}]
        });
        assert_eq!(
            tool_call_preview("edit_file", &input),
            "foo.rs  (1 replacement)",
        );
    }

    #[wasm_bindgen_test]
    #[test]
    fn preview_edit_file_many_replacements() {
        let input = serde_json::json!({
            "path": "bar.rs",
            "replacements": [
                {"old_text": "a", "new_text": "b"},
                {"old_text": "c", "new_text": "d"}
            ]
        });
        assert_eq!(
            tool_call_preview("edit_file", &input),
            "bar.rs  (2 replacements)",
        );
    }

    #[wasm_bindgen_test]
    #[test]
    fn preview_grep_files_no_glob() {
        let input = serde_json::json!({ "pattern": "fn main", "path": "src" });
        // Pattern is quoted (Debug format) to distinguish regex from path.
        assert_eq!(
            tool_call_preview("grep_files", &input),
            r#""fn main"  in src"#,
        );
    }

    #[wasm_bindgen_test]
    #[test]
    fn preview_grep_files_with_glob() {
        let input = serde_json::json!({
            "pattern": "TODO",
            "path": ".",
            "file_glob": "*.rs"
        });
        assert_eq!(
            tool_call_preview("grep_files", &input),
            r#""TODO"  in .  [*.rs]"#,
        );
    }

    #[wasm_bindgen_test]
    #[test]
    fn preview_find_files_no_type() {
        let input = serde_json::json!({ "pattern": "*.toml", "path": "." });
        assert_eq!(
            tool_call_preview("find_files", &input),
            "*.toml  in .",
        );
    }

    #[wasm_bindgen_test]
    #[test]
    fn preview_find_files_with_type() {
        let input = serde_json::json!({ "pattern": "mod.rs", "path": "src", "type": "f" });
        assert_eq!(
            tool_call_preview("find_files", &input),
            "mod.rs  in src  [type=f]",
        );
    }

    #[wasm_bindgen_test]
    #[test]
    fn preview_web_search() {
        let input = serde_json::json!({ "query": "leptos signals" });
        assert_eq!(tool_call_preview("web_search", &input), "leptos signals");
    }

    #[wasm_bindgen_test]
    #[test]
    fn preview_fetch_url_with_postprocess() {
        let input = serde_json::json!({
            "url": "https://example.com/data",
            "postprocess": "grep -n pattern"
        });
        assert_eq!(
            tool_call_preview("fetch_url", &input),
            "https://example.com/data  [grep -n pattern]",
        );
    }

    #[wasm_bindgen_test]
    #[test]
    fn preview_fetch_url_no_postprocess() {
        let input = serde_json::json!({ "url": "https://example.com", "postprocess": "" });
        assert_eq!(
            tool_call_preview("fetch_url", &input),
            "https://example.com",
        );
    }

    #[wasm_bindgen_test]
    #[test]
    fn preview_unknown_tool_falls_back_to_json() {
        let input = serde_json::json!({ "foo": "bar" });
        let result = tool_call_preview("my_custom_tool", &input);
        // Must contain the field name and value from the JSON.
        assert!(result.contains("foo"), "fallback must include field name: {result}");
        assert!(result.contains("bar"), "fallback must include field value: {result}");
    }

    // -----------------------------------------------------------------------
    // format_time
    // -----------------------------------------------------------------------

    #[wasm_bindgen_test]
    #[test]
    fn format_time_extracts_hms() {
        // With tz="UTC" the Intl path renders the input verbatim as
        // `HH:MM:SS.mmm`, matching the host-target slice fallback.
        assert_eq!(format_time("2025-01-15T12:34:56.789Z", "UTC"), "12:34:56.789");
    }

    #[wasm_bindgen_test]
    #[test]
    fn format_time_midnight() {
        assert_eq!(format_time("2025-01-15T00:00:00.000Z", "UTC"), "00:00:00.000");
    }

    #[wasm_bindgen_test]
    #[test]
    fn format_time_fallback_on_short_input() {
        // Malformed input must not panic; returns the raw string.
        assert_eq!(format_time("short", "UTC"), "short");
    }

    /// The TZ conversion only fires on wasm (browser-backed Intl).
    /// `2024-01-15T12:00:00.000Z` is winter time, so `Europe/Berlin`
    /// observes CET (+01:00) and the local wall-clock is `13:00:00.000`.
    #[wasm_bindgen_test]
    fn format_time_renders_in_agent_tz_winter() {
        assert_eq!(
            format_time("2024-01-15T12:00:00.000Z", "Europe/Berlin"),
            "13:00:00.000"
        );
    }

    /// Same instant in summer (`2024-07-15`) renders one hour later
    /// in `Europe/Berlin` (CEST, +02:00).  Demonstrates the DST
    /// correctness that motivated capturing the IANA name (rather
    /// than a fixed offset) at session-start time.
    #[wasm_bindgen_test]
    fn format_time_renders_in_agent_tz_summer() {
        assert_eq!(
            format_time("2024-07-15T12:00:00.000Z", "Europe/Berlin"),
            "14:00:00.000"
        );
    }
}
