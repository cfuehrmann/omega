//! Port of `e2e/leptos-markdown.spec.ts` (11 cases).
//!
//! Drives the conversation feed against a real WebSocket session
//! and validates that the assistant's `llm_response` text gains
//! the same markdown affordances as the SolidJS `MdBody`:
//!
//! * paragraphs, lists, headings, links, GFM tables
//! * fenced code blocks keep their `language-*` class
//! * raw HTML in source is escaped (no live `<script>`)
//! * diff colouring on `language-diff` and `language-patch`
//! * mermaid lazy-load (CDN; rendered SVG or error notice)
//! * streaming overlay shows raw text, finalised body parses

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::doc_markdown
)]

use std::time::Duration;

use omega_e2e::{MockResponse, TestHarness};

const MD_BODY: &str = "[data-testid=\"md-body\"]";
const TURN_END: &str = "[data-testid=\"leptos-feed\"] [data-event-type=\"turn_end\"]";

/// Drive a single assistant-only turn that emits exactly the given
/// markdown text in the `llm_response` event.  Mirrors
/// `runAssistantTurn` from the original Playwright spec.
async fn run_assistant_turn(h: &TestHarness, markdown: &str) {
    h.reset_calls().await.expect("reset_calls");
    h.load_script(vec![MockResponse::Text {
        text: markdown.to_string(),
        input_tokens: 10,
        output_tokens: 5,
    }])
    .await
    .expect("load_script");

    // A fresh session for each turn so previous feed events don't
    // pollute the assertions.
    h.new_session().await.expect("new_session");

    h.fill("[data-testid=\"leptos-composer-input\"]", "render markdown")
        .await
        .expect("fill composer");
    h.press_key("[data-testid=\"leptos-composer-input\"]", "Enter")
        .await
        .expect("submit");

    h.wait_for_count(TURN_END, 1, Duration::from_secs(15))
        .await
        .expect("turn_end never landed in feed");
}

// ---------------------------------------------------------------------------
// Markdown surfaces
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "browser"]
async fn md_assistant_text_renders_inside_md_body() {
    let h = TestHarness::launch().await.expect("launch harness");
    run_assistant_turn(&h, "**bold** and `inline code`").await;

    h.wait_for_selector(MD_BODY, Duration::from_secs(2))
        .await
        .expect("md-body never appeared");

    let strong: String = h
        .eval("document.querySelector('[data-testid=\"md-body\"] strong').textContent")
        .await
        .expect("read <strong>");
    assert_eq!(strong, "bold");

    let code: String = h
        .eval("document.querySelector('[data-testid=\"md-body\"] code').textContent")
        .await
        .expect("read <code>");
    assert_eq!(code, "inline code");
}

#[tokio::test]
#[ignore = "browser"]
async fn md_paragraph_lists_headings() {
    let h = TestHarness::launch().await.expect("launch");
    run_assistant_turn(
        &h,
        "## Steps\n\nDo the following:\n\n- one\n- two\n- three\n",
    )
    .await;

    let h2: String = h
        .eval("document.querySelector('[data-testid=\"md-body\"] h2').textContent")
        .await
        .expect("read h2");
    assert_eq!(h2, "Steps");

    let li_count: u32 = h
        .eval("document.querySelectorAll('[data-testid=\"md-body\"] ul li').length")
        .await
        .expect("count li");
    assert_eq!(li_count, 3);

    let first_li: String = h
        .eval("document.querySelector('[data-testid=\"md-body\"] ul li').textContent")
        .await
        .expect("first li");
    assert_eq!(first_li, "one");
}

#[tokio::test]
#[ignore = "browser"]
async fn md_gfm_table_renders() {
    let h = TestHarness::launch().await.expect("launch");
    run_assistant_turn(
        &h,
        "| col a | col b |\n|-------|-------|\n| 1     | 2     |\n",
    )
    .await;

    let table: bool = h
        .eval("!!document.querySelector('[data-testid=\"md-body\"] table')")
        .await
        .expect("table exists");
    assert!(table, "no <table> rendered");

    let th: String = h
        .eval("document.querySelector('[data-testid=\"md-body\"] th').textContent.trim()")
        .await
        .expect("first th");
    assert_eq!(th, "col a");

    let td: String = h
        .eval("document.querySelector('[data-testid=\"md-body\"] td').textContent.trim()")
        .await
        .expect("first td");
    assert_eq!(td, "1");
}

#[tokio::test]
#[ignore = "browser"]
async fn md_links_keep_href() {
    let h = TestHarness::launch().await.expect("launch");
    run_assistant_turn(&h, "see [omega](https://example.com/foo)").await;

    let text: String = h
        .eval("document.querySelector('[data-testid=\"md-body\"] a').textContent")
        .await
        .expect("a text");
    assert_eq!(text, "omega");

    let href: String = h
        .eval("document.querySelector('[data-testid=\"md-body\"] a').getAttribute('href')")
        .await
        .expect("a href");
    assert_eq!(href, "https://example.com/foo");
}

#[tokio::test]
#[ignore = "browser"]
async fn md_fenced_code_keeps_language_class() {
    let h = TestHarness::launch().await.expect("launch");
    run_assistant_turn(&h, "```rust\nlet x = 1;\n```\n").await;

    let sel = "[data-testid=\"md-body\"] pre code.language-rust";
    h.wait_for_selector(sel, Duration::from_secs(3))
        .await
        .expect("language-rust block missing");

    let body: String = h
        .eval(&format!("document.querySelector('{sel}').textContent"))
        .await
        .expect("code text");
    assert!(body.contains("let x = 1;"), "code body: {body:?}");
}

#[tokio::test]
#[ignore = "browser"]
async fn md_raw_html_is_escaped() {
    let h = TestHarness::launch().await.expect("launch");
    run_assistant_turn(&h, "hello <script>alert(1)</script>").await;

    let scripts: u32 = h
        .eval("document.querySelectorAll('[data-testid=\"md-body\"] script').length")
        .await
        .expect("script count");
    assert_eq!(scripts, 0, "<script> rendered as live DOM!");

    let body: String = h
        .eval("document.querySelector('[data-testid=\"md-body\"]').textContent")
        .await
        .expect("body text");
    assert!(
        body.contains("<script>alert(1)</script>"),
        "expected escaped script tag in text, got: {body:?}"
    );
}

// ---------------------------------------------------------------------------
// Diff colouring
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "browser"]
async fn md_diff_block_gets_line_classes() {
    let h = TestHarness::launch().await.expect("launch");
    run_assistant_turn(
        &h,
        "```diff\n--- a/foo.rs\n+++ b/foo.rs\n@@ -1 +1 @@\n- old\n+ new\n  ctx\n```\n",
    )
    .await;

    // Diff post-processing is async — give it the same 5 s window
    // the original spec used.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let counts: serde_json::Value = h
            .eval(
                r#"({
                    file: document.querySelectorAll('[data-testid="md-body"] .diff-file').length,
                    hunk: document.querySelectorAll('[data-testid="md-body"] .diff-hunk').length,
                    add: document.querySelectorAll('[data-testid="md-body"] .diff-add').length,
                    del: document.querySelectorAll('[data-testid="md-body"] .diff-del').length,
                    ctx: document.querySelectorAll('[data-testid="md-body"] .diff-ctx').length,
                    block: document.querySelectorAll('[data-testid="md-body"] pre.diff-block').length,
                    blockTestId: document.querySelectorAll('[data-testid="md-body"] pre[data-testid="diff-block"]').length,
                })"#,
            )
            .await
            .expect("eval diff counts");

        if counts["file"] == 2
            && counts["hunk"] == 1
            && counts["add"] == 1
            && counts["del"] == 1
            && counts["ctx"] == 1
            && counts["block"].as_u64().unwrap_or(0) >= 1
            && counts["blockTestId"].as_u64().unwrap_or(0) >= 1
        {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "diff classes never settled, last: {counts}"
        );
        tokio::time::sleep(Duration::from_millis(80)).await;
    }
}

#[tokio::test]
#[ignore = "browser"]
async fn md_patch_language_triggers_diff() {
    let h = TestHarness::launch().await.expect("launch");
    run_assistant_turn(&h, "```patch\n+ added\n```\n").await;

    h.wait_for_selector(
        "[data-testid=\"md-body\"] .diff-add",
        Duration::from_secs(5),
    )
    .await
    .expect(".diff-add never appeared on language-patch");
}

// ---------------------------------------------------------------------------
// Mermaid lazy-load
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "browser"]
async fn md_mermaid_renders_svg() {
    let h = TestHarness::launch().await.expect("launch");
    run_assistant_turn(&h, "```mermaid\ngraph LR\n  A --> B\n```\n").await;

    // Lazy-loaded from a CDN — give it the same 15 s window the
    // original spec used.
    h.wait_for_selector("[data-testid=\"mermaid-wrapper\"]", Duration::from_secs(15))
        .await
        .expect("mermaid-wrapper never mounted");

    h.wait_for_selector(
        "[data-testid=\"mermaid-diagram\"] svg",
        Duration::from_secs(5),
    )
    .await
    .expect("rendered SVG never appeared");
}

#[tokio::test]
#[ignore = "browser"]
async fn md_invalid_mermaid_surfaces_error() {
    let h = TestHarness::launch().await.expect("launch");
    run_assistant_turn(
        &h,
        "```mermaid\nthis is not valid mermaid syntax !!!\n```\n",
    )
    .await;

    h.wait_for_selector("[data-testid=\"mermaid-wrapper\"]", Duration::from_secs(15))
        .await
        .expect("mermaid-wrapper never mounted");

    h.wait_for_selector(
        "[data-testid=\"mermaid-error-notice\"]",
        Duration::from_secs(5),
    )
    .await
    .expect("mermaid-error-notice never mounted");

    let notice: String = h
        .text_content("[data-testid=\"mermaid-error-notice\"]")
        .await
        .expect("error notice text");
    assert!(
        notice.contains("⚠ Mermaid error"),
        "unexpected notice text: {notice:?}"
    );

    let source: String = h
        .text_content("[data-testid=\"mermaid-source\"]")
        .await
        .expect("source text");
    assert!(
        source.contains("this is not valid mermaid syntax"),
        "unexpected source text: {source:?}"
    );
}

// ---------------------------------------------------------------------------
// Streaming overlay stays plain
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "browser"]
async fn md_streaming_overlay_renders_raw_text() {
    let h = TestHarness::launch().await.expect("launch");

    h.reset_calls().await.expect("reset_calls");
    h.load_script(vec![MockResponse::SlowText {
        text: "**still streaming** and growing".into(),
        chunks: 4,
        delay_ms: 80,
    }])
    .await
    .expect("load_script");

    h.new_session().await.expect("new_session");
    h.fill(
        "[data-testid=\"leptos-composer-input\"]",
        "render streaming",
    )
    .await
    .expect("fill");
    h.press_key("[data-testid=\"leptos-composer-input\"]", "Enter")
        .await
        .expect("submit");

    h.wait_for_selector(
        "[data-testid=\"leptos-streaming-text\"]",
        Duration::from_secs(5),
    )
    .await
    .expect("streaming overlay never appeared");

    // Poll the streaming `<pre>` body until the full raw text
    // arrives.  Playwright's `toContainText` retries; we have to
    // do that explicitly.  The overlay must NEVER mount a <strong>
    // (parsing only happens on settled blocks) — assert that
    // invariant on every poll.
    let pre_sel = "[data-testid=\"leptos-streaming-text\"] pre.block-body";
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    loop {
        let snap: serde_json::Value = h
            .eval(&format!(
                r#"({{
                    text: (document.querySelector('{pre_sel}') || {{}}).textContent || '',
                    strongs: document.querySelectorAll('[data-testid="leptos-streaming-text"] strong').length,
                }})"#
            ))
            .await
            .expect("poll streaming overlay");
        assert_eq!(
            snap["strongs"].as_u64().unwrap_or(0),
            0,
            "<strong> mounted in streaming overlay"
        );
        if snap["text"]
            .as_str()
            .unwrap_or("")
            .contains("**still streaming**")
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "raw stars never arrived in overlay, last text: {:?}",
            snap["text"]
        );
        tokio::time::sleep(Duration::from_millis(40)).await;
    }

    // After the turn settles the persisted block parses markdown.
    h.wait_for_count(TURN_END, 1, Duration::from_secs(10))
        .await
        .expect("turn_end never landed");

    let strong: String = h
        .eval("document.querySelector('[data-testid=\"md-body\"] strong').textContent")
        .await
        .expect("settled strong");
    assert_eq!(strong, "still streaming");
}
