// Test-doc identifiers (TextModal, ContextModal, DirtyModal, tool_result, etc.)
// describe UI components and JSON fields; backticking every one is noise here.
#![allow(clippy::doc_markdown)]

//! Esc-key dismissal tests for every closable modal in the UI.
//!
//! ## Session picker (GREEN — already implemented)
//!
//! The picker backdrop has `tabindex="-1"`, is auto-focused on mount,
//! and handles `on:keydown` for `Escape`. The green test documents the
//! existing behaviour and guards regressions.
//!
//! ## TextModal / ContextModal / DirtyModal (RED → GREEN)
//!
//! Before the implementation these tests fail:
//! - TextModal has a ✕ close button but no keydown handler.
//! - ContextModal has a ✕ close button but no keydown handler.
//! - DirtyModal has Cancel/Proceed buttons but no keydown handler.
//!
//! After the implementation (focusable backdrop + `on:keydown`) all
//! four tests must be green.
//!
//! ## Dirty modal triggering
//!
//! `pending_changes_warning` is normally sent by the server when the
//! git working tree is dirty. The test harness launches the mock server
//! with `OMEGA_ALLOW_DIRTY=1`, which bypasses that gate, so we inject
//! the frame directly via [`TestHarness::inject_ws_frame`] instead.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::time::Duration;

use omega_e2e::{DEFAULT_TIMEOUT, MockResponse, TestHarness};

const PICKER: &str = "[data-testid='leptos-session-picker']";
const PICKER_BACKDROP: &str = "[data-testid='leptos-picker-backdrop']";
const TEXT_MODAL: &str = "[data-testid='leptos-text-modal']";
const TEXT_MODAL_BACKDROP: &str = "[data-testid='leptos-text-modal-backdrop']";
const CONTEXT_MODAL: &str = "[data-testid='leptos-context-modal']";
const CONTEXT_MODAL_BACKDROP: &str = "[data-testid='leptos-context-modal-backdrop']";
const DIRTY_MODAL: &str = "[data-testid='leptos-dirty-modal']";
const DIRTY_MODAL_BACKDROP: &str = "[data-testid='leptos-dirty-modal-backdrop']";
const COMPOSER: &str = "[data-testid='leptos-composer-input']";

const T: Duration = DEFAULT_TIMEOUT;
const T_TURN: Duration = Duration::from_secs(20);

async fn send_message(h: &TestHarness, content: &str) {
    h.fill(COMPOSER, content).await.expect("fill composer");
    h.press_key(COMPOSER, "Enter")
        .await
        .expect("submit composer");
}

async fn wait_for_one_turn_end(h: &TestHarness) {
    h.wait_for_count(
        "[data-testid='leptos-feed'] [data-event-type='turn_end']",
        1,
        T_TURN,
    )
    .await
    .expect("turn_end never landed");
}

// ---------------------------------------------------------------------------
// 1. Session picker — Esc closes it when a session exists (GREEN)
// ---------------------------------------------------------------------------

/// The session picker backdrop is auto-focused on mount and handles
/// `keydown`. Pressing Esc while a session exists must close the picker.
///
/// Guard: if the picker loses its keydown handler or auto-focus, the
/// test catches it. The picker must NOT close when there is no active
/// session (operator is forced to choose one first) — that branch is
/// deliberately not tested here; it is exercised by the behaviour that
/// the picker stays open before any session is created.
#[tokio::test]
#[ignore = "browser"]
async fn picker_esc_closes_with_session() {
    let h = TestHarness::launch().await.expect("launch");

    // Auto-opens when there's no session; create one (auto-closes picker).
    h.new_session().await.expect("new session");

    // Re-open the picker.
    h.open_picker().await.expect("re-open picker");
    h.wait_for_selector(PICKER_BACKDROP, T)
        .await
        .expect("backdrop present");

    // Press Esc on the backdrop and verify the picker closes.
    h.press_escape_on(PICKER_BACKDROP)
        .await
        .expect("press Esc on backdrop");
    h.wait_for_detached(PICKER, Duration::from_secs(3))
        .await
        .expect("picker did not close on Esc");
}

// ---------------------------------------------------------------------------
// 2. TextModal — Esc closes it (RED until implementation)
// ---------------------------------------------------------------------------

/// TextModal has a ✕ close button; pressing Esc must have the same
/// effect as clicking that button.
///
/// Setup: open a new session, trigger a tool-call turn (so the feed
/// gets a `tool_result` block with a payload button), click the
/// "output" button to open TextModal, then press Esc and verify
/// the modal is dismissed.
#[tokio::test]
#[ignore = "browser"]
async fn text_modal_esc_closes() {
    let h = TestHarness::launch().await.expect("launch");

    // Prime a tool-call + text turn.
    h.load_script(vec![
        MockResponse::ToolUse {
            id: "tool-1".into(),
            name: "run_command".into(),
            input: serde_json::json!({"command": "echo hi"}),
        },
        MockResponse::Text {
            text: "done".into(),
            input_tokens: 10,
            output_tokens: 5,
        },
    ])
    .await
    .expect("load script");

    h.new_session().await.expect("new session");
    send_message(&h, "hello").await;
    wait_for_one_turn_end(&h).await;

    // Wait for a tool_result block to appear in the feed; its "output"
    // button (leptos-tool-result-payload-btn) opens the TextModal.
    h.wait_for_selector(
        "[data-testid='leptos-feed'] [data-event-type='tool_result'] \
         [data-testid='leptos-tool-result-payload-btn']",
        T,
    )
    .await
    .expect("tool_result payload button appeared");

    // Click the "output" button — opens TextModal with the full tool output.
    h.click(
        "[data-testid='leptos-feed'] [data-event-type='tool_result'] \
         [data-testid='leptos-tool-result-payload-btn']",
    )
    .await
    .expect("click tool_result payload button");

    // TextModal must open.
    h.wait_for_selector(TEXT_MODAL, T)
        .await
        .expect("TextModal opened");

    // Press Esc on the backdrop — modal must close.
    h.press_escape_on(TEXT_MODAL_BACKDROP)
        .await
        .expect("press Esc");
    h.wait_for_detached(TEXT_MODAL, Duration::from_secs(3))
        .await
        .expect("TextModal did not close on Esc");
}

// ---------------------------------------------------------------------------
// 3. ContextModal — Esc closes it (RED until implementation)
// ---------------------------------------------------------------------------

/// ContextModal has a ✕ close button; pressing Esc must dismiss it.
///
/// Setup: trigger an `llm_call` block in the feed (requires a full
/// turn), click the `[context]` button, verify the modal opens, then
/// press Esc.
#[tokio::test]
#[ignore = "browser"]
async fn context_modal_esc_closes() {
    let h = TestHarness::launch().await.expect("launch");

    h.load_script(vec![MockResponse::Text {
        text: "ctx test".into(),
        input_tokens: 5,
        output_tokens: 3,
    }])
    .await
    .expect("load script");

    h.new_session().await.expect("new session");
    send_message(&h, "trigger context").await;
    wait_for_one_turn_end(&h).await;

    // At least one llm_call block must be in the feed.
    h.wait_for_selector(
        "[data-testid='leptos-feed'] [data-event-type='llm_call']",
        T,
    )
    .await
    .expect("llm_call block appeared");

    // Open the context modal via the [context] button.
    h.click(
        "[data-testid='leptos-feed'] [data-event-type='llm_call'] \
         [data-testid='leptos-llm-call-open-modal']",
    )
    .await
    .expect("click [context]");

    h.wait_for_selector(CONTEXT_MODAL, T)
        .await
        .expect("ContextModal opened");

    // Press Esc — modal must close.
    h.press_escape_on(CONTEXT_MODAL_BACKDROP)
        .await
        .expect("press Esc");
    h.wait_for_detached(CONTEXT_MODAL, Duration::from_secs(3))
        .await
        .expect("ContextModal did not close on Esc");
}

// ---------------------------------------------------------------------------
// 4. DirtyModal — Esc acts as Cancel (RED until implementation)
// ---------------------------------------------------------------------------

/// DirtyModal has no ✕ button but has a Cancel action; pressing Esc
/// must have the same effect as clicking Cancel (clears the signal,
/// dismisses the modal, leaves the server state untouched).
///
/// The modal is triggered by injecting a `pending_changes_warning`
/// WebSocket frame directly — the mock server runs with
/// `OMEGA_ALLOW_DIRTY=1` so the gate never fires naturally in tests.
#[tokio::test]
#[ignore = "browser"]
async fn dirty_modal_esc_cancels() {
    let h = TestHarness::launch_with_ws_spy().await.expect("launch");

    // Create a session so we have a connected WS.
    h.new_session().await.expect("new session");

    // Inject the pending_changes_warning frame. The store reducer sets
    // `pending_changes_warning = Some(Reset { .. })`, which mounts the modal.
    h.inject_ws_frame(r#"{"type":"pending_changes_warning","intent":{"kind":"reset"}}"#)
        .await
        .expect("inject pending_changes_warning");

    h.wait_for_selector(DIRTY_MODAL, T)
        .await
        .expect("DirtyModal opened");

    // Press Esc on the backdrop — must act as Cancel, dismissing the modal.
    h.press_escape_on(DIRTY_MODAL_BACKDROP)
        .await
        .expect("press Esc");
    h.wait_for_detached(DIRTY_MODAL, Duration::from_secs(3))
        .await
        .expect("DirtyModal did not close on Esc");
}

// Guard: picker stays open when there is no session (no Esc escape).
//
// This is a negative test — it is implied by the picker's `has_session`
// guard in the keydown handler, which prevents Esc from closing the
// picker before the operator has chosen a session. We do not write an
// explicit negative test here because asserting "picker is still open
// after N ms" would be a timing-dependent sleep; the behaviour is
// covered by the fact that `picker_esc_closes_with_session` can only
// pass after creating a session first.
const _: () = ();
