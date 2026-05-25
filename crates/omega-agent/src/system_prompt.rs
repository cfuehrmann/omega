//! System-prompt assembly.
//!
//! Two responsibilities, kept in this single module:
//!
//! 1. **Discovery.** Locate `AGENTS.md` files from the standard tiers
//!    (global config + repo root).
//! 2. **Assembly.** Build the ordered list of cacheable
//!    [`SystemBlock`]s the agent sends on every API call:
//!
//!    | # | Block             | Source                                  |
//!    |---|-------------------|-----------------------------------------|
//!    | 1 | Core prompt       | static template (this file)             |
//!    | 2 | Runtime context   | `cwd`, `max_output_tokens`              |
//!    | 3 | Global AGENTS.md  | `$XDG_CONFIG_HOME/omega/AGENTS.md`      |
//!    | 4 | Repo AGENTS.md    | `<repo-root>/AGENTS.md`                 |
//!
//! Blocks 3 and 4 are only present when the corresponding file exists.
//! The Anthropic provider stamps `cache_control: ephemeral` on the
//! **last** present block, so blocks 1..=N are all part of the cached
//! prefix.

use omega_types::FeatureFlags;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// One cacheable section of the assembled system prompt.
///
/// The order of [`SystemBlock`]s in the `Vec` returned by
/// [`build_system_blocks`] is the order the provider sees on the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemBlock {
    /// Stable label identifying the block's role.  One of:
    /// `"core"`, `"runtime"`, `"global-agents-md"`, `"repo-agents-md"`.
    pub label: &'static str,
    /// Fully rendered text of the block.  For instruction-file blocks
    /// the `"Instructions from: <path>\n\n"` prefix is already
    /// included.
    pub content: String,
    /// Path the content was loaded from, when applicable.  `None` for
    /// the core and runtime blocks (which have no on-disk source).
    pub source_path: Option<PathBuf>,
}

/// An `AGENTS.md` file that was located by [`discover_instruction_files`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstructionFile {
    /// The tier this file came from — `"global-agents-md"` or
    /// `"repo-agents-md"`.  Surfaces directly as the label of the
    /// resulting [`SystemBlock`].
    pub label: &'static str,
    /// Absolute path on disk.
    pub path: PathBuf,
    /// File contents, read verbatim.
    pub content: String,
}

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

/// File name used at every tier.  Single canonical spelling — no
/// `CLAUDE.md` / `AGENT.md` aliases.
pub const AGENTS_FILE: &str = "AGENTS.md";

/// Locate `AGENTS.md` files from the supported tiers, in the order the
/// agent should append them to the system prompt:
///
/// 1. **Global**: `$XDG_CONFIG_HOME/omega/AGENTS.md`
///    (default `~/.config/omega/AGENTS.md`).
/// 2. **Repo**: walk up from `cwd` to the git repository root (the
///    nearest ancestor containing a `.git` entry); use its
///    `AGENTS.md` if present.
///
/// Files that don't exist are silently skipped.  Read errors (e.g.
/// permission denied) are also skipped — the agent should never fail
/// to start because of an unreadable instruction file.
///
/// Tier C (subdirectory `AGENTS.md`, on-demand attachment) is **not**
/// implemented here.
#[must_use]
pub fn discover_instruction_files(cwd: &Path) -> Vec<InstructionFile> {
    discover_instruction_files_with_env(
        cwd,
        std::env::var_os("XDG_CONFIG_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )
}

/// Env-injected variant of [`discover_instruction_files`], used by
/// the unit tests so they don't have to mutate the process
/// environment (which is `unsafe` under edition 2024 and forbidden by
/// this crate's lints).
#[must_use]
pub fn discover_instruction_files_with_env(
    cwd: &Path,
    xdg_config_home: Option<&std::ffi::OsStr>,
    home: Option<&std::ffi::OsStr>,
) -> Vec<InstructionFile> {
    let mut out = Vec::new();

    if let Some(path) = global_agents_md_path_from_env(xdg_config_home, home)
        && let Some(content) = read_existing(&path)
    {
        out.push(InstructionFile {
            label: "global-agents-md",
            path,
            content,
        });
    }

    if let Some(path) = repo_agents_md_path(cwd)
        && let Some(content) = read_existing(&path)
    {
        out.push(InstructionFile {
            label: "repo-agents-md",
            path,
            content,
        });
    }

    out
}

/// Resolve the global `AGENTS.md` path.
///
/// Honours `$XDG_CONFIG_HOME`; falls back to `$HOME/.config/omega/AGENTS.md`
/// when unset.  Returns `None` only when neither variable is available
/// (very unusual — e.g. an unsandboxed CI worker with no `HOME`).
#[must_use]
pub fn global_agents_md_path() -> Option<PathBuf> {
    global_agents_md_path_from_env(
        std::env::var_os("XDG_CONFIG_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )
}

/// Env-injected variant of [`global_agents_md_path`].
#[must_use]
pub fn global_agents_md_path_from_env(
    xdg_config_home: Option<&std::ffi::OsStr>,
    home: Option<&std::ffi::OsStr>,
) -> Option<PathBuf> {
    let base = xdg_config_home
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| home.map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("omega").join(AGENTS_FILE))
}

/// Resolve `<repo-root>/AGENTS.md` by walking up from `cwd` to the
/// nearest ancestor that contains a `.git` entry.  Returns `None` when
/// `cwd` is not inside a git checkout.
#[must_use]
pub fn repo_agents_md_path(cwd: &Path) -> Option<PathBuf> {
    find_git_root(cwd).map(|root| root.join(AGENTS_FILE))
}

/// Walk up from `start`, returning the first ancestor that contains a
/// `.git` entry (file or directory — git worktrees use a `.git` file).
fn find_git_root(start: &Path) -> Option<PathBuf> {
    let mut current: Option<&Path> = Some(start);
    while let Some(dir) = current {
        if dir.join(".git").exists() {
            return Some(dir.to_path_buf());
        }
        current = dir.parent();
    }
    None
}

/// Read `path` to a string, returning `None` for any error (most
/// commonly `NotFound`).  We deliberately swallow non-`NotFound`
/// errors: a permission-denied AGENTS.md must not block session start.
fn read_existing(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

// ---------------------------------------------------------------------------
// Assembly
// ---------------------------------------------------------------------------

/// Build the ordered list of system-prompt blocks for one session.
///
/// `files` is typically the output of [`discover_instruction_files`].
/// Empty `content` files are skipped so we never push a stray header
/// onto the wire.
///
/// When `flags.repl` is `true` (session started with
/// `OMEGA_FEATURE_REPL=1`), a `"repl"` block is appended after all
/// instruction-file blocks.  It describes the `python_repl` tool and its
/// usage pattern to the model.
///
/// When `flags.repl_replaces_fileops` and/or `flags.repl_replaces_shell`
/// is `true`, an additional `"reduced-toolset"` block is appended after
/// `"repl"`, explaining which tools have been removed and how to
/// accomplish the same work with `python_repl` (and/or `subprocess` inside
/// `python_repl`).  The `"core"` and `"runtime"` blocks are generated
/// without references to the removed tools, so the model receives
/// consistent instructions that match the actual toolset.
#[must_use]
pub fn build_system_blocks(
    cwd: &str,
    max_output_tokens: u32,
    headless: bool,
    files: &[InstructionFile],
    flags: FeatureFlags,
) -> Vec<SystemBlock> {
    let mut out = Vec::new();

    // When the six file-op tools are absent, omit all guidance that
    // references them from the core and runtime blocks.
    let file_tools = !flags.repl_replaces_fileops;
    // When the four shell-execution tools are absent, omit all guidance that
    // references them from the core block.
    let shell_tools = !flags.repl_replaces_shell;

    out.push(SystemBlock {
        label: "core",
        content: core_prompt(headless, file_tools, shell_tools),
        source_path: None,
    });

    out.push(SystemBlock {
        label: "runtime",
        content: runtime_context(cwd, max_output_tokens, file_tools),
        source_path: None,
    });

    for file in files {
        if file.content.trim().is_empty() {
            continue;
        }
        out.push(SystemBlock {
            label: file.label,
            content: format!(
                "Instructions from: {}\n\n{}",
                file.path.display(),
                file.content
            ),
            source_path: Some(file.path.clone()),
        });
    }

    if flags.repl {
        out.push(SystemBlock {
            label: "repl",
            content: repl_addendum(),
            source_path: None,
        });
    }

    if flags.repl_replaces_fileops || flags.repl_replaces_shell {
        out.push(SystemBlock {
            label: "reduced-toolset",
            content: reduced_toolset_addendum(flags),
            source_path: None,
        });
    }

    out
}

/// System-prompt addendum injected when `OMEGA_FEATURE_REPL=1`.
///
/// When limit mode is not active, this is the last block, so the Anthropic
/// provider stamps `cache_control: ephemeral` on it.
#[must_use]
pub fn repl_addendum() -> String {
    "## Python REPL

\
     You have access to a `python_repl` tool that executes Python code in a \
     stateful interpreter.\n
\
     - Variables, imports, and definitions from one call persist into all \
       subsequent calls within this session.\n\
     - The tool returns combined stdout + stderr output, truncated at \
       200 lines or 2000 characters (whichever comes first). When truncated, \
       a `... [output truncated: N lines / M chars suppressed. Capture large \
       values in variables and inspect/slice them in subsequent calls rather \
       than printing them whole.]` marker appears at the end.\n\
     - Use it for arithmetic, data parsing, string manipulation, exploration, \
       and building up intermediate results step-by-step.\n\
     - Prefer a single call with all related statements over many small calls \
       — state persists, so you can build on previous results.\n\
     - Optional `timeout` parameter (default 60 s, max 600 s).  This is the \
       OUTER bound on a single call.  Any inner timeouts you set in the \
       code itself (`subprocess.run(..., timeout=N)`, `threading` joins, \
       etc.) must be **strictly less than** this outer timeout \u{2014} inner \
       timeouts equal to or greater than the outer never fire, because the \
       outer escalates first.  For known-slow operations (downloads, \
       password cracking, heavy computation), raise the OUTER timeout \
       (e.g. `python_repl(code=..., timeout=300)`) rather than rely on a \
       long inner subprocess timeout.  Sequential operations within a \
       single call (multiple `subprocess.run` invocations, `time.sleep`, \
       etc.) must total less than the outer timeout; when in doubt, split \
       into multiple calls.  On outer timeout, SIGINT is sent first; if \
       the kernel recovers, REPL state is preserved.  If it does not, the \
       kernel is killed and all prior state is lost.\n\
     - Variable pattern: store large intermediate results in variables \
       (`result = expensive_compute()`) and print only the summary needed \
       for the next decision. Variables persist across calls; printed bytes \
       do not."
        .to_owned()
}

/// System-prompt addendum injected when `OMEGA_FEATURE_REPL_REPLACES_FILEOPS=1`
/// and/or `OMEGA_FEATURE_REPL_REPLACES_SHELL=1`.
///
/// Placed after the `"repl"` block.  The Anthropic provider stamps
/// `cache_control: ephemeral` on the last block, so this is the block
/// that gets cached when either or both reduced-toolset flags are active.
///
/// The heading is always `## Reduced toolset`.  Then:
/// - If `flags.repl_replaces_fileops`: a paragraph explains the six
///   removed file-op tools and how to replace them.
/// - If `flags.repl_replaces_shell`: a paragraph explains the four
///   removed shell-execution tools and shows the `subprocess` pattern.
///
/// When both flags are set (Tier 2), both paragraphs appear under the
/// single heading.  In that case, the fileops paragraph does **not**
/// suggest `run_command` as an alternative (since it is also removed).
#[must_use]
pub fn reduced_toolset_addendum(flags: FeatureFlags) -> String {
    let mut sections: Vec<String> = Vec::new();

    if flags.repl_replaces_fileops {
        if flags.repl_replaces_shell {
            // Both flags set: run_command is also removed, so do not suggest it.
            sections.push(
                "This session does not expose `read_file`, `write_file`, `edit_file`, \
`find_files`, `grep_files`, or `list_files`.
\
For file operations, use `python_repl` — idiomatic Python \
(`pathlib`, `open`, `re`, `os`)."
                    .to_owned(),
            );
        } else {
            // Only fileops removed; shell tools are still available.
            // Text is intentionally neutral: experiment measures which the LLM picks.
            sections.push(
                "This session does not expose `read_file`, `write_file`, `edit_file`, \
`find_files`, `grep_files`, or `list_files`.
For any of these operations, use either:

- `python_repl` — idiomatic Python (`pathlib`, `open`, `re`, `os`).
- `run_command` — shell commands (`cat`, `sed`, `grep`, `find`).

Choose whichever is cleaner for the situation."
                    .to_owned(),
            );
        }
    }

    if flags.repl_replaces_shell {
        sections.push(
            "This session does not expose `run_command`, `run_background`, \
`wait_for_output`, or `write_stdin`.  To run shell commands,
\
use `subprocess` inside `python_repl`, for example:

    import subprocess
    r = subprocess.run([\"7z\", \"e\", \"secrets.7z\"],
                       capture_output=True, text=True)
    print(r.stdout, r.stderr, \"exit:\", r.returncode)

For long-running processes, use `subprocess.Popen`, keep
\
the handle in a REPL variable, and read from it
\
incrementally."
                .to_owned(),
        );
    }

    format!("## Reduced toolset\n\n{}", sections.join("\n\n"))
}

/// Concatenate every block's `content` with `\n\n` between them.
///
/// Used as the `system_prompt` field on `SessionStartedEvent`, so the
/// archived session faithfully shows everything the model saw.
#[must_use]
pub fn join_blocks(blocks: &[SystemBlock]) -> String {
    blocks
        .iter()
        .map(|b| b.content.as_str())
        .collect::<Vec<_>>()
        .join("\n\n")
}

// ---------------------------------------------------------------------------
// Static text
// ---------------------------------------------------------------------------

/// Runtime-context block (block #2).
///
/// Contains the two pieces of state that change between sessions:
/// `cwd` and `max_output_tokens`.  Kept separate from the core prompt
/// so the core text is byte-for-byte identical across sessions and
/// benefits from Anthropic's prefix cache the first time it appears.
///
/// When `file_tools` is `false` (limit mode), the advice about
/// `write_file` and `edit_file` is omitted so the model receives
/// consistent instructions that match the actual toolset.
fn runtime_context(cwd: &str, max_output_tokens: u32, file_tools: bool) -> String {
    let mut s = format!(
        "## Runtime context\n\
\n\
Your working directory is {cwd}. Treat it as the root of your work — use\n\
relative paths from there unless the user directs otherwise.\n\
\n\
The output token budget is {max_output_tokens} tokens per response. Tool call\n\
arguments count against this budget."
    );
    if file_tools {
        s.push_str(
            " Very large `write_file` calls risk\n\
hitting the limit mid-generation, leaving a broken turn. For large new\n\
files: write a skeleton first, then extend with `edit_file`. For large\n\
existing files: always prefer `edit_file` over a full rewrite.",
        );
    }
    s
}

/// Core prompt (block #1).
///
/// `headless` drops the two sections that require an interactive human UI:
/// output-format rendering guidance and the discussion-before-acting policy.
///
/// `file_tools` controls whether guidance referencing the six file-op tools
/// (`read_file`, `write_file`, `edit_file`, `find_files`, `grep_files`,
/// `list_files`) is included.  Pass `false` when those tools are absent from
/// the toolset (i.e. `flags.repl_replaces_fileops` is `true`) so that the
/// model receives consistent instructions that match the actual toolset.
///
/// `shell_tools` controls whether guidance referencing the four
/// shell-execution tools (`run_command`, `run_background`, `wait_for_output`,
/// `write_stdin`) is included.  Pass `false` when those tools are absent
/// (i.e. `flags.repl_replaces_shell` is `true`).
#[allow(clippy::too_many_lines)]
fn core_prompt(headless: bool, file_tools: bool, shell_tools: bool) -> String {
    let mut s = String::new();

    // --- Introduction + Project orientation ---
    s.push_str(
        "You are an expert assistant operating inside Omega, a software engineering agent harness. Use tools when needed.\n\
\n\
## Project orientation\n\
\n\
When you have no prior context about the project structure, check manifest\n\
files (e.g. `Cargo.toml`, `package.json`, `*.csproj`, `pyproject.toml`) to\n\
learn the stack. Project-specific conventions are in the attached `AGENTS.md`\n\
blocks (if any) — do not search the filesystem for them. ",
    );
    if file_tools {
        s.push_str(
            "Any file listed as\n\
`Instructions from: <path>` is already present here; a `read_file` call for\n\
it is unnecessary and will be blocked.",
        );
    } else {
        s.push_str(
            "Any file listed as\n\
`Instructions from: <path>` is already present here; reading it again is\n\
unnecessary and will be blocked.",
        );
    }

    // --- Tools section ---
    s.push_str(
        "\n\
\n\
## Tools\n\
\n\
The operator has pre-approved all tool calls. No confirmation is needed.\n",
    );

    // File-tool search/navigation guidance — omitted when those tools are absent.
    if file_tools {
        s.push_str(
            "\n\
Prefer `grep_files` over speculative `read_file` calls when searching for\n\
a symbol, string, or pattern across the codebase. It's faster and returns\n\
only what's relevant.\n\
Use `find_files` when you know a file's name or extension but not its exact\n\
path — don't brute-force with repeated `list_files` calls.\n",
        );
    }

    // run_command / run_background guidance — only when shell tools are present.
    if shell_tools {
        s.push_str(
            "Use `run_command` for builds, test suites, commits, and any finite command.\n\
The default timeout is 120 s; pass a higher `timeout` (e.g. 300) for commands\n\
you expect to take longer. Reserve `run_background` for processes that must\n\
stay alive indefinitely (dev servers, file watchers).\n\
All `run_command` and `wait_for_output` results are tee'd to a session-cache\n\
log and the path is surfaced in a footer:\n\
- `[full output: <path>]` when the output fit within the cap.\n\
- `[truncated; showed last 100 KB of 487 KB. Full output: <path>]` when capped.\n",
        );
    }

    // Truncation recovery hint — only mention file tools when they are present.
    // fetch_url also produces cached output, so this hint remains useful even
    // when shell tools are absent.
    if file_tools {
        s.push_str(
            "When a result is **truncated**, use `read_file` or `grep_files` on the cache\n\
path to recover the bytes that didn't fit inline. ",
        );
    }
    s.push_str(
        "The cache is also useful\n\
when an earlier (full) output has aged out of immediate context and you need\n\
to revisit it without re-running the command — re-running is slow and may\n\
produce different output. When the bytes you need are already inline and\n\
recent, read them directly rather than calling another tool over the same\n\
bytes.\n",
    );

    // truncation_bias, wait_for_output, write_stdin — only when shell tools are present.
    if shell_tools {
        s.push_str(
            "Pass `truncation_bias: \"tail\"` (default on failure), `\"head\"` (default on\n\
success), or `\"middle\"` to control which portion is returned when the\n\
output is truncated.\n\
To wait for a background process to become ready (e.g. a dev server), use\n\
`wait_for_output(logFile, pid, timeoutMs, pattern?)` instead of `sleep` + `tail`.\n\
Always pass the `pid` from `run_background` — if the process exits before the pattern matches,\n\
`wait_for_output` returns immediately with `processExited: true` and the exit code instead of\n\
waiting for the full timeout.\n\
The `pattern` is a **JavaScript regex** — use `|` for alternation (e.g. `\"ready|Error|done\"`).\n\
If a background process prompts for interactive input, use\n\
`write_stdin(pid, text)` to respond (include \\n to submit a line). Pass\n\
`end_stdin=true` to signal EOF after writing.\n",
        );
    }

    s.push_str(
        "Chain independent tool calls in parallel when results don't depend on each\n\
other.\n\
Check for a task runner and use it to discover available commands\n\
(`just --list`, `make help`, `npm run`, etc.).\n",
    );

    // edit_file workflow guidance — omitted when that tool is absent.
    if file_tools {
        s.push_str(
            "For `edit_file`: read or grep the file first to identify **all** needed\n\
changes, then apply them in a single call with `replacements`. Never call\n\
`edit_file` on the same file twice in a row — that is always a mistake.\n",
        );
    }

    // web_search / fetch_url guidance.
    s.push_str(
        "\n\
Use `web_search` freely for documentation, current information, API details,\n\
error messages, or anything not in local files. Prefer it over guessing or\n\
relying on potentially stale training data.\n",
    );

    // fetch_url: guidance depends on whether the shell pipeline is available.
    if shell_tools {
        // Shell tools present: postprocess pipeline is available.
        if file_tools {
            s.push_str(
                "`fetch_url` downloads a URL **once** and runs a single `postprocess` query\n\
on it. The result includes a cache path — for any further queries on the same\n\
content, use `grep_files`/`read_file` on that path.\n",
            );
        } else {
            // File tools absent but shell tools present: run_command is an option.
            s.push_str(
                "`fetch_url` downloads a URL **once** and runs a single `postprocess` query\n\
on it. The result includes a cache path — for any further queries on the same\n\
content, reuse it with `run_command` (e.g. grep) or `python_repl`.\n",
            );
        }
        s.push_str(
            "`postprocess` is required. Prefer `grep` or `awk` when you know what to\n\
look for, and `head -N` as the catch-all. Never use `cat` — `head -N`\n\
gives the same result on short pages and stays bounded on long ones.\n",
        );
    } else {
        // Shell-gated mode (repl_replaces_shell): postprocess is disabled to
        // close the shell-loophole.  Content is capped at 2000 lines / 50 KB;
        // use python_repl for further filtering.
        s.push_str(
            "`fetch_url` downloads a URL **once** and returns the content as text,\n\
capped at 2000 lines / 50\u{00a0}KB. For further filtering or analysis, pass\n\
the cache path to `python_repl`.\n",
        );
    }

    // Verbose-output inspection: only relevant when shell tools are present
    // (run_background log files / run_command redirected output).
    if shell_tools {
        s.push_str(
            "\nWhen a command produces verbose output — whether from `run_background`'s\n\
`logFile` or from a `run_command` redirected to a file — inspect it with\n",
        );
        if file_tools {
            s.push_str(
                "`read_file` (use `offset`/`limit` to paginate through large files) and\n\
`grep_files` to search for specific patterns. ",
            );
        } else {
            s.push_str("`python_repl` or `run_command` to inspect the output. ");
        }
    }

    s.push_str(
        "Never re-run a command just to\n\
see more output. Never re-run any command without making a code change in\n\
between.\n\
\n\
If a tool fails in a noteworthy way, mention it in your response.",
    );

    // Both sections below require an interactive human UI; omit in headless mode.
    if !headless {
        s.push_str(
            "

## Output format

Use markdown formatting where helpful — tables, code blocks, bold, and lists
are rendered in the UI. Plain prose is fine too; don't force structure where
it adds no value.

The UI renders Mermaid diagrams: use a ```mermaid code block when a diagram
would communicate structure more clearly than prose — architecture overviews,
component relationships, and sequence diagrams are particularly good candidates.
Don't force a diagram where plain text suffices.

For C4 diagrams specifically:
- Keep element descriptions to ≤ 6 words; move detail to prose. For anything
  longer, use `<br/>` to force a line break within the description string —
  the renderer splits on it even though automatic word-wrap is broken in
  Mermaid's C4 implementation:
    Component(foo, \"Name\", \"Tech\", \"First line.<br/>Second line.\")
- Always add `UpdateLayoutConfig($c4ShapeInRow=\"3\", $c4BoundaryInRow=\"1\")` on
  diagrams that contain boundaries. This prevents dagre from spreading shapes
  so wide that arrows route across boxes.
- Do not add `UpdateRelStyle` calls — CSS handles relationship colours globally.

## Design discipline

Before implementing a non-trivial change, state your chosen approach and the
alternatives you considered, then proceed. If the user raises a design
question — before, during, or after — stop and discuss before continuing.",
        );
    }
    s.push_str(
        "

## Bug fixes

When fixing a bug, first write a failing test that reproduces it (red), then
fix the code so the test passes (green). Skip this only when the reproduction
requires complex test infrastructure that doesn't already exist — in that case,
raise the trade-off with the user rather than silently skipping. If a test's
reliability is in doubt, run it several times before trusting a green result.

## Flaky tests

Flaky tests must be fixed immediately — never dismissed as pre-existing or
attributed to environment, timing, or infrastructure without strong evidence.
Assume the flakiness was introduced by a recent change until proven otherwise.

## Task completion

Before declaring a task done, verify the stated success criterion. If the
instruction names a concrete target — tests passing, a numeric threshold,
absence of specific warnings — run the check and confirm the measured
value meets it. If the criterion is implicit, state what you assumed \"done\"
means in your final response.

If the instruction names a time budget, commit a working solution before
refining; don't spend more than half the budget without producing
verifiable output.

If the task names a specific output path or submission directory, verify the
final state matches the spec before declaring done. Be careful with
relative-path assumptions — a path that resolves correctly from your current
working directory may not be the location the task requires. If the task
specifies which files should be present, list the directory and compare
against the spec.",
    );

    s
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    //! Inline carve-out tests for `system_prompt.rs`.
    //!
    //! Justification for carve-out:
    //!
    //! * `build_system_blocks`, `discover_instruction_files_with_env`,
    //!   `repo_agents_md_path`, `global_agents_md_path_from_env` are all pure
    //!   (or env-injected-pure) functions that would require boilerplate agent
    //!   setup + captured `LlmRequest` inspection to test through
    //!   `Agent::send_message` / `MockProvider`.  The inline tests are simpler
    //!   and more targeted.
    //!
    //! * Discovery tests require `tempdir` + `git init` which is already done
    //!   here.  Routing them through the `MockProvider` surface would require the
    //!   same disk setup plus agent wiring, adding setup with no benefit.
    //!
    //! * `global_agents_md_path` is tested directly (not via `_from_env`)
    //!   because it calls `std::env::var("HOME")` internally; a direct call in
    //!   the real CI environment is the simplest way to pin the two mutations
    //!   (return `None`, return `Some(Default::default())`) identified in
    //!   docs/mutation-testing/omega-agent/survivors.md.

    use super::*;
    use std::process::Command;

    fn git_init(dir: &Path) {
        let s = Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(dir)
            .status()
            .expect("git init");
        assert!(s.success());
    }

    // ---- Assembly ----------------------------------------------------

    fn flags_default() -> FeatureFlags {
        FeatureFlags::default()
    }

    fn flags_repl_only() -> FeatureFlags {
        FeatureFlags {
            repl: true,
            subagents: false,
            repl_replaces_fileops: false,
            repl_replaces_shell: false,
        }
    }

    fn flags_limit_mode() -> FeatureFlags {
        FeatureFlags {
            repl: true,
            subagents: false,
            repl_replaces_fileops: true,
            repl_replaces_shell: false,
        }
    }

    fn flags_repl_replaces_shell() -> FeatureFlags {
        FeatureFlags {
            repl: true,
            subagents: false,
            repl_replaces_fileops: false,
            repl_replaces_shell: true,
        }
    }

    fn flags_both_replaces() -> FeatureFlags {
        FeatureFlags {
            repl: true,
            subagents: false,
            repl_replaces_fileops: true,
            repl_replaces_shell: true,
        }
    }

    #[test]
    fn core_block_is_first_and_unheadered() {
        let blocks = build_system_blocks("/tmp/proj", 64_000, false, &[], flags_default());
        assert_eq!(blocks[0].label, "core");
        assert!(blocks[0].source_path.is_none());
        assert!(blocks[0].content.starts_with("You are an expert assistant"));
        // The core block must NOT contain the runtime values.
        assert!(!blocks[0].content.contains("64000"));
        assert!(!blocks[0].content.contains("/tmp/proj"));
    }

    #[test]
    fn runtime_block_contains_cwd_and_token_budget() {
        let blocks = build_system_blocks("/tmp/proj", 12_345, false, &[], flags_default());
        let rt = blocks.iter().find(|b| b.label == "runtime").expect("rt");
        assert!(rt.content.starts_with("## Runtime context"));
        assert!(rt.content.contains("/tmp/proj"));
        assert!(rt.content.contains("12345 tokens"));
    }

    #[test]
    fn no_instruction_files_yields_exactly_two_blocks() {
        let blocks = build_system_blocks("/x", 1000, false, &[], flags_default());
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].label, "core");
        assert_eq!(blocks[1].label, "runtime");
    }

    #[test]
    fn instruction_files_become_trailing_blocks_with_prefix() {
        let files = vec![
            InstructionFile {
                label: "global-agents-md",
                path: PathBuf::from("/etc/omega/AGENTS.md"),
                content: "GLOBAL".to_owned(),
            },
            InstructionFile {
                label: "repo-agents-md",
                path: PathBuf::from("/repo/AGENTS.md"),
                content: "REPO".to_owned(),
            },
        ];
        let blocks = build_system_blocks("/repo", 1000, false, &files, flags_default());
        assert_eq!(blocks.len(), 4);
        assert_eq!(blocks[2].label, "global-agents-md");
        assert!(
            blocks[2]
                .content
                .starts_with("Instructions from: /etc/omega/AGENTS.md\n\nGLOBAL")
        );
        assert_eq!(blocks[3].label, "repo-agents-md");
        assert!(
            blocks[3]
                .content
                .starts_with("Instructions from: /repo/AGENTS.md\n\nREPO")
        );
    }

    #[test]
    fn empty_instruction_file_is_skipped() {
        let files = vec![InstructionFile {
            label: "repo-agents-md",
            path: PathBuf::from("/repo/AGENTS.md"),
            content: "   \n  ".to_owned(),
        }];
        let blocks = build_system_blocks("/repo", 1000, false, &files, flags_default());
        assert_eq!(blocks.len(), 2, "whitespace-only file should be ignored");
    }

    #[test]
    fn join_blocks_separates_with_blank_line() {
        let blocks = build_system_blocks("/x", 1000, false, &[], flags_default());
        let joined = join_blocks(&blocks);
        assert!(joined.contains("\n\n## Runtime context"));
    }

    // ---- REPL addendum ---------------------------------------------------

    #[test]
    fn repl_flag_false_adds_no_extra_block() {
        let blocks = build_system_blocks("/x", 1000, false, &[], flags_default());
        assert!(
            blocks.iter().all(|b| b.label != "repl"),
            "repl block must not be present when repl=false"
        );
    }

    #[test]
    fn repl_flag_true_appends_repl_block() {
        let blocks = build_system_blocks("/x", 1000, false, &[], flags_repl_only());
        let repl_block = blocks.iter().find(|b| b.label == "repl");
        assert!(
            repl_block.is_some(),
            "repl block must be present when repl=true"
        );
        // Repl block must be last when limit mode is off.
        assert_eq!(blocks.last().unwrap().label, "repl");
    }

    #[test]
    fn repl_block_content_describes_tool() {
        let blocks = build_system_blocks("/x", 1000, false, &[], flags_repl_only());
        let repl = blocks.iter().find(|b| b.label == "repl").unwrap();
        assert!(
            repl.content.contains("python_repl"),
            "repl block must mention the tool name"
        );
        assert!(
            repl.content.contains("persist"),
            "repl block must describe state persistence"
        );
        assert!(
            repl.content.contains("truncated"),
            "repl block must describe truncation behaviour"
        );
    }

    #[test]
    fn repl_block_appended_after_instruction_files() {
        let files = vec![InstructionFile {
            label: "repo-agents-md",
            path: PathBuf::from("/repo/AGENTS.md"),
            content: "REPO".to_owned(),
        }];
        let blocks = build_system_blocks("/x", 1000, false, &files, flags_repl_only());
        // core + runtime + repo-agents-md + repl = 4
        assert_eq!(blocks.len(), 4);
        assert_eq!(blocks.last().unwrap().label, "repl");
    }

    // ---- Reduced-toolset (limit-mode) addendum ---------------------------

    #[test]
    fn limit_mode_appends_reduced_toolset_block_after_repl() {
        let blocks = build_system_blocks("/x", 1000, false, &[], flags_limit_mode());
        // core + runtime + repl + reduced-toolset = 4
        assert_eq!(blocks.len(), 4, "limit mode must produce 4 blocks");
        let labels: Vec<&str> = blocks.iter().map(|b| b.label).collect();
        assert_eq!(
            labels,
            vec!["core", "runtime", "repl", "reduced-toolset"],
            "block order must be core → runtime → repl → reduced-toolset"
        );
    }

    #[test]
    fn limit_mode_reduced_toolset_is_last_block() {
        let blocks = build_system_blocks("/x", 1000, false, &[], flags_limit_mode());
        assert_eq!(
            blocks.last().unwrap().label,
            "reduced-toolset",
            "reduced-toolset must be the last block (gets cache_control: ephemeral)"
        );
    }

    #[test]
    fn limit_mode_block_names_all_six_removed_tools() {
        // This tests the fileops-only configuration (repl_replaces_shell=false).
        let content = reduced_toolset_addendum(flags_limit_mode());
        for tool in &[
            "read_file",
            "write_file",
            "edit_file",
            "find_files",
            "grep_files",
            "list_files",
        ] {
            assert!(
                content.contains(tool),
                "reduced-toolset block must mention {tool}"
            );
        }
    }

    #[test]
    fn limit_mode_block_mentions_both_alternatives() {
        // Fileops-only mode: both python_repl and run_command are available
        // alternatives (run_command is still in the toolset).
        let content = reduced_toolset_addendum(flags_limit_mode());
        assert!(
            content.contains("python_repl"),
            "reduced-toolset block must mention python_repl"
        );
        assert!(
            content.contains("run_command"),
            "reduced-toolset block must mention run_command (shell tools are still present)"
        );
    }

    #[test]
    fn no_reduced_toolset_block_when_limit_mode_off() {
        let blocks = build_system_blocks("/x", 1000, false, &[], flags_repl_only());
        assert!(
            blocks.iter().all(|b| b.label != "reduced-toolset"),
            "reduced-toolset block must not appear when repl_replaces_fileops=false"
        );
    }

    #[test]
    fn limit_mode_with_instruction_files_has_correct_order() {
        let files = vec![InstructionFile {
            label: "repo-agents-md",
            path: PathBuf::from("/repo/AGENTS.md"),
            content: "REPO".to_owned(),
        }];
        let blocks = build_system_blocks("/x", 1000, false, &files, flags_limit_mode());
        // core + runtime + repo-agents-md + repl + reduced-toolset = 5
        assert_eq!(blocks.len(), 5);
        let labels: Vec<&str> = blocks.iter().map(|b| b.label).collect();
        assert_eq!(
            labels,
            vec![
                "core",
                "runtime",
                "repo-agents-md",
                "repl",
                "reduced-toolset"
            ]
        );
    }

    // ---- Headless mode ---------------------------------------------------

    #[test]
    fn headless_omits_output_format_and_discuss() {
        let blocks = build_system_blocks("/tmp", 1000, true, &[], flags_default());
        let core = &blocks[0].content;
        assert!(
            !core.contains("## Output format"),
            "headless must omit output-format"
        );
        assert!(
            !core.contains("stop and discuss"),
            "headless must omit discussion policy"
        );
    }

    #[test]
    fn interactive_includes_output_format_and_discuss() {
        let blocks = build_system_blocks("/tmp", 1000, false, &[], flags_default());
        let core = &blocks[0].content;
        assert!(
            core.contains("## Output format"),
            "interactive must include output-format"
        );
        assert!(
            core.contains("stop and discuss"),
            "interactive must include discussion policy"
        );
    }

    // ---- File-tool guidance gating ---------------------------------------
    //
    // Contract: when repl_replaces_fileops=true, the assembled prompt must
    // contain zero bare-backtick references to the six removed tools outside
    // the "Reduced toolset" block (which legitimately lists them as absent).
    // When repl_replaces_fileops=false, the assembled prompt must still
    // contain the existing file-tool guidance.

    const FILE_TOOLS: [&str; 6] = [
        "read_file",
        "write_file",
        "edit_file",
        "find_files",
        "grep_files",
        "list_files",
    ];

    /// In limit mode: strip the "Reduced toolset" block content from the
    /// assembled prompt, then verify no bare-backtick references remain for
    /// any of the six removed tools.
    #[test]
    fn limit_mode_assembled_prompt_has_no_file_tool_refs_outside_reduced_toolset_block() {
        let blocks = build_system_blocks("/x", 1000, false, &[], flags_limit_mode());
        let full_prompt = join_blocks(&blocks);

        // Strip the reduced-toolset block content from the assembled prompt.
        // This is the only place the six tool names are allowed to appear.
        let reduced_content = reduced_toolset_addendum(flags_limit_mode());
        let residue = full_prompt.replace(&reduced_content, "");

        for tool in &FILE_TOOLS {
            let backtick_ref = format!("`{tool}`");
            assert!(
                !residue.contains(&backtick_ref),
                "limit-mode residue must not contain `{tool}` — found a \
                 bare-backtick reference outside the Reduced toolset block"
            );
        }
    }

    /// In limit mode with headless=true: same contract — no file-tool
    /// references in the residue after stripping the reduced-toolset block.
    #[test]
    fn limit_mode_headless_has_no_file_tool_refs_outside_reduced_toolset_block() {
        let blocks = build_system_blocks("/x", 1000, true, &[], flags_limit_mode());
        let full_prompt = join_blocks(&blocks);
        let reduced_content = reduced_toolset_addendum(flags_limit_mode());
        let residue = full_prompt.replace(&reduced_content, "");

        for tool in &FILE_TOOLS {
            let backtick_ref = format!("`{tool}`");
            assert!(
                !residue.contains(&backtick_ref),
                "headless limit-mode residue must not contain `{tool}`"
            );
        }
    }

    /// In normal mode (`repl_replaces_fileops=false`), the assembled prompt must
    /// still contain guidance for the file-op tools.
    #[test]
    fn normal_mode_assembled_prompt_contains_file_tool_guidance() {
        let blocks = build_system_blocks("/x", 1000, false, &[], flags_default());
        let full_prompt = join_blocks(&blocks);

        // Each of these is representative of a specific guidance fragment.
        assert!(
            full_prompt.contains("`grep_files`"),
            "normal mode must contain grep_files guidance"
        );
        assert!(
            full_prompt.contains("`read_file`"),
            "normal mode must contain read_file guidance"
        );
        assert!(
            full_prompt.contains("`edit_file`"),
            "normal mode must contain edit_file guidance"
        );
        assert!(
            full_prompt.contains("`find_files`"),
            "normal mode must contain find_files guidance"
        );
        assert!(
            full_prompt.contains("`list_files`"),
            "normal mode must contain list_files guidance"
        );
        assert!(
            full_prompt.contains("`write_file`"),
            "normal mode must contain write_file guidance"
        );
    }

    /// With repl=true but `repl_replaces_fileops=false`, the file-tool guidance
    /// is still present (only the repl addendum is added).
    #[test]
    fn repl_only_mode_still_contains_file_tool_guidance() {
        let blocks = build_system_blocks("/x", 1000, false, &[], flags_repl_only());
        let full_prompt = join_blocks(&blocks);

        for tool in &FILE_TOOLS {
            let backtick_ref = format!("`{tool}`");
            assert!(
                full_prompt.contains(&backtick_ref),
                "repl-only mode must still contain `{tool}` guidance"
            );
        }
    }

    // ---- Shell-tool guidance gating (repl_replaces_shell) ----------------
    //
    // Contract: when repl_replaces_shell=true, the assembled prompt must
    // contain zero bare-backtick references to the four removed shell tools
    // outside the "Reduced toolset" block (which legitimately lists them as
    // absent).  When repl_replaces_shell=false, the assembled prompt must
    // still contain the existing shell-tool guidance.

    const SHELL_TOOLS: [&str; 4] = [
        "run_command",
        "run_background",
        "wait_for_output",
        "write_stdin",
    ];

    /// When `repl_replaces_shell=true`: strip the "Reduced toolset" block, then
    /// verify no bare-backtick references remain for any of the four removed
    /// shell tools.
    #[test]
    fn repl_replaces_shell_assembled_prompt_has_no_shell_tool_refs_outside_reduced_toolset_block() {
        let flags = flags_repl_replaces_shell();
        let blocks = build_system_blocks("/x", 1000, false, &[], flags);
        let full_prompt = join_blocks(&blocks);

        // Strip the reduced-toolset block — it legitimately lists the removed tools.
        let reduced_content = reduced_toolset_addendum(flags);
        let residue = full_prompt.replace(&reduced_content, "");

        for tool in &SHELL_TOOLS {
            let backtick_ref = format!("`{tool}`");
            assert!(
                !residue.contains(&backtick_ref),
                "repl_replaces_shell residue must not contain `{tool}` — found a \
                 bare-backtick reference outside the Reduced toolset block"
            );
        }
    }

    /// Headless variant of the shell-tool gating test.
    #[test]
    fn repl_replaces_shell_headless_has_no_shell_tool_refs_outside_reduced_toolset_block() {
        let flags = flags_repl_replaces_shell();
        let blocks = build_system_blocks("/x", 1000, true, &[], flags);
        let full_prompt = join_blocks(&blocks);
        let reduced_content = reduced_toolset_addendum(flags);
        let residue = full_prompt.replace(&reduced_content, "");

        for tool in &SHELL_TOOLS {
            let backtick_ref = format!("`{tool}`");
            assert!(
                !residue.contains(&backtick_ref),
                "headless repl_replaces_shell residue must not contain `{tool}`"
            );
        }
    }

    /// When only `repl_replaces_shell=true`, the six file-op tools are still
    /// present in the assembled prompt.
    #[test]
    fn repl_replaces_shell_still_contains_file_tool_guidance() {
        let blocks = build_system_blocks("/x", 1000, false, &[], flags_repl_replaces_shell());
        let full_prompt = join_blocks(&blocks);

        for tool in &FILE_TOOLS {
            let backtick_ref = format!("`{tool}`");
            assert!(
                full_prompt.contains(&backtick_ref),
                "repl_replaces_shell mode must still contain `{tool}` guidance (file tools remain)"
            );
        }
    }

    /// When `repl_replaces_shell=false` (normal or repl-only mode), the
    /// assembled prompt must contain the shell-tool guidance.
    #[test]
    fn normal_mode_assembled_prompt_contains_shell_tool_guidance() {
        let blocks = build_system_blocks("/x", 1000, false, &[], flags_default());
        let full_prompt = join_blocks(&blocks);

        assert!(
            full_prompt.contains("`run_command`"),
            "normal mode must contain run_command guidance"
        );
        assert!(
            full_prompt.contains("`run_background`"),
            "normal mode must contain run_background guidance"
        );
        assert!(
            full_prompt.contains("`wait_for_output`"),
            "normal mode must contain wait_for_output guidance"
        );
        // `write_stdin` is always referenced as `write_stdin(pid, text)` in the core
        // prompt, so check for the name without requiring the standalone backtick form.
        assert!(
            full_prompt.contains("write_stdin"),
            "normal mode must contain write_stdin guidance"
        );
    }

    // -----------------------------------------------------------------------
    // fetch_url / postprocess gating tests
    // -----------------------------------------------------------------------

    /// When `repl_replaces_shell` is off, the assembled prompt must mention
    /// `postprocess` in the `fetch_url` guidance.
    #[test]
    fn flag_off_assembled_prompt_mentions_postprocess() {
        let full_prompt = join_blocks(&build_system_blocks(
            "/x",
            1000,
            false,
            &[],
            flags_default(),
        ));
        assert!(
            full_prompt.contains("`postprocess`"),
            "flag-off prompt must mention postprocess: length={}",
            full_prompt.len()
        );
    }

    /// When `repl_replaces_shell` is on, the assembled prompt must NOT contain
    /// any bare-backtick reference to `postprocess`.
    #[test]
    fn flag_on_assembled_prompt_has_no_postprocess_reference() {
        let full_prompt = join_blocks(&build_system_blocks(
            "/x",
            1000,
            false,
            &[],
            flags_repl_replaces_shell(),
        ));
        assert!(
            !full_prompt.contains("`postprocess`"),
            "flag-on prompt must not contain `postprocess` reference"
        );
    }

    /// Tier 2 (both replaces flags on) prompt must also have no postprocess reference.
    #[test]
    fn tier2_assembled_prompt_has_no_postprocess_reference() {
        let full_prompt = join_blocks(&build_system_blocks(
            "/x",
            1000,
            false,
            &[],
            flags_both_replaces(),
        ));
        assert!(
            !full_prompt.contains("`postprocess`"),
            "Tier 2 prompt must not contain `postprocess` reference"
        );
    }

    /// Shell-gated: the `fetch_url` guidance must mention the byte/line cap.
    #[test]
    fn flag_on_fetch_url_guidance_mentions_cap() {
        let full_prompt = join_blocks(&build_system_blocks(
            "/x",
            1000,
            false,
            &[],
            flags_repl_replaces_shell(),
        ));
        // "2000" is the line cap; "50" appears in "50 KB".
        assert!(
            full_prompt.contains("2000") || full_prompt.contains("50"),
            "flag-on fetch_url guidance must mention the cap (2000 lines / 50 KB)"
        );
    }

    /// `repl_replaces_shell` mode appends a reduced-toolset block after repl.
    #[test]
    fn repl_replaces_shell_appends_reduced_toolset_block() {
        let blocks = build_system_blocks("/x", 1000, false, &[], flags_repl_replaces_shell());
        assert_eq!(
            blocks.len(),
            4,
            "must produce 4 blocks: core+runtime+repl+reduced-toolset"
        );
        let labels: Vec<&str> = blocks.iter().map(|b| b.label).collect();
        assert_eq!(labels, vec!["core", "runtime", "repl", "reduced-toolset"]);
    }

    /// `repl_replaces_shell` reduced-toolset block names all four removed tools.
    #[test]
    fn repl_replaces_shell_block_names_all_four_removed_shell_tools() {
        let content = reduced_toolset_addendum(flags_repl_replaces_shell());
        for tool in &SHELL_TOOLS {
            assert!(
                content.contains(tool),
                "reduced-toolset block must mention {tool}"
            );
        }
    }

    /// `repl_replaces_shell` reduced-toolset block describes the subprocess pattern.
    #[test]
    fn repl_replaces_shell_block_mentions_subprocess_pattern() {
        let content = reduced_toolset_addendum(flags_repl_replaces_shell());
        assert!(
            content.contains("subprocess"),
            "reduced-toolset block must mention subprocess"
        );
        assert!(
            content.contains("python_repl"),
            "reduced-toolset block must reference python_repl"
        );
    }

    // ---- Both replaces_* flags (Tier 2) ----------------------------------

    /// Tier 2: stripped residue must contain no backtick refs to any of
    /// the ten removable tools (6 file + 4 shell).
    #[test]
    fn both_replaces_assembled_prompt_has_no_removable_tool_refs_outside_reduced_toolset_block() {
        let flags = flags_both_replaces();
        let blocks = build_system_blocks("/x", 1000, false, &[], flags);
        let full_prompt = join_blocks(&blocks);
        let reduced_content = reduced_toolset_addendum(flags);
        let residue = full_prompt.replace(&reduced_content, "");

        for tool in FILE_TOOLS.iter().chain(SHELL_TOOLS.iter()) {
            let backtick_ref = format!("`{tool}`");
            assert!(
                !residue.contains(&backtick_ref),
                "Tier 2 residue must not contain `{tool}` — found a bare-backtick \
                 reference outside the Reduced toolset block"
            );
        }
    }

    /// Tier 2: the combined reduced-toolset block covers both sections.
    #[test]
    fn both_replaces_combined_block_has_both_sections() {
        let content = reduced_toolset_addendum(flags_both_replaces());
        // File section
        for tool in &FILE_TOOLS {
            assert!(content.contains(tool), "Tier 2 block must mention {tool}");
        }
        // Shell section
        for tool in &SHELL_TOOLS {
            assert!(content.contains(tool), "Tier 2 block must mention {tool}");
        }
        assert!(
            content.contains("subprocess"),
            "Tier 2 block must describe subprocess pattern"
        );
    }

    /// Tier 2: the fileops section does NOT mention `run_command` (shell tools removed).
    #[test]
    fn both_replaces_fileops_section_does_not_mention_run_command_as_alternative() {
        let content = reduced_toolset_addendum(flags_both_replaces());
        // run_command appears in the shell-removed section ("does not expose"),
        // but must NOT appear in the fileops section as an alternative.
        // The shell section legitimately lists it as removed; the fileops section
        // must only suggest python_repl.
        //
        // We verify this indirectly: the single heading "## Reduced toolset" is
        // present, the fileops paragraph does not say "run_command" as an option
        // ("choose whichever is cleaner" wording is absent).
        assert!(
            !content.contains("Choose whichever is cleaner"),
            "Tier 2 fileops section must not offer run_command as an alternative"
        );
    }

    /// Tier 2: block order is core → runtime → repl → reduced-toolset.
    #[test]
    fn both_replaces_block_order() {
        let blocks = build_system_blocks("/x", 1000, false, &[], flags_both_replaces());
        assert_eq!(blocks.len(), 4);
        let labels: Vec<&str> = blocks.iter().map(|b| b.label).collect();
        assert_eq!(labels, vec!["core", "runtime", "repl", "reduced-toolset"]);
    }

    // ---- Discovery: repo tier ----------------------------------------

    #[test]
    fn discovers_repo_agents_md_at_git_root() {
        let dir = tempfile::tempdir().unwrap();
        git_init(dir.path());
        std::fs::write(dir.path().join("AGENTS.md"), "REPO INSTRUCTIONS").unwrap();

        // Use an empty XDG dir to block the global tier so the test
        // is independent of the host's ~/.config/omega.  No env
        // mutation needed — we go through the env-injected helper.
        let empty_home = tempfile::tempdir().unwrap();
        let files = discover_instruction_files_with_env(
            dir.path(),
            Some(empty_home.path().as_os_str()),
            Some(empty_home.path().as_os_str()),
        );

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].label, "repo-agents-md");
        assert_eq!(files[0].content, "REPO INSTRUCTIONS");
        assert_eq!(files[0].path, dir.path().join("AGENTS.md"));
    }

    #[test]
    fn walks_up_from_subdir_to_repo_root() {
        let dir = tempfile::tempdir().unwrap();
        git_init(dir.path());
        std::fs::write(dir.path().join("AGENTS.md"), "TOP").unwrap();
        let sub = dir.path().join("a").join("b");
        std::fs::create_dir_all(&sub).unwrap();

        let path = repo_agents_md_path(&sub).expect("found");
        assert_eq!(path, dir.path().join("AGENTS.md"));
    }

    #[test]
    fn no_repo_agents_md_when_no_git() {
        let dir = tempfile::tempdir().unwrap();
        // No `git init` here, no `.git` anywhere up the chain (tempdir
        // is typically `/tmp/...` — no `.git` ancestor).
        assert!(repo_agents_md_path(dir.path()).is_none());
    }

    // ---- Discovery: global tier --------------------------------------

    #[test]
    fn global_path_honours_xdg_config_home() {
        let dir = tempfile::tempdir().unwrap();
        let p = global_agents_md_path_from_env(Some(dir.path().as_os_str()), None).expect("path");
        assert_eq!(p, dir.path().join("omega").join("AGENTS.md"));
    }

    #[test]
    fn global_path_falls_back_to_home_dot_config_when_xdg_unset() {
        let p = global_agents_md_path_from_env(None, Some(std::ffi::OsStr::new("/home/test")))
            .expect("path");
        assert_eq!(p, PathBuf::from("/home/test/.config/omega/AGENTS.md"));
    }

    #[test]
    fn global_path_is_none_when_both_env_vars_missing() {
        assert!(global_agents_md_path_from_env(None, None).is_none());
    }

    #[test]
    fn global_path_treats_empty_xdg_as_unset() {
        let p = global_agents_md_path_from_env(
            Some(std::ffi::OsStr::new("")),
            Some(std::ffi::OsStr::new("/h")),
        )
        .expect("path");
        assert_eq!(p, PathBuf::from("/h/.config/omega/AGENTS.md"));
    }

    // ---- Discovery: both tiers ---------------------------------------

    #[test]
    fn discovers_both_tiers_with_global_first() {
        let global_dir = tempfile::tempdir().unwrap();
        let xdg = global_dir.path();
        let omega_dir = xdg.join("omega");
        std::fs::create_dir_all(&omega_dir).unwrap();
        std::fs::write(omega_dir.join("AGENTS.md"), "GLOBAL").unwrap();

        let repo = tempfile::tempdir().unwrap();
        git_init(repo.path());
        std::fs::write(repo.path().join("AGENTS.md"), "REPO").unwrap();

        let files = discover_instruction_files_with_env(repo.path(), Some(xdg.as_os_str()), None);

        assert_eq!(files.len(), 2);
        assert_eq!(files[0].label, "global-agents-md");
        assert_eq!(files[0].content, "GLOBAL");
        assert_eq!(files[1].label, "repo-agents-md");
        assert_eq!(files[1].content, "REPO");
    }

    #[test]
    fn missing_global_file_is_silently_skipped() {
        // XDG points to a real dir but with no `omega/AGENTS.md` in it.
        let xdg = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        // No git init, no repo AGENTS.md either.
        let files =
            discover_instruction_files_with_env(repo.path(), Some(xdg.path().as_os_str()), None);
        assert!(files.is_empty());
    }

    // ---- global_agents_md_path (real env) ----------------------------

    #[test]
    fn global_agents_md_path_is_some_in_real_env() {
        // `global_agents_md_path` reads $XDG_CONFIG_HOME or $HOME from the
        // real environment.  In CI, $HOME is always set, so the function must
        // return `Some(_)`.
        //
        // Kills two mutants identified in
        // docs/mutation-testing/omega-agent/survivors.md:
        //   * "replace body with `None`" — obvious failure
        //   * "replace body with `Some(Default::default())`" — would return
        //     `Some(PathBuf::new())` (empty path), which does not end with
        //     `omega/AGENTS.md`.
        //
        // We do NOT mutate $HOME or $XDG_CONFIG_HOME; we call
        // `global_agents_md_path()` (not `_from_env`) so the function reads
        // the live environment variables itself.
        let path = global_agents_md_path();
        assert!(
            path.is_some(),
            "global_agents_md_path() must return Some(_) when $HOME is set (CI always has $HOME)"
        );
        let p = path.unwrap();
        assert!(
            p.ends_with("omega/AGENTS.md"),
            "path must end with omega/AGENTS.md, got {p:?}"
        );
    }
}
