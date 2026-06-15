//! Port of `e2e/leptos-composer.spec.ts` (8 cases).
//!
//! Drives the composer at the site root against `mock-omega-server`.
//! Covers the U3 three-controls model: Send (always enqueues) /
//! Halt-during-tool / Halt-then-steer (resume via queued message) /
//! Halt-then-Abort / Switch-model-idle / Switch-effort-idle /
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
/// Prompt button in the bottom bar — opens the collapsible prompt panel.
const PROMPT_BTN: &str = "[data-testid=\"leptos-composer-prompt\"]";
/// Textarea inside the open prompt panel.
const INPUT: &str = "[data-testid=\"leptos-prompt-panel-input\"]";
/// Send button inside the open prompt panel.
const PRIMARY: &str = "[data-testid=\"leptos-prompt-panel-send\"]";
const HALT: &str = "[data-testid=\"leptos-composer-halt\"]";
const RESUME: &str = "[data-testid=\"leptos-composer-resume\"]";
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

/// Mirror of `SCRIPTS.twoPauses()` from the original Playwright spec
/// (a string of short sleep tools, so there are several seams at which a
/// Halt can land).
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

/// Open the collapsible prompt panel by clicking the Prompt button, then
/// wait for the textarea to become available.
async fn open_prompt_panel(h: &TestHarness) {
    h.click(PROMPT_BTN).await.expect("click Prompt button");
    h.wait_for_selector(INPUT, Duration::from_secs(3))
        .await
        .expect("prompt panel textarea did not appear");
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
    open_prompt_panel(&h).await;

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

    // Re-open panel to verify it was cleared after send.
    open_prompt_panel(&h).await;
    let value: String = h
        .eval(&format!("document.querySelector('{INPUT}').value"))
        .await
        .expect("read input");
    assert_eq!(value, "");
    let action = h.attr(PRIMARY, "data-action").await.expect("attr");
    assert_eq!(action.as_deref(), Some("send"));
}

// ---------------------------------------------------------------------------
// 2. Halt-during-tool, then Resume (no input)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "browser"]
async fn composer_halt_during_tool() {
    let h = TestHarness::launch().await.expect("launch");

    h.reset_calls().await.expect("reset_calls");
    h.load_script(two_pauses_script())
        .await
        .expect("load_script");
    h.new_session().await.expect("new_session");
    open_prompt_panel(&h).await;

    h.fill(INPUT, "go halt").await.expect("fill");
    h.click(PRIMARY).await.expect("click send");

    wait_for_turn_state(&h, "running", Duration::from_secs(10)).await;

    // The panel collapsed on send; re-open it to confirm its Send button
    // never morphs into a Halt control. Halt is a separate, always-visible
    // bottom-bar button while running. The panel stays open from here on (it
    // pushes the feed up and never overlays the bottom bar), so the
    // halted-state check below reads the same Send button without re-opening.
    open_prompt_panel(&h).await;
    let action = h.attr(PRIMARY, "data-action").await.expect("attr");
    assert_eq!(action.as_deref(), Some("send"));
    h.wait_for_selector(HALT, Duration::from_secs(2))
        .await
        .expect("halt button missing while running");

    h.click(HALT).await.expect("click halt");
    wait_for_turn_state(&h, "halt_requested", Duration::from_secs(5)).await;
    // Parks at the next seam (after the in-flight tool result).
    wait_for_turn_state(&h, "halted", Duration::from_secs(15)).await;

    // In Halted: the panel (still open) keeps its Send button; Resume +
    // Abort are the secondary controls in the bottom bar.
    let action = h.attr(PRIMARY, "data-action").await.expect("attr");
    assert_eq!(action.as_deref(), Some("send"));
    h.wait_for_selector(RESUME, Duration::from_secs(2))
        .await
        .expect("resume button missing while halted");
    h.wait_for_selector(ABORT, Duration::from_secs(2))
        .await
        .expect("abort button missing while halted");

    // Resume (no input) → the loop carries on → back to idle.
    h.click(RESUME).await.expect("click resume");
    wait_for_turn_state(&h, "idle", Duration::from_secs(30)).await;

    // turn_halted was persisted as a feed event.
    h.wait_for_count(
        &format!("{FEED} [data-event-type=\"turn_halted\"]"),
        1,
        Duration::from_secs(2),
    )
    .await
    .expect("turn_halted never persisted");
}

// ---------------------------------------------------------------------------
// 3. Halt, then steer (resume via a queued message)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "browser"]
async fn composer_halt_then_steer() {
    let h = TestHarness::launch().await.expect("launch");

    h.reset_calls().await.expect("reset_calls");
    h.load_script(two_pauses_script())
        .await
        .expect("load_script");
    h.new_session().await.expect("new_session");
    open_prompt_panel(&h).await;

    h.fill(INPUT, "trigger steer").await.expect("fill");
    h.click(PRIMARY).await.expect("click send");
    wait_for_turn_state(&h, "running", Duration::from_secs(10)).await;

    // Halt mid-flight; it parks at the next seam.
    h.click(HALT).await.expect("click halt");
    wait_for_turn_state(&h, "halted", Duration::from_secs(15)).await;

    // Re-open the panel (it was closed after the first send) to compose a
    // steering message, then Send it. Send ALWAYS enqueues; the parked halt
    // loop pops it, injects it, and resumes.
    open_prompt_panel(&h).await;
    h.fill(INPUT, "actually focus on src/web/server.rs")
        .await
        .expect("fill steering message");
    h.click(PRIMARY).await.expect("click send (enqueue steer)");

    // Turn resumes.
    wait_for_turn_state(&h, "running", Duration::from_secs(5)).await;

    // Re-open panel to verify it was cleared after send.
    open_prompt_panel(&h).await;
    let value: String = h
        .eval(&format!("document.querySelector('{INPUT}').value"))
        .await
        .expect("read input");
    assert_eq!(value, "");

    // turn_resumed landed in the feed.
    h.wait_for_count(
        &format!("{FEED} [data-event-type=\"turn_resumed\"]"),
        1,
        Duration::from_secs(5),
    )
    .await
    .expect("turn_resumed never landed");

    // The steering message was injected as a user_message event.
    h.wait_for_count(
        &format!("{FEED} [data-event-type=\"user_message\"]"),
        2,
        Duration::from_secs(5),
    )
    .await
    .expect("steering user_message never landed");

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
        "turn_interrupted should not fire on clean resume"
    );
}

// ---------------------------------------------------------------------------
// 4. Halt then Abort
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "browser"]
async fn composer_halt_then_abort() {
    let h = TestHarness::launch().await.expect("launch");

    h.reset_calls().await.expect("reset_calls");
    h.load_script(abort_sleep_script())
        .await
        .expect("load_script");
    h.new_session().await.expect("new_session");
    open_prompt_panel(&h).await;

    h.fill(INPUT, "go abort").await.expect("fill");
    h.click(PRIMARY).await.expect("click send");
    wait_for_turn_state(&h, "running", Duration::from_secs(10)).await;

    // Halt first. The in-flight tool is `sleep 10`, so the halt cannot reach
    // a seam yet — the turn sits in halt_requested while the tool runs.
    h.click(HALT).await.expect("click halt");
    wait_for_turn_state(&h, "halt_requested", Duration::from_secs(5)).await;

    // Abort is available during halt_requested and cancels the in-flight
    // block NOW (it does not wait for a seam).
    h.wait_for_selector(ABORT, Duration::from_secs(2))
        .await
        .expect("abort secondary button missing during halt_requested");
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
    open_prompt_panel(&h).await;
    h.fill(INPUT, "ping").await.expect("fill");
    h.press_key(INPUT, "Enter").await.expect("submit");
    wait_for_turn_state(&h, "idle", Duration::from_secs(10)).await;

    // Sanity: server default is opus-4-8.
    let cur: String = h
        .eval(&format!("document.querySelector('{MODEL}').value"))
        .await
        .expect("model.value");
    assert_eq!(cur, "claude-opus-4-8");

    h.select_option(MODEL, "claude-sonnet-4-6")
        .await
        .expect("select sonnet");

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let v: String = h
            .eval(&format!("document.querySelector('{MODEL}').value"))
            .await
            .expect("model.value poll");
        if v == "claude-sonnet-4-6" {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "model select never reflected sonnet, last = {v:?}"
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
    assert_eq!(cur, "high");

    h.select_option(EFFORT, "medium")
        .await
        .expect("select medium");

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let v: String = h
            .eval(&format!("document.querySelector('{EFFORT}').value"))
            .await
            .expect("effort.value poll");
        if v == "medium" {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "effort never reflected medium, last = {v:?}"
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

    // Open prompt panel, then type @crates/ — popup should appear.
    open_prompt_panel(&h).await;
    h.fill(INPUT, "@crates/").await.expect("fill");
    h.wait_for_selector(
        "[data-testid=\"leptos-composer-completion\"]",
        Duration::from_secs(5),
    )
    .await
    .expect("completion popup never appeared");

    // Wait for the completion items to settle to the children of crates/.
    //
    // Timing race: `fill` types each character individually, so `on_input`
    // fires (and `query_completion` is called) for every prefix: "", "c",
    // "cr", "cra", "crat", "crate", "crates", "crates/".  `query_completion`
    // discards *stale* fetches (a result whose seq token has been superseded
    // by a later keystroke), but under heavy load the keystrokes are spaced
    // far enough apart that each prefix's fetch resolves and is applied in
    // the gap *before* the next keystroke fires.  So the popup churns through
    // every intermediate state: "" → root entries (".cargo/", ".git/", …,
    // "crates/"), the bare-name prefixes → the "crates/" sentinel, and only
    // the final "crates/" fetch → the actual children ("crates/omega-agent/",
    // …).  Breaking out of the wait on *any* non-"crates/" item is wrong: an
    // intermediate root entry like ".cargo/" passes that check, so `first`
    // captures a value the list later replaces, and the `format!("@{first}")`
    // assertion then disagrees with what Enter actually accepts.
    //
    // Deterministic settle condition: require the first item to be a *child*
    // of the typed directory — i.e. its `data-completion` starts with
    // "crates/" and is not the bare "crates/" sentinel.  Only the final
    // "crates/" fetch produces such entries (the server returns CWD-relative
    // paths), so this uniquely identifies the settled state regardless of
    // ordering.  Once reached, no further fetch fires (typing has stopped;
    // drill-in only happens on accept), so the list is stable and the
    // ArrowDown+Enter below deterministically accepts `first`.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let first = loop {
        let candidate: String = h
            .eval(
                "document.querySelector('[data-testid=\"leptos-composer-completion-item\"]')\
                 ?.getAttribute('data-completion') ?? ''",
            )
            .await
            .expect("first data-completion poll");
        if candidate.starts_with("crates/") && candidate != "crates/" {
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
