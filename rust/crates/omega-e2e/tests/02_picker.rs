//! Port of `e2e/leptos-session-picker.spec.ts` (9 cases).
//!
//! Drives the session picker against `mock-omega-server`'s real
//! WebSocket. Covers the four CRUD ops (Reset / Rename / Delete /
//! List) plus the open/close cycle and auto-close-on-action rules
//! introduced in Phase 3.9.
//!
//! Determinism note (from the original spec): `data-active="true"`
//! can briefly point at the *previous* row between click and server
//! ack. We always identify rows by `data-session-dir` (stable) and
//! read the active dir off `<main data-active-session-dir>` (ground
//! truth, updated only on `session_info`).

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::time::Duration;

use omega_e2e::{DEFAULT_TIMEOUT, TestHarness};

const PICKER: &str = "[data-testid='leptos-session-picker']";
const PICKER_BACKDROP: &str = "[data-testid='leptos-picker-backdrop']";
const PICKER_CLOSE: &str = "[data-testid='leptos-picker-close']";
const COMPOSER_SESSIONS: &str = "[data-testid='leptos-composer-sessions']";
const SESSION_NEW: &str = "[data-testid='leptos-session-new']";

fn item_sel(dir: &str) -> String {
    format!("[data-testid='leptos-session-item'][data-session-dir='{dir}']")
}
fn item_action_sel(dir: &str, action_testid: &str) -> String {
    format!("{} [data-testid='{action_testid}']", item_sel(dir))
}

// ---------------------------------------------------------------------------
// Create
// ---------------------------------------------------------------------------

/// `+ new session` creates and activates a row; auto-closes the
/// picker; re-opening shows the new dir as the single active row.
#[tokio::test]
#[ignore = "browser"]
async fn picker_new_session_creates_and_activates() {
    let h = TestHarness::launch().await.expect("launch");
    h.open_picker().await.expect("open picker");

    let dir = h.new_session().await.expect("new session");

    // `+ new session` auto-closes the picker (Phase 3.9 TODO-2).
    h.wait_for_detached(PICKER, Duration::from_secs(3))
        .await
        .expect("picker auto-closes on new");

    h.open_picker().await.expect("re-open picker");

    h.wait_for_attr(&item_sel(&dir), "data-active", "true", DEFAULT_TIMEOUT)
        .await
        .expect("new row marked active");

    h.wait_for_count(
        "[data-testid='leptos-session-item'][data-active='true']",
        1,
        DEFAULT_TIMEOUT,
    )
    .await
    .expect("exactly one active row");
}

// ---------------------------------------------------------------------------
// Rename
// ---------------------------------------------------------------------------

/// Rename roundtrip: button → input → submit → label updates.
#[tokio::test]
#[ignore = "browser"]
async fn picker_rename_updates_label() {
    let h = TestHarness::launch().await.expect("launch");
    h.open_picker().await.expect("open picker");

    let dir = h.new_session().await.expect("new session");
    h.open_picker().await.expect("re-open picker");

    // Clicking the label opens the inline rename input.
    h.click(&item_action_sel(&dir, "leptos-session-label"))
        .await
        .expect("click rename");

    let input_sel = format!(
        "{} [data-testid='leptos-session-rename-input']",
        item_sel(&dir)
    );
    h.wait_for_selector(&input_sel, DEFAULT_TIMEOUT)
        .await
        .expect("rename input visible");

    h.fill(&input_sel, "phase-3-2-renamed")
        .await
        .expect("fill rename");

    // Enter submits (no submit button — handled by on:keydown).
    h.press_key(&input_sel, "Enter")
        .await
        .expect("press Enter to submit");

    let label_sel = format!("{} [data-testid='leptos-session-label']", item_sel(&dir));
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        if let Ok(t) = h.text_content(&label_sel).await
            && t.trim() == "phase-3-2-renamed"
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "label did not update to phase-3-2-renamed"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

// ---------------------------------------------------------------------------
// Delete
// ---------------------------------------------------------------------------

/// Delete row: confirm dialog accepted → row vanishes.
#[tokio::test]
#[ignore = "browser"]
async fn picker_delete_removes_row() {
    let h = TestHarness::launch().await.expect("launch");
    h.open_picker().await.expect("open picker");

    let dir = h.new_session().await.expect("new session");
    h.open_picker().await.expect("re-open picker");

    h.auto_accept_dialogs()
        .await
        .expect("override window.confirm");
    h.click(&item_action_sel(&dir, "leptos-session-delete"))
        .await
        .expect("click delete");

    h.wait_for_detached(&item_sel(&dir), Duration::from_secs(3))
        .await
        .expect("row removed");
}

// ---------------------------------------------------------------------------
// List + active distinction
// ---------------------------------------------------------------------------

/// Two new sessions in succession: only the latest is active.
#[tokio::test]
#[ignore = "browser"]
async fn picker_two_sessions_only_latest_active() {
    let h = TestHarness::launch().await.expect("launch");
    h.open_picker().await.expect("open picker");

    let a = h.new_session().await.expect("first new session");
    h.open_picker().await.expect("re-open picker (1)");

    let b = h.new_session().await.expect("second new session");
    assert_ne!(a, b, "the two new sessions must have different dirs");

    h.open_picker().await.expect("re-open picker (2)");

    h.wait_for_count(
        "[data-testid='leptos-session-item'][data-active='true']",
        1,
        DEFAULT_TIMEOUT,
    )
    .await
    .expect("exactly one active row");

    h.wait_for_attr(&item_sel(&b), "data-active", "true", DEFAULT_TIMEOUT)
        .await
        .expect("b is active");
    h.wait_for_attr(&item_sel(&a), "data-active", "false", DEFAULT_TIMEOUT)
        .await
        .expect("a is no longer active");
}

// ---------------------------------------------------------------------------
// Open / close cycle (Phase 3.9 TODO-1)
// ---------------------------------------------------------------------------

/// ✕ button dismisses the picker; Sessions button re-opens it.
#[tokio::test]
#[ignore = "browser"]
async fn picker_close_button_then_reopen() {
    let h = TestHarness::launch().await.expect("launch");
    h.open_picker().await.expect("open picker");

    h.click(PICKER_CLOSE).await.expect("click ✕");
    h.wait_for_detached(PICKER, Duration::from_secs(3))
        .await
        .expect("picker dismissed");
    h.wait_for_detached(PICKER_BACKDROP, Duration::from_secs(3))
        .await
        .expect("backdrop also gone");

    h.click(COMPOSER_SESSIONS).await.expect("click Sessions");
    h.wait_for_selector(PICKER, DEFAULT_TIMEOUT)
        .await
        .expect("picker re-opens");
}

/// Clicking the backdrop (outside the panel) closes the picker.
#[tokio::test]
#[ignore = "browser"]
async fn picker_backdrop_click_closes() {
    let h = TestHarness::launch().await.expect("launch");
    h.open_picker().await.expect("open picker");

    // Click at the backdrop's top-left so we're guaranteed outside
    // the centred panel. We use an explicit JS dispatch on the
    // backdrop element to avoid CDP routing the click through the
    // panel layer.
    let _ = h
        .eval::<bool>(
            "(() => { const b = document.querySelector('[data-testid=\\\"leptos-picker-backdrop\\\"]'); \
              if (!b) return false; b.click(); return true; })()",
        )
        .await
        .expect("dispatch backdrop click");

    h.wait_for_detached(PICKER, Duration::from_secs(3))
        .await
        .expect("picker dismissed by backdrop click");
}

// ---------------------------------------------------------------------------
// Auto-close on Reset / Resume (Phase 3.9 TODO-2)
// ---------------------------------------------------------------------------

/// `+ new session` auto-closes the picker (before the server ack).
#[tokio::test]
#[ignore = "browser"]
async fn picker_new_session_auto_closes() {
    let h = TestHarness::launch().await.expect("launch");
    h.open_picker().await.expect("open picker");

    let prev = h.active_dir().await.unwrap_or_default();
    h.click(SESSION_NEW).await.expect("click + new");

    // Picker disappears immediately (don't wait for server ack).
    h.wait_for_detached(PICKER, Duration::from_secs(3))
        .await
        .expect("picker auto-closes");

    // Eventually the active dir reflects the new session.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let now = h.active_dir().await.unwrap_or_default();
        if !now.is_empty() && now != prev {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "active dir never changed from {prev:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Resume button auto-closes the picker.
#[tokio::test]
#[ignore = "browser"]
async fn picker_resume_auto_closes() {
    let h = TestHarness::launch().await.expect("launch");
    h.open_picker().await.expect("open picker");

    let a = h.new_session().await.expect("first new");
    h.open_picker().await.expect("re-open picker (1)");
    let _b = h.new_session().await.expect("second new");
    h.open_picker().await.expect("re-open picker (2)");

    h.click(&item_action_sel(&a, "leptos-session-resume"))
        .await
        .expect("click resume on a");

    h.wait_for_detached(PICKER, Duration::from_secs(3))
        .await
        .expect("picker closes on resume");
}

// ---------------------------------------------------------------------------
// `@ path` button — regression: must NEVER discard existing prompt text
// (issue: two clicks in a row, no manual typing in between, the second
// click was overwriting the first session's path).
// ---------------------------------------------------------------------------

/// Two `@ path` clicks on different sessions, with **no manual typing**
/// between them, must result in BOTH paths being present in the textarea.
///
/// Repro for the bug the operator hit: the textarea is empty, so after
/// the first click it contains exactly `@.omega/sessions/<a>/` — a string
/// that, walked back from `text.len()`, looks like one unbroken `@`-token
/// with no preceding whitespace. `insert_item_text` then routes through
/// `accept_completion`, which *replaces* that whole token with the
/// second session's path. The first session's reference is lost.
#[tokio::test]
#[ignore = "browser"]
async fn picker_at_path_twice_preserves_both_paths() {
    let h = TestHarness::launch().await.expect("launch");

    // Two fresh sessions. Picker auto-closes after each `+ new`,
    // so we re-open between them.
    h.open_picker().await.expect("open picker");
    let a = h.new_session().await.expect("first new session");
    h.open_picker().await.expect("re-open picker (1)");
    let b = h.new_session().await.expect("second new session");
    assert_ne!(a, b, "the two sessions must have distinct dirs");

    // Sanity: textarea starts empty (operator has typed nothing).
    let initial: String = h
        .eval("document.querySelector('[data-testid=\"leptos-composer-input\"]').value")
        .await
        .expect("read initial input value");
    assert_eq!(
        initial, "",
        "textarea must be empty before first @ path click"
    );

    // First @ path click — on session A. Picker auto-closes per
    // `on_insert_at` in picker.rs.
    h.open_picker().await.expect("re-open picker (2)");
    h.click(&item_action_sel(&a, "leptos-session-insert-at"))
        .await
        .expect("click @ path on a");
    h.wait_for_detached(PICKER, Duration::from_secs(3))
        .await
        .expect("picker auto-closes on @ path");

    // After the first click, the textarea should hold A's path —
    // and only A's path. Wait for it to actually land (the insert
    // happens via a leptos Effect on the next tick).
    let path_a = format!(".omega/sessions/{a}/");
    let path_b = format!(".omega/sessions/{b}/");
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        let v: String = h
            .eval("document.querySelector('[data-testid=\"leptos-composer-input\"]').value")
            .await
            .expect("read input after first click");
        if v.contains(&path_a) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "textarea never picked up A's path; last = {v:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // CRITICAL: do NOT type anything into the textarea between clicks.
    // The bug only surfaces when the textarea contents look like a
    // bare unbroken @-token at the time of the second click.

    // Second @ path click — on session B.
    h.open_picker().await.expect("re-open picker (3)");
    h.click(&item_action_sel(&b, "leptos-session-insert-at"))
        .await
        .expect("click @ path on b");
    h.wait_for_detached(PICKER, Duration::from_secs(3))
        .await
        .expect("picker auto-closes on second @ path");

    // Wait for B's path to appear — then assert A's path is STILL
    // there. The bug manifests as A being replaced by B.
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let final_value = loop {
        let v: String = h
            .eval("document.querySelector('[data-testid=\"leptos-composer-input\"]').value")
            .await
            .expect("read input after second click");
        if v.contains(&path_b) {
            break v;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "textarea never picked up B's path; last = {v:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    assert!(
        final_value.contains(&path_a),
        "second @ path click discarded A's path. \n  expected to contain: {path_a:?}\n  expected to contain: {path_b:?}\n  got: {final_value:?}"
    );
    assert!(
        final_value.contains(&path_b),
        "second @ path click did not insert B's path. got: {final_value:?}"
    );
}

/// Rename does NOT close the picker.
#[tokio::test]
#[ignore = "browser"]
async fn picker_rename_does_not_close() {
    let h = TestHarness::launch().await.expect("launch");
    h.open_picker().await.expect("open picker");

    let dir = h.new_session().await.expect("new session");
    h.open_picker().await.expect("re-open picker");

    // Clicking the label opens the inline rename input.
    h.click(&item_action_sel(&dir, "leptos-session-label"))
        .await
        .expect("click rename");

    // After 300 ms of "rename in progress", picker is still mounted.
    tokio::time::sleep(Duration::from_millis(300)).await;
    h.wait_for_selector(PICKER, Duration::from_secs(1))
        .await
        .expect("picker still open during rename");
}
