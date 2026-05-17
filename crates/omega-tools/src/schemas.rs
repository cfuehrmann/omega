//! Tool definitions: the JSON-Schema input descriptors sent to the LLM.
//!
//! Each definition's `name` and `description` are stable contracts — changing
//! them changes how the model invokes the tool. Keep these in sync with
//! `src/tools.schema.ts` and `src/tools.ts` in the TypeScript codebase.

use omega_core::ToolDefinition;
use serde_json::{Value, json};

/// All twelve tools Omega exposes, in stable order.
///
/// Order matches the TS `toolDefinitions` array. The agent does not depend
/// on order, but tests do (snapshot/round-trip).
#[must_use]
pub fn tool_definitions() -> Vec<ToolDefinition> {
    vec![
        read_file(),
        write_file(),
        run_command(),
        edit_file(),
        list_files(),
        web_search(),
        fetch_url(),
        grep_files(),
        find_files(),
        run_background(),
        wait_for_output(),
        write_stdin(),
    ]
}

// -----------------------------------------------------------------------
// Per-tool definitions
// -----------------------------------------------------------------------

fn read_file() -> ToolDefinition {
    ToolDefinition {
        name: "read_file".into(),
        description: "Read the contents of a file. Returns the file content as text. \
                      For large files, use offset and limit to read a specific line range."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "path":   { "type": "string", "description": "Path to the file (absolute or relative to cwd)" },
                "offset": { "type": "number", "description": "Starting line number (1-indexed, optional)" },
                "limit":  { "type": "number", "description": "Maximum number of lines to read (optional)" },
            },
            "required": ["path"],
        }),
    }
}

fn write_file() -> ToolDefinition {
    ToolDefinition {
        name: "write_file".into(),
        description: "Write content to a file. Creates the file if it doesn't exist, \
                      overwrites if it does. Creates parent directories as needed. \
                      WARNING: file content is generated inside the output token budget. \
                      Files longer than ~500 lines or ~20 000 characters risk being cut off mid-write. \
                      For large new files write a skeleton first, then extend with edit_file. \
                      For large existing files always prefer edit_file over a full rewrite."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "path":    { "type": "string", "description": "Path to the file (absolute or relative to cwd)" },
                "content": { "type": "string", "description": "Content to write to the file" },
            },
            "required": ["path", "content"],
        }),
    }
}

fn run_command() -> ToolDefinition {
    ToolDefinition {
        name: "run_command".into(),
        description: "Execute a shell command and return its stdout, stderr, and exit code. \
                      The command runs in the current working directory. \
                      The default timeout is 120\u{00a0}s \u{2014} pass a higher value for very slow commands. \
                      Output is always tee\u{2019}d to a session-cache log file and the path is \
                      surfaced in a footer on every result: \
                      `[full output: <path>]` when the result fits, or \
                      `[truncated; showed last 100 KB of 487 KB. Full output: <path>]` when capped. \
                      For follow-up queries on the same output, use `read_file` or `grep_files` \
                      on the cache path instead of re-running the command. \
                      Pass `truncation_bias` to control which portion is returned \
                      (default: tail on non-zero exit, head on exit 0)."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "The shell command to execute" },
                "timeout": { "type": "number", "description": "Timeout in seconds (optional, default 120)" },
                "truncation_bias": {
                    "type": "string",
                    "enum": ["head", "tail", "middle"],
                    "description": "Which part of the output to show when the result is truncated (optional). \
                                    Default: \"tail\" on non-zero exit (errors at end), \"head\" on exit 0. \
                                    Use \"middle\" to see both start and end."
                },
            },
            "required": ["command"],
        }),
    }
}

fn edit_file() -> ToolDefinition {
    let replacement: Value = json!({
        "type": "object",
        "properties": {
            "old_text": { "type": "string", "description": "Exact text to find (must match exactly, must appear once)" },
            "new_text": { "type": "string", "description": "Text to replace old_text with" },
        },
        "required": ["old_text", "new_text"],
    });

    ToolDefinition {
        name: "edit_file".into(),
        description: "Edit a file by replacing exact text. The old_text must match exactly \
                      (including whitespace and indentation). Use this for surgical edits \
                      instead of rewriting entire files with write_file. Each old_text must \
                      appear exactly once in the file. For multiple edits to the same file, \
                      pass a `replacements` array \u{2014} this is faster and avoids round-trips. \
                      Always pass ALL changes to a file in a single call."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to the file (absolute or relative to cwd)" },
                "replacements": {
                    "type": "array",
                    "items": replacement,
                    "description": "One or more replacements to apply in order. Each old_text must appear exactly once in the file. Pass all changes to this file together \u{2014} never call edit_file on the same file twice in a row.",
                },
            },
            "required": ["path", "replacements"],
        }),
    }
}

fn list_files() -> ToolDefinition {
    ToolDefinition {
        name: "list_files".into(),
        description: "List files and directories. Returns names with '/' suffix for directories. \
                      Use recursive to list the full tree (up to 1000 entries)."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "path":      { "type": "string",  "description": "Directory path (absolute or relative to cwd)" },
                "recursive": { "type": "boolean", "description": "List recursively (optional, default false)" },
            },
            "required": ["path"],
        }),
    }
}

fn web_search() -> ToolDefinition {
    ToolDefinition {
        name: "web_search".into(),
        description: "Search the web using Brave Search. Returns titles, URLs, and snippets for the top results. \
                      Use this to look up documentation, current information, or anything not in local files."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "The search query" },
            },
            "required": ["query"],
        }),
    }
}

fn fetch_url() -> ToolDefinition {
    ToolDefinition {
        name: "fetch_url".into(),
        description: "Download a URL to a session-local cache file (content-addressed by URL hash) and \
                      immediately run a shell postprocessing command on the full downloaded text. \
                      HTML is converted to readable text before caching. \
                      The tool result contains the cache file path and the postprocess output. \
                      The postprocess output is always tee\u{2019}d to a log file; a footer \
                      (`[full output: <path>]` or `[truncated; showed first N\u{00a0}KB of M\u{00a0}KB. Full output: <path>]`) \
                      appears on every result. For further queries on the same content, \
                      use read_file or grep_files on the cache path. \
                      postprocess is required and receives the full content on stdin. \
                      Prefer grep or awk when you know what to look for, head -N as the catch-all. \
                      Never use cat \u{2014} head -N gives the same result on short pages and stays bounded on long ones."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "url":         { "type": "string", "description": "The URL to fetch (must be http or https)" },
                "postprocess": {
                    "type": "string",
                    "description": "Shell command to run on the downloaded text, received on stdin. Examples: grep -n 'pattern', head -80, jq '.', awk '/foo/', python3 -c '...'. Required: decide what to extract before fetching.",
                },
            },
            "required": ["url", "postprocess"],
        }),
    }
}

fn grep_files() -> ToolDefinition {
    ToolDefinition {
        name: "grep_files".into(),
        description: "Search for a pattern across files in a directory using ripgrep (rg) with grep fallback. \
                      Returns structured file:line:text matches, capped at max_results (default 200). \
                      Use this to find all occurrences of a symbol, string, or regex across the codebase \
                      instead of reading files speculatively. Chain with read_file to inspect context. \
                      By default includes 2 context lines around each match; pass 0 for bare matches."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "pattern":        { "type": "string",  "description": "Regex or literal string to search for" },
                "path":           { "type": "string",  "description": "Directory (or file) path to search in" },
                "file_glob":      { "type": "string",  "description": "Optional glob to restrict which files are searched (e.g. '*.ts')" },
                "context_lines":  { "type": "number",  "default": 2, "description": "Number of context lines to include before and after each match (default 2, pass 0 for bare matches)" },
                "case_sensitive": { "type": "boolean", "description": "If true, match is case-sensitive. Default: false (case-insensitive)" },
                "max_results":    { "type": "number",  "description": "Maximum number of match lines to return (default 200)" },
            },
            "required": ["pattern", "path"],
        }),
    }
}

fn find_files() -> ToolDefinition {
    ToolDefinition {
        name: "find_files".into(),
        description: "Find files and directories by name/glob pattern using fd (with find fallback). \
                      Returns a list of matching paths, capped at max_results (default 200). \
                      Use this to locate files when you know the name or extension but not the exact path. \
                      Ignores hidden files and .gitignore'd paths by default (set hidden=true to include them). \
                      Chain with read_file or grep_files to inspect contents."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "pattern":     { "type": "string",  "description": "Glob or regex pattern to match against file/directory names" },
                "path":        { "type": "string",  "description": "Root directory to search in" },
                "type":        { "type": "string",  "description": "Filter by entry type: 'f' (files), 'd' (directories), 'l' (symlinks). Omit for all." },
                "hidden":      { "type": "boolean", "description": "Include hidden files and .gitignore'd paths (default false)" },
                "max_results": { "type": "number",  "description": "Maximum number of results to return (default 200)" },
            },
            "required": ["pattern", "path"],
        }),
    }
}

fn run_background() -> ToolDefinition {
    ToolDefinition {
        name: "run_background".into(),
        description: "Start a long-running process in the background and return immediately. \
                      stdout and stderr are redirected to a temporary log file. \
                      Returns { pid, logFile }. \
                      Use read_file on logFile (with offset/limit for large output) and grep_files to inspect output. \
                      Use run_command(\"kill <pid>\") to stop the process early. \
                      Reserve this for processes that must stay alive indefinitely \
                      (dev servers, file watchers, interactive processes that need write_stdin). \
                      For finite commands (builds, test suites, commits), prefer run_command with a sufficient timeout."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "Shell command to run in the background" },
                "cwd":     { "type": "string", "description": "Working directory for the process (optional, defaults to cwd)" },
            },
            "required": ["command"],
        }),
    }
}

fn wait_for_output() -> ToolDefinition {
    ToolDefinition {
        name: "wait_for_output".into(),
        description: "Poll a background-process log file until a condition is met, then return the log contents. \
                      Returns when the FIRST of these occurs: (1) pattern appears in the log, \
                      (2) log reaches minBytes in size, (3) the process exits, or (4) timeoutMs elapses. \
                      If neither pattern nor minBytes is given, returns as soon as any output appears. \
                      Returns { output, matched, minBytesReached, timedOut, processExited?, exitCode? }. \
                      Pass the pid returned by run_background so that an early process exit is detected immediately \
                      instead of waiting for the full timeout. \
                      Use this after run_background instead of sleep + tail to wait for a server or process to become ready. \
                      The pattern is interpreted as a JavaScript regex (e.g. 'ready|started|Error' for alternation). \
                      The polled output is also tee\u{2019}d to a session-cache snapshot; the cache path is surfaced \
                      in the `output` field\u{2019}s footer (`[full output: <path>]` or `[truncated; \u{2026}]`) and can be re-read \
                      with `read_file` / `grep_files` for follow-up queries."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "logFile":   { "type": "string", "description": "Path to the log file to monitor \u{2014} the logFile value returned by run_background." },
                "pid":       { "type": "number", "description": "The pid returned by run_background. Used to detect process exit: if the process dies before the pattern matches, wait_for_output returns immediately with processExited=true and the exit code, rather than waiting for the full timeout." },
                "timeoutMs": { "type": "number", "description": "Maximum milliseconds to wait before giving up and returning whatever the log contains." },
                "pattern":   { "type": "string", "description": "Return as soon as this pattern matches anywhere in the log. Interpreted as a JavaScript regex, so use '|' for alternation (e.g. 'ready|started|Error'). Simple strings like 'ready' also work as-is." },
                "minBytes":  { "type": "number", "description": "Return as soon as the log reaches this many bytes. Useful when you don't know the ready signal but want to wait for meaningful output." },
            },
            "required": ["logFile", "pid", "timeoutMs"],
        }),
    }
}

fn write_stdin() -> ToolDefinition {
    ToolDefinition {
        name: "write_stdin".into(),
        description: "Write text to the stdin of a background process started with run_background. \
                      Use this to answer interactive prompts (e.g. y/n confirmations, passwords, menu choices). \
                      Include a newline ('\\n') at the end of text to submit a line-based prompt. \
                      Set end_stdin=true to close stdin after writing, signalling EOF to the process \
                      (required for programs like cat that read until end of input). \
                      Returns an error if the pid is not a tracked background process or stdin is already closed."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "pid":       { "type": "number",  "description": "Process ID returned by run_background." },
                "text":      { "type": "string",  "description": "Text to write to the process stdin. Include a newline ('\\n') to submit a line-based prompt." },
                "end_stdin": { "type": "boolean", "description": "If true, close stdin after writing, signalling EOF to the process. Required for programs that read until end-of-input (e.g. cat). Default false." },
            },
            "required": ["pid", "text"],
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twelve_tools_in_stable_order() {
        let names: Vec<String> = tool_definitions().into_iter().map(|d| d.name).collect();
        assert_eq!(
            names,
            vec![
                "read_file",
                "write_file",
                "run_command",
                "edit_file",
                "list_files",
                "web_search",
                "fetch_url",
                "grep_files",
                "find_files",
                "run_background",
                "wait_for_output",
                "write_stdin",
            ]
        );
    }

    #[test]
    fn every_schema_is_an_object_with_required_array() {
        for def in tool_definitions() {
            let schema = &def.input_schema;
            assert_eq!(
                schema["type"], "object",
                "{} schema not an object",
                def.name
            );
            assert!(
                schema["properties"].is_object(),
                "{} missing properties",
                def.name
            );
            assert!(
                schema["required"].is_array(),
                "{} missing required[]",
                def.name
            );
        }
    }

    #[test]
    fn descriptions_are_non_empty() {
        for def in tool_definitions() {
            assert!(
                !def.description.is_empty(),
                "{} has empty description",
                def.name
            );
        }
    }
}
