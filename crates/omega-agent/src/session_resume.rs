//! Session-resumption helpers (pure functions over event lists).
//!
//! Mirrors `src/session-resume.ts` for the parts that don't touch the
//! agent or the LLM. Provides [`extract_resumption_basis`],
//! [`extract_summary_from_response`], the [`RESUMPTION_SUMMARY_INSTRUCTIONS`]
//! system prompt, and the [`RESUMPTION_MODEL`] / [`RESUMPTION_EFFORT`]
//! defaults consumed by
//! [`Agent::perform_resumption`](crate::Agent::perform_resumption).

use std::collections::HashMap;

use omega_types::{OmegaEvent, events::InterruptReason};

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

/// Extract the parent session's `tool_selection` from its event log.
///
/// The Phase 1.2 design pins the session's enabled toolset on the
/// `SessionStartedEvent`.  When a successor session is spawned via the
/// resume path, the new `AgentConfig.tool_selection` is seeded from this
/// value so the successor exposes the same toolset as its parent (rather
/// than re-defaulting to `DEFAULT_TOOL_NAMES`, which would silently widen
/// or narrow the toolset across the boundary).
///
/// Returns `Some(selection)` when a `SessionStartedEvent` with a
/// non-empty `tool_selection` is found in `events`.  Returns `None` when
/// the event log contains no such event (e.g. an empty / malformed log,
/// or one written by a pre-Phase-1.2 binary that has since been pruned).
/// In that case the caller falls back to the resolver in `Agent::new`,
/// which defaults to `DEFAULT_TOOL_NAMES`.
///
/// This is a pure function — no I/O.
#[must_use]
pub fn extract_tool_selection(events: &[OmegaEvent]) -> Option<Vec<String>> {
    events.iter().find_map(|e| match e {
        OmegaEvent::SessionStarted(s) if !s.tool_selection.is_empty() => {
            Some(s.tool_selection.clone())
        }
        _ => None,
    })
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
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]

    use omega_types::{
        ContinueMode, OmegaEvent,
        events::{
            AgentErrorEvent, InterruptReason, LlmResponseEndedEvent, LlmResponseUsage,
            SessionResumedEvent, TextBlockEvent, ToolCallEvent, ToolResultEvent,
            TurnContinuedEvent, TurnEndEvent, TurnInterruptedEvent, TurnMetrics, TurnPausedEvent,
            UserMessageEvent,
        },
    };

    use super::*;

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
    // extract_tool_selection (Phase 1.2)
    // -----------------------------------------------------------------------

    fn session_started_with(tool_selection: Vec<String>) -> OmegaEvent {
        use omega_types::FeatureFlags;
        use omega_types::events::SessionStartedEvent;
        use omega_types::ids::Origin;
        OmegaEvent::SessionStarted(SessionStartedEvent {
            time: t(),
            session_id: omega_types::ids::SessionId(uuid::Uuid::nil()),
            path: "sessions/session-1".to_owned(),
            model: "claude-sonnet-4-6".to_owned(),
            effort: "medium".to_owned(),
            system_prompt: "p".to_owned(),
            omega_commit: "u".to_owned(),
            agent_time_zone: "UTC".to_owned(),
            origin: Origin::Root,
            features: FeatureFlags::default(),
            tool_selection,
        })
    }

    /// Spec test 5 — the successor session sees the same toolset as the
    /// parent.  When the parent's event log contains a `SessionStarted`
    /// event with a non-empty `tool_selection`, `extract_tool_selection`
    /// returns that selection so the router can seed the new
    /// `AgentConfig.tool_selection` with it.
    #[test]
    fn extract_tool_selection_returns_parent_selection() {
        let parent_selection = vec![
            "python_repl".to_owned(),
            "web_search".to_owned(),
            "fetch_url".to_owned(),
        ];
        let events = vec![
            session_started_with(parent_selection.clone()),
            user_msg("hi"),
            turn_end(),
        ];
        assert_eq!(
            extract_tool_selection(&events),
            Some(parent_selection),
            "successor session must inherit the parent's tool_selection",
        );
    }

    /// When the parent log has no `SessionStarted` event — e.g. a
    /// malformed log, or one truncated before init — the helper returns
    /// `None`, and the caller (router) falls back to
    /// `DEFAULT_TOOL_NAMES` via the resolver in `Agent::new`.
    #[test]
    fn extract_tool_selection_returns_none_when_no_session_started() {
        let events = vec![user_msg("hi"), turn_end()];
        assert_eq!(extract_tool_selection(&events), None);
    }

    /// An explicit empty `tool_selection` on the parent event is treated
    /// as "no selection found" so the successor falls back to the
    /// default — better than starting a session with zero tools.
    #[test]
    fn extract_tool_selection_returns_none_when_selection_empty() {
        let events = vec![session_started_with(Vec::new())];
        assert_eq!(extract_tool_selection(&events), None);
    }
}
