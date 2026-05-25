#![allow(
    clippy::doc_markdown, // test-only docs reference tool names
    clippy::format_collect, // (0..N).map(|i| format!("...{i}\n")).collect::<String>() is clearer in tests
)]

//! Integration tests for the file-system tools:
//! read_file, write_file, edit_file, list_files, grep_files, find_files.
//!
//! All I/O goes to a unique temporary directory created per test so tests can
//! run in parallel without conflicts.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::json;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn exec(name: &str, input: serde_json::Value) -> Result<String, String> {
    let result = omega_tools::execute_tool(name, input, None, None).await;
    if result.is_error {
        Err(result.content)
    } else {
        Ok(result.content)
    }
}

// ---------------------------------------------------------------------------
// read_file
// ---------------------------------------------------------------------------

#[tokio::test]
async fn read_file_basic() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hello.txt");
    std::fs::write(&path, "line1\nline2\nline3").unwrap();

    let out = exec("read_file", json!({ "path": path.to_str().unwrap() }))
        .await
        .unwrap();
    assert!(out.contains("line1"));
    assert!(out.contains("line3"));
}

#[tokio::test]
async fn read_file_offset_limit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lines.txt");
    let content: String = (1..=10).map(|n| format!("line{n}\n")).collect();
    std::fs::write(&path, &content).unwrap();

    let out = exec(
        "read_file",
        json!({ "path": path.to_str().unwrap(), "offset": 3, "limit": 2 }),
    )
    .await
    .unwrap();
    assert!(out.contains("line3"), "got: {out}");
    assert!(out.contains("line4"), "got: {out}");
    assert!(!out.contains("line5"), "got: {out}");
}

#[tokio::test]
async fn read_file_continuation_message() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lines.txt");
    // 10 lines, read 5 starting at 1
    let content: String = (1..=10).map(|n| format!("line{n}\n")).collect();
    std::fs::write(&path, &content).unwrap();

    let out = exec(
        "read_file",
        json!({ "path": path.to_str().unwrap(), "offset": 1, "limit": 5 }),
    )
    .await
    .unwrap();
    assert!(out.contains("more lines"), "expected continuation: {out}");
    assert!(out.contains("offset=6"), "expected offset=6: {out}");
}

#[tokio::test]
async fn read_file_missing_returns_error() {
    let result = exec("read_file", json!({ "path": "/no/such/path.txt" })).await;
    assert!(result.is_err(), "expected error for missing file");
}

// ---------------------------------------------------------------------------
// read_file — system-prompt guard
//
// These tests exercise the pre-dispatch hook in `execute_tool` that
// short-circuits `read_file` when the requested file is already embedded in
// the system prompt.  They use a real `ToolCtx` (with `system_prompt_paths`
// populated) so they go through the full `execute_tool` dispatch path and
// are suitable as the primary target for *targeted mutation testing*:
//
//   cargo mutants -p omega-tools --file "src/lib.rs"
//
// Only `src/lib.rs` is mutated; the fast omega-tools test suite (no network,
// no subprocess) catches every surviving mutant quickly.
// ---------------------------------------------------------------------------

/// Build a `ToolCtx` whose `system_prompt_paths` contains the canonical form
/// of `protected`.  `cache_dir` is set to `protected`'s parent so the ctx is
/// structurally valid (guard tests don't tee output, so the exact dir is
/// irrelevant).
fn ctx_protecting(protected: &std::path::Path) -> omega_tools::ToolCtx {
    use std::collections::HashSet;
    use std::sync::Arc;
    let mut paths = HashSet::new();
    paths.insert(protected.canonicalize().expect("canonicalize"));
    omega_tools::ToolCtx {
        cache_dir: protected.parent().expect("parent").to_path_buf(),
        tool_call_id: "guard-test".to_owned(),
        system_prompt_paths: Arc::new(paths),
        python_repl: None,
        flags: omega_types::FeatureFlags::default(),
    }
}

/// When a file's canonical path is in `system_prompt_paths`, `read_file`
/// must be short-circuited: `is_error` is `false` and the content mentions
/// "system prompt".
#[tokio::test]
async fn read_file_blocked_when_path_in_system_prompt_paths() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("AGENTS.md");
    std::fs::write(&file, "# secret content").unwrap();

    let ctx = ctx_protecting(&file);
    let result = omega_tools::execute_tool(
        "read_file",
        json!({ "path": file.to_str().unwrap() }),
        None,
        Some(&ctx),
    )
    .await;

    assert!(
        !result.is_error,
        "block should not surface as an error; got: {}",
        result.content
    );
    assert!(
        result.content.contains("system prompt"),
        "expected \"system prompt\" in block message; got: {}",
        result.content
    );
    // The file's actual content must NOT appear in the response.
    assert!(
        !result.content.contains("secret content"),
        "file content must not leak through the guard; got: {}",
        result.content
    );
}

/// When `ctx` is `None` (e.g. a unit test calling `execute_tool` directly
/// without a session), the guard is inactive and the file is read normally.
#[tokio::test]
async fn read_file_not_blocked_when_ctx_is_none() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("data.txt");
    std::fs::write(&file, "hello from disk").unwrap();

    let result = omega_tools::execute_tool(
        "read_file",
        json!({ "path": file.to_str().unwrap() }),
        None,
        None,
    )
    .await;

    assert!(!result.is_error);
    assert!(result.content.contains("hello from disk"));
}

/// When the protected set is non-empty but contains a *different* file, the
/// requested file is read normally.
#[tokio::test]
async fn read_file_not_blocked_when_different_path_is_protected() {
    let dir = tempfile::tempdir().unwrap();
    let protected = dir.path().join("AGENTS.md");
    let other = dir.path().join("other.txt");
    std::fs::write(&protected, "protected").unwrap();
    std::fs::write(&other, "other content").unwrap();

    let ctx = ctx_protecting(&protected);
    let result = omega_tools::execute_tool(
        "read_file",
        json!({ "path": other.to_str().unwrap() }),
        None,
        Some(&ctx),
    )
    .await;

    assert!(!result.is_error);
    assert!(
        result.content.contains("other content"),
        "unprotected file should be readable; got: {}",
        result.content
    );
}

/// The guard must match via `canonicalize`, so a path that resolves to the
/// same inode as a protected path (e.g. through a symlink) is also blocked.
#[tokio::test]
async fn read_file_blocked_via_symlink_to_protected_file() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("AGENTS.md");
    let link = dir.path().join("link_to_agents.md");
    std::fs::write(&real, "real content").unwrap();
    std::os::unix::fs::symlink(&real, &link).unwrap();

    // Protect via the *real* path; read via the *symlink*.
    let ctx = ctx_protecting(&real);
    let result = omega_tools::execute_tool(
        "read_file",
        json!({ "path": link.to_str().unwrap() }),
        None,
        Some(&ctx),
    )
    .await;

    assert!(
        !result.is_error,
        "symlink to protected file should be blocked, not errored; got: {}",
        result.content
    );
    assert!(
        result.content.contains("system prompt"),
        "expected block message; got: {}",
        result.content
    );
}

// ---------------------------------------------------------------------------
// write_file
// ---------------------------------------------------------------------------

#[tokio::test]
async fn write_file_creates_dirs_and_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a").join("b").join("c.txt");

    let out = exec(
        "write_file",
        json!({ "path": path.to_str().unwrap(), "content": "hello" }),
    )
    .await
    .unwrap();

    assert!(out.contains("Wrote"), "got: {out}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");
}

#[tokio::test]
async fn write_file_overwrites_existing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("x.txt");
    std::fs::write(&path, "old content").unwrap();

    exec(
        "write_file",
        json!({ "path": path.to_str().unwrap(), "content": "new content" }),
    )
    .await
    .unwrap();

    assert_eq!(std::fs::read_to_string(&path).unwrap(), "new content");
}

// ---------------------------------------------------------------------------
// edit_file
// ---------------------------------------------------------------------------

#[tokio::test]
async fn edit_file_basic_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("edit.txt");
    std::fs::write(&path, "Hello, world!").unwrap();

    let out = exec(
        "edit_file",
        json!({
            "path": path.to_str().unwrap(),
            "replacements": [{ "old_text": "world", "new_text": "Rust" }]
        }),
    )
    .await
    .unwrap();

    assert!(out.contains("edit"), "got: {out}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "Hello, Rust!");
}

#[tokio::test]
async fn edit_file_multiple_replacements() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("multi.txt");
    std::fs::write(&path, "foo bar baz").unwrap();

    exec(
        "edit_file",
        json!({
            "path": path.to_str().unwrap(),
            "replacements": [
                { "old_text": "foo", "new_text": "FOO" },
                { "old_text": "baz", "new_text": "BAZ" }
            ]
        }),
    )
    .await
    .unwrap();

    assert_eq!(std::fs::read_to_string(&path).unwrap(), "FOO bar BAZ");
}

#[tokio::test]
async fn edit_file_not_found_returns_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("src.txt");
    std::fs::write(&path, "hello world").unwrap();

    let err = exec(
        "edit_file",
        json!({
            "path": path.to_str().unwrap(),
            "replacements": [{ "old_text": "MISSING", "new_text": "x" }]
        }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("not found"), "got: {err}");
}

#[tokio::test]
async fn edit_file_duplicate_returns_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dup.txt");
    std::fs::write(&path, "aa bb aa").unwrap();

    let err = exec(
        "edit_file",
        json!({
            "path": path.to_str().unwrap(),
            "replacements": [{ "old_text": "aa", "new_text": "zz" }]
        }),
    )
    .await
    .unwrap_err();
    assert!(
        err.contains("2 times") || err.contains("exactly once"),
        "got: {err}"
    );
}

// ---------------------------------------------------------------------------
// list_files
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_files_flat() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "").unwrap();
    std::fs::write(dir.path().join("b.txt"), "").unwrap();

    let out = exec(
        "list_files",
        json!({ "path": dir.path().to_str().unwrap() }),
    )
    .await
    .unwrap();
    assert!(out.contains("a.txt"), "got: {out}");
    assert!(out.contains("b.txt"), "got: {out}");
}

#[tokio::test]
async fn list_files_recursive() {
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("subdir");
    std::fs::create_dir(&sub).unwrap();
    std::fs::write(sub.join("deep.txt"), "").unwrap();
    std::fs::write(dir.path().join("root.txt"), "").unwrap();

    let out = exec(
        "list_files",
        json!({ "path": dir.path().to_str().unwrap(), "recursive": true }),
    )
    .await
    .unwrap();
    assert!(out.contains("subdir/"), "got: {out}");
    assert!(out.contains("deep.txt"), "got: {out}");
    assert!(out.contains("root.txt"), "got: {out}");
}

#[tokio::test]
async fn list_files_dirs_before_files() {
    // Creates 10 files ("a00.txt" – "a09.txt") and 5 dirs ("z0/" – "z4/").
    // With 15 entries, readdir returns them in a hash-based order that very
    // likely interleaves files and dirs.  The (false, true) match arm in the
    // sort_by comparator is the only thing that guarantees dirs precede files
    // when a file is the *first* argument of a comparison.  Deleting that arm
    // falls through to alphabetical order, which puts "a*" before "z*" — the
    // test then sees a file before a dir and fails.
    let dir = tempfile::tempdir().unwrap();
    for i in 0..10 {
        std::fs::write(dir.path().join(format!("a{i:02}.txt")), "").unwrap();
    }
    for i in 0..5 {
        std::fs::create_dir(dir.path().join(format!("z{i}"))).unwrap();
    }

    let out = exec(
        "list_files",
        json!({ "path": dir.path().to_str().unwrap() }),
    )
    .await
    .unwrap();

    // Collect positions of dirs ("z0/" … "z4/") and files ("a00.txt" … "a09.txt").
    let last_dir_pos = (0..5)
        .map(|i| out.find(&format!("z{i}/")).unwrap_or(usize::MAX))
        .max()
        .unwrap();
    let first_file_pos = (0..10)
        .map(|i| out.find(&format!("a{i:02}.txt")).unwrap_or(usize::MAX))
        .min()
        .unwrap();
    assert!(
        last_dir_pos < first_file_pos,
        "ALL dirs must appear before ANY file; last_dir_pos={last_dir_pos} first_file_pos={first_file_pos}: {out}"
    );
}

// ---------------------------------------------------------------------------
// read_file — paged-mode boundary conditions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn read_file_limit_only_enters_paged_mode() {
    // Passing only `limit` (no `offset`) must still enter paged mode.
    // Kills the `|| → &&` mutation on the `offset.is_some() || limit.is_some()` guard.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("paged.txt");
    let content: String = (1..=10).map(|n| format!("line{n}\n")).collect();
    std::fs::write(&path, &content).unwrap();

    let out = exec(
        "read_file",
        json!({ "path": path.to_str().unwrap(), "limit": 3 }),
    )
    .await
    .unwrap();
    // Paged mode: start=0, end=3, total=11 (10 lines + trailing "") → continuation shown.
    assert!(
        out.contains("more lines"),
        "limit-only should enter paged mode: {out}"
    );
}

#[tokio::test]
async fn read_file_paged_to_end_no_continuation() {
    // When the page covers every remaining line, no continuation hint must appear.
    // Kills the `< → <=` mutation on `if end < total`.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("paged.txt");
    let content: String = (1..=5).map(|n| format!("line{n}\n")).collect();
    std::fs::write(&path, &content).unwrap();

    let out = exec(
        "read_file",
        json!({ "path": path.to_str().unwrap(), "offset": 1, "limit": 100 }),
    )
    .await
    .unwrap();
    assert!(
        !out.contains("more lines"),
        "reading all lines should have no continuation: {out}"
    );
}

#[tokio::test]
async fn read_file_exactly_2000_split_elements_not_truncated() {
    // The code splits on '\n' and checks `lines.len() > MAX_LINES (2000)`.
    // A file with 1999 '\n'-terminated lines splits into exactly 2000 elements
    // (the trailing '\n' produces a final empty element).
    // `2000 > 2000` is false → no truncation.
    // Kills the `> → >=` mutation.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lines.txt");
    let content: String = (1..=1999).map(|n| format!("L{n}\n")).collect();
    // Verify the split count so the test documents its own invariant.
    assert_eq!(content.split('\n').count(), 2000);
    std::fs::write(&path, &content).unwrap();

    let out = exec("read_file", json!({ "path": path.to_str().unwrap() }))
        .await
        .unwrap();
    assert!(
        !out.contains("[Truncated"),
        "exactly 2000 split elements must not be truncated: {out}"
    );
}

#[tokio::test]
async fn read_file_2001_split_elements_is_truncated() {
    // 2000 '\n'-terminated lines → 2001 split elements.
    // `2001 > 2000` is true → truncated.
    // Kills the `> → ==` mutation.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lines.txt");
    let content: String = (1..=2000).map(|n| format!("L{n}\n")).collect();
    assert_eq!(content.split('\n').count(), 2001);
    std::fs::write(&path, &content).unwrap();

    let out = exec("read_file", json!({ "path": path.to_str().unwrap() }))
        .await
        .unwrap();
    assert!(
        out.contains("[Truncated"),
        "2001 split elements must be truncated: {out}"
    );
}

#[tokio::test]
async fn read_file_exactly_50000_bytes_not_truncated() {
    // Exactly MAX_BYTES (50 000) bytes: `content.len() > MAX_BYTES` is false → no truncation.
    // Kills the `> → >=` mutation on the byte check.
    // File is 2 lines (well below 2000) so the line limit doesn't fire first.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.txt");
    // 49 999 'x' bytes + '\n' + '\n' = 50 001 bytes ... adjust:
    // 49 998 'x' + '\n' + '\n' = 50 000 bytes exactly.
    let mut content = "x".repeat(49_998);
    content.push('\n');
    content.push('\n');
    assert_eq!(content.len(), 50_000);
    std::fs::write(&path, &content).unwrap();

    let out = exec("read_file", json!({ "path": path.to_str().unwrap() }))
        .await
        .unwrap();
    assert!(
        !out.contains("[Truncated"),
        "exactly 50 000 bytes must not be truncated: {out}"
    );
}

#[tokio::test]
async fn read_file_50001_bytes_is_truncated() {
    // 50 001 bytes: `content.len() > MAX_BYTES` is true → truncated.
    // Kills the `> → ==` and `> → <` mutations on the byte check.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.txt");
    let mut content = "x".repeat(49_999);
    content.push('\n');
    content.push('\n');
    assert_eq!(content.len(), 50_001);
    std::fs::write(&path, &content).unwrap();

    let out = exec("read_file", json!({ "path": path.to_str().unwrap() }))
        .await
        .unwrap();
    assert!(
        out.contains("[Truncated"),
        "50 001 bytes must be truncated: {out}"
    );
}

#[tokio::test]
async fn read_file_multibyte_char_at_boundary_is_trimmed_cleanly() {
    // Content: 49 999 ASCII 'a' bytes + '\u{4E2D}' (3 bytes) + 'a' = 50 003 bytes.
    // MAX_BYTES=50 000 falls in the middle of '\u{4E2D}' (start at byte 49 999).
    // char_boundary_at_or_before must back up to 49 999 → result is 49 999 'a's.
    // Kills all char_boundary_at_or_before mutations via the integration path.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("utf8.txt");
    let mut content = "a".repeat(49_999);
    content.push('\u{4E2D}'); // 3 UTF-8 bytes
    content.push('a');
    assert!(content.len() > 50_000);
    std::fs::write(&path, &content).unwrap();

    let out = exec("read_file", json!({ "path": path.to_str().unwrap() }))
        .await
        .unwrap();
    assert!(out.contains("[Truncated"), "should be truncated: {out}");
    // The multi-byte char must NOT appear in the truncated output.
    assert!(
        !out.contains('\u{4E2D}'),
        "multi-byte char must not appear in truncated output: {out}"
    );
    // Must be substantial — rules out `→ 0` and `→ 1` constant mutations.
    assert!(
        out.len() > 10_000,
        "truncated output must not be nearly empty: {out}"
    );
}

// ---------------------------------------------------------------------------
// edit_file — format and count_occurrences boundary conditions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn edit_file_single_replacement_uses_simple_format() {
    // With one replacement the output must NOT use the numbered-list format.
    // Kills the `== → !=` mutation on `if summaries.len() == 1`.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f.txt");
    std::fs::write(&path, "hello world").unwrap();

    let out = exec(
        "edit_file",
        json!({
            "path": path.to_str().unwrap(),
            "replacements": [{"old_text": "hello", "new_text": "hi"}]
        }),
    )
    .await
    .unwrap();
    assert!(
        !out.contains("replacements applied:"),
        "single replacement must use simple format, not numbered list: {out}"
    );
}

#[tokio::test]
async fn edit_file_single_replacement_error_has_no_index_label() {
    // When the replacement is not found and total==1, the error must NOT contain
    // "(replacement 1/1)".  Kills the `> → >=` mutation on `if total > 1` which
    // would add that label even for single replacements.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f.txt");
    std::fs::write(&path, "hello world").unwrap();

    let err = exec(
        "edit_file",
        json!({
            "path": path.to_str().unwrap(),
            "replacements": [{"old_text": "MISSING_TEXT", "new_text": "x"}]
        }),
    )
    .await
    .unwrap_err();
    assert!(
        !err.contains("replacement 1/1"),
        "single replacement error must not include index label: {err}"
    );
}

#[tokio::test]
async fn edit_file_multi_replacement_error_label_includes_index() {
    // With two replacements the error for the second one must include "(replacement 2/2)".
    // Kills the `> → ==`, `> → <`, and `> → >=` mutations on `if total > 1`.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f.txt");
    std::fs::write(&path, "foo bar").unwrap();

    let err = exec(
        "edit_file",
        json!({
            "path": path.to_str().unwrap(),
            "replacements": [
                {"old_text": "foo", "new_text": "FOO"},
                {"old_text": "MISSING", "new_text": "x"}
            ]
        }),
    )
    .await
    .unwrap_err();
    assert!(
        err.contains("replacement 2/2"),
        "error for second replacement must include label: {err}"
    );
}

#[tokio::test]
async fn edit_file_count_occurrences_exits_early_at_two() {
    // "aaaaa" contains "aaa" at step-1 positions 0, 1, 2 (three matches).
    // count_occurrences breaks after count reaches 2; so the error must say
    // "found 2 times", not "found 3 times".
    // Kills the `> → <` mutation on `if count > 1 { break }`.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f.txt");
    std::fs::write(&path, "aaaaa").unwrap();

    let err = exec(
        "edit_file",
        json!({
            "path": path.to_str().unwrap(),
            "replacements": [{"old_text": "aaa", "new_text": "x"}]
        }),
    )
    .await
    .unwrap_err();
    assert!(
        err.contains("found 2 times"),
        "count_occurrences must exit early at 2; got: {err}"
    );
}

// ---------------------------------------------------------------------------
// list_files — boundary conditions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_files_small_dir_has_no_truncation_notice() {
    // A small directory must never show "[Truncated".
    // Kills the `>= → <` mutation on `if results.len() >= MAX_ENTRIES`.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "").unwrap();
    std::fs::write(dir.path().join("b.txt"), "").unwrap();

    let out = exec(
        "list_files",
        json!({ "path": dir.path().to_str().unwrap() }),
    )
    .await
    .unwrap();
    assert!(
        !out.contains("[Truncated"),
        "small dir must not show truncation: {out}"
    );
}

#[tokio::test]
async fn list_files_hidden_file_excluded_by_default() {
    // Dotfiles must be hidden when listing non-recursively at depth 0.
    // Kills the `== → !=` mutation on `depth == 0`.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".hidden"), "").unwrap();
    std::fs::write(dir.path().join("visible.txt"), "").unwrap();

    let out = exec(
        "list_files",
        json!({ "path": dir.path().to_str().unwrap() }),
    )
    .await
    .unwrap();
    assert!(
        !out.contains(".hidden"),
        "hidden file must be excluded by default: {out}"
    );
    assert!(
        out.contains("visible.txt"),
        "visible file must appear: {out}"
    );
}

#[tokio::test]
async fn list_files_hidden_file_included_when_recursive() {
    // Dotfiles must appear when recursive=true (the `!recursive` condition is false).
    // Kills the `delete !` mutation on `!recursive`.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".hidden"), "").unwrap();
    std::fs::write(dir.path().join("visible.txt"), "").unwrap();

    let out = exec(
        "list_files",
        json!({ "path": dir.path().to_str().unwrap(), "recursive": true }),
    )
    .await
    .unwrap();
    assert!(
        out.contains(".hidden"),
        "hidden file must be included when recursive: {out}"
    );
}

#[tokio::test]
async fn list_files_non_recursive_does_not_descend_into_subdirs() {
    // Without `recursive`, subdirectory contents must not appear.
    // Kills the `&& → ||` mutation on the first `&&` in the recursion guard.
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("subdir");
    std::fs::create_dir(&sub).unwrap();
    std::fs::write(sub.join("deep.txt"), "").unwrap();
    std::fs::write(dir.path().join("root.txt"), "").unwrap();

    let out = exec(
        "list_files",
        json!({ "path": dir.path().to_str().unwrap() }),
    )
    .await
    .unwrap();
    assert!(out.contains("subdir/"), "subdir entry must appear: {out}");
    assert!(
        !out.contains("deep.txt"),
        "subdir contents must not appear non-recursively: {out}"
    );
}

#[tokio::test]
async fn list_files_git_dir_not_recursed_into() {
    // .git/ must appear in a recursive listing but its contents must not be
    // visited.  This exercises the `!name_str.starts_with(".git")` guard in
    // the recursion condition.
    // Kills the second `&& → ||` mutation: with `||`, the .git check becomes
    // irrelevant and we would descend into .git even when recursive=true.
    //
    // Note: the `name_str != "node_modules"` guard in the *same* condition is
    // dead code — an unconditional `continue` earlier in the loop already
    // skips any entry named "node_modules", so the recursion guard for it is
    // never reached.  That is a production-code observation worth discussing.
    let dir = tempfile::tempdir().unwrap();
    let git_dir = dir.path().join(".git");
    std::fs::create_dir(&git_dir).unwrap();
    std::fs::write(git_dir.join("config"), "").unwrap();
    std::fs::write(dir.path().join("README.md"), "").unwrap();

    let out = exec(
        "list_files",
        json!({ "path": dir.path().to_str().unwrap(), "recursive": true }),
    )
    .await
    .unwrap();
    // The .git dir itself appears (hidden-file skip only fires when !recursive).
    assert!(
        out.contains(".git/"),
        ".git/ must appear in recursive listing: {out}"
    );
    // Its contents must NOT be visited.
    assert!(
        !out.contains("config"),
        ".git contents must not be listed: {out}"
    );
}

#[tokio::test]
async fn list_files_dirs_sort_before_files_regardless_of_name() {
    // Directories must appear before files even when their names sort later.
    // Uses enough entries to force the sort to compare (file, dir) pairs,
    // which exercises the `(false, true) => Greater` arm.
    // Kills the `delete match arm (false, true)` mutation.
    let dir = tempfile::tempdir().unwrap();
    // Two dirs whose names sort *after* all the files.
    std::fs::create_dir(dir.path().join("zzz_dir1")).unwrap();
    std::fs::create_dir(dir.path().join("zzz_dir2")).unwrap();
    // Three files whose names sort *before* the dirs.
    std::fs::write(dir.path().join("aaa.txt"), "").unwrap();
    std::fs::write(dir.path().join("bbb.txt"), "").unwrap();
    std::fs::write(dir.path().join("ccc.txt"), "").unwrap();

    let out = exec(
        "list_files",
        json!({ "path": dir.path().to_str().unwrap() }),
    )
    .await
    .unwrap();
    // Both dirs must appear before the first file.
    let dir1_pos = out.find("zzz_dir1/").expect("zzz_dir1 not found");
    let dir2_pos = out.find("zzz_dir2/").expect("zzz_dir2 not found");
    let file_pos = out.find("aaa.txt").expect("aaa.txt not found");
    assert!(dir1_pos < file_pos, "zzz_dir1 must precede aaa.txt: {out}");
    assert!(dir2_pos < file_pos, "zzz_dir2 must precede aaa.txt: {out}");
}

// ---------------------------------------------------------------------------
// grep_files — context-lines and max-results boundary conditions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn grep_files_finds_match() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "fn hello() {}\nfn world() {}").unwrap();
    std::fs::write(dir.path().join("b.rs"), "fn other() {}").unwrap();

    let out = exec(
        "grep_files",
        json!({ "pattern": "hello", "path": dir.path().to_str().unwrap(), "context_lines": 0 }),
    )
    .await
    .unwrap();
    assert!(out.contains("hello"), "got: {out}");
    assert!(!out.contains("other"), "got: {out}");
}

#[tokio::test]
async fn grep_files_no_match() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "nothing here").unwrap();

    let out = exec(
        "grep_files",
        json!({ "pattern": "XYZZY", "path": dir.path().to_str().unwrap(), "context_lines": 0 }),
    )
    .await
    .unwrap();
    assert!(out.contains("No matches"), "got: {out}");
}

#[tokio::test]
async fn grep_files_glob_filter() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "needle in rust").unwrap();
    std::fs::write(dir.path().join("b.txt"), "needle in text").unwrap();

    let out = exec(
        "grep_files",
        json!({
            "pattern": "needle",
            "path": dir.path().to_str().unwrap(),
            "file_glob": "*.rs",
            "context_lines": 0
        }),
    )
    .await
    .unwrap();
    assert!(out.contains("rust"), "got: {out}");
    assert!(!out.contains("text"), "got: {out}");
}

#[tokio::test]
async fn grep_files_default_is_case_insensitive() {
    // The `case_sensitive` flag defaults to false, which is implemented as
    // `RegexBuilder::case_insensitive(!case_sensitive)` — i.e. when the
    // caller omits the flag we want INSENSITIVE matching. Search a
    // lowercase pattern against UPPERCASE content: the match only succeeds
    // under case-insensitive semantics.
    //
    // Kills `delete ! in search` (line 93 of grep_files.rs): that mutation
    // turns the default into case-SENSITIVE matching, which would fail to
    // find "hello" inside "HELLO".
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "HELLO WORLD\n").unwrap();

    let out = exec(
        "grep_files",
        json!({
            "pattern": "hello",
            "path": dir.path().to_str().unwrap(),
            "context_lines": 0
            // case_sensitive deliberately omitted — must default to false
        }),
    )
    .await
    .unwrap();
    assert!(
        out.contains("HELLO"),
        "lowercase pattern must match uppercase content under default case-insensitive search: {out}"
    );
}

#[tokio::test]
async fn grep_files_context_lines_include_surrounding_lines() {
    // With context_lines=2 the output must include lines before and after
    // the match, not just the match itself.
    // Kills the `> → ==`, `> → <`, `> → >=` mutations on `if context_lines > 0`.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("ctx.txt"),
        "before1\nbefore2\nTARGET\nafter1\nafter2\n",
    )
    .unwrap();

    let out = exec(
        "grep_files",
        json!({
            "pattern": "TARGET",
            "path": dir.path().to_str().unwrap(),
            "context_lines": 2
        }),
    )
    .await
    .unwrap();
    assert!(out.contains("TARGET"), "match must appear: {out}");
    assert!(
        out.contains("before1") || out.contains("before2"),
        "context lines before match must appear: {out}"
    );
    assert!(
        out.contains("after1") || out.contains("after2"),
        "context lines after match must appear: {out}"
    );
}

#[tokio::test]
async fn grep_files_max_results_truncation() {
    // max_results=3 with 5 matching lines: output must be truncated.
    // Kills `<= → ==`, `<= → >`, `<= → >=` mutations on `if lines.len() <= max_results`.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("m.txt"),
        "MATCH\nMATCH\nMATCH\nMATCH\nMATCH\n",
    )
    .unwrap();

    let out = exec(
        "grep_files",
        json!({
            "pattern": "MATCH",
            "path": dir.path().to_str().unwrap(),
            "context_lines": 0,
            "max_results": 3
        }),
    )
    .await
    .unwrap();
    assert!(
        out.contains("truncated"),
        "5 matches with max_results=3 must be truncated: {out}"
    );
}

#[tokio::test]
async fn grep_files_fewer_than_max_results_not_truncated() {
    // 3 matching lines with max_results=10: no truncation.
    // Kills the `<= → ==` mutation (3 == 10 is false → wrong truncation).
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("m.txt"), "MATCH\nMATCH\nMATCH\n").unwrap();

    let out = exec(
        "grep_files",
        json!({
            "pattern": "MATCH",
            "path": dir.path().to_str().unwrap(),
            "context_lines": 0,
            "max_results": 10
        }),
    )
    .await
    .unwrap();
    assert!(
        !out.contains("truncated"),
        "3 matches with max_results=10 must not be truncated: {out}"
    );
}

// ---------------------------------------------------------------------------
// find_files
// ---------------------------------------------------------------------------

#[tokio::test]
async fn find_files_by_glob() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("main.rs"), "").unwrap();
    std::fs::write(dir.path().join("lib.rs"), "").unwrap();
    std::fs::write(dir.path().join("README.md"), "").unwrap();

    let out = exec(
        "find_files",
        json!({ "pattern": "*.rs", "path": dir.path().to_str().unwrap() }),
    )
    .await
    .unwrap();
    assert!(out.contains("main.rs"), "got: {out}");
    assert!(out.contains("lib.rs"), "got: {out}");
    assert!(!out.contains("README"), "got: {out}");
}

#[tokio::test]
async fn find_files_no_match_returns_ok_not_error() {
    // When fd/find exits 1 (no results), the tool must return Ok with a message,
    // not Err.  Kills `!= 1 → == 1` on the exit-code guard.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("hello.txt"), "").unwrap();

    let out = exec(
        "find_files",
        json!({ "pattern": "*.rs", "path": dir.path().to_str().unwrap() }),
    )
    .await
    .unwrap(); // must be Ok
    assert!(
        out.contains("No files found"),
        "expected no-files message: {out}"
    );
}

#[tokio::test]
async fn find_files_max_results_truncated() {
    // 3 files, max_results=2: output must be truncated.
    // Kills the `> → ==`, `> → <`, `> → >=` mutations on `lines.len() > max_results`.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "").unwrap();
    std::fs::write(dir.path().join("b.txt"), "").unwrap();
    std::fs::write(dir.path().join("c.txt"), "").unwrap();

    let out = exec(
        "find_files",
        json!({
            "pattern": "*.txt",
            "path": dir.path().to_str().unwrap(),
            "max_results": 2
        }),
    )
    .await
    .unwrap();
    assert!(
        out.contains("Truncated"),
        "3 files with max_results=2 must be truncated: {out}"
    );
}

#[tokio::test]
async fn find_files_exactly_max_results_not_truncated() {
    // Exactly max_results=3 files — no truncation expected.
    // Kills `> → >=` (3 >= 3 = true would wrongly truncate).
    let dir = tempfile::tempdir().unwrap();
    for i in 0..3 {
        std::fs::write(dir.path().join(format!("f{i}.txt")), "").unwrap();
    }
    let out = exec(
        "find_files",
        json!({
            "pattern": "*.txt",
            "path": dir.path().to_str().unwrap(),
            "max_results": 3
        }),
    )
    .await
    .unwrap();
    assert!(
        !out.contains("Truncated"),
        "exactly max_results=3 files must not be truncated: {out}"
    );
}

#[tokio::test]
async fn find_files_fewer_than_max_results_not_truncated() {
    // 2 files, max_results=10: no truncation.
    // Kills the `> → ==` mutation (2 == 10 is false → wrong truncation).
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "").unwrap();
    std::fs::write(dir.path().join("b.txt"), "").unwrap();

    let out = exec(
        "find_files",
        json!({
            "pattern": "*.txt",
            "path": dir.path().to_str().unwrap(),
            "max_results": 10
        }),
    )
    .await
    .unwrap();
    assert!(
        !out.contains("Truncated"),
        "2 files with max_results=10 must not be truncated: {out}"
    );
}

// ---------------------------------------------------------------------------
// find_files — type filter (kills all 9 type-guard mutations)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn find_files_type_filter_files_only() {
    // type="f" must include files and exclude directories.
    // Kills the `guard with true` (no results), `guard with false` (dirs leak
    // through), and `delete !` (only dirs would appear) mutations on the
    // `Some("f") if !ft.is_file()` guard.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("file.txt"), "").unwrap();
    std::fs::create_dir(dir.path().join("subdir")).unwrap();

    let out = exec(
        "find_files",
        json!({ "pattern": "*", "path": dir.path().to_str().unwrap(), "type": "f" }),
    )
    .await
    .unwrap();
    assert!(
        out.contains("file.txt"),
        "files must be included with type=f: {out}"
    );
    assert!(
        !out.contains("subdir"),
        "dirs must be excluded with type=f: {out}"
    );
}

#[tokio::test]
async fn find_files_type_filter_dirs_only() {
    // type="d" must include directories and exclude files.
    // Kills all three guard mutations on `Some("d") if !ft.is_dir()`.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("file.txt"), "").unwrap();
    std::fs::create_dir(dir.path().join("subdir")).unwrap();

    let out = exec(
        "find_files",
        json!({ "pattern": "*", "path": dir.path().to_str().unwrap(), "type": "d" }),
    )
    .await
    .unwrap();
    assert!(
        out.contains("subdir"),
        "dirs must be included with type=d: {out}"
    );
    assert!(
        !out.contains("file.txt"),
        "files must be excluded with type=d: {out}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn find_files_type_filter_symlinks_only() {
    // type="l" must include symlinks and exclude regular files.
    // Kills all three guard mutations on `Some("l") if !ft.is_symlink()`.
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("real.txt");
    std::fs::write(&file, "").unwrap();
    std::os::unix::fs::symlink(&file, dir.path().join("link.txt")).unwrap();

    let out = exec(
        "find_files",
        json!({ "pattern": "*", "path": dir.path().to_str().unwrap(), "type": "l" }),
    )
    .await
    .unwrap();
    assert!(
        out.contains("link.txt"),
        "symlinks must be included with type=l: {out}"
    );
    assert!(
        !out.contains("real.txt"),
        "regular files must be excluded with type=l: {out}"
    );
}

// ---------------------------------------------------------------------------
// grep_files — output format: line numbers, separators, gap marker
// ---------------------------------------------------------------------------

#[tokio::test]
async fn grep_files_match_line_uses_colon_context_line_uses_dash() {
    // Match lines must use ':' and context lines must use '-' as the
    // separator between the line number and line text.
    // Kills the `== → !=` mutation on `if j == i { ':' } else { '-' }`.
    // Also kills the `+ → *` mutation on `let lnum = j + 1` (line numbers
    // must be 1-indexed: line 2 appears as ':2:').
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f.txt"), "before\nMATCH\nafter\n").unwrap();

    let out = exec(
        "grep_files",
        json!({
            "pattern": "MATCH",
            "path": dir.path().to_str().unwrap(),
            "context_lines": 1
        }),
    )
    .await
    .unwrap();

    // Match is on line 2 (1-indexed) → must appear as ":2:MATCH".
    assert!(
        out.contains(":2:MATCH"),
        "match line must use :N: format: {out}"
    );
    // Context lines (lines 1 and 3) must use '-' as separator.
    assert!(
        out.contains(":1-before") || out.contains(":3-after"),
        "context lines must use :N- format: {out}"
    );
}

#[tokio::test]
async fn grep_files_non_adjacent_match_groups_separated_by_dashes() {
    // Two matches with a gap of more than context_lines between them must be
    // separated by a '--' line in the output.
    // Kills the `> → ==`, `> → <`, and `> → >=` mutations on
    // `if want_start > pe { results.push("--") }`.
    let dir = tempfile::tempdir().unwrap();
    // Lines: MATCH(1), gap(2..5), MATCH(6). With context=1, window for first
    // match ends at line 2, window for second starts at line 5 — gap of 3.
    std::fs::write(
        dir.path().join("f.txt"),
        "MATCH\ngap2\ngap3\ngap4\ngap5\nMATCH\n",
    )
    .unwrap();

    let out = exec(
        "grep_files",
        json!({
            "pattern": "MATCH",
            "path": dir.path().to_str().unwrap(),
            "context_lines": 1
        }),
    )
    .await
    .unwrap();
    assert!(
        out.contains("\n--\n") || out.starts_with("--") || out.ends_with("--"),
        "non-adjacent match groups must be separated by '--': {out}"
    );
}

#[tokio::test]
async fn grep_files_adjacent_match_groups_not_separated_by_dashes() {
    // When the context window of the second match begins exactly where the
    // first one ended (want_start == prev_end), the groups are adjacent and
    // must NOT be separated by '--'.
    // Kills the `> → >=` mutation on `if want_start > pe`.
    //
    // File layout with context=1:
    //   MATCH at line 1 (i=0): want_end = 0 + 1 + 1 = 2
    //   MATCH at line 4 (i=3): want_start = 3 - 1 = 2   ← exactly == prev_end
    //   With `>`: 2 > 2 is false → no separator.  Good.
    //   With `>=`: 2 >= 2 is true → separator added.  Caught.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f.txt"), "MATCH\nline2\nline3\nMATCH\n").unwrap();

    let out = exec(
        "grep_files",
        json!({
            "pattern": "MATCH",
            "path": dir.path().to_str().unwrap(),
            "context_lines": 1
        }),
    )
    .await
    .unwrap();
    assert!(
        !out.contains("\n--\n"),
        "exactly-adjacent groups must not produce a '--' separator: {out}"
    );
}
