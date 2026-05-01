//! Integration tests for the network tools: web_search, fetch_url.
//!
//! `web_search` is skipped unless `BRAVE_SEARCH_API_KEY` is set.
//! `fetch_url` uses real HTTP and caches to a temp dir per-process.

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
    // Exactly POSTPROCESS_MAX_CHARS (8 000) output must NOT be truncated.
    // Kills the `> → >=` mutation (8000 >= 8000 = true would wrongly truncate).
    // python3 "print('x' * 7999)" outputs 7999 'x' + newline = 8 000 chars.
    let out = exec(
        "fetch_url",
        json!({
            "url":         "https://example.com",
            "postprocess": "python3 -c \"print('x' * 7999)\""
        }),
    )
    .await
    .unwrap();
    assert!(
        !out.contains("truncated"),
        "exactly 8 000 chars must not be truncated: {out}"
    );
}

#[tokio::test]
async fn fetch_url_large_postprocess_output_is_truncated() {
    // Postprocess output > POSTPROCESS_MAX_CHARS (8 000) must be truncated.
    // Kills the `> → ==`, `> → <`, `> → >=` mutations on the truncation guard.
    let out = exec(
        "fetch_url",
        json!({
            "url":         "https://example.com",
            "postprocess": "python3 -c \"print('Z' * 9000)\""
        }),
    )
    .await
    .unwrap();
    assert!(
        out.contains("truncated"),
        "9 000-char postprocess output must be truncated: {out}"
    );
}

#[tokio::test]
async fn fetch_url_small_postprocess_output_not_truncated() {
    // Small output must NOT show a truncation notice.
    // Kills the `> → >=` mutation (which would truncate even at exactly 0 chars).
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
}
