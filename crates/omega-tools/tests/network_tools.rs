#![allow(
    clippy::doc_markdown, // test-only docs reference tool names
    clippy::map_unwrap_or, // .map().unwrap_or() reads more clearly than .map_or() in tests
)]

//! Integration tests for the network tools: web_search, fetch_url.
//!
//! `web_search` is skipped unless `BRAVE_SEARCH_API_KEY` is set.
//! `fetch_url` uses real HTTP and caches to a temp dir per-process.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::json;

async fn exec(name: &str, input: serde_json::Value) -> Result<String, String> {
    let result = omega_tools::execute_tool(name, input, None, None).await;
    if result.is_error {
        Err(result.content)
    } else {
        Ok(result.content)
    }
}

/// Like `exec` but constructs a `ToolCtx` with the given `tool_selection`.
async fn exec_with_selection(
    name: &str,
    input: serde_json::Value,
    selection: Vec<String>,
) -> Result<String, String> {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut ctx = omega_tools::ToolCtx::new(dir.path(), "test-call");
    ctx.tool_selection = selection;
    let result = omega_tools::execute_tool(name, input, None, Some(&ctx)).await;
    if result.is_error {
        Err(result.content)
    } else {
        Ok(result.content)
    }
}

// ---------------------------------------------------------------------------
// web_search
// ---------------------------------------------------------------------------

#[tokio::test]
async fn web_search_requires_api_key() {
    if std::env::var("BRAVE_SEARCH_API_KEY").is_ok() {
        return; // skip — key is present, test below covers the live path
    }
    let err = exec("web_search", json!({ "query": "rust programming" }))
        .await
        .unwrap_err();
    assert!(err.contains("BRAVE_SEARCH_API_KEY"), "got: {err}");
}

#[tokio::test]
async fn web_search_live_returns_results() {
    if std::env::var("BRAVE_SEARCH_API_KEY").is_err() {
        // Skip without panic — no key configured.
        return;
    }
    // Soft assertion: live API calls can fail transiently; accept errors.
    match exec(
        "web_search",
        json!({ "query": "Rust programming language" }),
    )
    .await
    {
        Ok(out) => assert!(!out.is_empty(), "expected non-empty output"),
        Err(e) => eprintln!("web_search live test skipped due to API error: {e}"),
    }
}

#[tokio::test]
async fn web_search_empty_query_returns_error() {
    let err = exec("web_search", json!({ "query": "" }))
        .await
        .unwrap_err();
    assert!(!err.is_empty(), "expected error for empty query");
}

// ---------------------------------------------------------------------------
// fetch_url
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fetch_url_downloads_and_postprocesses() {
    // Use a stable, small plain-text URL.
    let out = exec(
        "fetch_url",
        json!({
            "url":         "https://example.com",
            "postprocess": "head -5"
        }),
    )
    .await
    .unwrap();
    assert!(
        out.contains("Cached:"),
        "expected cache notice in output: {out}"
    );
    assert!(
        out.contains("postprocess"),
        "expected postprocess section: {out}"
    );
}

#[tokio::test]
async fn fetch_url_uses_cache_on_second_call() {
    let first = exec(
        "fetch_url",
        json!({
            "url":         "https://example.com",
            "postprocess": "wc -l"
        }),
    )
    .await
    .unwrap();
    let second = exec(
        "fetch_url",
        json!({
            "url":         "https://example.com",
            "postprocess": "wc -l"
        }),
    )
    .await
    .unwrap();
    // Both calls should reference the same cache file path.
    let cache_path_first = first
        .lines()
        .find(|l| l.starts_with("Cached:"))
        .and_then(|l| l.split_once(':').map(|(_, p)| p.trim()));
    let cache_path_second = second
        .lines()
        .find(|l| l.starts_with("Cached:"))
        .and_then(|l| l.split_once(':').map(|(_, p)| p.trim()));
    assert_eq!(
        cache_path_first, cache_path_second,
        "should return same cache file on second call"
    );
}

#[tokio::test]
async fn fetch_url_invalid_url_returns_error() {
    let err = exec(
        "fetch_url",
        json!({
            "url":         "not-a-url",
            "postprocess": "head -1"
        }),
    )
    .await
    .unwrap_err();
    assert!(!err.is_empty(), "expected error for invalid URL");
}

#[tokio::test]
async fn fetch_url_unsupported_scheme_returns_error() {
    // ftp:// is a valid URL but not http/https → must error with a protocol message.
    // Kills the `!= → ==` mutation on the scheme check, which would make every
    // scheme pass through (turning the error into a network failure with a
    // different message).
    let err = exec(
        "fetch_url",
        json!({
            "url":         "ftp://example.com/file.txt",
            "postprocess": "head -1"
        }),
    )
    .await
    .unwrap_err();
    assert!(
        err.contains("unsupported") || err.contains("protocol"),
        "expected protocol error for ftp://, got: {err}"
    );
}

#[tokio::test]
async fn fetch_url_html_is_converted_to_plain_text() {
    // example.com returns text/html.  The cached file must contain plain text
    // (no raw HTML tags), proving html_to_text was applied.
    // Kills:
    //   * `|| → &&` on the HTML content-type check (makes non-xhtml pages skip conversion)
    //   * `html_to_text → String::new()` (empties the cache)
    //   * `html_to_text → "xyzzy"` (replaces content with a sentinel)
    let out = exec(
        "fetch_url",
        json!({
            "url":         "https://example.com",
            "postprocess": "head -3"
        }),
    )
    .await
    .unwrap();

    // Extract the cache file path from the output line "Cached: /path/to/file"
    let cache_path = out
        .lines()
        .find(|l| l.starts_with("Cached:"))
        .and_then(|l| l.split_once(':'))
        .map(|(_, rest)| rest.trim().split(" (").next().unwrap_or("").trim())
        .unwrap_or("");

    assert!(
        !cache_path.is_empty(),
        "expected Cached: line in output: {out}"
    );

    let cached = std::fs::read_to_string(cache_path).unwrap_or_else(|_| String::new());

    // Must be substantial — rules out the `→ String::new()` mutation.
    assert!(
        cached.len() > 20,
        "cached content must not be empty: len={}",
        cached.len()
    );
    // Must not be the "xyzzy" sentinel.
    assert!(
        !cached.contains("xyzzy"),
        "cached content must not be 'xyzzy' sentinel"
    );
    // Must not contain raw HTML tags — proves html_to_text ran.
    assert!(
        !cached.contains("<html") && !cached.contains("</html>"),
        "cached content must be plain text, not HTML: first 200 chars = {}",
        &cached[..200.min(cached.len())]
    );
}

#[tokio::test]
async fn fetch_url_cache_path_is_absolute() {
    // cache_dir() must return a real temp path, not an empty default.
    // Kills the `cache_dir → Box::leak(Box::new(Default::default()))` mutation,
    // which returns an empty PathBuf and makes the cache path relative.
    let out = exec(
        "fetch_url",
        json!({
            "url":         "https://example.com",
            "postprocess": "echo ok"
        }),
    )
    .await
    .unwrap();

    let cache_path = out
        .lines()
        .find(|l| l.starts_with("Cached:"))
        .and_then(|l| l.split_once(':'))
        .map(|(_, rest)| rest.trim().split(" (").next().unwrap_or("").trim())
        .unwrap_or("");

    assert!(
        cache_path.starts_with('/'),
        "cache path must be absolute, got: {cache_path:?}"
    );
}

#[tokio::test]
async fn fetch_url_successful_postprocess_not_marked_as_error() {
    // A postprocess command that exits 0 must not produce an "[error]" prefix.
    // Kills the `&& → ||` mutation on `pp_is_error = code != 0 && code != 1`
    // (with ||, every exit code is treated as an error).
    // Also kills the `!= 0 → == 0` mutation (success treated as error).
    let out = exec(
        "fetch_url",
        json!({
            "url":         "https://example.com",
            "postprocess": "echo SUCCESS_MARKER"
        }),
    )
    .await
    .unwrap();
    assert!(
        !out.contains("[error]"),
        "successful postprocess must not be marked as error: {out}"
    );
    assert!(
        out.contains("SUCCESS_MARKER"),
        "postprocess output must appear in result: {out}"
    );
}

#[tokio::test]
async fn fetch_url_postprocess_killed_by_signal_reports_signal_error() {
    // A bash postprocess that signal-kills itself has no exit code.
    // The output must be an in-band postprocess error — NOT a silent success
    // path that drops the failure on the floor.
    //
    // Kills the `delete -` mutation on the (former) `unwrap_or(-1)` sentinel
    // by removing the sentinel altogether: a `code: Option<i32>` field with
    // `None` for signal-kill collapses both pre- and post-mutation behaviours
    // into a single, observably-correct `[killed by signal]` notice.
    let out = exec(
        "fetch_url",
        json!({
            "url":         "https://example.com",
            // "$" expands inside bash -c to its own PID; this kills the bash
            // postprocess shell with SIGKILL so its `output.status.code()` is None.
            "postprocess": concat!("kill -KILL $", "$")
        }),
    )
    .await
    .unwrap();
    assert!(
        out.contains("[killed by signal]") || out.contains("[error]"),
        "expected signal-kill marker in output: {out}"
    );
    assert!(
        !out.contains("[exit code"),
        "signal-killed bash must not be reported as an exit code: {out}"
    );
}

#[tokio::test]
async fn fetch_url_postprocess_exit_1_not_marked_as_error() {
    // grep exits 1 when there are no matches; this is NOT an error.
    // Kills the `!= 1 → == 1` mutation on `pp_is_error = code != 0 && code != 1`.
    let out = exec(
        "fetch_url",
        json!({
            "url":         "https://example.com",
            "postprocess": "grep XYZZY_NEVER_MATCHES_12345"
        }),
    )
    .await
    .unwrap();
    assert!(
        !out.contains("[exit code 1]"),
        "grep exit-1 (no match) must not be treated as postprocess error: {out}"
    );
}

#[tokio::test]
async fn fetch_url_exactly_max_chars_postprocess_not_truncated() {
    // Exactly PP_CAP (16 000) bytes must NOT be truncated.
    // Kills the `> → >=` mutation (16_000 >= 16_000 = true would wrongly truncate).
    // python3 "print('x' * 15999)" outputs 15 999 'x' + newline = 16 000 bytes.
    let out = exec(
        "fetch_url",
        json!({
            "url":         "https://example.com",
            "postprocess": "python3 -c \"print('x' * 15999)\""
        }),
    )
    .await
    .unwrap();
    assert!(
        !out.contains("truncated"),
        "exactly 16 000 bytes must not be truncated: {out}"
    );
    assert!(
        out.contains("[full output:"),
        "exactly-at-cap output must still carry a tee footer: {out}"
    );
}

#[tokio::test]
async fn fetch_url_large_postprocess_output_is_truncated() {
    // Postprocess output > PP_CAP (16 000 bytes) must be truncated.
    // Kills the `> → ==`, `> → <`, `> → >=` mutations on the truncation guard.
    // python3 "print('Z' * 20000)" outputs 20 000 'Z' + newline = 20 001 bytes.
    let out = exec(
        "fetch_url",
        json!({
            "url":         "https://example.com",
            "postprocess": "python3 -c \"print('Z' * 20000)\""
        }),
    )
    .await
    .unwrap();
    assert!(
        out.contains("truncated"),
        "20 001-byte postprocess output must be truncated: {out}"
    );
    assert!(
        out.contains("Full output:"),
        "truncated output must carry a log-path footer: {out}"
    );
}

#[tokio::test]
async fn fetch_url_small_postprocess_output_not_truncated() {
    // Small output must NOT show a truncation notice, but MUST carry a
    // [full output: ...] footer — the "tee always" property.
    let out = exec(
        "fetch_url",
        json!({
            "url":         "https://example.com",
            "postprocess": "echo hi"
        }),
    )
    .await
    .unwrap();
    assert!(
        !out.contains("truncated"),
        "tiny postprocess output must not show truncation notice: {out}"
    );
    assert!(
        out.contains("[full output:"),
        "tiny postprocess output must still carry a tee footer: {out}"
    );
}

// ---------------------------------------------------------------------------
// fetch_url — shell-gated mode (no_shell_tools = true)
// ---------------------------------------------------------------------------

/// Selection without any shell-execution tool: `fetch_url` is forced into
/// shell-gated mode (postprocess pipeline disabled).
fn selection_no_shell_tools() -> Vec<String> {
    vec![
        "web_search".into(),
        "fetch_url".into(),
        "python_repl".into(),
    ]
}

/// Shell-gated: a successful fetch returns content without running a shell pipeline.
#[tokio::test]
async fn fetch_url_shell_gated_returns_content_without_postprocess() {
    let out = exec_with_selection(
        "fetch_url",
        json!({ "url": "https://example.com" }),
        selection_no_shell_tools(),
    )
    .await
    .unwrap();
    // Should contain the "Cached:" header and some content.
    assert!(
        out.contains("Cached:"),
        "shell-gated result must contain cache path: {out}"
    );
    // Must not contain the postprocess section header.
    assert!(
        !out.contains("--- postprocess:"),
        "shell-gated result must not contain postprocess section: {out}"
    );
}

/// Shell-gated: if a postprocess field is provided in the input despite the schema
/// not listing it (defensive path), it must be ignored — no shell command runs.
#[tokio::test]
async fn fetch_url_shell_gated_ignores_postprocess_field() {
    // Provide a postprocess that would fail loudly if executed.
    let out = exec_with_selection(
        "fetch_url",
        json!({
            "url": "https://example.com",
            "postprocess": "false",  // exits 1; would surface as error in normal mode
        }),
        selection_no_shell_tools(),
    )
    .await
    .unwrap();
    // The call must succeed (not return an error) because the postprocess field
    // was ignored entirely.
    assert!(
        out.contains("Cached:"),
        "shell-gated with ignored postprocess must still succeed: {out}"
    );
    assert!(
        !out.contains("[error]"),
        "shell-gated must not show a postprocess error: {out}"
    );
}

/// Shell-gated: invalid URL returns an error (same guard as normal mode).
#[tokio::test]
async fn fetch_url_shell_gated_invalid_url_returns_error() {
    let err = exec_with_selection(
        "fetch_url",
        json!({ "url": "not-a-url" }),
        selection_no_shell_tools(),
    )
    .await
    .unwrap_err();
    assert!(
        err.contains("invalid URL"),
        "shell-gated invalid URL must produce error: {err}"
    );
}

/// Shell-gated: unsupported scheme returns an error.
#[tokio::test]
async fn fetch_url_shell_gated_unsupported_scheme_returns_error() {
    let err = exec_with_selection(
        "fetch_url",
        json!({ "url": "ftp://example.com/file.txt" }),
        selection_no_shell_tools(),
    )
    .await
    .unwrap_err();
    assert!(
        err.contains("unsupported protocol"),
        "shell-gated unsupported scheme must produce protocol error: {err}"
    );
}

/// Shell-gated: result must not exceed the configured caps.
/// We can't manufacture a large URL easily, so we verify the cap constants
/// are respected by checking the output length on a real small page.
#[tokio::test]
async fn fetch_url_shell_gated_result_is_bounded() {
    let out = exec_with_selection(
        "fetch_url",
        json!({ "url": "https://example.com" }),
        selection_no_shell_tools(),
    )
    .await
    .unwrap();
    // example.com is tiny; result must be well within 50 KB.
    assert!(
        out.len() <= 60_000,
        "shell-gated result must be bounded: {} bytes",
        out.len()
    );
}

// ---------------------------------------------------------------------------
// Wiremock-backed tests (no real network, no #[ignore])
// ---------------------------------------------------------------------------

/// Shell-gated: `text/html` content is converted to plain text.
///
/// The `content_type.contains("text/html") || content_type.contains("application/xhtml")`
/// branch is tested with a mock server that returns `text/html`.  If the `||`
/// were mutated to `&&`, the HTML-to-text conversion would not fire and the raw
/// HTML tags would appear in the output.
#[tokio::test]
async fn fetch_url_shell_gated_html_content_is_converted_to_text() {
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let html_body =
        "<html><head><title>Test Page</title></head><body><p>Hello World</p></body></html>";
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(html_body, "text/html; charset=utf-8"),
        )
        .mount(&server)
        .await;

    let out = exec_with_selection(
        "fetch_url",
        json!({ "url": server.uri() }),
        selection_no_shell_tools(),
    )
    .await
    .expect("shell-gated HTML fetch must succeed");

    // The HTML must have been converted: no raw tags in the body.
    assert!(
        !out.contains("<html>") && !out.contains("<body>") && !out.contains("<p>"),
        "HTML tags must be stripped by conversion: {out}"
    );
    // The text content must be present.
    assert!(
        out.contains("Hello World"),
        "text content must survive HTML-to-text conversion: {out}"
    );
}

/// Normal mode (postprocess): `text/html` content is also converted to plain text.
///
/// Mirrors the shell-gated test for the existing `execute` path so that
/// `content_type.contains("text/html") || content_type.contains("application/xhtml")`
/// is covered in both branches.
#[tokio::test]
async fn fetch_url_normal_html_content_is_converted_to_text() {
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let html_body = "<html><head><title>Test</title></head><body><p>Converted</p></body></html>";
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(html_body, "text/html; charset=utf-8"),
        )
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().expect("tempdir");
    let ctx = omega_tools::ToolCtx::new(dir.path(), "test-html-normal");
    let result = omega_tools::execute_tool(
        "fetch_url",
        json!({ "url": server.uri(), "postprocess": "head -10" }),
        None,
        Some(&ctx),
    )
    .await;
    assert!(
        !result.is_error,
        "normal HTML fetch must succeed: {}",
        result.content
    );
    assert!(
        !result.content.contains("<html>") && !result.content.contains("<p>"),
        "HTML tags must be stripped: {}",
        result.content
    );
    assert!(
        result.content.contains("Converted"),
        "text content must appear: {}",
        result.content
    );
}
