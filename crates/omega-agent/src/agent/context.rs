// ---------------------------------------------------------------------------
// Context management and projection helpers
// ---------------------------------------------------------------------------

use omega_core::{Message, Role};
use omega_types::events::MonitorStopReason;

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
pub(in crate::agent) fn build_context_management() -> serde_json::Value {
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

// ---------------------------------------------------------------------------
// Message projection
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

// ---------------------------------------------------------------------------
// Monitor formatting helpers
// ---------------------------------------------------------------------------

/// Format monitor stdout lines for injection into the LLM context.
///
/// The XML-style tag unambiguously marks the text as automated monitor
/// output — not a human user message — even when it lands in a merged
/// `role:user` turn alongside real human text.
pub(in crate::agent) fn format_monitor_lines(monitor_id: &str, lines: &[String]) -> String {
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
pub(in crate::agent) fn format_monitor_stopped(
    id: &str,
    reason: &MonitorStopReason,
    exit_code: Option<i32>,
) -> String {
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
