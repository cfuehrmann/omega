//! Inspectable input queue for the persistent agent run loop (§15 U1).
//
// Lint exceptions
// ---------------
// `expect_used`: the internal `Mutex` only poisons if a thread panics while
//   holding the lock.  There is no recovery strategy at that point;
//   propagating the panic is correct — identical rationale to `controls.rs`.
#![allow(clippy::expect_used, clippy::missing_panics_doc)]
//!
//! The bare `mpsc::Receiver<InputItem>` used in U1 is not inspectable —
//! pending items cannot be read without consuming them.  `InputQueue`
//! replaces it with a `VecDeque`-backed queue whose pending items are
//! always visible via [`InputQueue::snapshot`].
//!
//! ## Single-consumer contract
//!
//! Only the agent's `run()` loop calls [`InputQueue::pop`].  Multiple
//! producers may call [`InputQueue::push`] concurrently (the server's WS
//! handler, future monitor callbacks in U2).  The implementation is
//! `Clone` — every clone shares the same underlying queue via `Arc`.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use chrono::SecondsFormat;
use tokio::sync::Notify;

use crate::agent::InputItem;

/// Preview of one pending item in the queue.
///
/// Transport-only projection — never written to `events.jsonl`.
/// Structured to admit `"monitor:<id>"` sources in U2.
#[derive(Debug, Clone)]
pub struct QueuedItemView {
    /// Who queued this item.  Currently always `"human"`; will gain
    /// `"monitor:<id>"` variants in U2.
    pub source: String,
    /// First 120 characters of the content string, for UI display.
    pub content_preview: String,
    /// RFC 3339 timestamp (millisecond precision) when this item was pushed.
    pub enqueued_at: String,
}

// ---------------------------------------------------------------------------
// Internal storage
// ---------------------------------------------------------------------------

struct InnerItem {
    item: InputItem,
    view: QueuedItemView,
}

struct Inner {
    queue: VecDeque<InnerItem>,
}

// ---------------------------------------------------------------------------
// Public type
// ---------------------------------------------------------------------------

/// Shared, inspectable input queue for the persistent agent run loop.
///
/// Implements the same semantic contract as `tokio::sync::mpsc::Receiver`:
/// [`pop`](InputQueue::pop) parks when the queue is empty and returns
/// exactly one item per call; cancellation of the agent's run loop is
/// handled by the external `CancellationToken` via `tokio::select!`.
///
/// Unlike `mpsc::Receiver`, the full set of pending items is always
/// visible via [`snapshot`](InputQueue::snapshot), enabling the server to
/// push queue snapshots to the UI.
///
/// `Clone` is cheap — each clone shares the same underlying state.
#[derive(Clone)]
pub struct InputQueue {
    inner: Arc<Mutex<Inner>>,
    notify: Arc<Notify>,
}

impl std::fmt::Debug for InputQueue {
    #[mutants::skip] // display-only: body-replacement cannot be meaningfully tested
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let len = self.inner.lock().map_or(0, |g| g.queue.len());
        f.debug_struct("InputQueue")
            .field("len", &len)
            .finish_non_exhaustive()
    }
}

impl Default for InputQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl InputQueue {
    /// Create an empty queue.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                queue: VecDeque::new(),
            })),
            notify: Arc::new(Notify::new()),
        }
    }

    /// Push one item onto the back of the queue.
    ///
    /// Returns a snapshot of the pending items taken **atomically while
    /// the item is in the queue** (before any consumer can pop it), so the
    /// caller can send a queue frame that is guaranteed to include the
    /// just-pushed item.
    // The snapshot return value is intentionally optional: callers that
    // need it (e.g. the server's WS handler) use it; test code may not.
    #[allow(clippy::must_use_candidate)]
    pub fn push(&self, item: InputItem) -> Vec<QueuedItemView> {
        let snapshot = {
            let mut guard = self.inner.lock().expect("InputQueue lock poisoned in push");
            let view = make_view(&item);
            guard.queue.push_back(InnerItem { item, view });
            guard.queue.iter().map(|i| i.view.clone()).collect()
        };
        // Notify outside the lock so a waking consumer can acquire
        // immediately without finding the lock still held.
        self.notify.notify_one();
        snapshot
    }

    /// Remove and return the front item, parking until one is available.
    ///
    /// Parks via [`tokio::sync::Notify`] — zero CPU cost while the queue is
    /// empty.  This is the only pop path; it preserves one-item-per-Gather
    /// semantics (U1).
    ///
    /// `Notify` stores one permit if the producer fires between the
    /// empty-check and the `.await`, so no wake-up is ever missed.
    pub async fn pop(&self) -> InputItem {
        loop {
            {
                let mut guard = self.inner.lock().expect("InputQueue lock poisoned in pop");
                if let Some(inner_item) = guard.queue.pop_front() {
                    return inner_item.item;
                }
            }
            self.notify.notified().await;
        }
    }

    /// Snapshot of the currently pending (not yet popped) items.
    ///
    /// The snapshot is taken under the internal lock, reflecting a
    /// consistent point in time.  Items already popped by the agent will
    /// not appear.
    #[must_use]
    pub fn snapshot(&self) -> Vec<QueuedItemView> {
        self.inner
            .lock()
            .expect("InputQueue lock poisoned in snapshot")
            .queue
            .iter()
            .map(|i| i.view.clone())
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Content-preview character limit (Unicode codepoints, not bytes).
const PREVIEW_LEN: usize = 120;

/// Build the display view for one `InputItem`.
fn make_view(item: &InputItem) -> QueuedItemView {
    let (source, raw_content) = match item {
        InputItem::Human { content } => ("human", content.as_str()),
    };
    let content_preview = truncate_preview(raw_content);
    QueuedItemView {
        source: source.to_owned(),
        content_preview,
        enqueued_at: chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
    }
}

/// Truncate `s` to at most [`PREVIEW_LEN`] Unicode codepoints, appending
/// `'\u{2026}'` (`'…'`) when truncation occurs.
fn truncate_preview(s: &str) -> String {
    let mut chars = s.chars();
    // Collect up to PREVIEW_LEN codepoints, then peek to see if more follow.
    let prefix: String = chars.by_ref().take(PREVIEW_LEN).collect();
    if chars.next().is_some() {
        format!("{prefix}\u{2026}")
    } else {
        prefix
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    // Justification for unit tests here rather than end-to-end tests:
    // `InputQueue` is a pure data structure — push/pop/snapshot semantics
    // can be verified directly without an Agent, MockProvider, or server.
    // The Agent-level tests in tests/internal.rs verify that the queue is
    // wired into `Agent::run` correctly end-to-end.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use tokio_util::sync::CancellationToken;

    fn human(s: &str) -> InputItem {
        InputItem::Human {
            content: s.to_owned(),
        }
    }

    // --- push / snapshot ----------------------------------------------------

    #[test]
    fn new_queue_snapshot_is_empty() {
        let q = InputQueue::new();
        assert!(q.snapshot().is_empty());
    }

    #[test]
    fn push_returns_snapshot_with_item() {
        let q = InputQueue::new();
        let snap = q.push(human("hello"));
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].source, "human");
        assert!(snap[0].content_preview.contains("hello"));
    }

    #[test]
    fn push_two_items_snapshot_shows_both_in_order() {
        let q = InputQueue::new();
        q.push(human("first"));
        let snap = q.push(human("second"));
        assert_eq!(snap.len(), 2);
        assert!(snap[0].content_preview.contains("first"));
        assert!(snap[1].content_preview.contains("second"));
    }

    #[test]
    fn snapshot_after_push_still_shows_item_before_pop() {
        let q = InputQueue::new();
        q.push(human("pending"));
        let snap = q.snapshot();
        assert_eq!(snap.len(), 1, "item must appear in snapshot before pop");
    }

    // --- pop ----------------------------------------------------------------

    #[tokio::test]
    async fn pop_returns_item_and_queue_is_empty_after() {
        let q = InputQueue::new();
        q.push(human("hello"));
        // Bounded wait so a `push → vec![]` mutation (which omits notify_one)
        // causes a timeout-failure rather than hanging the test binary.
        let item = tokio::time::timeout(tokio::time::Duration::from_millis(500), q.pop())
            .await
            .expect("pop must resolve within 500 ms when item is already queued");
        assert!(
            matches!(item, InputItem::Human { ref content } if content == "hello"),
            "unexpected item: {item:?}"
        );
        assert!(q.snapshot().is_empty(), "queue must be empty after pop");
    }

    #[tokio::test]
    async fn pop_two_items_processed_in_fifo_order() {
        let q = InputQueue::new();
        q.push(human("first"));
        q.push(human("second"));
        let a = tokio::time::timeout(tokio::time::Duration::from_millis(500), q.pop())
            .await
            .expect("first pop must resolve within 500 ms");
        let b = tokio::time::timeout(tokio::time::Duration::from_millis(500), q.pop())
            .await
            .expect("second pop must resolve within 500 ms");
        assert!(
            matches!(&a, InputItem::Human { content } if content == "first"),
            "wrong first: {a:?}"
        );
        assert!(
            matches!(&b, InputItem::Human { content } if content == "second"),
            "wrong second: {b:?}"
        );
    }

    #[tokio::test]
    async fn pop_parks_until_push_arrives() {
        let q = InputQueue::new();
        let q2 = q.clone();
        // Spawn a task that pushes after a short delay.
        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
            q2.push(human("delayed"));
        });
        // pop() must park and return after the push.
        let item = tokio::time::timeout(tokio::time::Duration::from_secs(5), q.pop())
            .await
            .expect("pop timed out");
        assert!(
            matches!(item, InputItem::Human { ref content } if content == "delayed"),
            "unexpected item: {item:?}"
        );
    }

    #[tokio::test]
    async fn pop_via_select_with_cancel_returns_item_when_available() {
        let q = InputQueue::new();
        q.push(human("hello"));
        let cancel = CancellationToken::new();
        // Bounded wait: a `push → vec![]` mutation omits notify_one, causing
        // pop() to hang.  The timeout converts that hang into a fast failure.
        let item = tokio::time::timeout(tokio::time::Duration::from_millis(500), async {
            tokio::select! {
                () = cancel.cancelled() => panic!("unexpected cancel"),
                item = q.pop() => item,
            }
        })
        .await
        .expect("select must resolve within 500 ms when item is already queued");
        assert!(
            matches!(item, InputItem::Human { ref content } if content == "hello"),
            "unexpected item: {item:?}"
        );
    }

    // --- content_preview truncation -----------------------------------------

    #[test]
    fn short_content_preserved_verbatim() {
        let q = InputQueue::new();
        let snap = q.push(human("hello world"));
        assert_eq!(snap[0].content_preview, "hello world");
    }

    #[test]
    fn long_content_truncated_to_120_chars_with_ellipsis() {
        let q = InputQueue::new();
        let long = "x".repeat(200);
        let snap = q.push(human(&long));
        // preview = 120 'x' chars + '…' (U+2026)
        let expected: String = "x".repeat(120) + "\u{2026}";
        assert_eq!(snap[0].content_preview, expected);
    }

    #[test]
    fn exactly_120_chars_not_truncated() {
        let q = InputQueue::new();
        let exactly = "y".repeat(120);
        let snap = q.push(human(&exactly));
        assert_eq!(
            snap[0].content_preview, exactly,
            "120-char content must not be truncated"
        );
    }

    // --- QueuedItemView fields ----------------------------------------------

    #[test]
    fn view_source_is_human_for_human_item() {
        let q = InputQueue::new();
        let snap = q.push(human("test"));
        assert_eq!(snap[0].source, "human");
    }

    #[test]
    fn view_enqueued_at_is_rfc3339_with_millis() {
        let q = InputQueue::new();
        let snap = q.push(human("test"));
        // RFC 3339 with millis: "2025-01-01T00:00:00.000Z"
        assert!(
            snap[0].enqueued_at.ends_with('Z'),
            "enqueued_at must end with Z (UTC): {}",
            snap[0].enqueued_at
        );
        // Must be parseable as RFC 3339
        chrono::DateTime::parse_from_rfc3339(&snap[0].enqueued_at)
            .expect("enqueued_at must be valid RFC 3339");
    }

    // --- clone semantics ----------------------------------------------------

    #[tokio::test]
    async fn clone_shares_same_underlying_queue() {
        let q1 = InputQueue::new();
        let q2 = q1.clone();
        q1.push(human("from q1"));
        let snap2 = q2.snapshot();
        assert_eq!(snap2.len(), 1, "clone must see q1's push");
        assert!(snap2[0].content_preview.contains("from q1"));
    }

    // --- truncate_preview unit --------------------------------------------------
    // Carve-out: pure function; testing directly avoids constructing InputItems
    // just to exercise the truncation boundary.

    #[test]
    fn truncate_preview_empty_string() {
        assert_eq!(truncate_preview(""), "");
    }

    #[test]
    fn truncate_preview_exactly_120_not_truncated() {
        let s = "a".repeat(120);
        assert_eq!(truncate_preview(&s), s);
    }

    #[test]
    fn truncate_preview_121_chars_gets_ellipsis() {
        let s = "a".repeat(121);
        let expected = "a".repeat(120) + "\u{2026}";
        assert_eq!(truncate_preview(&s), expected);
    }

    #[test]
    fn truncate_preview_multibyte_chars_counted_correctly() {
        // '€' is 3 bytes but 1 codepoint.
        let s = "€".repeat(121);
        let result = truncate_preview(&s);
        // Should be 120 '€' chars + '…'
        let expected: String = "€".repeat(120) + "\u{2026}";
        assert_eq!(result, expected);
    }
}
