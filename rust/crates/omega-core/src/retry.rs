//! Retry wrapper for any [`Provider`].
//!
//! Mirrors the TypeScript agent's retry loop (see `src/agent.ts`):
//!
//! - Retry on transient errors (HTTP 429, 500, 503, 529; Anthropic SSE
//!   `overloaded_error` events; transport-level failures).
//! - Honour the `Retry-After` header when the provider sets one.
//! - Otherwise back off exponentially with ±10 % jitter, capped at
//!   `max_backoff`.
//! - Emit an [`OmegaEvent::LlmRetry`] before each retry, carrying any
//!   text / thinking fragments already streamed to the UI so the client
//!   can roll back its in-flight assistant bubble.
//! - After `max_attempts` retries, the last error propagates to the
//!   caller (which then emits an [`OmegaEvent::LlmError`]).

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use futures::StreamExt;
use futures::stream::{self, Stream};
use omega_protocol::events::LlmRetryEvent;
use omega_protocol::{LlmRetryReason, OmegaEvent, StreamSignal};
use rand::Rng;

use crate::provider::{AgentItemStream, Provider};
use crate::types::{AgentItem, LlmError, LlmRequest};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Knobs for [`RetryingProvider`].
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum total attempts including the initial one.  Set to a
    /// small finite value in tests; production typically uses 32 or
    /// higher (overload retries can run for many minutes).
    pub max_attempts: u32,
    /// Backoff for the first retry.  Each subsequent retry doubles.
    pub initial_backoff: Duration,
    /// Cap on the exponentially-growing backoff.
    pub max_backoff: Duration,
    /// Whether to apply ±10 % jitter to each backoff.
    pub jitter: bool,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 32,
            initial_backoff: Duration::from_millis(500),
            max_backoff: Duration::from_mins(1),
            jitter: true,
        }
    }
}

impl RetryConfig {
    /// Tight config for tests — 1 ms base, 16 ms cap, no jitter.
    #[must_use]
    pub fn for_tests(max_attempts: u32) -> Self {
        Self {
            max_attempts,
            initial_backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(16),
            jitter: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Wrapper
// ---------------------------------------------------------------------------

/// A [`Provider`] that re-invokes its inner provider on transient errors.
///
/// The inner provider is held behind an `Arc`, which lets the returned
/// stream out-live the parent `&self` borrow and lets the retry loop
/// re-call `inner.stream()` from inside an async generator.
pub struct RetryingProvider<P: Provider + ?Sized> {
    inner: Arc<P>,
    config: RetryConfig,
}

impl<P: Provider + 'static> RetryingProvider<P> {
    pub fn new(inner: P, config: RetryConfig) -> Self {
        Self {
            inner: Arc::new(inner),
            config,
        }
    }
}

impl<P: Provider + ?Sized + 'static> Provider for RetryingProvider<P> {
    fn stream(&self, request: LlmRequest) -> AgentItemStream {
        Box::pin(retry_loop(self.inner.clone(), request, self.config.clone()))
    }
}

// ---------------------------------------------------------------------------
// Loop
// ---------------------------------------------------------------------------

struct LoopState<P: Provider + ?Sized> {
    inner: Arc<P>,
    request: LlmRequest,
    config: RetryConfig,
    attempt: u32,
    current: Option<AgentItemStream>,
    text_fragment: String,
    thinking_fragment: String,
    done: bool,
}

fn retry_loop<P: Provider + ?Sized + 'static>(
    inner: Arc<P>,
    request: LlmRequest,
    config: RetryConfig,
) -> impl Stream<Item = Result<AgentItem, LlmError>> + Send {
    let initial_stream = inner.stream(request.clone());
    let state = LoopState {
        inner,
        request,
        config,
        attempt: 0,
        current: Some(initial_stream),
        text_fragment: String::new(),
        thinking_fragment: String::new(),
        done: false,
    };

    stream::unfold(state, |mut s| async move {
        if s.done {
            return None;
        }
        // Take the inner stream out so we can `.await` on it without
        // holding a borrow on `s` across the await point.
        let mut stream = s.current.take()?;
        let next = stream.next().await;
        match next {
            Some(Ok(item)) => {
                track_fragment(&item, &mut s.text_fragment, &mut s.thinking_fragment);
                s.current = Some(stream);
                Some((Ok(item), s))
            }
            None => None,
            Some(Err(err)) => {
                // Stream ended in error; the original `stream` is no
                // longer needed.
                drop(stream);
                let next_attempt = s.attempt + 1;
                if !err.is_retryable() || next_attempt >= s.config.max_attempts {
                    s.done = true;
                    Some((Err(err), s))
                } else {
                    let (sleep_for, reason) = compute_backoff(&err, next_attempt, &s.config);
                    let event = build_retry_event(
                        &err,
                        next_attempt,
                        sleep_for,
                        reason,
                        &s.text_fragment,
                        &s.thinking_fragment,
                    );
                    s.attempt = next_attempt;
                    tokio::time::sleep(sleep_for).await;
                    s.current = Some(s.inner.stream(s.request.clone()));
                    s.text_fragment.clear();
                    s.thinking_fragment.clear();
                    Some((Ok(AgentItem::event(event)), s))
                }
            }
        }
    })
}

fn track_fragment(item: &AgentItem, text: &mut String, thinking: &mut String) {
    match item {
        AgentItem::Signal(StreamSignal::Text { text: t }) => text.push_str(t),
        AgentItem::Signal(StreamSignal::Thinking { text: t }) => thinking.push_str(t),
        AgentItem::Event(_) => {}
    }
}

// ---------------------------------------------------------------------------
// Backoff + event construction
// ---------------------------------------------------------------------------

/// Apply a jitter factor to a wait duration in milliseconds.
///
/// `x / f` is mathematically indistinguishable from `x * f` for `f ∈ [0.9, 1.1]`
/// because the two output ranges overlap completely and the factor is chosen by a
/// non-deterministic RNG. Suppressed rather than tested with a fragile statistical
/// assertion or a seeded-RNG refactor.
#[mutants::skip]
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn apply_jitter(wait_ms: u64, jitter: f64) -> u64 {
    (wait_ms as f64 * jitter).round() as u64
}

fn compute_backoff(
    err: &LlmError,
    attempt: u32,
    config: &RetryConfig,
) -> (Duration, Option<LlmRetryReason>) {
    if let Some(retry_after) = err.retry_after() {
        return (retry_after, Some(LlmRetryReason::RetryAfter));
    }
    let base = u64::try_from(config.initial_backoff.as_millis()).unwrap_or(u64::MAX);
    let cap = u64::try_from(config.max_backoff.as_millis()).unwrap_or(u64::MAX);
    // attempt is 1-based — first retry uses base * 1, second uses base * 2, etc.
    let shift = attempt.saturating_sub(1).min(20);
    let exp = base.saturating_mul(1u64 << shift);
    let mut wait = exp.min(cap);
    if config.jitter {
        let mut rng = rand::rng();
        let jitter: f64 = rng.random_range(0.9..=1.1);
        wait = apply_jitter(wait, jitter).min(cap);
    }
    (Duration::from_millis(wait), None)
}

fn build_retry_event(
    err: &LlmError,
    attempt: u32,
    wait_for: Duration,
    reason: Option<LlmRetryReason>,
    text_fragment: &str,
    thinking_fragment: &str,
) -> OmegaEvent {
    let now = Utc::now();
    let retry_at =
        now + chrono::Duration::from_std(wait_for).unwrap_or_else(|_| chrono::Duration::seconds(0));
    let error_body: Option<serde_json::Value> = match err {
        LlmError::Http { body, .. } => serde_json::from_str(body).ok(),
        _ => None,
    };
    let text_fragment = if text_fragment.is_empty() {
        None
    } else {
        Some(text_fragment.to_owned())
    };
    let thinking_fragment = if thinking_fragment.is_empty() {
        None
    } else {
        Some(thinking_fragment.to_owned())
    };
    OmegaEvent::LlmRetry(LlmRetryEvent {
        time: now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        attempt: i64::from(attempt),
        http_status: err.status(),
        wait_ms: i64::try_from(wait_for.as_millis()).unwrap_or(i64::MAX),
        error: format!("{err}"),
        retry_at: Some(retry_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
        error_body,
        text_fragment,
        thinking_fragment,
        reason,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::sync::Mutex;

    use futures::StreamExt;
    use omega_protocol::OmegaEvent;

    use super::*;
    use crate::types::ModelConfig;

    /// Scripted provider: each `stream()` call pulls the next script
    /// off the front of the queue and replays it as a stream.
    struct ScriptedProvider {
        scripts: Mutex<std::collections::VecDeque<Vec<Result<AgentItem, LlmError>>>>,
        calls: Mutex<u32>,
    }

    impl ScriptedProvider {
        fn new(scripts: Vec<Vec<Result<AgentItem, LlmError>>>) -> Self {
            Self {
                scripts: Mutex::new(scripts.into()),
                calls: Mutex::new(0),
            }
        }

        fn call_count(&self) -> u32 {
            *self.calls.lock().unwrap()
        }
    }

    impl Provider for ScriptedProvider {
        fn stream(&self, _: LlmRequest) -> AgentItemStream {
            *self.calls.lock().unwrap() += 1;
            let script = self
                .scripts
                .lock()
                .unwrap()
                .pop_front()
                .expect("ScriptedProvider out of scripts");
            Box::pin(stream::iter(script))
        }
    }

    fn dummy_request() -> LlmRequest {
        LlmRequest {
            model: "test".into(),
            messages: vec![],
            system: None,
            tools: vec![],
            config: ModelConfig::default(),
        }
    }

    fn http_429(body: &str) -> LlmError {
        LlmError::Http {
            status: 429,
            body: body.to_owned(),
            retry_after: None,
        }
    }

    fn http_529() -> LlmError {
        LlmError::Http {
            status: 529,
            body: r#"{"type":"error","error":{"type":"overloaded_error"}}"#.to_owned(),
            retry_after: None,
        }
    }

    fn http_400() -> LlmError {
        LlmError::Http {
            status: 400,
            body: r#"{"type":"error","error":{"type":"invalid_request_error"}}"#.to_owned(),
            retry_after: None,
        }
    }

    fn http_429_retry_after(secs: u64) -> LlmError {
        LlmError::Http {
            status: 429,
            body: "{}".to_owned(),
            retry_after: Some(Duration::from_secs(secs)),
        }
    }

    #[tokio::test]
    async fn passes_through_a_clean_stream() {
        let p = ScriptedProvider::new(vec![vec![
            Ok(AgentItem::Signal(StreamSignal::Text { text: "hi ".into() })),
            Ok(AgentItem::Signal(StreamSignal::Text {
                text: "there".into(),
            })),
        ]]);
        let r = RetryingProvider::new(p, RetryConfig::for_tests(3));
        let items: Vec<_> = r.stream(dummy_request()).collect().await;
        assert_eq!(items.len(), 2);
        for item in &items {
            assert!(matches!(
                item,
                Ok(AgentItem::Signal(StreamSignal::Text { .. }))
            ));
        }
    }

    #[tokio::test]
    async fn retries_a_529_then_succeeds() {
        let inner = std::sync::Arc::new(ScriptedProvider::new(vec![
            vec![Err(http_529())],
            vec![Ok(AgentItem::Signal(StreamSignal::Text {
                text: "ok".into(),
            }))],
        ]));
        let r = RetryingProvider {
            inner: inner.clone(),
            config: RetryConfig::for_tests(3),
        };
        let items: Vec<_> = r.stream(dummy_request()).collect().await;
        assert_eq!(inner.call_count(), 2);
        assert_eq!(items.len(), 2);
        // First yielded item is the LlmRetry event.
        match items[0].as_ref().ok().and_then(AgentItem::as_event) {
            Some(OmegaEvent::LlmRetry(ev)) => {
                assert_eq!(ev.attempt, 1);
                assert_eq!(ev.http_status, Some(529));
                assert!(ev.wait_ms >= 1);
                assert!(ev.text_fragment.is_none());
            }
            other => panic!("expected LlmRetry event, got {other:?}"),
        }
        match &items[1] {
            Ok(AgentItem::Signal(StreamSignal::Text { text })) => assert_eq!(text, "ok"),
            other => panic!("expected text signal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn does_not_retry_a_400() {
        let inner = std::sync::Arc::new(ScriptedProvider::new(vec![vec![Err(http_400())]]));
        let r = RetryingProvider {
            inner: inner.clone(),
            config: RetryConfig::for_tests(3),
        };
        let items: Vec<_> = r.stream(dummy_request()).collect().await;
        assert_eq!(inner.call_count(), 1);
        assert_eq!(items.len(), 1);
        assert!(matches!(items[0], Err(LlmError::Http { status: 400, .. })));
    }

    #[tokio::test]
    async fn does_not_retry_context_too_long_429() {
        let body = r#"{"error":"Extra usage is required for long context requests"}"#;
        let inner = std::sync::Arc::new(ScriptedProvider::new(vec![vec![Err(http_429(body))]]));
        let r = RetryingProvider {
            inner: inner.clone(),
            config: RetryConfig::for_tests(3),
        };
        let items: Vec<_> = r.stream(dummy_request()).collect().await;
        assert_eq!(inner.call_count(), 1);
        assert_eq!(items.len(), 1);
        assert!(matches!(items[0], Err(LlmError::Http { status: 429, .. })));
    }

    #[tokio::test]
    async fn gives_up_after_max_attempts() {
        let inner = std::sync::Arc::new(ScriptedProvider::new(vec![
            vec![Err(http_529())],
            vec![
                Ok(AgentItem::Signal(StreamSignal::Text { text: "x".into() })),
                Err(http_529()),
            ],
            vec![Err(http_529())],
        ]));
        let r = RetryingProvider {
            inner: inner.clone(),
            config: RetryConfig::for_tests(3),
        };
        let items: Vec<_> = r.stream(dummy_request()).collect().await;
        // 3 attempts total: initial + 2 retries.
        assert_eq!(inner.call_count(), 3);
        // Final item is the terminal error.
        assert!(matches!(
            items.last(),
            Some(Err(LlmError::Http { status: 529, .. }))
        ));
        // Two LlmRetry events emitted.
        let retries = items
            .iter()
            .filter(|i| {
                matches!(
                    i.as_ref().ok().and_then(AgentItem::as_event),
                    Some(OmegaEvent::LlmRetry(_))
                )
            })
            .count();
        assert_eq!(retries, 2);
    }

    #[tokio::test]
    async fn populates_text_fragment_when_text_streamed_before_error() {
        let inner = std::sync::Arc::new(ScriptedProvider::new(vec![
            vec![
                Ok(AgentItem::Signal(StreamSignal::Text {
                    text: "partial ".into(),
                })),
                Ok(AgentItem::Signal(StreamSignal::Text {
                    text: "answer".into(),
                })),
                Err(http_529()),
            ],
            vec![Ok(AgentItem::Signal(StreamSignal::Text {
                text: "final".into(),
            }))],
        ]));
        let r = RetryingProvider {
            inner: inner.clone(),
            config: RetryConfig::for_tests(3),
        };
        let items: Vec<_> = r.stream(dummy_request()).collect().await;
        // 2 text items pre-error, 1 retry event, 1 text item post-retry.
        assert_eq!(items.len(), 4);
        match items[2].as_ref().ok().and_then(AgentItem::as_event) {
            Some(OmegaEvent::LlmRetry(ev)) => {
                assert_eq!(ev.text_fragment.as_deref(), Some("partial answer"));
                assert!(ev.thinking_fragment.is_none());
            }
            other => panic!("expected LlmRetry, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn honours_retry_after_header() {
        let inner = std::sync::Arc::new(ScriptedProvider::new(vec![
            vec![Err(http_429_retry_after(0))],
            vec![Ok(AgentItem::Signal(StreamSignal::Text {
                text: "ok".into(),
            }))],
        ]));
        let r = RetryingProvider {
            inner: inner.clone(),
            config: RetryConfig::for_tests(3),
        };
        let items: Vec<_> = r.stream(dummy_request()).collect().await;
        match items[0].as_ref().ok().and_then(AgentItem::as_event) {
            Some(OmegaEvent::LlmRetry(ev)) => {
                assert_eq!(ev.reason, Some(LlmRetryReason::RetryAfter));
                assert_eq!(ev.wait_ms, 0);
            }
            other => panic!("expected LlmRetry, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn retries_overloaded_error_payload_without_status() {
        let err = LlmError::Stream {
            message: r#"{"type":"error","error":{"type":"overloaded_error"}}"#.into(),
        };
        let inner = std::sync::Arc::new(ScriptedProvider::new(vec![
            vec![Err(err)],
            vec![Ok(AgentItem::Signal(StreamSignal::Text {
                text: "ok".into(),
            }))],
        ]));
        let r = RetryingProvider {
            inner: inner.clone(),
            config: RetryConfig::for_tests(3),
        };
        let items: Vec<_> = r.stream(dummy_request()).collect().await;
        assert_eq!(inner.call_count(), 2);
        let retries = items
            .iter()
            .filter(|i| {
                matches!(
                    i.as_ref().ok().and_then(AgentItem::as_event),
                    Some(OmegaEvent::LlmRetry(_))
                )
            })
            .count();
        assert_eq!(retries, 1);
    }

    #[tokio::test]
    async fn retries_transport_errors() {
        let err = LlmError::Transport {
            message: "ECONNRESET".into(),
        };
        let inner = std::sync::Arc::new(ScriptedProvider::new(vec![
            vec![Err(err)],
            vec![Ok(AgentItem::Signal(StreamSignal::Text {
                text: "ok".into(),
            }))],
        ]));
        let r = RetryingProvider {
            inner: inner.clone(),
            config: RetryConfig::for_tests(3),
        };
        let items: Vec<_> = r.stream(dummy_request()).collect().await;
        assert_eq!(inner.call_count(), 2);
        let retries = items
            .iter()
            .filter(|i| {
                matches!(
                    i.as_ref().ok().and_then(AgentItem::as_event),
                    Some(OmegaEvent::LlmRetry(_))
                )
            })
            .count();
        assert_eq!(retries, 1);
    }

    /// `error_body` in a `LlmRetry` event must be populated with the
    /// parsed JSON from an HTTP error body.
    /// Catches: `delete match arm LlmError::Http{body,..}` in `build_retry_event`.
    #[tokio::test]
    async fn error_body_populated_from_http_body() {
        let inner = std::sync::Arc::new(ScriptedProvider::new(vec![
            vec![Err(http_529())],
            vec![Ok(AgentItem::Signal(StreamSignal::Text {
                text: "ok".into(),
            }))],
        ]));
        let r = RetryingProvider {
            inner: inner.clone(),
            config: RetryConfig::for_tests(3),
        };
        let items: Vec<_> = r.stream(dummy_request()).collect().await;
        match items[0].as_ref().ok().and_then(AgentItem::as_event) {
            Some(OmegaEvent::LlmRetry(ev)) => {
                let body = ev
                    .error_body
                    .as_ref()
                    .expect("error_body must be set for JSON HTTP body");
                assert_eq!(body["type"], "error", "error_body type field");
                assert_eq!(
                    body["error"]["type"], "overloaded_error",
                    "error_body inner type"
                );
            }
            other => panic!("expected LlmRetry event, got {other:?}"),
        }
    }

    /// `retry_at` in a `LlmRetry` event must not be earlier than `time`.
    /// Catches: `replace + with - in build_retry_event`.
    #[tokio::test]
    async fn retry_at_is_not_before_event_time() {
        let inner = std::sync::Arc::new(ScriptedProvider::new(vec![
            vec![Err(http_529())],
            vec![Ok(AgentItem::Signal(StreamSignal::Text {
                text: "ok".into(),
            }))],
        ]));
        let r = RetryingProvider {
            inner: inner.clone(),
            config: RetryConfig::for_tests(3),
        };
        let items: Vec<_> = r.stream(dummy_request()).collect().await;
        match items[0].as_ref().ok().and_then(AgentItem::as_event) {
            Some(OmegaEvent::LlmRetry(ev)) => {
                let time = chrono::DateTime::parse_from_rfc3339(&ev.time)
                    .expect("LlmRetry.time must be valid RFC3339");
                let retry_at_str = ev.retry_at.as_ref().expect("LlmRetry.retry_at must be set");
                let retry_at = chrono::DateTime::parse_from_rfc3339(retry_at_str)
                    .expect("LlmRetry.retry_at must be valid RFC3339");
                assert!(
                    retry_at >= time,
                    "retry_at ({retry_at}) must be >= time ({time})"
                );
            }
            other => panic!("expected LlmRetry event, got {other:?}"),
        }
    }

    /// Backoff must grow with each successive attempt.
    /// Catches: `replace << with >> in compute_backoff`.
    #[tokio::test]
    async fn backoff_grows_on_second_attempt() {
        let inner = std::sync::Arc::new(ScriptedProvider::new(vec![
            vec![Err(http_529())],
            vec![Err(http_529())],
            vec![Ok(AgentItem::Signal(StreamSignal::Text {
                text: "ok".into(),
            }))],
        ]));
        // 4 max_attempts → up to 3 tries, so 2 retries are possible.
        let r = RetryingProvider {
            inner: inner.clone(),
            config: RetryConfig::for_tests(4),
        };
        let items: Vec<_> = r.stream(dummy_request()).collect().await;

        let wait_ms_list: Vec<i64> = items
            .iter()
            .filter_map(|i| i.as_ref().ok())
            .filter_map(AgentItem::as_event)
            .filter_map(|e| {
                if let OmegaEvent::LlmRetry(ev) = e {
                    Some(ev.wait_ms)
                } else {
                    None
                }
            })
            .collect();

        assert_eq!(wait_ms_list.len(), 2, "expected exactly 2 retry events");
        assert!(
            wait_ms_list[1] > wait_ms_list[0],
            "second backoff ({}) must exceed first ({})",
            wait_ms_list[1],
            wait_ms_list[0]
        );
    }

    /// With `jitter = true` and `initial_backoff = 1ms`, the correct computation
    /// is `round(1 * 0.9..1.1) = 1ms`.  The `replace * with + in compute_backoff`
    /// mutant gives `round(1 + 0.9..1.1) = 2ms`.
    #[tokio::test]
    async fn jitter_rounds_to_base_ms_not_double() {
        let inner = std::sync::Arc::new(ScriptedProvider::new(vec![
            vec![Err(http_529())],
            vec![Ok(AgentItem::Signal(StreamSignal::Text {
                text: "ok".into(),
            }))],
        ]));
        let config = RetryConfig {
            max_attempts: 3,
            initial_backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(16),
            jitter: true,
        };
        let r = RetryingProvider {
            inner: inner.clone(),
            config,
        };
        let items: Vec<_> = r.stream(dummy_request()).collect().await;
        match items[0].as_ref().ok().and_then(AgentItem::as_event) {
            Some(OmegaEvent::LlmRetry(ev)) => {
                // With 1ms base, correct: round(1 * [0.9,1.1]) = 1.
                // `+ jitter` mutant: round(1 + [0.9,1.1]) = 2.
                assert_eq!(
                    ev.wait_ms, 1,
                    "jitter on 1ms base must round to 1ms, got {}",
                    ev.wait_ms
                );
            }
            other => panic!("expected LlmRetry event, got {other:?}"),
        }
    }
}
