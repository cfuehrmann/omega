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
#[must_use]
pub fn build_system_blocks(
    cwd: &str,
    max_output_tokens: u32,
    headless: bool,
    files: &[InstructionFile],
) -> Vec<SystemBlock> {
    let mut out = Vec::with_capacity(2 + files.len());

    out.push(SystemBlock {
        label: "core",
        content: core_prompt(headless),
        source_path: None,
    });

    out.push(SystemBlock {
        label: "runtime",
        content: runtime_context(cwd, max_output_tokens),
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

    out
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
fn runtime_context(cwd: &str, max_output_tokens: u32) -> String {
    format!(
        "## Runtime context\n\
\n\
Your working directory is {cwd}. Treat it as the root of your work — use\n\
relative paths from there unless the user directs otherwise.\n\
\n\
The output token budget is {max_output_tokens} tokens per response. Tool call\n\
arguments count against this budget. Very large `write_file` calls risk\n\
hitting the limit mid-generation, leaving a broken turn. For large new\n\
files: write a skeleton first, then extend with `edit_file`. For large\n\
existing files: always prefer `edit_file` over a full rewrite."
    )
}

/// Core prompt (block #1).
/// `headless` drops the two sections that require an interactive human UI:
/// output-format rendering guidance and the discussion-before-acting policy.
#[allow(clippy::too_many_lines)]
fn core_prompt(headless: bool) -> String {
    let mut s = "\
You are an expert assistant operating inside Omega, a software engineering agent harness. Use tools when needed.

## Project orientation

When you have no prior context about the project structure, check manifest
files (e.g. `Cargo.toml`, `package.json`, `*.csproj`, `pyproject.toml`) to
learn the stack. Project-specific conventions are in the attached `AGENTS.md`
blocks (if any) — do not search the filesystem for them.

**`AGENTS.md` is already injected into this system prompt.** Reading it with
a tool call (`read_file`, `find_files`, etc.) is always wrong — the content
is already here.

## Tools

The operator has pre-approved all tool calls. No confirmation is needed.

Prefer `grep_files` over speculative `read_file` calls when searching for
a symbol, string, or pattern across the codebase. It's faster and returns
only what's relevant.
Use `find_files` when you know a file's name or extension but not its exact
path — don't brute-force with repeated `list_files` calls.
Use `run_command` for builds, test suites, commits, and any finite command.
The default timeout is 120 s; pass a higher `timeout` (e.g. 300) for commands
you expect to take longer. Reserve `run_background` for processes that must
stay alive indefinitely (dev servers, file watchers).
All `run_command` and `wait_for_output` results are tee'd to a session-cache
log and the path is surfaced in a footer:
- `[full output: <path>]` when the output fit within the cap.
- `[truncated; showed last 100 KB of 487 KB. Full output: <path>]` when capped.
When a result is **truncated**, use `read_file` or `grep_files` on the cache
path to recover the bytes that didn't fit inline. The cache is also useful
when an earlier (full) output has aged out of immediate context and you need
to revisit it without re-running the command — re-running is slow and may
produce different output. When the bytes you need are already inline and
recent, read them directly rather than calling another tool over the same
bytes.
Pass `truncation_bias: \"tail\"` (default on failure), `\"head\"` (default on
success), or `\"middle\"` to control which portion is returned when the
output is truncated.
To wait for a background process to become ready (e.g. a dev server), use
`wait_for_output(logFile, pid, timeoutMs, pattern?)` instead of `sleep` + `tail`.
Always pass the `pid` from `run_background` — if the process exits before the pattern matches,
`wait_for_output` returns immediately with `processExited: true` and the exit code instead of
waiting for the full timeout.
The `pattern` is a **JavaScript regex** — use `|` for alternation (e.g. `\"ready|Error|done\"`).
If a background process prompts for interactive input, use
`write_stdin(pid, text)` to respond (include \\n to submit a line). Pass
`end_stdin=true` to signal EOF after writing.
Chain independent tool calls in parallel when results don't depend on each
other.
Check for a task runner and use it to discover available commands
(`just --list`, `make help`, `npm run`, etc.).
For `edit_file`: read or grep the file first to identify **all** needed
changes, then apply them in a single call with `replacements`. Never call
`edit_file` on the same file twice in a row — that is always a mistake.

Use `web_search` freely for documentation, current information, API details,
error messages, or anything not in local files. Prefer it over guessing or
relying on potentially stale training data.
`fetch_url` downloads a URL **once** and runs a single `postprocess` query
on it. The result includes a cache path — for any further queries on the same
content, use `grep_files`/`read_file` on that path.
`postprocess` is required. Prefer `grep` or `awk` when you know what to
look for, and `head -N` as the catch-all. Never use `cat` — `head -N`
gives the same result on short pages and stays bounded on long ones.

When a command produces verbose output — whether from `run_background`'s
`logFile` or from a `run_command` redirected to a file — inspect it with
`read_file` (use `offset`/`limit` to paginate through large files) and
`grep_files` to search for specific patterns. Never re-run a command just to
see more output. Never re-run any command without making a code change in
between.

If a tool fails in a noteworthy way, mention it in your response."
        .to_owned();

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

    #[test]
    fn core_block_is_first_and_unheadered() {
        let blocks = build_system_blocks("/tmp/proj", 64_000, false, &[]);
        assert_eq!(blocks[0].label, "core");
        assert!(blocks[0].source_path.is_none());
        assert!(blocks[0].content.starts_with("You are an expert assistant"));
        // The core block must NOT contain the runtime values.
        assert!(!blocks[0].content.contains("64000"));
        assert!(!blocks[0].content.contains("/tmp/proj"));
    }

    #[test]
    fn runtime_block_contains_cwd_and_token_budget() {
        let blocks = build_system_blocks("/tmp/proj", 12_345, false, &[]);
        let rt = blocks.iter().find(|b| b.label == "runtime").expect("rt");
        assert!(rt.content.starts_with("## Runtime context"));
        assert!(rt.content.contains("/tmp/proj"));
        assert!(rt.content.contains("12345 tokens"));
    }

    #[test]
    fn no_instruction_files_yields_exactly_two_blocks() {
        let blocks = build_system_blocks("/x", 1000, false, &[]);
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
        let blocks = build_system_blocks("/repo", 1000, false, &files);
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
        let blocks = build_system_blocks("/repo", 1000, false, &files);
        assert_eq!(blocks.len(), 2, "whitespace-only file should be ignored");
    }

    #[test]
    fn join_blocks_separates_with_blank_line() {
        let blocks = build_system_blocks("/x", 1000, false, &[]);
        let joined = join_blocks(&blocks);
        assert!(joined.contains("\n\n## Runtime context"));
    }

    // ---- Headless mode ---------------------------------------------------

    #[test]
    fn headless_omits_output_format_and_discuss() {
        let blocks = build_system_blocks("/tmp", 1000, true, &[]);
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
        let blocks = build_system_blocks("/tmp", 1000, false, &[]);
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
