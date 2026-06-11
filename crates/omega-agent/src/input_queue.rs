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
use omega_tools::MonitorSink;
use omega_types::OmegaEvent;
use omega_types::events::{MonitorStderrEvent, MonitorStopReason};
use tokio::sync::Notify;

use crate::agent::InputItem;
use crate::event_sink::EventSink;

/// Callback fired on every [`InputQueue::push`], receiving the atomic
/// post-push snapshot.  The server registers one to forward a
/// `WsMessage::InputQueue` frame so the queue badge updates on **any**
/// enqueue — human *or* monitor (§15 U2 / §9 always-visible).
pub type OnChange = Arc<dyn Fn(Vec<QueuedItemView>) + Send + Sync>;

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
    /// Optional push-notification callback (§15 U2 queue-viz).
    on_change: Option<OnChange>,
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
                on_change: None,
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
        let (snapshot, on_change): (Vec<QueuedItemView>, Option<OnChange>) = {
            let mut guard = self.inner.lock().expect("InputQueue lock poisoned in push");
            let view = make_view(&item);
            guard.queue.push_back(InnerItem { item, view });
            let snapshot = guard.queue.iter().map(|i| i.view.clone()).collect();
            (snapshot, guard.on_change.clone())
        };
        // Notify outside the lock so a waking consumer can acquire
        // immediately without finding the lock still held.
        self.notify.notify_one();
        // Fire the queue-viz callback (§15 U2) with the atomic snapshot so
        // the just-pushed item is guaranteed visible — even when the push
        // comes from a background monitor reader task, not a WS handler.
        if let Some(cb) = on_change {
            cb(snapshot.clone());
        }
        snapshot
    }

    /// Register the on-push callback (§15 U2 queue-viz).  Idempotent
    /// replace; the server installs one per active session so monitor
    /// enqueues reach the WS layer.
    pub fn set_on_change(&self, cb: OnChange) {
        self.inner
            .lock()
            .expect("InputQueue lock poisoned in set_on_change")
            .on_change = Some(cb);
    }

    /// Remove the pending item whose `enqueued_at` timestamp matches
    /// `enqueued_at` (if any).  Returns the post-deletion snapshot and fires
    /// the `on_change` callback so the server can push a fresh queue frame.
    /// If no item matches, the queue is unchanged and the current snapshot is
    /// returned.
    #[allow(clippy::must_use_candidate)]
    pub fn delete(&self, enqueued_at: &str) -> Vec<QueuedItemView> {
        let (snapshot, on_change) = {
            let mut guard = self
                .inner
                .lock()
                .expect("InputQueue lock poisoned in delete");
            guard.queue.retain(|i| i.view.enqueued_at != enqueued_at);
            let snapshot: Vec<QueuedItemView> =
                guard.queue.iter().map(|i| i.view.clone()).collect();
            (snapshot, guard.on_change.clone())
        };
        if let Some(cb) = on_change {
            cb(snapshot.clone());
        }
        snapshot
    }

    /// Drain **all** currently-pending items without parking (§15 U2 Seam
    /// drain).  Returns them in FIFO order; the queue is left empty.
    /// Complements [`pop`](Self::pop) (which parks and takes exactly one):
    /// the run loop pops/parks for the first item of a Gather, then drains
    /// the rest at each seam.
    #[must_use]
    pub fn drain_pending(&self) -> Vec<InputItem> {
        let mut guard = self
            .inner
            .lock()
            .expect("InputQueue lock poisoned in drain_pending");
        guard.queue.drain(..).map(|i| i.item).collect()
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
///
/// `source` is `"human"` for human input and `"monitor:<id>"` for monitor
/// deliveries (§15 U2 queue-viz), letting the UI tag each pending item.
fn make_view(item: &InputItem) -> QueuedItemView {
    let (source, raw_content): (String, String) = match item {
        InputItem::Human { content } => ("human".to_owned(), content.clone()),
        InputItem::MonitorStdout { monitor_id, lines } => {
            (format!("monitor:{monitor_id}"), lines.join("\n"))
        }
        InputItem::MonitorStopped {
            monitor_id,
            reason,
            exit_code,
        } => (
            format!("monitor:{monitor_id}"),
            format!(
                "[stopped: {reason:?}{}]",
                match exit_code {
                    Some(c) => format!(" exit={c}"),
                    None => String::new(),
                }
            ),
        ),
    };
    let content_preview = truncate_preview(&raw_content);
    QueuedItemView {
        source,
        content_preview,
        enqueued_at: chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
    }
}

/// [`MonitorSink`] implementation that routes monitor stdout/stop onto an
/// [`InputQueue`] (§15 U2).  The agent's `run()` loop attaches one of these
/// to the [`MonitorManager`] so monitor output flows through the *same*
/// inbox as human input.  Pushing fires the queue's `on_change` callback,
/// so monitor enqueues reach the WS layer for free.
///
/// Monitor **stderr** does NOT flow through the inbox: it is non-projected
/// diagnostic output (§17).  [`Self::deliver_stderr`] emits a
/// [`MonitorStderr`](OmegaEvent::MonitorStderr) event the instant the reader
/// reads a line — stamped at production time — through the out-of-band
/// [`EventSink`] (event-log + WS only), never the inbox / LLM context.
#[derive(Debug, Clone)]
pub struct InboxSink {
    inbox: InputQueue,
    event_sink: Arc<EventSink>,
}

impl InboxSink {
    /// Wrap `inbox` (for stdout/stop delivery) and `event_sink` (for the
    /// non-projected stderr path) as a monitor delivery sink.
    #[must_use]
    pub fn new(inbox: InputQueue, event_sink: Arc<EventSink>) -> Self {
        Self { inbox, event_sink }
    }
}

impl MonitorSink for InboxSink {
    fn deliver_stdout(&self, monitor_id: &str, line: String) {
        let _ = self.inbox.push(InputItem::MonitorStdout {
            monitor_id: monitor_id.to_owned(),
            lines: vec![line],
        });
    }

    fn deliver_stopped(&self, monitor_id: &str, reason: MonitorStopReason, exit_code: Option<i32>) {
        let _ = self.inbox.push(InputItem::MonitorStopped {
            monitor_id: monitor_id.to_owned(),
            reason,
            exit_code,
        });
    }

    fn deliver_stderr(&self, monitor_id: &str, chunk: String) {
        // §17 (Phase A): stamp at production time (the instant the reader read
        // the line) and emit out-of-band through the sink — event-log + WS
        // only.  NON-PROJECTED: never the inbox, never role:user context,
        // never a token.  `emit_detached` broadcasts synchronously (so wire
        // order matches read order) and spawns the disk append.
        let ev = OmegaEvent::MonitorStderr(MonitorStderrEvent {
            id: monitor_id.to_owned(),
            chunk,
            time: chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        });
        self.event_sink.emit_detached(ev);
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
    use omega_store::EventStore;
    use tokio_util::sync::CancellationToken;

    // An `EventSink` whose store path is never written: the stdout/stop
    // delivery paths under test never emit, so no IO occurs.  (`deliver_stderr`
    // — the only emitting path — is exercised end-to-end in tests/internal.rs.)
    fn dummy_event_sink() -> Arc<EventSink> {
        Arc::new(EventSink::new(Arc::new(EventStore::new(
            std::path::PathBuf::from("/nonexistent/stderr-unit-test/events.jsonl"),
        ))))
    }

    fn human(s: &str) -> InputItem {
        InputItem::Human {
            content: s.to_owned(),
        }
    }

    fn mon_stdout(id: &str, line: &str) -> InputItem {
        InputItem::MonitorStdout {
            monitor_id: id.to_owned(),
            lines: vec![line.to_owned()],
        }
    }

    // --- delete ---------------------------------------------------------------

    #[test]
    fn delete_removes_matching_item_by_enqueued_at() {
        let q = InputQueue::new();
        let snap = q.push(human("target"));
        let ts = snap[0].enqueued_at.clone();
        // Sleep >1 ms so the second push gets a distinct millisecond timestamp.
        std::thread::sleep(std::time::Duration::from_millis(2));
        q.push(human("other"));
        let after = q.delete(&ts);
        assert_eq!(after.len(), 1, "only the non-deleted item must remain");
        assert!(after[0].content_preview.contains("other"));
        assert_eq!(q.snapshot().len(), 1);
    }

    #[test]
    fn delete_with_no_match_leaves_queue_unchanged() {
        let q = InputQueue::new();
        q.push(human("a"));
        q.push(human("b"));
        let after = q.delete("1970-01-01T00:00:00.000Z");
        assert_eq!(after.len(), 2, "no-match delete must leave all items");
    }

    #[test]
    fn delete_on_empty_queue_returns_empty_snapshot() {
        let q = InputQueue::new();
        let after = q.delete("1970-01-01T00:00:00.000Z");
        assert!(after.is_empty());
    }

    #[test]
    fn delete_fires_on_change_with_post_deletion_snapshot() {
        use std::sync::Mutex as StdMutex;
        let q = InputQueue::new();
        let seen: Arc<StdMutex<Vec<Vec<QueuedItemView>>>> = Arc::new(StdMutex::new(Vec::new()));
        let seen2 = Arc::clone(&seen);
        q.set_on_change(Arc::new(move |snap| seen2.lock().unwrap().push(snap)));
        let snap = q.push(human("to-delete"));
        let ts = snap[0].enqueued_at.clone();
        // Sleep >1 ms so the second push gets a distinct millisecond timestamp.
        std::thread::sleep(std::time::Duration::from_millis(2));
        q.push(human("keeper"));
        seen.lock().unwrap().clear(); // ignore push callbacks
        q.delete(&ts);
        let calls = seen.lock().unwrap();
        assert_eq!(calls.len(), 1, "on_change must fire exactly once on delete");
        assert_eq!(calls[0].len(), 1, "post-deletion snapshot has one item");
        assert!(calls[0][0].content_preview.contains("keeper"));
    }

    #[test]
    fn delete_no_on_change_set_still_works() {
        let q = InputQueue::new();
        let snap = q.push(human("x"));
        let ts = snap[0].enqueued_at.clone();
        let after = q.delete(&ts); // must not panic
        assert!(after.is_empty());
    }

    // --- drain_pending (§15 U2 Seam drain) ----------------------------------

    #[test]
    fn drain_pending_on_empty_queue_is_empty() {
        let q = InputQueue::new();
        assert!(q.drain_pending().is_empty());
    }

    #[test]
    fn drain_pending_returns_all_in_fifo_and_empties_queue() {
        let q = InputQueue::new();
        q.push(human("a"));
        q.push(mon_stdout("m1", "b"));
        q.push(human("c"));
        let drained = q.drain_pending();
        assert_eq!(drained.len(), 3, "drain must take ALL pending items");
        assert!(matches!(&drained[0], InputItem::Human { content } if content == "a"));
        assert!(matches!(&drained[1], InputItem::MonitorStdout { lines, .. } if lines == &["b"]));
        assert!(matches!(&drained[2], InputItem::Human { content } if content == "c"));
        assert!(
            q.snapshot().is_empty(),
            "queue must be empty after drain_pending"
        );
    }

    #[test]
    fn drain_pending_then_push_does_not_resurrect_drained_items() {
        let q = InputQueue::new();
        q.push(human("old"));
        let _ = q.drain_pending();
        q.push(human("new"));
        let again = q.drain_pending();
        assert_eq!(again.len(), 1, "only the post-drain item remains");
        assert!(matches!(&again[0], InputItem::Human { content } if content == "new"));
    }

    // --- monitor-source views (§15 U2 queue-viz) ----------------------------

    #[test]
    fn view_source_is_monitor_id_for_monitor_stdout() {
        let q = InputQueue::new();
        let snap = q.push(mon_stdout("build-watch", "compiling"));
        assert_eq!(snap[0].source, "monitor:build-watch");
        assert!(snap[0].content_preview.contains("compiling"));
    }

    #[test]
    fn view_source_is_monitor_id_for_monitor_stopped() {
        let q = InputQueue::new();
        let snap = q.push(InputItem::MonitorStopped {
            monitor_id: "ticker".to_owned(),
            reason: MonitorStopReason::ProcessExited,
            exit_code: Some(0),
        });
        assert_eq!(snap[0].source, "monitor:ticker");
        assert!(
            snap[0].content_preview.contains("stopped"),
            "stopped preview must mention the stop: {}",
            snap[0].content_preview
        );
        assert!(
            snap[0].content_preview.contains("exit=0"),
            "stopped preview must carry the exit code: {}",
            snap[0].content_preview
        );
    }

    #[test]
    fn view_monitor_stopped_without_exit_code_omits_exit() {
        let q = InputQueue::new();
        let snap = q.push(InputItem::MonitorStopped {
            monitor_id: "x".to_owned(),
            reason: MonitorStopReason::ProcessCrashed,
            exit_code: None,
        });
        assert!(
            !snap[0].content_preview.contains("exit="),
            "no exit code → no exit= segment: {}",
            snap[0].content_preview
        );
    }

    // --- on_change callback (§15 U2 WS queue-viz) ---------------------------

    #[test]
    fn on_change_fires_on_push_with_the_atomic_snapshot() {
        use std::sync::Mutex as StdMutex;
        let q = InputQueue::new();
        let seen: Arc<StdMutex<Vec<Vec<QueuedItemView>>>> = Arc::new(StdMutex::new(Vec::new()));
        let seen2 = Arc::clone(&seen);
        q.set_on_change(Arc::new(move |snap| seen2.lock().unwrap().push(snap)));
        q.push(human("first"));
        q.push(mon_stdout("m", "second"));
        let calls = seen.lock().unwrap();
        assert_eq!(calls.len(), 2, "on_change must fire once per push");
        assert_eq!(calls[0].len(), 1, "first snapshot has one item");
        assert_eq!(calls[1].len(), 2, "second snapshot has both items");
        assert_eq!(calls[1][1].source, "monitor:m");
    }

    #[test]
    fn no_on_change_set_push_still_works() {
        // Absence of a callback must not panic; the else branch is exercised.
        let q = InputQueue::new();
        let snap = q.push(human("x"));
        assert_eq!(snap.len(), 1);
    }

    // --- InboxSink (§15 U2 manager → inbox) ---------------------------------

    #[test]
    fn inbox_sink_deliver_stdout_enqueues_monitor_stdout_item() {
        let q = InputQueue::new();
        let sink = InboxSink::new(q.clone(), dummy_event_sink());
        sink.deliver_stdout("mon-a", "hello line".to_owned());
        let snap = q.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].source, "monitor:mon-a");
        assert!(snap[0].content_preview.contains("hello line"));
        // Confirm the variant/payload, not just the view.
        let drained = q.drain_pending();
        assert!(
            matches!(&drained[0], InputItem::MonitorStdout { monitor_id, lines }
                if monitor_id == "mon-a" && lines == &["hello line"]),
            "deliver_stdout must enqueue MonitorStdout: {:?}",
            drained[0]
        );
    }

    #[test]
    fn inbox_sink_deliver_stopped_enqueues_monitor_stopped_item() {
        let q = InputQueue::new();
        let sink = InboxSink::new(q.clone(), dummy_event_sink());
        sink.deliver_stopped("mon-b", MonitorStopReason::ProcessExited, Some(3));
        let drained = q.drain_pending();
        assert_eq!(drained.len(), 1);
        assert!(
            matches!(&drained[0], InputItem::MonitorStopped { monitor_id, reason, exit_code }
                if monitor_id == "mon-b"
                    && matches!(reason, MonitorStopReason::ProcessExited)
                    && *exit_code == Some(3)),
            "deliver_stopped must enqueue MonitorStopped with reason+exit: {:?}",
            drained[0]
        );
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
