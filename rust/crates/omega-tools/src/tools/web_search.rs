//! `web_search` — search the web using the Brave Search API.
//!
//! Requires the `BRAVE_SEARCH_API_KEY` environment variable.

use serde_json::Value;
use tokio_util::sync::CancellationToken;

const MAX_OUTPUT_CHARS: usize = 8_000;
const REQUEST_TIMEOUT_S: u64 = 10;

pub async fn execute(input: Value, _cancel: Option<&CancellationToken>) -> Result<String, String> {
    let query = input["query"]
        .as_str()
        .ok_or("web_search: query is required")?
        .trim()
        .to_owned();

    if query.is_empty() {
        return Err("web_search: query must not be empty".into());
    }

    let api_key = std::env::var("BRAVE_SEARCH_API_KEY").map_err(|_| {
        "BRAVE_SEARCH_API_KEY is not set. Web search requires a Brave Search API key.".to_owned()
    })?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(REQUEST_TIMEOUT_S))
        .build()
        .map_err(|e| format!("web_search: failed to build HTTP client: {e}"))?;

    let response = client
        .get("https://api.search.brave.com/res/v1/web/search")
        .query(&[
            ("q", query.as_str()),
            ("count", "10"),
            ("text_decorations", "0"),
            ("search_lang", "en"),
        ])
        .header("Accept", "application/json")
        .header("X-Subscription-Token", &api_key)
        .send()
        .await
        .map_err(|e| format!("web_search: request failed: {e}"))?;

    check_status(response.status())?;

    let data: Value = response
        .json()
        .await
        .map_err(|e| format!("web_search: failed to parse response: {e}"))?;

    Ok(render_results(&data))
}

// ---------------------------------------------------------------------------
// Pure helpers — split out so they can be unit-tested without a live API key
// or a mock HTTP server. These exist so cargo-mutants can attack the boundary
// logic (status check, truncation threshold) at function granularity.
// ---------------------------------------------------------------------------

/// Convert a Brave API HTTP status into either `Ok(())` (2xx) or a formatted
/// `Err(String)` describing the failure. Pure; no I/O.
fn check_status(status: reqwest::StatusCode) -> Result<(), String> {
    if !status.is_success() {
        return Err(format!("Brave Search API error: HTTP {status}"));
    }
    Ok(())
}

/// Render the parsed Brave response body into the user-facing string,
/// applying the [`MAX_OUTPUT_CHARS`] truncation guard. Pure; no I/O.
fn render_results(data: &Value) -> String {
    let results = data["web"]["results"]
        .as_array()
        .cloned()
        .unwrap_or_default();

    if results.is_empty() {
        return "No results found.".to_owned();
    }

    let mut lines = vec!["Results:".to_owned()];
    for r in &results {
        if let Some(url) = r["url"].as_str() {
            lines.push(format!("• {url}"));
        }
        if let Some(title) = r["title"].as_str() {
            lines.push(format!("  {title}"));
        }
        if let Some(desc) = r["description"].as_str() {
            lines.push(format!("  {desc}"));
        }
    }

    let output = lines.join("\n");
    if output.chars().count() > MAX_OUTPUT_CHARS {
        let end = output
            .char_indices()
            .nth(MAX_OUTPUT_CHARS)
            .map_or(output.len(), |(i, _)| i);
        return format!("{}\n[truncated]", &output[..end]);
    }

    output
}

#[cfg(test)]
mod tests {
    use super::{MAX_OUTPUT_CHARS, check_status, render_results};
    use serde_json::json;

    // -- check_status --------------------------------------------------------

    #[test]
    fn check_status_ok_for_2xx() {
        // Kills the `delete !` mutation (which would invert the success path).
        assert!(check_status(reqwest::StatusCode::OK).is_ok());
        assert!(check_status(reqwest::StatusCode::CREATED).is_ok());
        assert!(check_status(reqwest::StatusCode::NO_CONTENT).is_ok());
    }

    #[test]
    fn check_status_err_for_non_2xx() {
        // Kills the `delete !` mutation (with `!` removed, 5xx would be Ok).
        let err = check_status(reqwest::StatusCode::INTERNAL_SERVER_ERROR).unwrap_err();
        assert!(err.contains("Brave"), "got: {err}");
        assert!(err.contains("500"), "got: {err}");
    }

    #[test]
    fn check_status_err_for_4xx() {
        let err = check_status(reqwest::StatusCode::UNAUTHORIZED).unwrap_err();
        assert!(err.contains("401"), "got: {err}");
    }

    // -- render_results: empty / shape ---------------------------------------

    #[test]
    fn render_results_empty_returns_no_results_marker() {
        assert_eq!(render_results(&json!({})), "No results found.");
        assert_eq!(
            render_results(&json!({ "web": { "results": [] } })),
            "No results found."
        );
    }

    #[test]
    fn render_results_single_result_includes_all_fields() {
        let data = json!({
            "web": { "results": [
                { "url": "https://example.com", "title": "Example", "description": "A demo site." }
            ]}
        });
        let out = render_results(&data);
        assert!(out.starts_with("Results:"), "{out}");
        assert!(out.contains("https://example.com"), "{out}");
        assert!(out.contains("Example"), "{out}");
        assert!(out.contains("A demo site."), "{out}");
    }

    // -- render_results: truncation boundary ---------------------------------

    /// Build a response whose rendered output is approximately `target_chars`
    /// characters long. Each result contributes a known number of chars; we
    /// pad the description of the last result to land on the requested length.
    fn rendered_of_length(target_chars: usize) -> serde_json::Value {
        // "Results:" header plus one result: "• u\n  t\n  d".
        // We use a single result and tune `description` to hit the target.
        let prefix = "Results:\n• u\n  t\n  "; // count chars (• is one char)
        let prefix_chars = prefix.chars().count();
        assert!(target_chars > prefix_chars, "target too small");
        let desc_len = target_chars - prefix_chars;
        let desc = "x".repeat(desc_len);
        json!({
            "web": { "results": [
                { "url": "u", "title": "t", "description": desc }
            ]}
        })
    }

    #[test]
    fn render_results_exactly_max_chars_not_truncated() {
        // Pins the strict `>` boundary: exactly MAX_OUTPUT_CHARS must NOT
        // truncate. Kills the `> → >=` mutation (which would truncate at
        // equality) and `> → ==` (which truncates only at exact equality).
        let data = rendered_of_length(MAX_OUTPUT_CHARS);
        let out = render_results(&data);
        assert_eq!(
            out.chars().count(),
            MAX_OUTPUT_CHARS,
            "setup: rendered length must equal MAX_OUTPUT_CHARS"
        );
        assert!(
            !out.contains("[truncated]"),
            "exactly MAX_OUTPUT_CHARS must not be truncated"
        );
    }

    #[test]
    fn render_results_above_max_chars_is_truncated() {
        // Output length > MAX_OUTPUT_CHARS must trigger truncation.
        // Kills the `> → <` mutation (which would never truncate large output)
        // and `> → ==` (which only truncates at exact equality).
        let data = rendered_of_length(MAX_OUTPUT_CHARS + 100);
        let out = render_results(&data);
        assert!(
            out.contains("[truncated]"),
            "output > MAX_OUTPUT_CHARS must be truncated, got len={}",
            out.chars().count()
        );
    }

    #[test]
    fn render_results_well_below_max_chars_not_truncated() {
        // A small response must never carry a truncation notice.
        // Kills the `> → <` and `> → ==` mutations: with `<`, every short
        // output would be truncated; with `==`, nothing would be unless the
        // length matched exactly.
        let data = json!({
            "web": { "results": [
                { "url": "https://example.com", "title": "t", "description": "d" }
            ]}
        });
        let out = render_results(&data);
        assert!(!out.contains("[truncated]"), "short output: {out}");
    }
}
