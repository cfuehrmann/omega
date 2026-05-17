//! The [`Provider`] trait — the single interface every LLM backend
//! implements.

use futures::stream::BoxStream;

use crate::types::{AgentItem, LlmError, LlmRequest};

/// A boxed stream of provider items.
///
/// `'static` lifetime — the stream owns everything it needs (request,
/// HTTP client, response body) so it can be passed across `tokio::spawn`
/// boundaries by callers (`omega-server`).
pub type AgentItemStream = BoxStream<'static, Result<AgentItem, LlmError>>;

/// A streaming LLM backend.
///
/// One method, [`stream`](Self::stream), opens a new request and returns
/// a stream of [`AgentItem`]s.  Streams are cold — no HTTP traffic
/// happens until the caller polls.
///
/// # Why `BoxStream`?
///
/// Returning `impl Stream<…>` from a trait method is technically
/// possible (RPITIT, stable since Rust 1.75), but it makes generic
/// composition (the [`RetryingProvider`](crate::RetryingProvider)
/// wrapper) noisier and forces every caller into a concrete-type chain.
/// `BoxStream` allocates one `Box` per call and unlocks trait-object
/// dispatch (`Arc<dyn Provider>`), which is what `omega-server` needs
/// for its provider registry.
pub trait Provider: Send + Sync {
    /// Open a streaming LLM call.
    fn stream(&self, request: LlmRequest) -> AgentItemStream;
}

impl<P: Provider + ?Sized> Provider for std::sync::Arc<P> {
    fn stream(&self, request: LlmRequest) -> AgentItemStream {
        (**self).stream(request)
    }
}

impl<P: Provider + ?Sized> Provider for Box<P> {
    fn stream(&self, request: LlmRequest) -> AgentItemStream {
        (**self).stream(request)
    }
}
