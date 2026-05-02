//! Session-resumption helpers (pure functions over event lists).
//!
//! Mirrors `src/session-resume.ts` for the parts that don't touch the
//! agent or the LLM. Phase 1d.1a ports [`extract_last_model_and_effort`];
//! Phase 1d.1b adds [`extract_resumption_basis`],
//! [`extract_summary_from_response`], and [`extract_description_from_response`].

use std::collections::HashMap;

use omega_protocol::{OmegaEvent, events::InterruptReason};

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
            OmegaEvent::LlmResponse(e) => {
                if let Some(text) = &e.text {
                    lines.push(format!("\nAgent: {}", text.trim()));
                }
            }
            OmegaEvent::ToolCall(e) => {
                tool_calls.insert(e.id.clone(), (e.name.clone(), e.input.clone()));
            }
            OmegaEvent::ToolResult(e) => {
                let arg = tool_calls
                    .get(&e.id)
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
            OmegaEvent::Compacted(_) => {
                lines.push("\n[Context compacted by server]".to_owned());
            }
            _ => {}
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
// Public: summary/description extraction from LLM response (Phase 1d.1b)
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

/// Extract the description from an LLM response.
///
/// Parses the `<description>…</description>` block if present, hard-capped
/// at 120 characters. Returns `None` when the block is absent.
///
/// Mirrors `extractDescriptionFromResponse` in `src/session-resume.ts`.
#[must_use]
pub fn extract_description_from_response(response_text: &str) -> Option<String> {
    let content = extract_block(response_text, "description")?;
    let truncated: String = content.chars().take(120).collect();
    Some(truncated)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]

    use omega_protocol::{
        ContinueMode, OmegaEvent,
        events::{
            AgentErrorEvent, CompactedEvent, EffortChangedEvent, InterruptReason, LlmResponseEvent,
            LlmResponseUsage, ModelChangedEvent, SessionResumedEvent, ToolCallEvent,
            ToolResultEvent, TurnContinuedEvent, TurnEndEvent, TurnInterruptedEvent, TurnMetrics,
            TurnPausedEvent, UserMessageEvent,
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

    fn llm_response(text: Option<&str>) -> OmegaEvent {
        OmegaEvent::LlmResponse(LlmResponseEvent {
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
            },
            context_hash: "aabbcc".to_owned(),
            text: text.map(str::to_owned),
            thinking: None,
            streaming_start: None,
            response_summary: None,
        })
    }

    fn tool_call(id: &str, name: &str, input: serde_json::Value) -> OmegaEvent {
        OmegaEvent::ToolCall(ToolCallEvent {
            time: t(),
            id: id.to_owned(),
            name: name.to_owned(),
            input,
            context_hash: "aabbcc".to_owned(),
        })
    }

    fn tool_result(id: &str, name: &str, is_error: bool, output: &str) -> OmegaEvent {
        OmegaEvent::ToolResult(ToolResultEvent {
            time: t(),
            id: id.to_owned(),
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

    fn compacted() -> OmegaEvent {
        OmegaEvent::Compacted(CompactedEvent {
            time: t(),
            usage: serde_json::json!({}),
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
    // extract_description_from_response
    // -----------------------------------------------------------------------

    #[test]
    fn description_extracts_block_when_present() {
        let text = "<description>Added login endpoint</description>";
        assert_eq!(
            extract_description_from_response(text),
            Some("Added login endpoint".to_owned())
        );
    }

    #[test]
    fn description_returns_none_when_absent() {
        assert_eq!(extract_description_from_response("no tags"), None);
    }

    #[test]
    fn description_trims_whitespace_inside_block() {
        let text = "<description>  padded  </description>";
        assert_eq!(
            extract_description_from_response(text),
            Some("padded".to_owned())
        );
    }

    #[test]
    fn description_truncates_at_120_chars() {
        let long_desc: String = "a".repeat(150);
        let text = format!("<description>{long_desc}</description>");
        let result = extract_description_from_response(&text).unwrap();
        assert_eq!(result.len(), 120);
        assert_eq!(&result, &"a".repeat(120));
    }

    #[test]
    fn description_exactly_120_chars_not_truncated() {
        let exactly: String = "b".repeat(120);
        let text = format!("<description>{exactly}</description>");
        let result = extract_description_from_response(&text).unwrap();
        assert_eq!(result.len(), 120);
    }

    #[test]
    fn description_119_chars_not_truncated() {
        // Boundary: one below 120 must survive intact.
        let s: String = "c".repeat(119);
        let text = format!("<description>{s}</description>");
        let result = extract_description_from_response(&text).unwrap();
        assert_eq!(result.len(), 119);
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
        // LlmResponse before any user_message — no turns formed.
        let evs = vec![llm_response(Some("hi"))];
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
        let evs = vec![user_msg("q"), llm_response(Some("answer")), turn_end()];
        let result = extract_resumption_basis(&evs);
        assert!(result.contains("Agent: answer"), "got: {result}");
    }

    #[test]
    fn basis_llm_response_without_text_emits_nothing() {
        let evs = vec![user_msg("q"), llm_response(None), turn_end()];
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
    // extract_resumption_basis — compacted
    // -----------------------------------------------------------------------

    #[test]
    fn basis_compacted_renders_bracketed_text() {
        let evs = vec![user_msg("q"), compacted(), turn_end()];
        let result = extract_resumption_basis(&evs);
        assert!(
            result.contains("[Context compacted by server]"),
            "got: {result}"
        );
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
        // LlmResponse before first user_message is outside any turn.
        let evs = vec![
            llm_response(Some("stray")),
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
}
