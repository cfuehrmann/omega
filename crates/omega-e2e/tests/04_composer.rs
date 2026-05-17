//! Port of `e2e/leptos-composer.spec.ts` (8 cases).
//!
//! Drives the composer at the site root against `mock-omega-server`.
//! Covers Send / Pause-during-tool / Continue-with-interjection /
//! Pause-then-Abort / Switch-model-idle / Switch-effort-idle /
//! @-completion / Stub-composer-removed.
//!
//! Determinism note: every flow polls
//! `[data-testid="leptos-composer"][data-turn-state]` (ground truth
//! mirrored from the server's `session_info`) rather than rendered
//! button text.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::doc_markdown
)]

use std::time::Duration;

use omega_e2e::{MockResponse, TestHarness, ToolUseSpec};
use serde_json::json;

const COMPOSER: &str = "[data-testid=\"leptos-composer\"]";
const INPUT: &str = "[data-testid=\"leptos-composer-input\"]";
const PRIMARY: &str = "[data-testid=\"leptos-composer-primary\"]";
const ABORT: &str = "[data-testid=\"leptos-composer-abort\"]";
const MODEL: &str = "[data-testid=\"leptos-composer-model\"]";
const EFFORT: &str = "[data-testid=\"leptos-composer-effort\"]";
const FEED: &str = "[data-testid=\"leptos-feed\"]";
const TURN_END: &str = "[data-testid=\"leptos-feed\"] [data-event-type=\"turn_end\"]";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn pong_script() -> Vec<MockResponse> {
    vec![MockResponse::Text {
        text: "pong".into(),
        input_tokens: 10,
        output_tokens: 5,
    }]
}

fn sleep_tool(id: &str, secs: &str) -> MockResponse {
    MockResponse::ToolUse {
        id: id.into(),
        name: "run_command".into(),
        input: json!({ "command": format!("sleep {secs}") }),
    }
}

/// Mirror of `SCRIPTS.twoPauses()` from the original Playwright spec.
fn two_pauses_script() -> Vec<MockResponse> {
    vec![
        sleep_tool("toolu_tp_1", "0.6"),
        sleep_tool("toolu_tp_2", "0.6"),
        sleep_tool("toolu_tp_3", "0.6"),
        sleep_tool("toolu_tp_4", "0.6"),
        MockResponse::Text {
            text: "done two pauses".into(),
            input_tokens: 10,
            output_tokens: 5,
        },
    ]
}

/// Mirror of `SCRIPTS.abortSleep()`.
fn abort_sleep_script() -> Vec<MockResponse> {
    vec![MockResponse::ToolUse {
        id: "toolu_sleep_abort".into(),
        name: "run_command".into(),
        input: json!({ "command": "sleep 10" }),
    }]
}

/// Suppress an unused-helper warning (we only use this through
/// `tools` argument).
#[allow(dead_code)]
fn tool_spec(id: &str, name: &str, input: serde_json::Value) -> ToolUseSpec {
    ToolUseSpec {
        id: id.into(),
        name: name.into(),
        input,
    }
}

/// Wait for `[data-testid="leptos-composer"][data-turn-state="…"]`.
async fn wait_for_turn_state(h: &TestHarness, expected: &str, timeout: Duration) {
    h.wait_for_attr(COMPOSER, "data-turn-state", expected, timeout)
        .await
        .unwrap_or_else(|e| panic!("turn_state never reached {expected:?}: {e}"));
}

// ---------------------------------------------------------------------------
// 1. Send — happy path
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "browser"]
async fn composer_send_pong() {
    let h = TestHarness::launch().await.expect("launch");

    h.reset_calls().await.expect("reset_calls");
    h.load_script(pong_script()).await.expect("load_script");
    h.new_session().await.expect("new_session");

    // Primary starts as data-action="send".
    let action = h.attr(PRIMARY, "data-action").await.expect("attr");
    assert_eq!(action.as_deref(), Some("send"));
    let label = h.text_content(PRIMARY).await.expect("primary text");
    assert_eq!(label.trim(), "Send ⏎");

    h.fill(INPUT, "ping").await.expect("fill");
    h.press_key(INPUT, "Enter").await.expect("submit");

    h.wait_for_count(TURN_END, 1, Duration::from_secs(10))
        .await
        .expect("turn_end never landed");

    // Final assistant text block carries "pong" (SCHEMA-8 Phase 4c —
    // assistant body now lives in `text_block`, not in `llm_response`).
    let body: String = h
        .eval(
            r#"(() => {
                const blocks = document.querySelectorAll(
                    '[data-testid="leptos-feed"] [data-event-type="text_block"] [data-testid="leptos-assistant-text"]'
                );
                if (blocks.length === 0) return '';
                return blocks[blocks.length - 1].textContent;
            })()"#,
        )
        .await
        .expect("read assistant text");
    assert!(body.contains("pong"), "expected 'pong' in: {body:?}");

    // Composer cleared; back to "send".
    let value: String = h
        .eval(&format!("document.querySelector('{INPUT}').value"))
        .await
        .expect("read input");
    assert_eq!(value, "");
    let action = h.attr(PRIMARY, "data-action").await.expect("attr");
    assert_eq!(action.as_deref(), Some("send"));
}

// ---------------------------------------------------------------------------
// 2. Pause-during-tool
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "browser"]
async fn composer_pause_during_tool() {
    let h = TestHarness::launch().await.expect("launch");

    h.reset_calls().await.expect("reset_calls");
    h.load_script(two_pauses_script())
        .await
        .expect("load_script");
    h.new_session().await.expect("new_session");

    h.fill(INPUT, "go pause").await.expect("fill");
    h.click(PRIMARY).await.expect("click send");

    wait_for_turn_state(&h, "running", Duration::from_secs(10)).await;

    // Primary flips to "pause".
    let action = h.attr(PRIMARY, "data-action").await.expect("attr");
    assert_eq!(action.as_deref(), Some("pause"));

    h.click(PRIMARY).await.expect("click pause");
    wait_for_turn_state(&h, "pause_requested", Duration::from_secs(5)).await;
    wait_for_turn_state(&h, "paused", Duration::from_secs(15)).await;

    // In Paused: primary becomes "continue", abort button visible.
    let action = h.attr(PRIMARY, "data-action").await.expect("attr");
    assert_eq!(action.as_deref(), Some("continue"));
    h.wait_for_selector(ABORT, Duration::from_secs(2))
        .await
        .expect("abort button missing");

    // Continue → back to idle.
    h.click(PRIMARY).await.expect("click continue");
    wait_for_turn_state(&h, "idle", Duration::from_secs(30)).await;

    // turn_paused was persisted as a feed event.
    h.wait_for_count(
        &format!("{FEED} [data-event-type=\"turn_paused\"]"),
        1,
        Duration::from_secs(2),
    )
    .await
    .expect("turn_paused never persisted");
}

// ---------------------------------------------------------------------------
// 3. Continue with interjection
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "browser"]
async fn composer_continue_with_interjection() {
    let h = TestHarness::launch().await.expect("launch");

    h.reset_calls().await.expect("reset_calls");
    h.load_script(two_pauses_script())
        .await
        .expect("load_script");
    h.new_session().await.expect("new_session");

    h.fill(INPUT, "trigger interjection").await.expect("fill");
    h.click(PRIMARY).await.expect("click send");
    wait_for_turn_state(&h, "running", Duration::from_secs(10)).await;

    // Pause mid-flight.
    h.click(PRIMARY).await.expect("click pause");
    wait_for_turn_state(&h, "paused", Duration::from_secs(15)).await;

    // Type interjection while paused, then continue.
    h.fill(INPUT, "actually focus on src/web/server.rs")
        .await
        .expect("fill interjection");
    h.click(PRIMARY).await.expect("click continue");

    // Turn resumes.
    wait_for_turn_state(&h, "running", Duration::from_secs(5)).await;

    // Textarea cleared on continue.
    let value: String = h
        .eval(&format!("document.querySelector('{INPUT}').value"))
        .await
        .expect("read input");
    assert_eq!(value, "");

    // turn_continued landed in the feed.
    h.wait_for_count(
        &format!("{FEED} [data-event-type=\"turn_continued\"]"),
        1,
        Duration::from_secs(5),
    )
    .await
    .expect("turn_continued never landed");

    // Wait for completion.
    wait_for_turn_state(&h, "idle", Duration::from_secs(30)).await;

    // No spurious turn_interrupted.
    let interrupted: u32 = h
        .eval(&format!(
            "document.querySelectorAll('{FEED} [data-event-type=\"turn_interrupted\"]').length"
        ))
        .await
        .expect("count interrupted");
    assert_eq!(
        interrupted, 0,
        "turn_interrupted should not fire on clean continue"
    );
}

// ---------------------------------------------------------------------------
// 4. Pause then Abort
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "browser"]
async fn composer_pause_then_abort() {
    let h = TestHarness::launch().await.expect("launch");

    h.reset_calls().await.expect("reset_calls");
    h.load_script(abort_sleep_script())
        .await
        .expect("load_script");
    h.new_session().await.expect("new_session");

    h.fill(INPUT, "go abort").await.expect("fill");
    h.click(PRIMARY).await.expect("click send");
    wait_for_turn_state(&h, "running", Duration::from_secs(10)).await;

    // Pause first — abort is available as secondary during PauseRequested.
    h.click(PRIMARY).await.expect("click pause");

    // While in pause_requested, primary is "continue" (pre-commit); abort is
    // the secondary button (Abort ⎋).
    h.wait_for_attr(PRIMARY, "data-action", "continue", Duration::from_secs(5))
        .await
        .expect("primary never flipped to continue in pause_requested");
    h.wait_for_selector(ABORT, Duration::from_secs(2))
        .await
        .expect("abort secondary button missing during pause_requested");

    // Click the secondary Abort button.
    h.click(ABORT).await.expect("click abort");

    wait_for_turn_state(&h, "idle", Duration::from_secs(15)).await;

    h.wait_for_count(
        &format!("{FEED} [data-event-type=\"turn_interrupted\"]"),
        1,
        Duration::from_secs(2),
    )
    .await
    .expect("turn_interrupted never landed");
}

// ---------------------------------------------------------------------------
// 5. Switch model while idle (regression for 8e2106b)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "browser"]
async fn composer_switch_model_idle() {
    let h = TestHarness::launch().await.expect("launch");

    h.reset_calls().await.expect("reset_calls");
    h.load_script(pong_script()).await.expect("load_script");
    h.new_session().await.expect("new_session");

    // Run one full turn so the bug-prone "stale lastTurnEnd.model"
    // path is exercised.
    h.fill(INPUT, "ping").await.expect("fill");
    h.press_key(INPUT, "Enter").await.expect("submit");
    wait_for_turn_state(&h, "idle", Duration::from_secs(10)).await;

    // Sanity: server default is sonnet-4-6.
    let cur: String = h
        .eval(&format!("document.querySelector('{MODEL}').value"))
        .await
        .expect("model.value");
    assert_eq!(cur, "claude-sonnet-4-6");

    h.select_option(MODEL, "claude-opus-4-7")
        .await
        .expect("select opus");

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let v: String = h
            .eval(&format!("document.querySelector('{MODEL}').value"))
            .await
            .expect("model.value poll");
        if v == "claude-opus-4-7" {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "model select never reflected opus, last = {v:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

// ---------------------------------------------------------------------------
// 6. Switch effort while idle
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "browser"]
async fn composer_switch_effort_idle() {
    let h = TestHarness::launch().await.expect("launch");

    h.reset_calls().await.expect("reset_calls");
    h.load_script(pong_script()).await.expect("load_script");
    h.new_session().await.expect("new_session");

    let cur: String = h
        .eval(&format!("document.querySelector('{EFFORT}').value"))
        .await
        .expect("effort.value");
    assert_eq!(cur, "medium");

    h.select_option(EFFORT, "high").await.expect("select high");

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let v: String = h
            .eval(&format!("document.querySelector('{EFFORT}').value"))
            .await
            .expect("effort.value poll");
        if v == "high" {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "effort never reflected high, last = {v:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

// ---------------------------------------------------------------------------
// 7. File-completion accept
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "browser"]
async fn composer_completion_accept() {
    let h = TestHarness::launch().await.expect("launch");

    h.reset_calls().await.expect("reset_calls");
    h.load_script(pong_script()).await.expect("load_script");
    h.new_session().await.expect("new_session");

    // Type @crates/ — popup should appear.
    h.fill(INPUT, "@crates/").await.expect("fill");
    h.wait_for_selector(
        "[data-testid=\"leptos-composer-completion\"]",
        Duration::from_secs(5),
    )
    .await
    .expect("completion popup never appeared");

    // Wait for the completion items to settle to the children of rust/.
    //
    // Timing race: `fill` types each character individually, so `on_input`
    // fires (and `query_completion` is called) for every prefix: "", "c",
    // "cr", "cra", "crat", "crate", "crates", "crates/".  The fetch for
    // prefix "crates" may return first and open the popup with ["crates/"],
    // before the fetch for prefix "crates/" arrives and replaces it with
    // the actual children ("crates/omega-agent/", …).  Reading `first`
    // while the popup still shows the stale ["crates/"] entry causes the
    // subsequent `format!("@{first}")` assertion to disagree with what
    // Enter actually accepted (which by that point is the settled item).
    //
    // Solution: poll until no item is exactly "crates/" — which is the
    // intermediate sentinel — and at least one item is present.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let first = loop {
        let candidate: String = h
            .eval(
                "document.querySelector('[data-testid=\"leptos-composer-completion-item\"]')\
                 ?.getAttribute('data-completion') ?? ''",
            )
            .await
            .expect("first data-completion poll");
        // Intermediate state: popup shows ["rust/"] from the "rust" prefix
        // query.  Settled state: children such as "rust/.cargo/".
        if !candidate.is_empty() && candidate != "crates/" {
            break candidate;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "completion items never settled to children of crates/ (last = {candidate:?})"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert!(!first.is_empty(), "first item missing data-completion");

    // ArrowDown highlights first; Enter accepts.
    h.press_key(INPUT, "ArrowDown").await.expect("ArrowDown");
    h.press_key(INPUT, "Enter").await.expect("Enter");

    let value: String = h
        .eval(&format!("document.querySelector('{INPUT}').value"))
        .await
        .expect("input.value");
    assert_eq!(
        value,
        format!("@{first}"),
        "expected @<accepted> in textarea"
    );

    // If it's a file (no trailing /), popup closes.
    if !first.ends_with('/') {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            let visible: u32 = h
                .eval(
                    "document.querySelectorAll('[data-testid=\"leptos-composer-completion\"]').length",
                )
                .await
                .expect("popup count");
            if visible == 0 {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "popup did not close after file accept"
            );
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
    }
}

// ---------------------------------------------------------------------------
// 8. Composer hidden before any session exists
// ---------------------------------------------------------------------------

/// The composer must not be in the DOM until a session is active.
/// Before any session: picker auto-opens, WS is connected, but
/// `session_info` is None so the `<Show>` guard keeps the composer out.
#[tokio::test]
#[ignore = "browser"]
async fn composer_hidden_without_session() {
    let h = TestHarness::launch().await.expect("launch");
    // WS is connected and the picker has auto-opened, but no session
    // exists yet. Give the reactive system a tick to settle.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let present: bool = h
        .eval("!!document.querySelector('[data-testid=\"leptos-composer\"]')")
        .await
        .expect("eval composer presence");
    assert!(
        !present,
        "composer must not be in the DOM before a session exists"
    );
}

// ---------------------------------------------------------------------------
// 9. Composer appears once a session is created
// ---------------------------------------------------------------------------

/// The inverse: creating a session via `+ new session` must cause the
/// composer to mount. Confirms the `<Show when=session_has_loaded>` guard
/// fires in both directions.
#[tokio::test]
#[ignore = "browser"]
async fn composer_visible_after_session() {
    let h = TestHarness::launch().await.expect("launch");
    // No session yet — composer absent.
    let before: bool = h
        .eval("!!document.querySelector('[data-testid=\"leptos-composer\"]')")
        .await
        .expect("eval before session");
    assert!(!before, "composer must be absent before session creation");

    h.new_session().await.expect("new session");

    // Session now active — composer must mount.
    h.wait_for_selector(COMPOSER, Duration::from_secs(5))
        .await
        .expect("composer did not appear after session was created");
}

// ---------------------------------------------------------------------------
// 10. Stub composer is gone (negative)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "browser"]
async fn composer_stub_is_gone() {
    let h = TestHarness::launch().await.expect("launch");
    // Composer only renders once a session is active.
    h.new_session().await.expect("new session");

    // Real composer mounted.
    h.wait_for_selector(COMPOSER, Duration::from_secs(2))
        .await
        .expect("real composer missing");

    let stubs: u32 = h
        .eval(
            "(() => document.querySelectorAll('[data-testid=\"leptos-stub-composer\"]').length \
             + document.querySelectorAll('[data-testid=\"leptos-stub-composer-input\"]').length \
             + document.querySelectorAll('[data-testid=\"leptos-stub-composer-send\"]').length)()",
        )
        .await
        .expect("count stubs");
    assert_eq!(stubs, 0, "stub composer still rendered");
}
