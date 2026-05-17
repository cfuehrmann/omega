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
use omega_types::events::LlmRetryEvent;
use omega_types::{LlmRetryReason, OmegaEvent};
use rand::RngExt;

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
                    let event = build_retry_event(&err, next_attempt, sleep_for, reason);
                    s.attempt = next_attempt;
                    tokio::time::sleep(sleep_for).await;
                    s.current = Some(s.inner.stream(s.request.clone()));
                    Some((Ok(AgentItem::event(event)), s))
                }
            }
        }
    })
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
) -> OmegaEvent {
    let now = Utc::now();
    let retry_at =
        now + chrono::Duration::from_std(wait_for).unwrap_or_else(|_| chrono::Duration::seconds(0));
    let error_body: Option<serde_json::Value> = match err {
        LlmError::Http { body, .. } => serde_json::from_str(body).ok(),
        _ => None,
    };
    OmegaEvent::LlmRetry(LlmRetryEvent {
        time: now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        attempt: i64::from(attempt),
        http_status: err.status(),
        wait_ms: i64::try_from(wait_for.as_millis()).unwrap_or(i64::MAX),
        error: format!("{err}"),
        retry_at: Some(retry_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
        error_body,
        reason,
    })
}
