//! Integration tests for the file-system tools:
//! read_file, write_file, edit_file, list_files, grep_files, find_files.
//!
//! All I/O goes to a unique temporary directory created per test so tests can
//! run in parallel without conflicts.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::json;

async fn exec(name: &str, input: serde_json::Value) -> Result<String, String> {
    let result = omega_tools::execute_tool(name, input, None).await;
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
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("zzz_dir");
    std::fs::create_dir(&sub).unwrap();
    std::fs::write(dir.path().join("aaa.txt"), "").unwrap();

    let out = exec(
        "list_files",
        json!({ "path": dir.path().to_str().unwrap() }),
    )
    .await
    .unwrap();
    // Directory should appear before the file even though "zzz" sorts after "aaa".
    let dir_pos = out.find("zzz_dir/").expect("directory not found");
    let file_pos = out.find("aaa.txt").expect("file not found");
    assert!(dir_pos < file_pos, "dirs should precede files: {out}");
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
