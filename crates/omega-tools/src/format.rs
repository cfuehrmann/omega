//! Human-readable rendering of `name(input)` for log lines.
//!
//! Mirrors `formatToolCall` in the TypeScript codebase. The output format is
//! a UI/CLI contract — changes here surface in the user-facing event log.

use serde_json::Value;
use std::fmt::Write as _;

/// Render a one-line summary of a tool call.
///
/// For each known tool, surfaces the most useful identifying field
/// (path, command, query, etc.) plus a few small flags. For unknown tools,
/// falls back to the full JSON — this is intentional so a misbehaving
/// model leaves a debuggable trace rather than a vague placeholder.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn format_tool_call(name: &str, input: &Value) -> String {
    match name {
        "read_file" => {
            let mut s = format!("read_file: {}", string_field(input, "path"));
            if let Some(offset) = num_field(input, "offset") {
                let _ = write!(s, " (from line {offset})");
            }
            if let Some(limit) = num_field(input, "limit") {
                let _ = write!(s, " ({limit} lines)");
            }
            s
        }
        "write_file" => {
            let path = string_field(input, "path");
            let bytes = input
                .get("content")
                .and_then(Value::as_str)
                .map_or(0, str::len);
            format!("write_file: {path} ({bytes} bytes)")
        }
        "edit_file" => {
            let count = input
                .get("replacements")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            let suffix = if count == 1 { "" } else { "s" };
            format!(
                "edit_file: {} ({count} replacement{suffix})",
                string_field(input, "path"),
            )
        }
        "run_command" => format!("run_command: {}", string_field(input, "command")),
        "list_files" => {
            let mut s = format!("list_files: {}", string_field(input, "path"));
            if input
                .get("recursive")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                s.push_str(" (recursive)");
            }
            s
        }
        "web_search" => format!("web_search: {}", string_field(input, "query")),
        "fetch_url" => format!(
            "fetch_url: {} | {}",
            string_field(input, "url"),
            input
                .get("postprocess")
                .and_then(Value::as_str)
                .unwrap_or(""),
        ),
        "grep_files" => {
            let mut s = format!(
                "grep_files: {} in {}",
                string_field(input, "pattern"),
                string_field(input, "path"),
            );
            if let Some(g) = input.get("file_glob").and_then(Value::as_str) {
                let _ = write!(s, " [{g}]");
            }
            s
        }
        "find_files" => {
            let mut s = format!(
                "find_files: {} in {}",
                string_field(input, "pattern"),
                string_field(input, "path"),
            );
            if let Some(t) = input.get("type").and_then(Value::as_str) {
                let _ = write!(s, " [type={t}]");
            }
            s
        }
        "run_background" => format!("run_background: {}", string_field(input, "command")),
        "wait_for_output" => {
            let timeout = num_field(input, "timeoutMs").unwrap_or(0.0);
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let ms = timeout as u64;
            let mut s = format!(
                "wait_for_output: {} (timeout {ms}ms)",
                string_field(input, "logFile"),
            );
            if let Some(p) = input.get("pattern").and_then(Value::as_str) {
                let _ = write!(s, " pattern=\"{p}\"");
            }
            if let Some(b) = num_field(input, "minBytes") {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let bb = b as u64;
                let _ = write!(s, " minBytes={bb}");
            }
            s
        }
        "write_stdin" => {
            let pid = num_field(input, "pid").unwrap_or(0.0);
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let pid_u = pid as u64;
            let chars = input
                .get("text")
                .and_then(Value::as_str)
                .map_or(0, |s| s.chars().count());
            let mut s = format!("write_stdin: pid {pid_u} ({chars} chars)");
            if input
                .get("end_stdin")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                s.push_str(" [close stdin]");
            }
            s
        }
        other => format!("{other}: {input}"),
    }
}

fn string_field(input: &Value, key: &str) -> String {
    input
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn num_field(input: &Value, key: &str) -> Option<f64> {
    input.get(key).and_then(Value::as_f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn read_file_basic() {
        assert_eq!(
            format_tool_call("read_file", &json!({"path": "src/lib.rs"})),
            "read_file: src/lib.rs",
        );
    }

    #[test]
    fn read_file_with_offset_and_limit() {
        let out = format_tool_call(
            "read_file",
            &json!({"path": "x", "offset": 10, "limit": 50}),
        );
        assert_eq!(out, "read_file: x (from line 10) (50 lines)");
    }

    #[test]
    fn write_file_byte_count() {
        let out = format_tool_call("write_file", &json!({"path": "a", "content": "hello"}));
        assert_eq!(out, "write_file: a (5 bytes)");
    }

    #[test]
    fn edit_file_pluralisation() {
        let one = format_tool_call(
            "edit_file",
            &json!({"path": "p", "replacements": [{"old_text":"a","new_text":"b"}]}),
        );
        assert_eq!(one, "edit_file: p (1 replacement)");
        let two = format_tool_call(
            "edit_file",
            &json!({"path": "p", "replacements": [{"old_text":"a","new_text":"b"},{"old_text":"c","new_text":"d"}]}),
        );
        assert_eq!(two, "edit_file: p (2 replacements)");
        let zero = format_tool_call("edit_file", &json!({"path": "p"}));
        assert_eq!(zero, "edit_file: p (0 replacements)");
    }

    #[test]
    fn list_files_recursive() {
        assert_eq!(
            format_tool_call("list_files", &json!({"path":"x","recursive":true})),
            "list_files: x (recursive)",
        );
        assert_eq!(
            format_tool_call("list_files", &json!({"path":"x"})),
            "list_files: x",
        );
    }

    #[test]
    fn grep_with_glob() {
        let out = format_tool_call(
            "grep_files",
            &json!({"pattern":"foo","path":".","file_glob":"*.rs"}),
        );
        assert_eq!(out, "grep_files: foo in . [*.rs]");
    }

    #[test]
    fn write_stdin_chars_count_unicode() {
        // chars(), not bytes — "café" is 4 chars / 5 bytes.
        let out = format_tool_call("write_stdin", &json!({"pid": 42, "text": "café"}));
        assert_eq!(out, "write_stdin: pid 42 (4 chars)");
    }

    #[test]
    fn write_stdin_with_close() {
        let out = format_tool_call(
            "write_stdin",
            &json!({"pid": 7, "text": "x", "end_stdin": true}),
        );
        assert_eq!(out, "write_stdin: pid 7 (1 chars) [close stdin]");
    }

    #[test]
    fn unknown_tool_falls_back_to_json() {
        let out = format_tool_call("mystery", &json!({"x": 1}));
        assert!(out.starts_with("mystery: "));
        assert!(out.contains("\"x\":1"));
    }
}
