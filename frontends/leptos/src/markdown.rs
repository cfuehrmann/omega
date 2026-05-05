//! Markdown rendering for assistant text (Phase 3.6).
//!
//! Mirrors the SolidJS UI's [`MdBody`] surface:
//!
//! * `marked` (TS) → `pulldown-cmark` (Rust). Same GFM subset
//!   (`ENABLE_TABLES | ENABLE_STRIKETHROUGH`).
//! * `marked.use({ renderer })` HTML-escape override → the [`escape_inline_html`]
//!   filter that replaces `Event::Html` / `Event::InlineHtml` with
//!   text events containing the escaped source. Mirrors the
//!   `_renderer.html = ({text}) => text.replace(/</g, "&lt;").replace(/>/g, "&gt;")`
//!   semantics: raw HTML in markdown source is rendered as visible
//!   text, never as live DOM.
//!
//! The output of [`render_to_html`] is a single HTML string suitable
//! for `inner_html=` on a leptos `<div>`. Post-mount enhancements
//! (copy buttons, diff colouring, mermaid lazy-render) live in
//! [`crate::feed::enhance_md_body`] and operate on the live DOM after
//! the markup is mounted \u2014 mirrors `App.tsx::enhanceCodeBlocks` +
//! `renderMermaidBlocks`.
//!
//! ## Mutation-test coverage
//!
//! Every helper in this module is pure (target-agnostic, no DOM, no
//! reactive state). The acceptance criterion is `cargo mutants
//! --target wasm32-unknown-unknown` reporting **0 missed** on
//! `markdown.rs` once the `tests/snapshots.rs` insta harness lands.

use pulldown_cmark::{Event, Options, Parser};

/// HTML-escape every char that has a special meaning inside an HTML
/// element body or quoted attribute value. We escape the full set
/// (`&`, `<`, `>`, `"`, `'`) rather than the `marked`-renderer subset
/// (`<`, `>`) because pulldown-cmark only invokes us with raw inline
/// HTML \u2014 we may legitimately receive `&` and `'` and we'd rather not
/// surface them as live entities.
///
/// Pure; mutation-tested.
#[must_use]
pub fn escape_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for c in input.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

/// Markdown render options. Public so the snapshot tests can pin the
/// option set explicitly.
#[must_use]
pub fn render_options() -> Options {
    // Match the SolidJS UI: GFM (tables + strikethrough), no `breaks`
    // (a single newline does not become `<br/>`), no raw HTML
    // passthrough.
    Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH
}

/// Replace each `Event::Html` and `Event::InlineHtml` with an
/// equivalent `Event::Text` carrying the **un**-escaped source.
///
/// `pulldown_cmark::html::push_html` runs every `Event::Text`
/// through its own HTML-text escaper (`&` → `&amp;`, `<` → `&lt;`,
/// `>` → `&gt;`), so passing the source verbatim produces correctly
/// escaped output — pre-escaping here would double-escape (`<` →
/// `&amp;lt;`). Mirrors `marked`'s `_renderer.html = ({text}) =>
/// text.replace(/</g, "&lt;")` semantics: raw HTML in markdown source
/// is rendered as visible text, never as live DOM.
///
/// Pure; mutation-tested.
#[must_use]
pub fn escape_inline_html(event: Event<'_>) -> Event<'_> {
    match event {
        Event::Html(s) | Event::InlineHtml(s) => Event::Text(s),
        other => other,
    }
}

/// Render markdown text to a single HTML string.
///
/// * GFM tables + strikethrough on; CommonMark elsewhere.
/// * Raw HTML in source is escaped to visible text (see
///   [`escape_inline_html`]).
/// * No syntax highlighting at the markdown layer; diff/patch
///   colouring and Mermaid post-processing happen post-mount in
///   [`crate::feed::enhance_md_body`].
///
/// Pure; mutation-tested.
#[must_use]
pub fn render_to_html(text: &str) -> String {
    let parser = Parser::new_ext(text, render_options()).map(escape_inline_html);
    let mut out = String::with_capacity(text.len());
    pulldown_cmark::html::push_html(&mut out, parser);
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]

    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test;

    // --- escape_html -------------------------------------------------------

    #[wasm_bindgen_test]
    #[test]
    fn escape_html_passes_plain_text_through() {
        assert_eq!(escape_html("hello world"), "hello world");
    }

    #[wasm_bindgen_test]
    #[test]
    fn escape_html_escapes_lt() {
        assert_eq!(escape_html("a < b"), "a &lt; b");
    }

    #[wasm_bindgen_test]
    #[test]
    fn escape_html_escapes_gt() {
        assert_eq!(escape_html("a > b"), "a &gt; b");
    }

    #[wasm_bindgen_test]
    #[test]
    fn escape_html_escapes_amp() {
        assert_eq!(escape_html("a & b"), "a &amp; b");
    }

    #[wasm_bindgen_test]
    #[test]
    fn escape_html_escapes_double_quote() {
        assert_eq!(escape_html("\"x\""), "&quot;x&quot;");
    }

    #[wasm_bindgen_test]
    #[test]
    fn escape_html_escapes_single_quote() {
        assert_eq!(escape_html("'x'"), "&#39;x&#39;");
    }

    #[wasm_bindgen_test]
    #[test]
    fn escape_html_handles_full_attack_vector() {
        // `&` MUST come first so the test catches the order-swap mutation.
        assert_eq!(
            escape_html("<img src=\"x\" onerror='alert(1)'>"),
            "&lt;img src=&quot;x&quot; onerror=&#39;alert(1)&#39;&gt;"
        );
    }

    #[wasm_bindgen_test]
    #[test]
    fn escape_html_preserves_unicode() {
        assert_eq!(escape_html("\u{1F600} hi"), "\u{1F600} hi");
    }

    #[wasm_bindgen_test]
    #[test]
    fn escape_html_empty_input() {
        assert_eq!(escape_html(""), "");
    }

    // --- render_options ----------------------------------------------------

    #[wasm_bindgen_test]
    #[test]
    fn render_options_enables_tables() {
        let opts = render_options();
        assert!(opts.contains(Options::ENABLE_TABLES));
    }

    #[wasm_bindgen_test]
    #[test]
    fn render_options_enables_strikethrough() {
        let opts = render_options();
        assert!(opts.contains(Options::ENABLE_STRIKETHROUGH));
    }

    #[wasm_bindgen_test]
    #[test]
    fn render_options_does_not_enable_smart_punctuation() {
        // SolidJS doesn't enable smart punctuation; pin the negative
        // so a future flip is conscious.
        let opts = render_options();
        assert!(!opts.contains(Options::ENABLE_SMART_PUNCTUATION));
    }

    #[wasm_bindgen_test]
    #[test]
    fn render_options_does_not_enable_footnotes() {
        let opts = render_options();
        assert!(!opts.contains(Options::ENABLE_FOOTNOTES));
    }

    #[wasm_bindgen_test]
    #[test]
    fn render_options_exact_bits() {
        // Pinning the exact bitflag value catches operator mutations
        // (`|` → `^`, `|` → `&`) that disjoint-bit `contains`
        // checks alone cannot distinguish. ENABLE_TABLES and
        // ENABLE_STRIKETHROUGH are disjoint flags so `T | S == T ^ S`
        // bit-pattern — only an exact equality catches the swap.
        let opts = render_options();
        let expected = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH;
        assert_eq!(opts.bits(), expected.bits());
    }

    // --- escape_inline_html ------------------------------------------------

    #[wasm_bindgen_test]
    #[test]
    fn escape_inline_html_replaces_block_html() {
        // Replacement carries the raw source verbatim; the actual
        // escaping happens later inside `push_html` for every
        // `Event::Text`. Pre-escaping here would produce
        // `&amp;lt;` (double-escaped); pin against that mistake.
        let ev = Event::Html("<script>alert(1)</script>".into());
        match escape_inline_html(ev) {
            Event::Text(s) => assert_eq!(s.as_ref(), "<script>alert(1)</script>"),
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[wasm_bindgen_test]
    #[test]
    fn escape_inline_html_replaces_inline_html() {
        let ev = Event::InlineHtml("<b>".into());
        match escape_inline_html(ev) {
            Event::Text(s) => assert_eq!(s.as_ref(), "<b>"),
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[wasm_bindgen_test]
    #[test]
    fn escape_inline_html_passes_text_through() {
        let ev = Event::Text("plain".into());
        match escape_inline_html(ev) {
            Event::Text(s) => assert_eq!(s.as_ref(), "plain"),
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[wasm_bindgen_test]
    #[test]
    fn escape_inline_html_passes_softbreak_through() {
        let ev = Event::SoftBreak;
        assert!(matches!(escape_inline_html(ev), Event::SoftBreak));
    }

    // --- render_to_html ----------------------------------------------------

    #[wasm_bindgen_test]
    #[test]
    fn render_paragraph() {
        let out = render_to_html("hello");
        assert_eq!(out.trim(), "<p>hello</p>");
    }

    #[wasm_bindgen_test]
    #[test]
    fn render_strong() {
        let out = render_to_html("**bold** text");
        assert!(out.contains("<strong>bold</strong>"), "got: {out}");
    }

    #[wasm_bindgen_test]
    #[test]
    fn render_inline_code() {
        let out = render_to_html("`code`");
        assert!(out.contains("<code>code</code>"));
    }

    #[wasm_bindgen_test]
    #[test]
    fn render_unordered_list() {
        let out = render_to_html("- a\n- b\n- c\n");
        assert!(out.contains("<ul>"));
        assert!(out.contains("<li>a</li>"));
        assert!(out.contains("<li>c</li>"));
    }

    #[wasm_bindgen_test]
    #[test]
    fn render_ordered_list() {
        let out = render_to_html("1. one\n2. two\n");
        assert!(out.contains("<ol>"));
        assert!(out.contains("<li>one</li>"));
    }

    #[wasm_bindgen_test]
    #[test]
    fn render_link() {
        let out = render_to_html("[omega](https://example.com)");
        assert!(out.contains(r#"href="https://example.com""#));
        assert!(out.contains(">omega</a>"));
    }

    #[wasm_bindgen_test]
    #[test]
    fn render_fenced_code_block_with_language() {
        let out = render_to_html("```rust\nlet x = 1;\n```\n");
        assert!(
            out.contains(r#"<code class="language-rust">"#),
            "got: {out}"
        );
        assert!(out.contains("let x = 1;"));
    }

    #[wasm_bindgen_test]
    #[test]
    fn render_fenced_code_block_without_language() {
        let out = render_to_html("```\nplain\n```\n");
        // No `class=` attribute when no language given.
        assert!(out.contains("<pre><code>plain"), "got: {out}");
    }

    #[wasm_bindgen_test]
    #[test]
    fn render_gfm_table() {
        let out = render_to_html("| a | b |\n|---|---|\n| 1 | 2 |\n");
        assert!(out.contains("<table>"));
        assert!(out.contains("<th>a</th>"));
        assert!(out.contains("<td>1</td>"));
    }

    #[wasm_bindgen_test]
    #[test]
    fn render_strikethrough() {
        let out = render_to_html("~~gone~~");
        assert!(out.contains("<del>gone</del>"));
    }

    #[wasm_bindgen_test]
    #[test]
    fn render_escapes_inline_html_block() {
        // Raw HTML in source is escaped to text \u2014 the SolidJS UI
        // does the same via marked's renderer.html override.
        let out = render_to_html("<script>alert(1)</script>");
        assert!(!out.contains("<script>"), "got: {out}");
        assert!(out.contains("&lt;script&gt;"));
    }

    #[wasm_bindgen_test]
    #[test]
    fn render_escapes_inline_html_inline() {
        let out = render_to_html("hello <span>x</span>");
        assert!(!out.contains("<span>"));
        assert!(out.contains("&lt;span&gt;"));
    }

    #[wasm_bindgen_test]
    #[test]
    fn render_preserves_amp_in_text() {
        let out = render_to_html("a & b");
        // pulldown-cmark text-escapes `&` itself in normal text.
        assert!(out.contains("a &amp; b"), "got: {out}");
    }

    #[wasm_bindgen_test]
    #[test]
    fn render_mermaid_language_class() {
        let out = render_to_html("```mermaid\ngraph LR\n  A --> B\n```\n");
        // The language class is used by the post-mount enhancer to
        // detect mermaid blocks. Pin the exact class string so the
        // detector and the renderer never drift apart.
        assert!(
            out.contains(r#"<code class="language-mermaid">"#),
            "got: {out}"
        );
    }

    #[wasm_bindgen_test]
    #[test]
    fn render_diff_language_class() {
        let out = render_to_html("```diff\n+ added\n- removed\n```\n");
        assert!(out.contains(r#"<code class="language-diff">"#));
    }

    #[wasm_bindgen_test]
    #[test]
    fn render_patch_language_class() {
        let out = render_to_html("```patch\n@@ -1 +1 @@\n```\n");
        assert!(out.contains(r#"<code class="language-patch">"#));
    }

    #[wasm_bindgen_test]
    #[test]
    fn render_empty_input() {
        assert_eq!(render_to_html(""), "");
    }

    #[wasm_bindgen_test]
    #[test]
    fn render_heading_levels() {
        let out = render_to_html("# h1\n## h2\n### h3\n");
        assert!(out.contains("<h1>h1</h1>"));
        assert!(out.contains("<h2>h2</h2>"));
        assert!(out.contains("<h3>h3</h3>"));
    }

    #[wasm_bindgen_test]
    #[test]
    fn render_blockquote() {
        let out = render_to_html("> quoted\n");
        assert!(out.contains("<blockquote>"));
        assert!(out.contains("quoted"));
    }
}
