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
    match exec("web_search", json!({ "query": "Rust programming language" })).await {
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
