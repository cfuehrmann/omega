// Phase 4 step 4 — port of `e2e/leptos-conversation-feed.spec.ts`.
//
// Four cases:
// 1. multi-tool turn — three sequential `run_command` tool turns +
//    final text. Asserts every event family/kind shows up: user_message,
//    tool_call (×3), tool_result (×3), llm_response (final), status.
// 2. streaming text — `longStream` emits 8 chunks × 100 ms; the
//    streaming overlay is visible during the turn and clears after
//    `turn_end`, with the assembled text living in the persisted
//    `llm_response`.
// 3. tool-result truncation (TODO-C, Phase 3.10) — `read_file` against
//    `rust-migration.md`. The inline preview is bounded to 2 lines,
//    no `[show more]` toggle exists, the `[payload]` button opens
//    `TextModal` with the full content (longer than the preview).
// 4. terminal `llm_error` from `httpError(400)` renders with
//    `data-event-kind="error"`.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::doc_markdown,
    clippy::too_many_lines
)]

use std::time::Duration;

use omega_e2e::{MockResponse, TestHarness};

const T: Duration = Duration::from_secs(5);
const T_TURN: Duration = Duration::from_secs(15);
const T_LONG: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// Local script builders (mirror SCRIPTS.* from real-server-control.ts)
// ---------------------------------------------------------------------------

fn multi_tool_script() -> Vec<MockResponse> {
    let sleep = |id: &str| MockResponse::ToolUse {
        id: id.into(),
        name: "run_command".into(),
        input: serde_json::json!({ "command": "sleep 0.6" }),
    };
    vec![
        sleep("toolu_multi_1"),
        sleep("toolu_multi_2"),
        sleep("toolu_multi_3"),
        MockResponse::Text {
            text: "done multi".into(),
            input_tokens: 10,
            output_tokens: 5,
        },
    ]
}

fn long_stream_script() -> Vec<MockResponse> {
    vec![MockResponse::SlowText {
        text: "This is a deliberately long streaming response emitted in chunks. done stream"
            .into(),
        chunks: 8,
        delay_ms: 100,
    }]
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

async fn count(h: &TestHarness, sel: &str) -> u64 {
    let js = format!(
        "document.querySelectorAll({}).length",
        omega_e2e::js_string(sel),
    );
    h.eval(&js).await.expect("count selector")
}

// ---------------------------------------------------------------------------
// 1. Multi-tool turn — every visible event family renders
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "browser"]
async fn multi_tool_turn_renders_every_family() {
    let h = TestHarness::launch().await.expect("launch");
    h.reset_calls().await.expect("reset");
    h.load_script(multi_tool_script())
        .await
        .expect("load script");

    h.new_session().await.expect("new session");
    send_message(&h, "go multi tool").await;

    // ~1.8 s of sleeps + agent overhead → use 30 s upper bound.
    h.wait_for_count(
        "[data-testid=\"leptos-feed\"] \
         [data-testid=\"leptos-event-block\"][data-event-type=\"turn_end\"]",
        1,
        T_LONG,
    )
    .await
    .expect("turn_end never landed");

    // user_message
    let user_msgs = count(
        &h,
        "[data-testid=\"leptos-feed\"] [data-event-type=\"user_message\"]",
    )
    .await;
    assert_eq!(user_msgs, 1, "expected 1 user_message, got {user_msgs}");
    let user_text = h
        .text_content(
            "[data-testid=\"leptos-feed\"] [data-event-type=\"user_message\"] \
             [data-testid=\"leptos-user-content\"]",
        )
        .await
        .expect("read user content");
    assert!(
        user_text.contains("go multi tool"),
        "user content missing prompt: {user_text:?}"
    );

    // 3 tool_calls, first has run_command + sleep 0.6
    let tool_calls = count(
        &h,
        "[data-testid=\"leptos-feed\"] [data-event-type=\"tool_call\"]",
    )
    .await;
    assert_eq!(tool_calls, 3, "expected 3 tool_calls, got {tool_calls}");
    let first_name = h
        .text_content(
            "[data-testid=\"leptos-feed\"] [data-event-type=\"tool_call\"] \
             [data-testid=\"leptos-tool-name\"]",
        )
        .await
        .expect("read tool name");
    assert!(
        first_name.contains("run_command"),
        "tool name not run_command: {first_name:?}"
    );
    // SCHEMA-8 Phase 5e — ToolCallBlock is now a slim identity row; the
    // input preview moved to the sibling ToolUseBlock (data-testid
    // `leptos-tool-use-input`) which arrived earlier in the stream.
    // The 3 tool_use_block events should mirror the 3 tool_calls
    // (paired by provider tool_use_id).
    let tool_use_blocks = count(
        &h,
        "[data-testid=\"leptos-feed\"] [data-event-type=\"tool_use_block\"]",
    )
    .await;
    assert_eq!(
        tool_use_blocks, 3,
        "expected 3 tool_use_blocks paired with tool_calls, got {tool_use_blocks}"
    );
    let first_input = h
        .text_content(
            "[data-testid=\"leptos-feed\"] [data-event-type=\"tool_use_block\"] \
             [data-testid=\"leptos-tool-use-input\"]",
        )
        .await
        .expect("read tool_use_block input preview");
    assert!(
        first_input.contains("sleep 0.6"),
        "tool_use_block preview missing sleep 0.6: {first_input:?}"
    );

    // 3 tool_results, all kind=tool_result
    let tool_results = count(
        &h,
        "[data-testid=\"leptos-feed\"] [data-event-type=\"tool_result\"]",
    )
    .await;
    assert_eq!(
        tool_results, 3,
        "expected 3 tool_results, got {tool_results}"
    );
    let mismatched: u64 = h
        .eval(
            "Array.from(document.querySelectorAll(\
             '[data-testid=\"leptos-feed\"] [data-event-type=\"tool_result\"]'))\
             .filter(e => e.getAttribute('data-event-kind') !== 'tool_result').length",
        )
        .await
        .expect("count mismatched tool_result kinds");
    assert_eq!(mismatched, 0, "tool_result blocks with wrong kind");

    // last llm_response_ended is kind=status; last text_block carries
    // the final assistant text (SCHEMA-8 Phase 6.5 — LlmResponse removed;
    // markdown lives in TextBlock siblings, LlmResponseEnded carries
    // only affordances: context hash, [compacted] badge).
    let last_kind: String = h
        .eval(
            "(() => {\
               const xs = document.querySelectorAll(\
                 '[data-testid=\"leptos-feed\"] [data-event-type=\"llm_response_ended\"]');\
               return xs.length ? xs[xs.length - 1].getAttribute('data-event-kind') : '';\
             })()",
        )
        .await
        .expect("read last llm_response_ended kind");
    assert_eq!(last_kind, "status", "last llm_response_ended not status");
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
        .expect("read last assistant text");
    assert!(
        last_text.contains("done multi"),
        "last assistant text missing 'done multi': {last_text:?}"
    );

    // status family present
    let status_count = count(
        &h,
        "[data-testid=\"leptos-feed\"] [data-event-kind=\"status\"]",
    )
    .await;
    assert!(
        status_count >= 1,
        "expected ≥1 status block, got {status_count}"
    );

    // every block has kind + type
    let total = count(
        &h,
        "[data-testid=\"leptos-feed\"] [data-testid=\"leptos-event-block\"]",
    )
    .await;
    assert!(total > 5, "expected >5 event blocks, got {total}");
    let no_kind = count(
        &h,
        "[data-testid=\"leptos-feed\"] [data-testid=\"leptos-event-block\"]:not([data-event-kind])",
    )
    .await;
    assert_eq!(no_kind, 0, "{no_kind} blocks missing data-event-kind");
    let no_type = count(
        &h,
        "[data-testid=\"leptos-feed\"] [data-testid=\"leptos-event-block\"]:not([data-event-type])",
    )
    .await;
    assert_eq!(no_type, 0, "{no_type} blocks missing data-event-type");
}

// ---------------------------------------------------------------------------
// 2. Streaming text overlay appears live and resolves into llm_response
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "browser"]
async fn streaming_overlay_appears_live_and_resolves() {
    let h = TestHarness::launch().await.expect("launch");
    h.reset_calls().await.expect("reset");
    h.load_script(long_stream_script())
        .await
        .expect("load script");

    h.new_session().await.expect("new session");
    send_message(&h, "go long stream").await;

    // Overlay visible during the stream, eventually carries the
    // characteristic prefix.
    h.wait_for_selector("[data-testid=\"leptos-streaming-text\"]", T)
        .await
        .expect("streaming overlay never appeared");

    // Poll the streaming <pre> until we see "This is a deliberately long".
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut saw = false;
    let mut last = String::new();
    while std::time::Instant::now() < deadline {
        let txt: String = h
            .eval(
                "(() => {\
                   const o = document.querySelector(\
                     '[data-testid=\"leptos-streaming-text\"] pre.block-body');\
                   return o ? o.textContent : '';\
                 })()",
            )
            .await
            .unwrap_or_default();
        last = txt;
        if last.contains("This is a deliberately long") {
            saw = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        saw,
        "streaming overlay never showed expected prefix; last={last:?}"
    );

    // After turn_end, overlay is gone and llm_response carries the
    // post-stream "done stream" final.
    h.wait_for_count(
        "[data-testid=\"leptos-feed\"] [data-event-type=\"turn_end\"]",
        1,
        Duration::from_secs(10),
    )
    .await
    .expect("turn_end never landed");

    h.wait_for_detached("[data-testid=\"leptos-streaming-text\"]", T)
        .await
        .expect("streaming overlay never cleared");

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
        .expect("read final assistant text");
    assert!(
        last_text.contains("done stream"),
        "expected 'done stream' in final llm_response: {last_text:?}"
    );
}

// ---------------------------------------------------------------------------
// 3. Tool-result truncation — [payload] modal has full body
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "browser"]
async fn long_tool_result_preview_is_two_lines_payload_modal_full() {
    let h = TestHarness::launch().await.expect("launch");
    h.reset_calls().await.expect("reset");
    h.load_script(vec![
        MockResponse::ToolUse {
            id: "toolu_long_read".into(),
            name: "read_file".into(),
            input: serde_json::json!({ "path": "rust-migration.md" }),
        },
        MockResponse::Text {
            text: "done long read".into(),
            input_tokens: 10,
            output_tokens: 5,
        },
    ])
    .await
    .expect("load script");

    h.new_session().await.expect("new session");
    send_message(&h, "trigger long read").await;

    h.wait_for_count(
        "[data-testid=\"leptos-feed\"] [data-event-type=\"turn_end\"]",
        1,
        T_TURN,
    )
    .await
    .expect("turn_end never landed");

    // Exactly 1 tool_result.
    let result_count = count(
        &h,
        "[data-testid=\"leptos-feed\"] [data-event-type=\"tool_result\"]",
    )
    .await;
    assert_eq!(
        result_count, 1,
        "expected 1 tool_result, got {result_count}"
    );

    // Old [show more] gone.
    let expand_count = count(
        &h,
        "[data-testid=\"leptos-feed\"] [data-event-type=\"tool_result\"] \
         [data-testid=\"leptos-tool-result-expand\"]",
    )
    .await;
    assert_eq!(expand_count, 0, "leptos-tool-result-expand should be gone");

    // [payload] visible.
    h.wait_for_selector(
        "[data-testid=\"leptos-feed\"] [data-event-type=\"tool_result\"] \
         [data-testid=\"leptos-tool-result-payload\"]",
        T,
    )
    .await
    .expect("payload button missing");

    // Inline preview body — no old marker, ≤ 2 non-empty lines.
    let preview: String = h
        .eval(
            "(() => {\
               const e = document.querySelector(\
                 '[data-testid=\"leptos-feed\"] [data-event-type=\"tool_result\"] \
                  [data-testid=\"leptos-tool-result-body\"]');\
               return e ? e.innerText : '';\
             })()",
        )
        .await
        .expect("read preview");
    assert!(
        !preview.contains("chars total — showing first"),
        "preview still has old truncation marker: {preview:?}"
    );
    let nonempty_lines = preview.split('\n').filter(|l| !l.is_empty()).count();
    assert!(
        nonempty_lines <= 2,
        "preview has {nonempty_lines} non-empty lines (>2): {preview:?}"
    );
    let preview_len = preview.len();

    // Click [payload].
    h.click(
        "[data-testid=\"leptos-feed\"] [data-event-type=\"tool_result\"] \
         [data-testid=\"leptos-tool-result-payload\"]",
    )
    .await
    .expect("click [payload]");

    h.wait_for_selector("[data-testid=\"leptos-text-modal\"]", T)
        .await
        .expect("text modal never appeared");

    let title = h
        .text_content("[data-testid=\"leptos-text-modal-title\"]")
        .await
        .expect("read title");
    assert!(
        title.contains("read_file"),
        "title missing tool name: {title:?}"
    );

    let full: String = h
        .eval(
            "(() => {\
               const e = document.querySelector(\
                 '[data-testid=\"leptos-text-modal-body\"]');\
               return e ? e.innerText : '';\
             })()",
        )
        .await
        .expect("read modal body");
    assert!(
        full.len() > preview_len + 100,
        "modal body ({} bytes) not substantially longer than preview ({} bytes)",
        full.len(),
        preview_len
    );

    h.click("[data-testid=\"leptos-text-modal-close\"]")
        .await
        .expect("click close");
    h.wait_for_detached("[data-testid=\"leptos-text-modal\"]", T)
        .await
        .expect("text modal never dismissed");
}

// ---------------------------------------------------------------------------
// 4. Terminal llm_error renders in the error family
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "browser"]
async fn terminal_llm_error_renders_in_error_family() {
    let h = TestHarness::launch().await.expect("launch");
    h.reset_calls().await.expect("reset");
    h.load_script(vec![MockResponse::HttpError {
        status: 400,
        body: r#"{"type":"error","error":{"type":"invalid_request_error","message":"bad input"}}"#
            .into(),
    }])
    .await
    .expect("load script");

    h.new_session().await.expect("new session");
    send_message(&h, "trigger 400").await;

    // At least one [data-event-kind="error"] block appears.
    h.wait_for_selector(
        "[data-testid=\"leptos-feed\"] [data-event-kind=\"error\"]",
        T_TURN,
    )
    .await
    .expect("no error-kind block ever appeared");

    let llm_err = count(
        &h,
        "[data-testid=\"leptos-feed\"] [data-event-type=\"llm_error\"]",
    )
    .await;
    let interrupted = count(
        &h,
        "[data-testid=\"leptos-feed\"] [data-event-type=\"turn_interrupted\"]",
    )
    .await;
    assert!(
        llm_err + interrupted > 0,
        "expected ≥1 llm_error or turn_interrupted, got {llm_err}+{interrupted}"
    );
}
