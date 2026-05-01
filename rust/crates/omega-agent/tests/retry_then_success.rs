//! `LlmRetry` events emitted by the provider mid-stream must be
//! forwarded as-is, partial assistant content discarded, and the final
//! response still recorded normally.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::doc_markdown,
    clippy::too_many_lines,
    clippy::wildcard_enum_match_arm,
    clippy::missing_panics_doc
)]

mod common;

use common::{collect_stream, make_llm_response, make_test_agent, tags};
use omega_core::AgentItem;
use omega_protocol::events::LlmRetryEvent;
use omega_protocol::{LlmRetryReason, OmegaEvent, StreamSignal};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn llm_retry_event_is_forwarded_partial_text_dropped() {
    let (mut agent, provider, _tmp) = make_test_agent();

    // Provider emits a partial text delta, then an LlmRetry event (as
    // RetryingProvider would after sleeping), then on the same logical
    // call hands us the real response.  The agent must:
    //   1) forward the LlmRetry event,
    //   2) discard "partial-" so the assistant record only stores "ok",
    //   3) still emit a TurnEnd.
    provider.push_response(vec![
        Ok(AgentItem::Signal(StreamSignal::Text {
            text: "partial-".to_owned(),
        })),
        Ok(AgentItem::event(OmegaEvent::LlmRetry(LlmRetryEvent {
            time: "2024-01-01T00:00:00.000Z".to_owned(),
            attempt: 1,
            http_status: Some(529),
            wait_ms: 250,
            error: "overloaded_error".to_owned(),
            retry_at: None,
            error_body: None,
            thinking_fragment: None,
            text_fragment: None,
            reason: Some(LlmRetryReason::RetryAfter),
        }))),
        Ok(AgentItem::Signal(StreamSignal::Text {
            text: "ok".to_owned(),
        })),
        Ok(make_llm_response("end_turn", Some("ok"), 4, 1)),
    ]);

    let stream = agent.send_message("hi".to_owned(), CancellationToken::new());
    let items = collect_stream(stream).await;

    let t = tags(&items);
    assert!(
        t.iter().any(|x| *x == "LlmRetry"),
        "LlmRetry event must be forwarded; got {t:?}"
    );
    assert_eq!(t.last().copied(), Some("TurnEnd"));

    // The assistant context record must contain only "ok" — the
    // pre-retry partial text was discarded.
    let assistant = agent
        .history()
        .iter()
        .find(|m| matches!(m.role, omega_core::Role::Assistant))
        .expect("assistant history entry");
    let text = assistant
        .content
        .iter()
        .find_map(|b| match b {
            omega_core::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .expect("assistant text block");
    assert_eq!(text, "ok", "partial text must be dropped on retry");
}
