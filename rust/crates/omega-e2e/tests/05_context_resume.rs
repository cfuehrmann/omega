// Phase 4 step 4 — port of `e2e/leptos-context-resume.spec.ts`.
//
// Three cases:
// 1. clicking the `[context]` button on an `llm_call` block opens
//    `leptos-context-modal`, fires GET /api/context?hashes=…, and
//    renders one `leptos-context-modal-record` per record. Close
//    button removes the modal.
// 2. clicking the `[payload]` button opens the generic `TextModal`
//    overlay with the four metadata fields.
// 3. resuming a source session from the picker drives the full
//    flow: new active dir, `resuming_session` block (referencing
//    the source dir), then `session_resumed` block (containing
//    the synthetic "Resumed session summary" text from the script).

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::doc_markdown
)]

use std::time::Duration;

use omega_e2e::{MockResponse, TestHarness};
use regex::Regex;

const T: Duration = Duration::from_secs(5);
const T_TURN: Duration = Duration::from_secs(15);

// Mirror of `SCRIPTS.resumeBasis()` from the Playwright fixtures.
fn resume_basis_script() -> Vec<MockResponse> {
    vec![
        MockResponse::ToolUse {
            id: "toolu_rb_1".into(),
            name: "run_command".into(),
            input: serde_json::json!({ "command": "sleep 0.3" }),
        },
        MockResponse::Text {
            text: "done basis".into(),
            input_tokens: 10,
            output_tokens: 5,
        },
        MockResponse::Text {
            text: "<summary>Resumed session summary.</summary>\n<description>Resumed work.</description>".into(),
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

async fn wait_for_one_turn_end(h: &TestHarness) {
    h.wait_for_count(
        "[data-testid=\"leptos-feed\"] [data-event-type=\"turn_end\"]",
        1,
        T_TURN,
    )
    .await
    .expect("turn_end never landed");
}

// ---------------------------------------------------------------------------
// Case 1: open llm_call modal → fetches records → close dismisses
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "browser"]
async fn context_modal_opens_on_llm_call_and_close_dismisses() {
    let h = TestHarness::launch().await.expect("launch");
    h.reset_calls().await.expect("reset");
    h.load_script(vec![
        MockResponse::ToolUse {
            id: "toolu_ctx_1".into(),
            name: "run_command".into(),
            input: serde_json::json!({ "command": "echo ctx" }),
        },
        MockResponse::Text {
            text: "done ctx".into(),
            input_tokens: 10,
            output_tokens: 5,
        },
    ])
    .await
    .expect("load script");

    h.new_session().await.expect("new session");
    send_message(&h, "trigger llm_call").await;
    wait_for_one_turn_end(&h).await;

    // At least one llm_call rendered.
    let llm_calls: u64 = h
        .eval(
            "document.querySelectorAll('[data-testid=\"leptos-feed\"] \
             [data-event-type=\"llm_call\"]').length",
        )
        .await
        .expect("count llm_call");
    assert!(llm_calls >= 1, "expected >=1 llm_call, got {llm_calls}");

    // Modal not in the DOM initially.
    let initial: u64 = h
        .eval("document.querySelectorAll('[data-testid=\"leptos-context-modal\"]').length")
        .await
        .expect("count modal");
    assert_eq!(initial, 0, "modal should be unmounted before click");

    // Click the first llm_call's [context] button.
    h.click(
        "[data-testid=\"leptos-feed\"] [data-event-type=\"llm_call\"] \
         [data-testid=\"leptos-llm-call-open-modal\"]",
    )
    .await
    .expect("click [context]");

    // Modal mounts.
    h.wait_for_selector("[data-testid=\"leptos-context-modal\"]", T)
        .await
        .expect("modal never appeared");

    // Loading clears within 5 s.
    h.wait_for_detached("[data-testid=\"leptos-context-modal-loading\"]", T)
        .await
        .expect("loading never cleared");

    // ≥1 record rendered.
    let rec_count: u64 = h
        .eval("document.querySelectorAll('[data-testid=\"leptos-context-modal-record\"]').length")
        .await
        .expect("count records");
    assert!(rec_count >= 1, "expected >=1 record, got {rec_count}");

    // First record has data-role user|assistant + visible body.
    let role = h
        .attr("[data-testid=\"leptos-context-modal-record\"]", "data-role")
        .await
        .expect("read data-role")
        .expect("data-role missing");
    assert!(
        role == "user" || role == "assistant",
        "unexpected data-role={role}"
    );

    h.wait_for_selector(
        "[data-testid=\"leptos-context-modal-record\"] \
         [data-testid=\"leptos-context-modal-record-body\"]",
        T,
    )
    .await
    .expect("record body missing");

    // Meta line matches `\d+ hash(es) · \d+ bytes`.
    let meta = h
        .text_content("[data-testid=\"leptos-context-modal-meta\"]")
        .await
        .expect("read meta");
    let re = Regex::new(r"\d+ hash\(es\) · \d+ bytes").unwrap();
    assert!(re.is_match(&meta), "meta text didn't match: {meta:?}");

    // Close → unmounts.
    h.click("[data-testid=\"leptos-context-modal-close\"]")
        .await
        .expect("click close");
    h.wait_for_detached("[data-testid=\"leptos-context-modal\"]", T)
        .await
        .expect("modal never dismissed");
}

// ---------------------------------------------------------------------------
// Case 2: payload modal on llm_call reveals metadata fields
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "browser"]
async fn llm_call_payload_modal_shows_metadata() {
    let h = TestHarness::launch().await.expect("launch");
    h.reset_calls().await.expect("reset");
    h.load_script(vec![MockResponse::Text {
        text: "ping".into(),
        input_tokens: 10,
        output_tokens: 5,
    }])
    .await
    .expect("load script");

    h.new_session().await.expect("new session");
    send_message(&h, "trigger payload modal").await;
    wait_for_one_turn_end(&h).await;

    // Old <details> is gone (TODO-B Phase 3.10).
    let details_count: u64 = h
        .eval(
            "document.querySelectorAll('[data-testid=\"leptos-feed\"] \
             [data-event-type=\"llm_call\"] \
             [data-testid=\"leptos-llm-call-details\"]').length",
        )
        .await
        .expect("count details");
    assert_eq!(details_count, 0, "leptos-llm-call-details should be gone");

    // Payload button visible.
    h.wait_for_selector(
        "[data-testid=\"leptos-feed\"] [data-event-type=\"llm_call\"] \
         [data-testid=\"leptos-llm-call-payload\"]",
        T,
    )
    .await
    .expect("payload button missing");

    // Modal not mounted initially.
    let initial: u64 = h
        .eval("document.querySelectorAll('[data-testid=\"leptos-text-modal\"]').length")
        .await
        .expect("count modal");
    assert_eq!(initial, 0, "text modal should be unmounted before click");

    // Click [payload].
    h.click(
        "[data-testid=\"leptos-feed\"] [data-event-type=\"llm_call\"] \
         [data-testid=\"leptos-llm-call-payload\"]",
    )
    .await
    .expect("click [payload]");

    h.wait_for_selector("[data-testid=\"leptos-text-modal\"]", T)
        .await
        .expect("text modal never appeared");

    // Title contains "llm_call payload".
    let title = h
        .text_content("[data-testid=\"leptos-text-modal-title\"]")
        .await
        .expect("read title");
    assert!(
        title.contains("llm_call payload"),
        "unexpected title: {title:?}"
    );

    // Body contains all four metadata fields.
    let body = h
        .text_content("[data-testid=\"leptos-text-modal-body\"]")
        .await
        .expect("read body");
    for needle in [
        "request_bytes:",
        "request_summary",
        "\"model\"", // JSON key present in request_summary block
        "\"tools\"", // tool list always present in elided request
    ] {
        assert!(
            body.contains(needle),
            "expected body to contain {needle:?}; got: {body:?}"
        );
    }

    // request_bytes is a positive integer.
    let bytes_re = Regex::new(r"request_bytes:\s*(\d+)").unwrap();
    let caps = bytes_re
        .captures(&body)
        .expect("request_bytes pattern not found");
    let bytes: u64 = caps[1].parse().expect("parse request_bytes value");
    assert!(bytes > 0, "expected request_bytes > 0, got {bytes}");

    // Close.
    h.click("[data-testid=\"leptos-text-modal-close\"]")
        .await
        .expect("click close");
    h.wait_for_detached("[data-testid=\"leptos-text-modal\"]", T)
        .await
        .expect("text modal never dismissed");
}

// ---------------------------------------------------------------------------
// Case 3: resume from picker drives the full resumption flow
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "browser"]
async fn resume_from_picker_runs_full_flow() {
    let h = TestHarness::launch().await.expect("launch");
    h.reset_calls().await.expect("reset");
    h.load_script(resume_basis_script())
        .await
        .expect("load script");

    let source_dir = h.new_session().await.expect("create source session");
    send_message(&h, "seed the source session").await;
    wait_for_one_turn_end(&h).await;

    // Create a second (scratch) session so that source_dir becomes
    // inactive — the resume button is only shown on inactive rows.
    h.open_picker()
        .await
        .expect("open picker for scratch session");
    h.new_session().await.expect("create scratch session");

    // Open picker and locate the source row.
    h.open_picker().await.expect("open picker");
    let row_sel =
        format!("[data-testid=\"leptos-session-item\"][data-session-dir=\"{source_dir}\"]");
    h.wait_for_selector(&row_sel, T)
        .await
        .expect("source row missing from picker");

    // Click the resume button on that specific row.
    let resume_sel = format!("{row_sel} [data-testid=\"leptos-session-resume\"]");
    h.click(&resume_sel).await.expect("click resume");

    // Active dir flips to a new (non-source, non-null) value within 10 s.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut last_dir = String::new();
    let mut flipped = false;
    while std::time::Instant::now() < deadline {
        let cur = h.active_dir().await.unwrap_or_default();
        if !cur.is_empty() && cur != source_dir {
            flipped = true;
            break;
        }
        last_dir = cur;
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        flipped,
        "active dir never flipped within 10 s (last seen: {last_dir:?}, source: {source_dir:?})"
    );

    // Feed renders a `resuming_session` block referencing source_dir.
    h.wait_for_count(
        "[data-testid=\"leptos-feed\"] [data-event-type=\"resuming_session\"]",
        1,
        T_TURN,
    )
    .await
    .expect("resuming_session never appeared");
    let resuming_text = h
        .text_content("[data-testid=\"leptos-feed\"] [data-event-type=\"resuming_session\"]")
        .await
        .expect("read resuming_session");
    assert!(
        resuming_text.contains(&source_dir),
        "resuming_session text doesn't mention source dir {source_dir:?}: {resuming_text:?}"
    );

    // Feed renders a `session_resumed` block with the synthetic
    // summary from resume_basis_script().
    h.wait_for_count(
        "[data-testid=\"leptos-feed\"] [data-event-type=\"session_resumed\"]",
        1,
        T_TURN,
    )
    .await
    .expect("session_resumed never appeared");
    let resumed_text = h
        .text_content("[data-testid=\"leptos-feed\"] [data-event-type=\"session_resumed\"]")
        .await
        .expect("read session_resumed");
    assert!(
        resumed_text.contains("Resumed session summary"),
        "session_resumed text missing summary: {resumed_text:?}"
    );
}
