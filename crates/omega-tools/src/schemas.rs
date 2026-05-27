//! Tool definitions: the JSON-Schema input descriptors sent to the LLM.
//!
//! Each definition's `name` and `description` are stable contracts — changing
//! them changes how the model invokes the tool. Keep these in sync with
//! `src/tools.schema.ts` and `src/tools.ts` in the TypeScript codebase.

use omega_core::ToolDefinition;
use serde_json::{Value, json};

/// The default toolset — twelve tools, no `python_repl`.
///
/// Used when [`AgentConfig::tool_selection`] is `None`.  Order is canonical
/// and matches the order `tool_definitions` emits.
///
/// [`AgentConfig::tool_selection`]: ../../omega_agent/struct.AgentConfig.html#structfield.tool_selection
pub const DEFAULT_TOOL_NAMES: &[&str] = &[
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
];

/// Every tool Omega knows how to expose, in canonical order.
///
/// `python_repl` is in `ALL_TOOL_NAMES` but not in [`DEFAULT_TOOL_NAMES`] —
/// it must be requested explicitly via
/// `AgentConfig::tool_selection`.
///
/// Names not present in this list are rejected by the agent at session
/// creation time.
pub const ALL_TOOL_NAMES: &[&str] = &[
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
    "python_repl",
];

/// Build the tool definitions exposed to the LLM for this session.
///
/// Iterates [`ALL_TOOL_NAMES`] in canonical order and emits the definition
/// for each name that appears in `tool_selection`.
///
/// `fetch_url` keeps its current shell-aware schema variant: when no
/// shell-execution tool is present in the selection, `fetch_url` switches
/// to a postprocess-free schema (so it cannot be used as a shell
/// loophole).  When any of `run_command`, `run_background`,
/// `wait_for_output`, or `write_stdin` is selected, the full schema with
/// a required `postprocess` argument is used.
///
/// Order in the returned `Vec` matches `ALL_TOOL_NAMES`, not the order of
/// `tool_selection`.
#[must_use]
pub fn tool_definitions(tool_selection: &[String]) -> Vec<ToolDefinition> {
    let shell_tools_present = tool_selection.iter().any(|n| {
        matches!(
            n.as_str(),
            "run_command" | "run_background" | "wait_for_output" | "write_stdin"
        )
    });
    let mut out = Vec::new();
    for &name in ALL_TOOL_NAMES {
        if !tool_selection.iter().any(|n| n == name) {
            continue;
        }
        let def = match name {
            "read_file" => read_file(),
            "write_file" => write_file(),
            "run_command" => run_command(),
            "edit_file" => edit_file(),
            "list_files" => list_files(),
            "web_search" => web_search(),
            "fetch_url" => fetch_url(shell_tools_present),
            "grep_files" => grep_files(),
            "find_files" => find_files(),
            "run_background" => run_background(),
            "wait_for_output" => wait_for_output(),
            "write_stdin" => write_stdin(),
            "python_repl" => python_repl(),
            // Unreachable: ALL_TOOL_NAMES is the source of truth and is
            // covered by an exhaustive match here — every name we iterate
            // came from this list.  If a new entry is added to
            // ALL_TOOL_NAMES without an arm here, the `unreachable!` will
            // fire in tests immediately.
            other => unreachable!("unhandled tool name in ALL_TOOL_NAMES: {other}"),
        };
        out.push(def);
    }
    out
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
                      surfaced in a footer: \
                      `[full output: <path>]` when the result fits, or \
                      `[truncated; showed last 100 KB of 487 KB. Full output: <path>]` when capped. \
                      When the result is **truncated**, use `read_file` or `grep_files` on the \
                      cache path to recover the bytes that didn\u{2019}t fit inline. The cache is also \
                      useful when an earlier (full) output has aged out of immediate context and \
                      you need to revisit it without re-running. When the bytes you need are \
                      already inline and recent, read them directly. \
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

fn fetch_url(shell_tools_present: bool) -> ToolDefinition {
    if shell_tools_present {
        // Default mode: postprocess is a required shell pipeline applied to the
        // fetched content.
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
    } else {
        // Shell-gated mode: postprocess is disabled to prevent using
        // fetch_url as a shell-command loophole.  Content is returned
        // with a byte/line-level cap; use python_repl for further filtering.
        ToolDefinition {
            name: "fetch_url".into(),
            description: "Download a URL to a session-local cache file (content-addressed by URL hash) \
                          and return the content as text. HTML is converted to readable text before caching. \
                          The result is capped at 2000 lines / 50\u{00a0}KB (whichever is hit first); \
                          a truncation marker is appended when the cap is reached. \
                          For further filtering or analysis, pass the cache path to python_repl."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "The URL to fetch (must be http or https)" },
                },
                "required": ["url"],
            }),
        }
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
                      in the `output` field\u{2019}s footer (`[full output: <path>]` or `[truncated; \u{2026}]`). \
                      When the output is **truncated**, use `read_file` / `grep_files` on the cache path \
                      to recover what didn\u{2019}t fit inline; when bytes are already inline and recent, \
                      read them directly."
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

fn python_repl() -> ToolDefinition {
    ToolDefinition {
        name: "python_repl".into(),
        description: "Execute Python code in a stateful REPL. Variables defined \
                      in one call persist to subsequent calls within the session. \
                      Returns combined stdout/stderr output, truncated if long. \
                      Useful for calculations, data parsing, exploration, and \
                      composing intermediate results. \
                      Optional `timeout` parameter (default 60 s, max 600 s) is the \
                      OUTER bound on the call; any inner timeouts in the code itself \
                      (subprocess.run timeout, threading joins, time.sleep) must be \
                      strictly less than this outer value, and sequential operations \
                      must total less than it.  For known-slow operations raise the \
                      outer timeout rather than rely on inner subprocess timeouts."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "code":    { "type": "string", "description": "Python code to execute" },
                "timeout": { "type": "number", "description": "Outer per-call timeout in seconds (optional, default 60, max 600). Any inner timeouts in the code (subprocess.run timeout, threading joins, time.sleep durations) must be strictly less than this value to do useful work \u{2014} the outer fires first. On outer timeout: SIGINT is sent first; if the kernel does not recover within 2 s, the process group is SIGKILL'd and all REPL state is lost." },
            },
            "required": ["code"],
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
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    fn sel(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| (*s).to_owned()).collect()
    }

    fn sel_default() -> Vec<String> {
        sel(DEFAULT_TOOL_NAMES)
    }

    fn sel_default_plus_repl() -> Vec<String> {
        let mut v = sel(DEFAULT_TOOL_NAMES);
        v.push("python_repl".into());
        v
    }

    /// Shell-only + `python_repl` + web/fetch — the "no file tools" selection.
    fn sel_no_file_tools() -> Vec<String> {
        sel(&[
            "run_command",
            "run_background",
            "wait_for_output",
            "write_stdin",
            "web_search",
            "fetch_url",
            "python_repl",
        ])
    }

    /// File tools + web/fetch + `python_repl` — the "no shell tools" selection.
    fn sel_no_shell_tools() -> Vec<String> {
        sel(&[
            "read_file",
            "write_file",
            "edit_file",
            "list_files",
            "web_search",
            "fetch_url",
            "grep_files",
            "find_files",
            "python_repl",
        ])
    }

    /// Minimal selection: `web_search` + `fetch_url` + `python_repl` only.
    fn sel_minimal() -> Vec<String> {
        sel(&["web_search", "fetch_url", "python_repl"])
    }

    #[test]
    fn default_tool_names_are_twelve_in_canonical_order() {
        assert_eq!(DEFAULT_TOOL_NAMES.len(), 12);
        assert_eq!(
            DEFAULT_TOOL_NAMES,
            &[
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
    fn all_tool_names_is_default_plus_python_repl() {
        assert_eq!(ALL_TOOL_NAMES.len(), DEFAULT_TOOL_NAMES.len() + 1);
        assert_eq!(*ALL_TOOL_NAMES.last().unwrap(), "python_repl");
        // Defaults appear in `ALL` in the same order.
        for (i, name) in DEFAULT_TOOL_NAMES.iter().enumerate() {
            assert_eq!(ALL_TOOL_NAMES[i], *name);
        }
    }

    #[test]
    fn empty_selection_yields_no_definitions() {
        assert!(tool_definitions(&[]).is_empty());
    }

    #[test]
    fn twelve_tools_for_default_selection() {
        let names: Vec<String> = tool_definitions(&sel_default())
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert_eq!(names, sel_default());
    }

    #[test]
    fn thirteen_tools_for_default_plus_python_repl() {
        let names: Vec<String> = tool_definitions(&sel_default_plus_repl())
            .into_iter()
            .map(|d| d.name)
            .collect();
        let mut expected = sel_default();
        expected.push("python_repl".into());
        assert_eq!(names, expected);
    }

    #[test]
    fn output_order_follows_all_tool_names_not_input_order() {
        // Reverse-order input — output must still be in ALL_TOOL_NAMES order.
        let mut reversed = sel_default();
        reversed.reverse();
        let names: Vec<String> = tool_definitions(&reversed)
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert_eq!(names, sel_default());
    }

    #[test]
    fn unknown_tool_names_in_selection_are_ignored_by_tool_definitions() {
        // `tool_definitions` itself is permissive — name validation is the
        // agent's responsibility.  Unknown names simply do not match any
        // entry in `ALL_TOOL_NAMES`.
        let sel = sel(&["read_file", "does_not_exist", "write_file"]);
        let names: Vec<String> = tool_definitions(&sel).into_iter().map(|d| d.name).collect();
        assert_eq!(names, vec!["read_file", "write_file"]);
    }

    // -----------------------------------------------------------------------
    // No-file-tools selection
    // -----------------------------------------------------------------------

    #[test]
    fn no_file_tools_selection_emits_seven_tools() {
        // Output order follows ALL_TOOL_NAMES (canonical), not selection
        // order: run_command → web_search/fetch_url → run_background → … →
        // python_repl.
        let names: Vec<String> = tool_definitions(&sel_no_file_tools())
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert_eq!(
            names,
            vec![
                "run_command",
                "web_search",
                "fetch_url",
                "run_background",
                "wait_for_output",
                "write_stdin",
                "python_repl",
            ],
        );
    }

    #[test]
    fn no_file_tools_selection_excludes_all_six_file_op_tools() {
        let names: Vec<String> = tool_definitions(&sel_no_file_tools())
            .into_iter()
            .map(|d| d.name)
            .collect();
        for removed in &[
            "read_file",
            "write_file",
            "edit_file",
            "find_files",
            "grep_files",
            "list_files",
        ] {
            assert!(!names.contains(&(*removed).to_string()));
        }
    }

    // -----------------------------------------------------------------------
    // No-shell-tools selection
    // -----------------------------------------------------------------------

    #[test]
    fn no_shell_tools_selection_emits_nine_tools() {
        let names: Vec<String> = tool_definitions(&sel_no_shell_tools())
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert_eq!(
            names,
            vec![
                "read_file",
                "write_file",
                "edit_file",
                "list_files",
                "web_search",
                "fetch_url",
                "grep_files",
                "find_files",
                "python_repl",
            ],
        );
    }

    #[test]
    fn no_shell_tools_selection_excludes_all_four_shell_tools() {
        let names: Vec<String> = tool_definitions(&sel_no_shell_tools())
            .into_iter()
            .map(|d| d.name)
            .collect();
        for removed in &[
            "run_command",
            "run_background",
            "wait_for_output",
            "write_stdin",
        ] {
            assert!(!names.contains(&(*removed).to_string()));
        }
    }

    // -----------------------------------------------------------------------
    // Minimal selection (web + fetch + python_repl)
    // -----------------------------------------------------------------------

    #[test]
    fn minimal_selection_emits_three_tools() {
        let names: Vec<String> = tool_definitions(&sel_minimal())
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert_eq!(names, vec!["web_search", "fetch_url", "python_repl"]);
    }

    #[test]
    fn minimal_selection_excludes_all_ten_other_tools() {
        let names: Vec<String> = tool_definitions(&sel_minimal())
            .into_iter()
            .map(|d| d.name)
            .collect();
        for removed in &[
            "read_file",
            "write_file",
            "edit_file",
            "find_files",
            "grep_files",
            "list_files",
            "run_command",
            "run_background",
            "wait_for_output",
            "write_stdin",
        ] {
            assert!(!names.contains(&(*removed).to_string()));
        }
    }

    // -----------------------------------------------------------------------
    // Schema shape sanity (every selection produces well-formed schemas)
    // -----------------------------------------------------------------------

    fn assert_schemas_well_formed(defs: &[ToolDefinition]) {
        for def in defs {
            let schema = &def.input_schema;
            assert_eq!(schema["type"], "object", "{} not an object", def.name);
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
            assert!(!def.description.is_empty(), "{} empty desc", def.name);
        }
    }

    #[test]
    fn default_plus_repl_schemas_are_well_formed() {
        assert_schemas_well_formed(&tool_definitions(&sel_default_plus_repl()));
    }

    #[test]
    fn no_file_tools_schemas_are_well_formed() {
        assert_schemas_well_formed(&tool_definitions(&sel_no_file_tools()));
    }

    #[test]
    fn no_shell_tools_schemas_are_well_formed() {
        assert_schemas_well_formed(&tool_definitions(&sel_no_shell_tools()));
    }

    #[test]
    fn minimal_schemas_are_well_formed() {
        assert_schemas_well_formed(&tool_definitions(&sel_minimal()));
    }

    // -----------------------------------------------------------------------
    // fetch_url schema gating (depends on whether any shell-execution tool
    // is present in the selection)
    // -----------------------------------------------------------------------

    fn fetch_url_def(sel: &[String]) -> ToolDefinition {
        tool_definitions(sel)
            .into_iter()
            .find(|d| d.name == "fetch_url")
            .expect("fetch_url must be present")
    }

    #[test]
    fn fetch_url_with_shell_tools_requires_url_and_postprocess() {
        let def = fetch_url_def(&sel_default());
        let required: Vec<&str> = def.input_schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(required.contains(&"url"));
        assert!(required.contains(&"postprocess"));
        assert!(def.input_schema["properties"]["postprocess"].is_object());
    }

    #[test]
    fn fetch_url_without_shell_tools_requires_only_url() {
        let def = fetch_url_def(&sel_no_shell_tools());
        let required: Vec<&str> = def.input_schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(required, vec!["url"]);
    }

    #[test]
    fn fetch_url_without_shell_tools_has_no_postprocess_anywhere() {
        let def = fetch_url_def(&sel_no_shell_tools());
        assert!(def.input_schema["properties"]["postprocess"].is_null());
        let raw = serde_json::to_string(&def.input_schema).unwrap();
        assert!(!raw.contains("postprocess"));
        assert!(!def.description.contains("postprocess"));
    }

    #[test]
    fn fetch_url_minimal_selection_requires_only_url() {
        let def = fetch_url_def(&sel_minimal());
        let required: Vec<&str> = def.input_schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(required, vec!["url"]);
    }

    #[test]
    fn fetch_url_without_shell_tools_description_mentions_cap() {
        let def = fetch_url_def(&sel_no_shell_tools());
        assert!(def.description.contains("2000") || def.description.contains("50"));
    }
}
