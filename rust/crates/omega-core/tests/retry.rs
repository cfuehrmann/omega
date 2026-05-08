//! Integration tests for [`RetryingProvider`] driven through the real
//! [`AnthropicProvider`] / [`OllamaProvider`] stack against a `wiremock`
//! server.
//!
//! These tests replace the in-module `ScriptedProvider` unit tests
//! that previously lived in `src/retry.rs` (Phase 1b.6).  Driving the
//! retry loop through a real provider exercises the composition seam
//! that the scripted tests skipped — if a provider produces an error
//! shape `RetryingProvider` doesn't recognise, the tests catch it.
//!
//! Sequential responses are scripted via `Mock::up_to_n_times(1)`
//! mounted in order: wiremock matches the first un-exhausted mock, so
//! attempt #1 hits the first mounted mock and attempt #2 falls through
//! to the next.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use futures::StreamExt;
use omega_core::{
    AgentItem, AnthropicProvider, LlmError, LlmRequest, OllamaProvider, Provider, RetryingProvider,
};
use omega_types::{LlmRetryReason, OmegaEvent};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

mod common;
use common::{
    fast_retry_config, fast_retry_config_with_jitter, minimal_anthropic_sse, minimal_ollama_ndjson,
    simple_request, sse_body,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// `LlmRequest` for the Anthropic-flavoured tests.
fn anthropic_request() -> LlmRequest {
    simple_request("claude-sonnet-4-6")
}

/// `LlmRequest` for the Ollama-flavoured cross-check.
fn ollama_request() -> LlmRequest {
    simple_request("llama3.2")
}

/// A `RetryingProvider` wrapping `AnthropicProvider` pointed at `server`,
/// configured with [`fast_retry_config`].
fn anthropic_with_retry(
    server: &MockServer,
    max_attempts: u32,
) -> RetryingProvider<AnthropicProvider> {
    RetryingProvider::new(
        AnthropicProvider::new("test-key").with_base_url(server.uri()),
        fast_retry_config(max_attempts),
    )
}

/// Mount an Anthropic SSE success response (one text token, then
/// `message_stop`).  Matches up to `n_times` calls, after which the
/// next mounted mock takes over.
async fn mount_anthropic_success(server: &MockServer, n_times: u64) {
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(minimal_anthropic_sse())
                .insert_header("content-type", "text/event-stream"),
        )
        .up_to_n_times(n_times)
        .mount(server)
        .await;
}

/// Mount a fixed HTTP error response with optional `retry-after` header
/// and a JSON body.  Matches up to `n_times` calls.
async fn mount_anthropic_http_error(
    server: &MockServer,
    status: u16,
    body: &str,
    retry_after: Option<&str>,
    n_times: u64,
) {
    let mut tpl = ResponseTemplate::new(status).set_body_string(body);
    if let Some(v) = retry_after {
        tpl = tpl.insert_header("retry-after", v);
    }
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(tpl)
        .up_to_n_times(n_times)
        .mount(server)
        .await;
}

/// Drain a stream into a vector — fails the test on an unexpected
/// terminal error, but tolerates retry events (which surface as
/// `Ok(LlmRetry)` items).
async fn collect_all(
    provider: &RetryingProvider<AnthropicProvider>,
    req: LlmRequest,
) -> Vec<Result<AgentItem, LlmError>> {
    provider.stream(req).collect().await
}

/// Filter `LlmRetry` events out of a result vec.
fn retry_events(items: &[Result<AgentItem, LlmError>]) -> Vec<&omega_types::events::LlmRetryEvent> {
    items
        .iter()
        .filter_map(|r| r.as_ref().ok())
        .filter_map(AgentItem::as_event)
        .filter_map(|e| match e {
            OmegaEvent::LlmRetry(ev) => Some(ev),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Happy path: a clean stream wrapped in retry passes through unchanged.
// ---------------------------------------------------------------------------

/// Composition smoke test: when the inner provider succeeds on the
/// first try, [`RetryingProvider`] forwards every item without
/// inserting `LlmRetry` events.
#[tokio::test]
async fn passes_through_a_clean_stream() {
    let server = MockServer::start().await;
    mount_anthropic_success(&server, u64::MAX).await;

    let provider = anthropic_with_retry(&server, 3);
    let items = collect_all(&provider, anthropic_request()).await;

    // No retry events; a single Text signal + LlmResponse event.
    assert!(retry_events(&items).is_empty(), "no retry should fire");
    assert!(
        items.iter().all(Result::is_ok),
        "all items must be Ok: {items:?}"
    );
    let response_count = items
        .iter()
        .filter_map(|r| r.as_ref().ok())
        .filter_map(AgentItem::as_event)
        .filter(|e| matches!(e, OmegaEvent::LlmResponse(_)))
        .count();
    assert_eq!(response_count, 1, "exactly one LlmResponse event");
}

// ---------------------------------------------------------------------------
// 529 → retry → success
// ---------------------------------------------------------------------------

#[tokio::test]
async fn retries_a_529_then_succeeds() {
    let server = MockServer::start().await;
    mount_anthropic_http_error(
        &server,
        529,
        r#"{"type":"error","error":{"type":"overloaded_error"}}"#,
        None,
        1,
    )
    .await;
    mount_anthropic_success(&server, u64::MAX).await;

    let provider = anthropic_with_retry(&server, 3);
    let items = collect_all(&provider, anthropic_request()).await;

    let retries = retry_events(&items);
    assert_eq!(retries.len(), 1, "exactly one LlmRetry event");
    assert_eq!(retries[0].attempt, 1);
    assert_eq!(retries[0].http_status, Some(529));
    assert!(retries[0].wait_ms >= 1);
    assert!(retries[0].text_fragment.is_none());

    let response_count = items
        .iter()
        .filter_map(|r| r.as_ref().ok())
        .filter_map(AgentItem::as_event)
        .filter(|e| matches!(e, OmegaEvent::LlmResponse(_)))
        .count();
    assert_eq!(response_count, 1, "second attempt's LlmResponse propagates");
}

// ---------------------------------------------------------------------------
// Provider-agnostic check: same retry behaviour through Ollama.
// ---------------------------------------------------------------------------

/// Smoke test against `OllamaProvider` to ensure the retry policy
/// is provider-agnostic.  All other retry tests use `AnthropicProvider`
/// because the retry loop only reads `LlmError::status`/`is_retryable`/
/// `retry_after`, which both providers populate identically.
#[tokio::test]
async fn retries_a_500_then_succeeds_with_ollama() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(ResponseTemplate::new(500).set_body_string("ollama down"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(minimal_ollama_ndjson())
                .insert_header("content-type", "application/x-ndjson"),
        )
        .mount(&server)
        .await;

    let provider = RetryingProvider::new(
        OllamaProvider::new().with_base_url(server.uri()),
        fast_retry_config(3),
    );
    let items: Vec<_> = provider.stream(ollama_request()).collect().await;

    let retries: Vec<_> = items
        .iter()
        .filter_map(|r| r.as_ref().ok())
        .filter_map(AgentItem::as_event)
        .filter_map(|e| match e {
            OmegaEvent::LlmRetry(r) => Some(r),
            _ => None,
        })
        .collect();
    assert_eq!(retries.len(), 1, "exactly one LlmRetry event");
    assert_eq!(retries[0].http_status, Some(500));

    let response_count = items
        .iter()
        .filter_map(|r| r.as_ref().ok())
        .filter_map(AgentItem::as_event)
        .filter(|e| matches!(e, OmegaEvent::LlmResponse(_)))
        .count();
    assert_eq!(response_count, 1);
}

// ---------------------------------------------------------------------------
// Non-retryable: 400 surfaces as a terminal error
// ---------------------------------------------------------------------------

#[tokio::test]
async fn does_not_retry_a_400() {
    let server = MockServer::start().await;
    mount_anthropic_http_error(
        &server,
        400,
        r#"{"type":"error","error":{"type":"invalid_request_error"}}"#,
        None,
        u64::MAX,
    )
    .await;

    let provider = anthropic_with_retry(&server, 3);
    let items = collect_all(&provider, anthropic_request()).await;

    assert!(retry_events(&items).is_empty(), "400 must not retry");
    assert_eq!(items.len(), 1);
    assert!(matches!(items[0], Err(LlmError::Http { status: 400, .. })));
}

// ---------------------------------------------------------------------------
// Non-retryable: 429 with "context too long" body is terminal
// ---------------------------------------------------------------------------

#[tokio::test]
async fn does_not_retry_context_too_long_429() {
    let server = MockServer::start().await;
    let body = r#"{"error":"Extra usage is required for long context requests"}"#;
    mount_anthropic_http_error(&server, 429, body, None, u64::MAX).await;

    let provider = anthropic_with_retry(&server, 3);
    let items = collect_all(&provider, anthropic_request()).await;

    assert!(
        retry_events(&items).is_empty(),
        "context-too-long 429 must not retry"
    );
    assert_eq!(items.len(), 1);
    assert!(matches!(items[0], Err(LlmError::Http { status: 429, .. })));
}

// ---------------------------------------------------------------------------
// Gives up after `max_attempts`
// ---------------------------------------------------------------------------

#[tokio::test]
async fn gives_up_after_max_attempts() {
    let server = MockServer::start().await;
    // Always 529 — every attempt fails.
    mount_anthropic_http_error(
        &server,
        529,
        r#"{"type":"error","error":{"type":"overloaded_error"}}"#,
        None,
        u64::MAX,
    )
    .await;

    let provider = anthropic_with_retry(&server, 3);
    let items = collect_all(&provider, anthropic_request()).await;

    // 3 attempts total: initial + 2 retries → 2 LlmRetry events.
    assert_eq!(retry_events(&items).len(), 2);
    // Final item is the terminal HTTP error.
    assert!(matches!(
        items.last(),
        Some(Err(LlmError::Http { status: 529, .. }))
    ));
}

// ---------------------------------------------------------------------------
// `text_fragment` is populated when text streamed before the error
// ---------------------------------------------------------------------------

/// To exercise the "text streamed before error" branch through a real
/// provider we need a *mid-stream* retryable failure.  An HTTP 529
/// happens at response-status time so no text can have been yielded;
/// instead we use Anthropic's `event: error` SSE event with an
/// `overloaded_error` payload, which the provider lifts to
/// `LlmError::Stream { message }` whose `message` contains
/// `"overloaded_error"` — making it retryable per `is_retryable`.
#[tokio::test]
async fn populates_text_fragment_when_text_streamed_before_error() {
    let server = MockServer::start().await;

    // First response: a text delta then an SSE error event (mid-stream).
    let body_with_error = sse_body(&[
        (
            "message_start",
            json!({
                "type": "message_start",
                "message": {
                    "id": "msg_partial",
                    "model": "claude-sonnet-4-6",
                    "usage": { "input_tokens": 1, "output_tokens": 0 }
                }
            }),
        ),
        (
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": { "type": "text", "text": "" }
            }),
        ),
        (
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": { "type": "text_delta", "text": "partial " }
            }),
        ),
        (
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": { "type": "text_delta", "text": "answer" }
            }),
        ),
        (
            "error",
            json!({
                "type": "error",
                "error": { "type": "overloaded_error", "message": "server overloaded" }
            }),
        ),
    ]);

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(body_with_error)
                .insert_header("content-type", "text/event-stream"),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    mount_anthropic_success(&server, u64::MAX).await;

    let provider = anthropic_with_retry(&server, 3);
    let items = collect_all(&provider, anthropic_request()).await;

    let retries = retry_events(&items);
    assert_eq!(retries.len(), 1, "exactly one retry");
    assert_eq!(
        retries[0].text_fragment.as_deref(),
        Some("partial answer"),
        "text streamed before error should appear in text_fragment"
    );
    assert!(retries[0].thinking_fragment.is_none());
}

// ---------------------------------------------------------------------------
// `Retry-After` header is honoured
// ---------------------------------------------------------------------------

#[tokio::test]
async fn honours_retry_after_header() {
    let server = MockServer::start().await;
    // retry-after: 0 keeps the test fast — the wait_ms must still be 0
    // and the `reason` must be `RetryAfter` rather than the default.
    mount_anthropic_http_error(&server, 429, "{}", Some("0"), 1).await;
    mount_anthropic_success(&server, u64::MAX).await;

    let provider = anthropic_with_retry(&server, 3);
    let items = collect_all(&provider, anthropic_request()).await;

    let retries = retry_events(&items);
    assert_eq!(retries.len(), 1);
    assert_eq!(retries[0].reason, Some(LlmRetryReason::RetryAfter));
    assert_eq!(retries[0].wait_ms, 0);
}

// ---------------------------------------------------------------------------
// SSE `overloaded_error` event without HTTP status → retried
// ---------------------------------------------------------------------------

#[tokio::test]
async fn retries_overloaded_error_payload_without_status() {
    let server = MockServer::start().await;
    // First response is HTTP 200 but its only SSE event is a
    // `overloaded_error` — which the provider surfaces as
    // `LlmError::Stream` whose body contains "overloaded_error".
    let body = sse_body(&[(
        "error",
        json!({
            "type": "error",
            "error": { "type": "overloaded_error", "message": "server overloaded" }
        }),
    )]);
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(body)
                .insert_header("content-type", "text/event-stream"),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    mount_anthropic_success(&server, u64::MAX).await;

    let provider = anthropic_with_retry(&server, 3);
    let items = collect_all(&provider, anthropic_request()).await;

    let retries = retry_events(&items);
    assert_eq!(retries.len(), 1, "overloaded_error must trigger retry");
    // No HTTP status because the failure was inside the SSE body.
    assert!(retries[0].http_status.is_none());
}

// ---------------------------------------------------------------------------
// Transport error (TCP close before HTTP response) → retried
// ---------------------------------------------------------------------------

/// Spawn a TCP listener that drops the first incoming connection
/// without writing any bytes, then serves `success_response` on
/// subsequent connections.  Returned address is bound to `127.0.0.1`.
///
/// Closing the socket before any HTTP response causes hyper (under
/// reqwest) to surface an `IncompleteMessage`, which the provider's
/// `.send().await` mapping converts into `LlmError::Transport`.
async fn flaky_listener(success_response: Vec<u8>) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let resp = Arc::new(success_response);
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = match listener.accept().await {
                Ok(c) => c,
                Err(_) => return,
            };
            let n = count.fetch_add(1, Ordering::SeqCst);
            let resp = resp.clone();
            tokio::spawn(async move {
                if n == 0 {
                    // First attempt: drop without writing → transport error.
                    drop(stream);
                    return;
                }
                // Drain the request (best-effort, with timeout) so the
                // client's `send()` future resolves; then write a
                // hand-rolled HTTP response with `Connection: close`.
                let mut buf = vec![0u8; 16384];
                let _ =
                    tokio::time::timeout(Duration::from_millis(200), stream.read(&mut buf)).await;
                let _ = stream.write_all(&resp).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    addr
}

/// Build a raw HTTP/1.1 response carrying an SSE body, terminated by
/// `Connection: close` (no `Content-Length` / chunked encoding —
/// reqwest reads until EOF).
fn raw_http_sse(body: &str) -> Vec<u8> {
    let resp = format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: text/event-stream\r\n\
         Connection: close\r\n\
         \r\n\
         {body}"
    );
    resp.into_bytes()
}

#[tokio::test]
async fn retries_transport_errors() {
    // A real `LlmError::Transport` is produced when reqwest's `.send()`
    // resolves with a hyper-level connection error — for example, the
    // server accepting the TCP connection and then closing it before
    // sending any HTTP response.  Both providers map this case via
    //     `.send().await.map_err(|e| LlmError::Transport { … })`.
    let success_body = raw_http_sse(&minimal_anthropic_sse());
    let addr = flaky_listener(success_body).await;
    let base_url = format!("http://{addr}");

    let provider = RetryingProvider::new(
        AnthropicProvider::new("test-key").with_base_url(base_url),
        fast_retry_config(3),
    );
    let items: Vec<_> = provider.stream(anthropic_request()).collect().await;

    let retries: Vec<_> = items
        .iter()
        .filter_map(|r| r.as_ref().ok())
        .filter_map(AgentItem::as_event)
        .filter_map(|e| match e {
            OmegaEvent::LlmRetry(r) => Some(r),
            _ => None,
        })
        .collect();
    assert_eq!(retries.len(), 1, "transport error must trigger one retry");
    assert!(
        retries[0].http_status.is_none(),
        "transport errors carry no HTTP status"
    );
    let response_count = items
        .iter()
        .filter_map(|r| r.as_ref().ok())
        .filter_map(AgentItem::as_event)
        .filter(|e| matches!(e, OmegaEvent::LlmResponse(_)))
        .count();
    assert_eq!(response_count, 1, "second attempt must succeed");
}

// ---------------------------------------------------------------------------
// `error_body` populated from the JSON HTTP body
// ---------------------------------------------------------------------------

#[tokio::test]
async fn error_body_populated_from_http_body() {
    let server = MockServer::start().await;
    mount_anthropic_http_error(
        &server,
        529,
        r#"{"type":"error","error":{"type":"overloaded_error"}}"#,
        None,
        1,
    )
    .await;
    mount_anthropic_success(&server, u64::MAX).await;

    let provider = anthropic_with_retry(&server, 3);
    let items = collect_all(&provider, anthropic_request()).await;

    let retries = retry_events(&items);
    assert_eq!(retries.len(), 1);
    let body = retries[0]
        .error_body
        .as_ref()
        .expect("error_body must be populated for JSON HTTP body");
    assert_eq!(body["type"], "error");
    assert_eq!(body["error"]["type"], "overloaded_error");
}

// ---------------------------------------------------------------------------
// `retry_at` is never earlier than `time`
// ---------------------------------------------------------------------------

#[tokio::test]
async fn retry_at_is_not_before_event_time() {
    let server = MockServer::start().await;
    mount_anthropic_http_error(
        &server,
        529,
        r#"{"type":"error","error":{"type":"overloaded_error"}}"#,
        None,
        1,
    )
    .await;
    mount_anthropic_success(&server, u64::MAX).await;

    let provider = anthropic_with_retry(&server, 3);
    let items = collect_all(&provider, anthropic_request()).await;

    let retries = retry_events(&items);
    assert_eq!(retries.len(), 1);
    let time =
        chrono::DateTime::parse_from_rfc3339(&retries[0].time).expect("time must be RFC3339");
    let retry_at_str = retries[0].retry_at.as_ref().expect("retry_at must be set");
    let retry_at =
        chrono::DateTime::parse_from_rfc3339(retry_at_str).expect("retry_at must be RFC3339");
    assert!(
        retry_at >= time,
        "retry_at ({retry_at}) must be >= time ({time})"
    );
}

// ---------------------------------------------------------------------------
// Backoff grows with each successive attempt
// ---------------------------------------------------------------------------

#[tokio::test]
async fn backoff_grows_on_second_attempt() {
    let server = MockServer::start().await;
    // Two consecutive 529s, then success on attempt 3.
    mount_anthropic_http_error(
        &server,
        529,
        r#"{"type":"error","error":{"type":"overloaded_error"}}"#,
        None,
        2,
    )
    .await;
    mount_anthropic_success(&server, u64::MAX).await;

    let provider = anthropic_with_retry(&server, 4);
    let items = collect_all(&provider, anthropic_request()).await;

    let retries = retry_events(&items);
    assert_eq!(retries.len(), 2, "expected exactly 2 retry events");
    assert!(
        retries[1].wait_ms > retries[0].wait_ms,
        "second backoff ({}) must exceed first ({})",
        retries[1].wait_ms,
        retries[0].wait_ms
    );
}

// ---------------------------------------------------------------------------
// Jitter at 1 ms base rounds to 1 ms (catches `* with +` mutant)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn jitter_rounds_to_base_ms_not_double() {
    let server = MockServer::start().await;
    mount_anthropic_http_error(
        &server,
        529,
        r#"{"type":"error","error":{"type":"overloaded_error"}}"#,
        None,
        1,
    )
    .await;
    mount_anthropic_success(&server, u64::MAX).await;

    let provider = RetryingProvider::new(
        AnthropicProvider::new("test-key").with_base_url(server.uri()),
        fast_retry_config_with_jitter(3),
    );
    let items: Vec<Result<AgentItem, LlmError>> =
        provider.stream(anthropic_request()).collect().await;

    let retries: Vec<_> = items
        .iter()
        .filter_map(|r| r.as_ref().ok())
        .filter_map(AgentItem::as_event)
        .filter_map(|e| match e {
            OmegaEvent::LlmRetry(r) => Some(r),
            _ => None,
        })
        .collect();
    assert_eq!(retries.len(), 1);
    // Correct: round(1 * [0.9, 1.1]) = 1.  `+ jitter` mutant: round(1 + [0.9, 1.1]) = 2.
    assert_eq!(
        retries[0].wait_ms, 1,
        "jitter on 1ms base must round to 1ms, got {}",
        retries[0].wait_ms
    );
}
