#![allow(clippy::format_collect)]
// (0..N).map(|i| format!("...{i}\n")).collect::<String>() is clearer in tests

// Phase WEB-1 — scroll-tailing + jump-to-bottom button e2e test.
//
// State-machine covered:
//
//   tailing (established via scroll-to-bottom)
//     → scroll up  → non-tailing (button appears, new events don't move view)
//     → click ↓   → tailing (button gone, new events follow sentinel)
//
// Observable surface: `data-auto-scroll` on `[data-testid="leptos-feed"]`
// and `[data-testid="scroll-to-bottom"]`.  No pixel-position assertions
// except in `scroll_lands_at_bottom_after_copy_buttons` which explicitly
// measures the scroll gap to pin the rAF-deferred scroll fix.
//
// `tailing_survives_rapid_streaming_after_button_click` is the regression
// test for the bug where the ↓ button called `sentinel.scroll_into_view()`
// without updating `prog_state`.  Chromium 148+ deferred that scroll event
// as a macrotask; new streaming content arrived before the event fired,
// making `should_autoscroll` return false and silently killing tailing.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::doc_markdown,
    clippy::too_many_lines
)]

use std::time::Duration;

use omega_e2e::{MockResponse, TestHarness};

const T: Duration = Duration::from_secs(8);
const T_TURN: Duration = Duration::from_secs(20);

// ---------------------------------------------------------------------------
// Script builder — ≥ 10 events with substantial rendered height
// ---------------------------------------------------------------------------

/// Two tool-call steps (llm_call + tool_call + tool_result each) followed
/// by a long text reply, plus user_message / session events.
/// That gives comfortably > 10 persisted event blocks.  The final text is
/// 20 lines so the rendered height exceeds any reasonable viewport.
fn tall_script() -> Vec<MockResponse> {
    vec![
        MockResponse::ToolUse {
            id: "toolu_scroll_0".into(),
            name: "run_command".into(),
            input: serde_json::json!({ "command": "echo scroll_test_a" }),
        },
        MockResponse::ToolUse {
            id: "toolu_scroll_1".into(),
            name: "run_command".into(),
            input: serde_json::json!({ "command": "echo scroll_test_b" }),
        },
        MockResponse::Text {
            text: (0..20)
                .map(|i| format!("Scroll test line {i}: content to fill the feed panel.\n"))
                .collect::<String>(),
            input_tokens: 30,
            output_tokens: 60,
        },
    ]
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn send_message(h: &TestHarness, content: &str) {
    h.fill("[data-testid=\"leptos-composer-input\"]", content)
        .await
        .expect("fill composer");
    h.press_key("[data-testid=\"leptos-composer-input\"]", "Enter")
        .await
        .expect("submit composer");
}

// ---------------------------------------------------------------------------
// scroll_tailing — full state-machine
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "browser"]
async fn scroll_tailing() {
    let h = TestHarness::launch().await.expect("launch");
    h.reset_calls().await.expect("reset");
    h.load_script(tall_script()).await.expect("load script");
    h.new_session().await.expect("new session");

    // Send the first turn and wait for it to complete.
    send_message(&h, "go scroll test").await;
    h.wait_for_count(
        "[data-testid=\"leptos-feed\"] [data-event-type=\"turn_end\"]",
        1,
        T_TURN,
    )
    .await
    .expect("first turn_end never landed");

    // ── Establish tailing mode ────────────────────────────────────────────
    //
    // Scroll the feed to the very bottom programmatically.  This exercises
    // the "scroll back to bottom → tailing restored" path and gives us a
    // deterministic starting point regardless of any scroll events that may
    // have fired during the first turn.
    h.eval::<bool>(
        "(() => { \
           const el = document.querySelector('[data-testid=\"leptos-feed\"]'); \
           if (el) el.scrollTop = el.scrollHeight; \
           return true; \
         })()",
    )
    .await
    .expect("scroll feed to bottom");

    h.wait_for_attr(
        "[data-testid=\"leptos-feed\"]",
        "data-auto-scroll",
        "true",
        T,
    )
    .await
    .expect("data-auto-scroll should be true after scrolling to bottom");

    // ── Phase 1: tailing — button must be absent ──────────────────────────
    let btn_present: bool = h
        .eval("!!document.querySelector('[data-testid=\"scroll-to-bottom\"]')")
        .await
        .expect("eval btn_present");
    assert!(
        !btn_present,
        "scroll-to-bottom button should be absent while tailing"
    );

    // ── Phase 2: scroll up → non-tailing ─────────────────────────────────
    h.eval::<bool>(
        "(() => { \
           document.querySelector('[data-testid=\"leptos-feed\"]').scrollTop = 0; \
           return true; \
         })()",
    )
    .await
    .expect("scroll feed to top");

    h.wait_for_attr(
        "[data-testid=\"leptos-feed\"]",
        "data-auto-scroll",
        "false",
        T,
    )
    .await
    .expect("data-auto-scroll should become false after scroll-up");

    // The ↓ button must appear in non-tailing mode.
    h.wait_for_selector("[data-testid=\"scroll-to-bottom\"]", T)
        .await
        .expect("scroll-to-bottom button should appear in non-tailing mode");

    // ── Phase 3: new events arrive while non-tailing ──────────────────────
    //
    // Queue a second turn and confirm that completing it does NOT flip
    // auto_scroll back to true and does NOT cause the button to disappear.
    // The button being present after new events is the observable proof that
    // the view did not scroll to the bottom.
    h.load_script(vec![MockResponse::Text {
        text: "second turn — should not move view".into(),
        input_tokens: 5,
        output_tokens: 5,
    }])
    .await
    .expect("load second script");

    send_message(&h, "go second turn").await;
    h.wait_for_count(
        "[data-testid=\"leptos-feed\"] [data-event-type=\"turn_end\"]",
        2,
        T_TURN,
    )
    .await
    .expect("second turn_end never landed");

    // Feed must still be in non-tailing mode.
    let auto_scroll_after: String = h
        .eval(
            "document.querySelector('[data-testid=\"leptos-feed\"]')\
              .getAttribute('data-auto-scroll')",
        )
        .await
        .expect("read data-auto-scroll after second turn");
    assert_eq!(
        auto_scroll_after, "false",
        "data-auto-scroll should stay false while non-tailing after new events"
    );

    // Button must still be present (the view did not auto-scroll to bottom).
    h.wait_for_selector("[data-testid=\"scroll-to-bottom\"]", T)
        .await
        .expect("scroll-to-bottom button should remain present after non-tailing new events");

    // ── Phase 4: click ↓ → back to tailing ──────────────────────────────
    h.click("[data-testid=\"scroll-to-bottom\"]")
        .await
        .expect("click scroll-to-bottom button");

    h.wait_for_attr(
        "[data-testid=\"leptos-feed\"]",
        "data-auto-scroll",
        "true",
        T,
    )
    .await
    .expect("data-auto-scroll should return to true after button click");

    h.wait_for_detached("[data-testid=\"scroll-to-bottom\"]", T)
        .await
        .expect("scroll-to-bottom button should disappear after returning to tailing");

    // ── Phase 5: new turn while tailing → sentinel followed ──────────────
    //
    // A new streamed turn is issued while the feed is in tailing mode.
    // Tailing must be maintained throughout (data-auto-scroll stays "true"
    // and the button stays absent).
    h.load_script(vec![MockResponse::SlowText {
        text: "third turn streaming to confirm tailing".into(),
        chunks: 4,
        delay_ms: 80,
    }])
    .await
    .expect("load third script");

    send_message(&h, "go third turn").await;
    h.wait_for_count(
        "[data-testid=\"leptos-feed\"] [data-event-type=\"turn_end\"]",
        3,
        T_TURN,
    )
    .await
    .expect("third turn_end never landed");

    // Tailing mode must be maintained throughout the third turn.
    h.wait_for_attr(
        "[data-testid=\"leptos-feed\"]",
        "data-auto-scroll",
        "true",
        T,
    )
    .await
    .expect("data-auto-scroll should remain true after tailing through third turn");

    // Button must remain absent while tailing.
    let btn_final: bool = h
        .eval("!!document.querySelector('[data-testid=\"scroll-to-bottom\"]')")
        .await
        .expect("eval btn_final");
    assert!(
        !btn_final,
        "scroll-to-bottom button should remain absent while tailing"
    );
}

// ---------------------------------------------------------------------------
// scroll_lands_at_bottom_after_copy_buttons
// ---------------------------------------------------------------------------

/// Regression test for the rAF-deferred auto-scroll fix.
///
/// `js_add_copy_buttons` (called from `MarkdownBody`'s post-mount Effect)
/// appends a `<button class="code-copy-btn">` to every `<pre>` in the
/// rendered markdown, increasing `scrollHeight` *after* the naive
/// synchronous scroll would have fired.  The rAF-deferred scroll in
/// `ConversationFeed` must read `scrollHeight` only after all child Effects
/// have committed their DOM mutations.
///
/// **Red path** (without the rAF fix): the auto-scroll Effect reads
/// `scrollHeight` before copy buttons are injected; the scroll lands several
/// tens of pixels short of the true bottom, so `wait_for_feed_at_bottom`
/// times out and the test fails.
///
/// **Green path** (with the rAF fix): the rAF callback fires after all
/// synchronous Effects, sees the final `scrollHeight`, and the gap is < 3 px.
#[tokio::test]
#[ignore = "browser"]
async fn scroll_lands_at_bottom_after_copy_buttons() {
    let h = TestHarness::launch().await.expect("launch");
    h.reset_calls().await.expect("reset");

    // 30 filler lines overflow the viewport; five fenced code blocks each
    // produce one copy button (~30 px each) via js_add_copy_buttons — that’s
    // ~150 px of height added after the naive scroll would have been set.
    let filler: String = (0..30)
        .map(|i| format!("Filler line {i} — padding so the feed overflows the viewport.\n"))
        .collect();
    let code_blocks: String = ["rust", "python", "javascript", "bash", "go"]
        .iter()
        .enumerate()
        .map(|(i, lang)| format!("\n```{lang}\nfn example_{i}() {{ /* {lang} */ }}\n```\n"))
        .collect();
    let text = format!("{filler}\nCode blocks (each adds a copy button):\n{code_blocks}");

    h.load_script(vec![MockResponse::Text {
        text,
        input_tokens: 20,
        output_tokens: 80,
    }])
    .await
    .expect("load script");

    h.new_session().await.expect("new session");

    send_message(&h, "show me some code").await;

    // Wait for all events to persist, including turn_end.
    h.wait_for_count(
        "[data-testid=\"leptos-feed\"] [data-event-type=\"turn_end\"]",
        1,
        T_TURN,
    )
    .await
    .expect("turn_end never landed");

    // Wait for copy buttons — proof that js_add_copy_buttons has run and
    // expanded scrollHeight beyond the pre-enhancement value.
    h.wait_for_selector("[data-testid=\"code-copy-btn\"]", T)
        .await
        .expect("copy buttons should appear after markdown rendering");

    // The feed must be scrolled to the physical bottom (gap < 3 px).
    // Without the rAF fix, the naive scroll targets the pre-copy-button
    // scrollHeight, leaving a gap of ~150 px that never closes.
    h.wait_for_feed_at_bottom(3.0, T).await.expect(
        "feed should be at the true bottom after copy-button injection \
         (rAF-deferred scroll fix)",
    );
}

// ---------------------------------------------------------------------------
// tailing_survives_rapid_streaming_after_button_click
// ---------------------------------------------------------------------------

/// Regression test for the deferred-scroll-event bug.
///
/// **Root cause**: the ↓ button previously called
/// `sentinel.scroll_into_view()` *without* updating `prog_state`.  On
/// Chromium 148+ that scroll is dispatched as a macrotask.  When a
/// streaming chunk arrives in the gap (rAF fires, `set_scroll_top` runs,
/// `prog_state` is updated to a *newer* scrollHeight), the old event
/// fails the echo check (wrong position) and
/// `should_autoscroll(old_sh − clientH, clientH, new_sh, 40)` returns
/// `false` — silently killing tailing mid-turn.
///
/// **Red path** (without fix): at least one of the ~50 rapid-arrival
/// polls sees `data-auto-scroll = "false"` and the assertion fires.
///
/// **Green path** (with fix): the button handler uses
/// `section.set_scroll_top(sh)` and stamps `prog_state`, so the deferred
/// echo is correctly suppressed and tailing is never disabled.
#[tokio::test]
#[ignore = "browser"]
async fn tailing_survives_rapid_streaming_after_button_click() {
    let h = TestHarness::launch().await.expect("launch");
    h.reset_calls().await.expect("reset");

    // ── Phase 1: overflow the feed so scroll-up is possible ──────────────
    h.load_script(tall_script())
        .await
        .expect("load tall script");
    h.new_session().await.expect("new session");
    send_message(&h, "fill the feed").await;
    h.wait_for_count(
        "[data-testid=\"leptos-feed\"] [data-event-type=\"turn_end\"]",
        1,
        T_TURN,
    )
    .await
    .expect("first turn_end");

    // ── Phase 2: disable tailing ─────────────────────────────────────────
    h.eval::<bool>(
        "(() => { \
           const el = document.querySelector('[data-testid=\"leptos-feed\"]'); \
           if (el) el.scrollTop = 0; \
           return true; \
         })()",
    )
    .await
    .expect("scroll to top");
    h.wait_for_attr(
        "[data-testid=\"leptos-feed\"]",
        "data-auto-scroll",
        "false",
        T,
    )
    .await
    .expect("data-auto-scroll should be false after scroll-up");

    // ── Phase 3: start a rapid-streaming turn ────────────────────────────
    //
    // 60 chunks × 15 ms = 900 ms total stream window.  Each chunk is a
    // full line of text (≈20 px), so after a few chunks arrive the
    // scrollHeight grows well past the 40-px threshold that triggers the
    // bug.  The ↓ button is already visible (tailing is off from Phase 2).
    h.load_script(vec![MockResponse::SlowText {
        text: (0..60)
            .map(|i| format!("Rapid stream line {i}: padding to grow scrollHeight quickly.\n"))
            .collect(),
        chunks: 60,
        delay_ms: 15,
    }])
    .await
    .expect("load rapid script");
    send_message(&h, "start rapid streaming").await;

    // Wait for the first streaming chunk to land — the turn is now live.
    h.wait_for_selector("[data-testid=\"leptos-streaming-text\"]", T)
        .await
        .expect("streaming text should appear");

    // ── Phase 4: click ↓ while streaming is active ───────────────────────
    h.click("[data-testid=\"scroll-to-bottom\"]")
        .await
        .expect("click ↓");
    h.wait_for_attr(
        "[data-testid=\"leptos-feed\"]",
        "data-auto-scroll",
        "true",
        T,
    )
    .await
    .expect("tailing should be re-enabled immediately after button click");

    // ── Phase 5: tailing must stay ON through streaming AND turn completion ──
    //
    // We poll every 20 ms and continue until the turn-end event has been
    // in the feed for at least 300 ms.  This covers two failure modes:
    //   a) The deferred scroll_into_view echo arrives during streaming and
    //      flips auto_scroll to false (the original sentinel.scroll_into_view
    //      race).
    //   b) When the streaming text is removed at turn completion the browser
    //      fires a scroll event with a scroll_top that is below the new
    //      bottom (because event blocks have already been added).  Without
    //      the scroll_pending guard, should_autoscroll() returns false and
    //      tailing is killed right as the turn finishes.
    let poll_start = std::time::Instant::now();
    let mut grace_start: Option<std::time::Instant> = None;
    loop {
        let val: String = h
            .eval(
                "document.querySelector('[data-testid=\"leptos-feed\"]')\
                  .getAttribute('data-auto-scroll')",
            )
            .await
            .expect("read data-auto-scroll");
        assert_eq!(
            val, "true",
            "tailing was disabled mid-turn (scroll_pending race or \
             deferred sentinel echo)"
        );

        // Check whether the second turn_end has landed.
        let turn_count: i64 = h
            .eval(
                "document.querySelectorAll(\
                  '[data-testid=\"leptos-feed\"] [data-event-type=\"turn_end\"]'\
                ).length",
            )
            .await
            .unwrap_or(0);
        if turn_count >= 2 {
            match grace_start {
                None => grace_start = Some(std::time::Instant::now()),
                Some(t) if t.elapsed() >= Duration::from_millis(300) => break,
                Some(_) => {}
            }
        } else {
            // Reset grace if somehow turn_end disappeared (shouldn't happen).
            grace_start = None;
        }

        assert!(
            poll_start.elapsed() <= Duration::from_secs(10),
            "second turn_end never landed within 10 s",
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
