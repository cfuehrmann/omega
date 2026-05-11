// SCHEMA-8 Phase 4e — T6 browser-refresh replay test.
//
// Acceptance criterion (from `backlog/schema-8.md` § T6):
//   Reloading the page after a turn completes must reconstruct the
//   feed with the same blocks in the same order with the same
//   `data-block-id`s — i.e. replaying `events.jsonl` from disk
//   reproduces the live-streamed UI exactly.
//
// `data-block-id` is the position of each event in the store's
// `events` vector (`EventBlock` reads it from the `<For>` index).
// Since `events.jsonl` is replayed in disk order on a fresh WS
// connection and `into_omega_event` is deterministic, the indices
// must be identical pre- and post-reload.
//
// Two variants:
//   1. Post-TurnEnd reload (`data_block_ids_stable_across_reload_post_turn_end`):
//      reload after the turn completes; asserts byte-stable feed reconstruction.
//   2. Mid-turn reload (`data_block_ids_stable_across_reload_mid_turn`):
//      reload between `LlmResponseStarted` and `LlmResponseEnded` while the
//      provider is still streaming; asserts that the blocks persisted before
//      the reload have stable IDs, and that the completed post-reload feed
//      contains those blocks as a prefix.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::doc_markdown,
    clippy::too_many_lines
)]

use std::time::Duration;

use omega_e2e::{MockResponse, TestHarness};

const T_TURN: Duration = Duration::from_secs(30);

/// Multi-tool + final text script: produces ≥10 events on the wire
/// (`UserMessage`, three `LlmCall`/`LlmResponseStarted`/`ToolUseBlock`
/// /`LlmResponseEnded`/`ToolCall`/`ToolResult` cycles, plus a final
/// `TextBlock` close, plus `TurnEnd`). Enough surface to make the
/// `data-block-id` equality check meaningful.
fn multi_tool_script() -> Vec<MockResponse> {
    let sleep = |id: &str| MockResponse::ToolUse {
        id: id.into(),
        name: "run_command".into(),
        input: serde_json::json!({ "command": "sleep 0.2" }),
    };
    vec![
        sleep("toolu_refresh_1"),
        sleep("toolu_refresh_2"),
        sleep("toolu_refresh_3"),
        MockResponse::Text {
            text: "done refresh".into(),
            input_tokens: 10,
            output_tokens: 5,
        },
    ]
}

async fn send_message(h: &TestHarness, content: &str) {
    h.fill("[data-testid=\"leptos-composer-input\"]", content)
        .await
        .expect("fill composer");
    h.press_key("[data-testid=\"leptos-composer-input\"]", "Enter")
        .await
        .expect("submit composer");
}

/// Snapshot the feed: for every `[data-testid="leptos-event-block"]`
/// element, capture `(data-block-id, data-event-type, data-event-kind)`.
/// We deliberately do NOT capture textContent because some renderers
/// embed live signals (streaming overlays, status pulses) that may
/// transiently differ between live-streamed and replayed traversals
/// of the same persisted state.  The structural triple is what T6
/// guarantees.
async fn snapshot_blocks(h: &TestHarness) -> Vec<(String, String, String)> {
    let js = "(() => Array.from(document.querySelectorAll(\
              '[data-testid=\"leptos-feed\"] [data-testid=\"leptos-event-block\"]'\
            )).map(el => [\
              el.getAttribute('data-block-id') || '',\
              el.getAttribute('data-event-type') || '',\
              el.getAttribute('data-event-kind') || ''\
            ]))()";
    h.eval::<Vec<(String, String, String)>>(js)
        .await
        .expect("snapshot data-block-id triples")
}

// ---------------------------------------------------------------------------
// T6 — post-TurnEnd reload reconstructs the same feed
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "browser"]
async fn data_block_ids_stable_across_reload_post_turn_end() {
    let h = TestHarness::launch().await.expect("launch");
    h.reset_calls().await.expect("reset");
    h.load_script(multi_tool_script())
        .await
        .expect("load script");

    h.new_session().await.expect("new session");
    send_message(&h, "drive a multi-tool turn").await;

    // Wait for the turn to complete.
    h.wait_for_count(
        "[data-testid=\"leptos-feed\"] \
         [data-testid=\"leptos-event-block\"][data-event-type=\"turn_end\"]",
        1,
        T_TURN,
    )
    .await
    .expect("turn_end never landed pre-reload");

    let pre = snapshot_blocks(&h).await;
    assert!(
        pre.len() >= 5,
        "expected a multi-block turn pre-reload, got {} blocks",
        pre.len()
    );

    // Capture the final assistant text from a TextBlock — this should
    // survive the reload byte-stable (the Phase 4c muting only affects
    // the legacy LlmResponse block; the TextBlock owns the body).
    let pre_text: String = h
        .eval(
            "(() => {\
               const xs = document.querySelectorAll(\
                 '[data-testid=\"leptos-feed\"] [data-event-type=\"text_block\"]');\
               if (!xs.length) return '';\
               const t = xs[xs.length - 1].querySelector(\
                 '[data-testid=\"leptos-assistant-text\"]');\
               return t ? t.textContent : '';\
             })()",
        )
        .await
        .expect("read last assistant text pre-reload");
    assert!(
        pre_text.contains("done refresh"),
        "pre-reload missing final assistant text: {pre_text:?}"
    );

    // Reload — the server replays events.jsonl from disk on the new
    // WS connection.
    h.reload().await.expect("reload page");

    // Wait until the reconstructed feed has the same block count.
    h.wait_for_count(
        "[data-testid=\"leptos-feed\"] [data-testid=\"leptos-event-block\"]",
        pre.len(),
        T_TURN,
    )
    .await
    .expect("post-reload feed never reached pre-reload size");

    // Also wait for the final turn_end to reappear so we know
    // replay finished, not just landed partial.
    h.wait_for_count(
        "[data-testid=\"leptos-feed\"] \
         [data-testid=\"leptos-event-block\"][data-event-type=\"turn_end\"]",
        1,
        T_TURN,
    )
    .await
    .expect("turn_end missing post-reload");

    let post = snapshot_blocks(&h).await;

    assert_eq!(
        pre, post,
        "data-block-id / event-type / event-kind triples drifted across reload"
    );

    // Body content sanity: the final assistant TextBlock still
    // contains the scripted "done refresh".
    let post_text: String = h
        .eval(
            "(() => {\
               const xs = document.querySelectorAll(\
                 '[data-testid=\"leptos-feed\"] [data-event-type=\"text_block\"]');\
               if (!xs.length) return '';\
               const t = xs[xs.length - 1].querySelector(\
                 '[data-testid=\"leptos-assistant-text\"]');\
               return t ? t.textContent : '';\
             })()",
        )
        .await
        .expect("read last assistant text post-reload");
    assert_eq!(
        pre_text, post_text,
        "assistant text drifted across reload: pre={pre_text:?} post={post_text:?}"
    );
}

// ---------------------------------------------------------------------------
// T6 — mid-turn reload reconstructs the same persisted prefix
// ---------------------------------------------------------------------------

/// Reload the page while the provider is still streaming (between
/// `LlmResponseStarted` and `LlmResponseEnded`) and assert that the blocks
/// that were persisted to `events.jsonl` before the reload have stable
/// `data-block-id`s in the reconstructed feed.
///
/// Uses `MockResponse::SlowText` (8 chunks × 800 ms = 6.4 s streaming window)
/// to guarantee a wide-enough gap between `LlmResponseStarted` and
/// `LlmResponseEnded` for the reload to land mid-stream.
///
/// The `TextBlock`, `LlmResponseEnded`, and `TurnEnd` events are NOT yet
/// persisted at the moment of snapshot; they arrive after the server finishes
/// streaming on the new WS connection.  The assertion therefore checks that
/// the pre-reload blocks are a prefix of the completed post-reload feed.
#[tokio::test]
#[ignore = "browser"]
async fn data_block_ids_stable_across_reload_mid_turn() {
    let h = TestHarness::launch().await.expect("launch");
    h.reset_calls().await.expect("reset");
    // 8 chunks × 800 ms inter-chunk delay ≈ 6.4 s total streaming time.
    // This gives ample time to see LlmResponseStarted, snapshot, and reload
    // before content_block_stop (which triggers TextBlock persistence).
    h.load_script(vec![MockResponse::SlowText {
        text: "mid-turn reload streaming text for replay test".into(),
        chunks: 8,
        delay_ms: 800,
    }])
    .await
    .expect("load script");

    h.new_session().await.expect("new session");
    send_message(&h, "drive a slow streaming turn").await;

    // Wait for the LlmResponseStarted event block — this confirms the turn is
    // in-flight and at least one event past UserMessage/LlmCall is persisted.
    h.wait_for_count(
        "[data-testid=\"leptos-feed\"] \
         [data-testid=\"leptos-event-block\"][data-event-type=\"llm_response_started\"]",
        1,
        T_TURN,
    )
    .await
    .expect("llm_response_started never appeared pre-reload");

    // Snapshot while the stream is in-flight (only persisted event blocks).
    let pre = snapshot_blocks(&h).await;
    assert!(
        pre.len() >= 2,
        "expected ≥2 blocks mid-stream (user_message + llm_response_started \
         at minimum), got {}",
        pre.len()
    );
    // Confirm we are genuinely mid-turn: TurnEnd must not have landed yet.
    let has_turn_end_pre = pre.iter().any(|(_, t, _)| t == "turn_end");
    assert!(
        !has_turn_end_pre,
        "turn_end appeared before reload — missed the mid-turn window; pre={pre:?}"
    );

    // Reload while the provider is still streaming.
    h.reload().await.expect("reload page");

    // The server-side agent continues to stream.  Wait for the turn to
    // complete on the reconnected WS connection.
    h.wait_for_count(
        "[data-testid=\"leptos-feed\"] \
         [data-testid=\"leptos-event-block\"][data-event-type=\"turn_end\"]",
        1,
        T_TURN,
    )
    .await
    .expect("turn_end never landed post-reload");

    let post = snapshot_blocks(&h).await;

    // The post-reload feed must have at least as many blocks as pre-reload
    // (the turn has now completed, so new blocks arrived after reconnect).
    assert!(
        post.len() >= pre.len(),
        "post-reload feed ({} blocks) is shorter than pre-reload ({} blocks); \
         post={post:?}",
        post.len(),
        pre.len()
    );

    // The blocks that were persisted before the reload must appear at the
    // same positions (same data-block-id, data-event-type, data-event-kind)
    // in the reconstructed post-reload feed.
    let post_prefix = &post[..pre.len()];
    assert_eq!(
        pre, post_prefix,
        "pre-reload blocks are not a stable prefix of the post-reload feed:\n\
         pre       = {pre:?}\n\
         post[..N] = {post_prefix:?}\n\
         post (full) = {post:?}"
    );

    // Sanity: the completed feed must contain the scripted text in a TextBlock.
    let last_text: String = h
        .eval(
            "(() => {\
               const xs = document.querySelectorAll(\
                 '[data-testid=\"leptos-feed\"] [data-event-type=\"text_block\"]');\
               if (!xs.length) return '';\
               const t = xs[xs.length - 1].querySelector(\
                 '[data-testid=\"leptos-assistant-text\"]');\
               return t ? t.textContent : '';\
             })()",
        )
        .await
        .expect("read last assistant text post-reload");
    assert!(
        last_text.contains("mid-turn reload streaming text"),
        "post-reload TextBlock missing scripted text: {last_text:?}"
    );
}
