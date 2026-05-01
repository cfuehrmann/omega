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
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "Hello, Rust!"
    );
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

    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "FOO bar BAZ"
    );
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
    assert!(err.contains("2 times") || err.contains("exactly once"), "got: {err}");
}

// ---------------------------------------------------------------------------
// list_files
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_files_flat() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "").unwrap();
    std::fs::write(dir.path().join("b.txt"), "").unwrap();

    let out = exec("list_files", json!({ "path": dir.path().to_str().unwrap() }))
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

    let out = exec("list_files", json!({ "path": dir.path().to_str().unwrap() }))
        .await
        .unwrap();
    // Directory should appear before the file even though "zzz" sorts after "aaa".
    let dir_pos = out.find("zzz_dir/").expect("directory not found");
    let file_pos = out.find("aaa.txt").expect("file not found");
    assert!(dir_pos < file_pos, "dirs should precede files: {out}");
}

// ---------------------------------------------------------------------------
// grep_files
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
