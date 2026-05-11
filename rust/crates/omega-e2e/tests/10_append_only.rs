// SCHEMA-8 Phase 6 — T5 append-only DOM invariant.
//
// Acceptance criterion (from `backlog/schema-8.md` § T5 / item 53):
//
//   Once a feed `<EventBlock>` element has been rendered with a given
//   `data-block-id`, that id must remain present in the DOM for the
//   remainder of the session.  Events are append-only on the wire and
//   the feed renders them by their position in the store's `events`
//   vector, so the *set* of `data-block-id` strings must be
//   monotonically non-decreasing as new frames arrive.
//
// The most demanding case is a mid-stream abandon + retry: the agent
// emits `partial: true` block events for whatever content streamed
// before the abandonment, follows them with `LlmResponseDiscarded`,
// then opens a fresh `LlmResponseStarted` and re-streams.  The
// pre-discard partial blocks MUST survive — the operator should still
// be able to see what the assistant was saying when the network
// blipped.  This is the contract that distinguishes "abandon + retry"
// from "rewind".
//
// We drive the scenario via `inject_ws_frame` (a [`launch_with_ws_spy`]
// affordance) so the test exercises the leptos store + feed renderer
// in isolation from the real agent / mock server.  The synthetic
// frames mirror what the agent emits at the wire boundary on a
// mid-response retry; see `agent.rs` `make_abandonment_closers` for
// the production source of these events.
//
// Snapshot points:
//   S0  after `new_session()` settles
//   S1  + `LlmResponseStarted`
//   S2  + partial `TextBlock`         (slot 0, pre-abandon)
//   S3  + partial `ThinkingBlock`     (slot 1, pre-abandon)
//   S4  + partial `TextBlock`         (slot 2, pre-abandon)
//   S5  + `LlmResponseDiscarded`
//   S6  + `LlmResponseStarted`        (retry)
//   S7  + final `TextBlock`           (slot 0, partial=false)
//   S8  + `LlmResponseEnded`
//
// Invariant: for every i, snapshots[i-1] ⊆ snapshots[i].

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::doc_markdown,
    clippy::too_many_lines
)]

use std::time::Duration;

use omega_e2e::TestHarness;

const BLOCK_SELECTOR: &str = "[data-testid=\"leptos-feed\"] [data-testid=\"leptos-event-block\"]";

/// Capture every `data-block-id` currently rendered in the feed, in
/// document order. We only need the id strings — the invariant is
/// about set membership, not content equality.
async fn snapshot_block_ids(h: &TestHarness) -> Vec<String> {
    let js = "(() => Array.from(document.querySelectorAll(\
              '[data-testid=\"leptos-feed\"] [data-testid=\"leptos-event-block\"]'\
            )).map(el => el.getAttribute('data-block-id') || ''))()";
    h.eval::<Vec<String>>(js)
        .await
        .expect("snapshot data-block-id list")
}

/// Inject a WS frame and wait until the feed reflects exactly
/// `expected_count` event blocks. Returns the post-update snapshot.
async fn inject_and_snapshot(h: &TestHarness, frame: &str, expected_count: usize) -> Vec<String> {
    h.inject_ws_frame(frame).await.expect("inject_ws_frame");
    h.wait_for_count(BLOCK_SELECTOR, expected_count, Duration::from_secs(5))
        .await
        .unwrap_or_else(|e| {
            panic!(
                "feed never reached {expected_count} blocks after injecting:\n  \
                 {frame}\nerror: {e}"
            )
        });
    snapshot_block_ids(h).await
}

#[tokio::test]
#[ignore = "browser"]
async fn data_block_ids_are_append_only_across_abandon_retry() {
    let h = TestHarness::launch_with_ws_spy().await.expect("launch");
    h.new_session().await.expect("new session");

    // S0 — baseline after the session is established.  The feed may
    // already contain a `session_started` block depending on the
    // session-renderer scope; whatever's there now must persist.
    let s0 = snapshot_block_ids(&h).await;
    let base = s0.len();

    // -- Phase 1: open a response, stream three partial blocks. --
    //
    // The frames below replicate exactly what the agent emits on the
    // wire when a turn is mid-stream and a network hiccup forces a
    // retry: a partial-flagged block per surviving slot, followed by
    // `LlmResponseDiscarded`.  (Slot indices are agent-internal; the
    // wire events do not carry them — they're collapsed into emission
    // order.  We pick distinctive payloads so a renderer bug that
    // swapped or merged blocks would be visible in the failure dump.)

    let s1 = inject_and_snapshot(
        &h,
        r#"{"type":"llm_response_started","time":"2024-01-01T00:00:00.000Z"}"#,
        base + 1,
    )
    .await;

    let s2 = inject_and_snapshot(
        &h,
        r#"{"type":"text_block","time":"2024-01-01T00:00:01.000Z","text":"T5-pre-abandon-text-0","partial":true}"#,
        base + 2,
    )
    .await;

    let s3 = inject_and_snapshot(
        &h,
        r#"{"type":"thinking_block","time":"2024-01-01T00:00:02.000Z","thinking":"T5-pre-abandon-thinking-1","signature":"T5-SIG-PRE-1","partial":true}"#,
        base + 3,
    )
    .await;

    let s4 = inject_and_snapshot(
        &h,
        r#"{"type":"text_block","time":"2024-01-01T00:00:03.000Z","text":"T5-pre-abandon-text-2","partial":true}"#,
        base + 4,
    )
    .await;

    // -- Phase 2: discard the in-flight response, open a fresh one,
    //    stream a final non-partial text block, then close.

    let s5 = inject_and_snapshot(
        &h,
        r#"{"type":"llm_response_discarded","time":"2024-01-01T00:00:04.000Z"}"#,
        base + 5,
    )
    .await;

    let s6 = inject_and_snapshot(
        &h,
        r#"{"type":"llm_response_started","time":"2024-01-01T00:00:05.000Z"}"#,
        base + 6,
    )
    .await;

    let s7 = inject_and_snapshot(
        &h,
        r#"{"type":"text_block","time":"2024-01-01T00:00:06.000Z","text":"T5-final-text-0","partial":false}"#,
        base + 7,
    )
    .await;

    let s8 = inject_and_snapshot(
        &h,
        r#"{"type":"llm_response_ended","time":"2024-01-01T00:00:07.000Z","stopReason":"end_turn","usage":{"input_tokens":10,"output_tokens":5},"contextHash":""}"#,
        base + 8,
    )
    .await;

    // -- Invariant: every snapshot is a superset of the previous one.

    let snapshots = [
        ("S0 (baseline)", &s0),
        ("S1 (response_started)", &s1),
        ("S2 (+partial text 0)", &s2),
        ("S3 (+partial thinking 1)", &s3),
        ("S4 (+partial text 2)", &s4),
        ("S5 (response_discarded)", &s5),
        ("S6 (response_started retry)", &s6),
        ("S7 (+final text)", &s7),
        ("S8 (response_ended)", &s8),
    ];

    for window in snapshots.windows(2) {
        let (prev_name, prev) = window[0];
        let (curr_name, curr) = window[1];
        for id in prev {
            assert!(
                curr.contains(id),
                "append-only invariant violated: data-block-id {id:?} \
                 present at {prev_name} but absent at {curr_name}.\n  \
                 prev: {prev:?}\n  curr: {curr:?}"
            );
        }
        assert!(
            curr.len() >= prev.len(),
            "feed shrank: {prev_name} had {} blocks, {curr_name} has {} blocks",
            prev.len(),
            curr.len()
        );
    }

    // Sanity: end-to-end growth — 8 injections, 8 new blocks expected
    // (LlmResponseStarted / partial blocks / LlmResponseDiscarded /
    // LlmResponseStarted / final TextBlock / LlmResponseEnded all
    // render as their own `<EventBlock>` rows).
    assert_eq!(
        s8.len() - s0.len(),
        8,
        "expected exactly 8 new blocks across the abandon+retry sequence; \
         s0={s0:?} s8={s8:?}"
    );
}
