//! `web_search` — search the web using the Brave Search API.
//!
//! Requires the `BRAVE_SEARCH_API_KEY` environment variable.

use serde_json::Value;
use tokio_util::sync::CancellationToken;

const MAX_OUTPUT_CHARS: usize = 8_000;
const REQUEST_TIMEOUT_S: u64 = 10;

pub async fn execute(
    input: Value,
    _cancel: Option<&CancellationToken>,
) -> Result<String, String> {
    let query = input["query"]
        .as_str()
        .ok_or("web_search: query is required")?
        .trim()
        .to_owned();

    if query.is_empty() {
        return Err("web_search: query must not be empty".into());
    }

    let api_key = std::env::var("BRAVE_SEARCH_API_KEY").map_err(|_| {
        "BRAVE_SEARCH_API_KEY is not set. Web search requires a Brave Search API key."
            .to_owned()
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

    if !response.status().is_success() {
        return Err(format!(
            "Brave Search API error: HTTP {}",
            response.status()
        ));
    }

    let data: Value = response
        .json()
        .await
        .map_err(|e| format!("web_search: failed to parse response: {e}"))?;

    let results = data["web"]["results"]
        .as_array()
        .cloned()
        .unwrap_or_default();

    if results.is_empty() {
        return Ok("No results found.".to_owned());
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
        return Ok(format!("{}\n[truncated]", &output[..end]));
    }

    Ok(output)
}
