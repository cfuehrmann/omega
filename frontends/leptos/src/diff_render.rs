//! Diff/patch syntax colouring (Phase 3.6).
//!
//! Mirrors the SolidJS UI's [`renderDiff`] in `App.tsx` (line 35\u201355):
//! each line of a fenced `language-diff` / `language-patch` code
//! block becomes a `<span>` with one of five class names, so CSS can
//! paint the gutter colours uniformly.
//!
//! Pure helpers \u2014 no DOM. The actual `<code>.innerHTML = ...`
//! mutation lives in [`crate::feed::enhance_md_body`] and operates on
//! the live DOM after markdown markup is mounted.
//!
//! ## Mutation-test coverage
//!
//! Every helper in this module is target-agnostic. Acceptance:
//! `cargo mutants --target wasm32-unknown-unknown` reports
//! **0 missed** on `diff_render.rs`.

use crate::markdown::escape_html;

/// One of the five line-classes the SolidJS UI's CSS recognises.
///
/// The variants and the priority order in [`classify_line`] are
/// pinned by Playwright specs (`web-ui-4.spec.ts`) and snapshot
/// tests (`tests/snapshots.rs`).
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum DiffLine {
    /// `--- a/foo.ts` or `+++ b/foo.ts` \u2014 file headers. Detected
    /// **before** `Add` / `Del` because `+++` starts with `+` and
    /// `---` starts with `-`.
    File,
    /// `@@ -1,3 +1,4 @@` \u2014 hunk header.
    Hunk,
    /// Line starting with `+` (and not a file header).
    Add,
    /// Line starting with `-` (and not a file header).
    Del,
    /// Anything else \u2014 context line.
    Ctx,
}

impl DiffLine {
    /// CSS class name written into the rendered span. Pinned by both
    /// Playwright assertions (`.diff-add`, `.diff-del`, etc.) and
    /// the SolidJS UI's CSS file.
    #[must_use]
    pub fn class(self) -> &'static str {
        match self {
            Self::File => "diff-file",
            Self::Hunk => "diff-hunk",
            Self::Add => "diff-add",
            Self::Del => "diff-del",
            Self::Ctx => "diff-ctx",
        }
    }
}

/// Project a single diff source line into its visual class.
///
/// Priority: `File` (`+++`/`---`) wins before `Add`/`Del` so that
/// file headers don't get mis-painted as add/del lines. `Hunk`
/// (`@@`) is independent. Mirrors `App.tsx::renderDiff`'s if-chain.
///
/// Pure; mutation-tested.
#[must_use]
pub fn classify_line(line: &str) -> DiffLine {
    if line.starts_with("+++") || line.starts_with("---") {
        DiffLine::File
    } else if line.starts_with('+') {
        DiffLine::Add
    } else if line.starts_with('-') {
        DiffLine::Del
    } else if line.starts_with("@@") {
        DiffLine::Hunk
    } else {
        DiffLine::Ctx
    }
}

/// Render the body of a diff `<code>` element: each input line
/// becomes a `<span class="diff-...">{escaped}</span>`. Concatenated
/// with no separator (the CSS uses `display: block` to put each on
/// its own line, matching `App.tsx::renderDiff`).
///
/// Trailing-empty-string from a final newline is dropped to mirror
/// `App.tsx::renderDiff`'s `if (lines[lines.length - 1] === "")
/// lines.pop();`.
///
/// Pure; mutation-tested.
#[must_use]
pub fn render_diff_html(source: &str) -> String {
    // `split('\n')` matches the JS `split("\n")` (drops the empty
    // trailing element only via the explicit pop below; intermediate
    // empty lines are preserved as empty `diff-ctx` rows).
    let mut lines: Vec<&str> = source.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    let mut out = String::with_capacity(source.len() + 32 * lines.len());
    for line in &lines {
        let kind = classify_line(line);
        out.push_str("<span class=\"");
        out.push_str(kind.class());
        out.push_str("\">");
        out.push_str(&escape_html(line));
        out.push_str("</span>");
    }
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

    // --- classify_line: every variant + boundaries -----------------------

    #[wasm_bindgen_test]
    #[test]
    fn classify_add_line() {
        assert_eq!(classify_line("+ added"), DiffLine::Add);
    }

    #[wasm_bindgen_test]
    #[test]
    fn classify_del_line() {
        assert_eq!(classify_line("- removed"), DiffLine::Del);
    }

    #[wasm_bindgen_test]
    #[test]
    fn classify_hunk_line() {
        assert_eq!(classify_line("@@ -1,3 +1,4 @@"), DiffLine::Hunk);
    }

    #[wasm_bindgen_test]
    #[test]
    fn classify_file_minus_header() {
        assert_eq!(classify_line("--- a/foo.rs"), DiffLine::File);
    }

    #[wasm_bindgen_test]
    #[test]
    fn classify_file_plus_header() {
        assert_eq!(classify_line("+++ b/foo.rs"), DiffLine::File);
    }

    #[wasm_bindgen_test]
    #[test]
    fn classify_context_line() {
        assert_eq!(classify_line("  unchanged"), DiffLine::Ctx);
    }

    #[wasm_bindgen_test]
    #[test]
    fn classify_empty_line() {
        // Empty line is context (every match arm fails).
        assert_eq!(classify_line(""), DiffLine::Ctx);
    }

    #[wasm_bindgen_test]
    #[test]
    fn classify_file_priority_over_add() {
        // `+++` starts with `+` \u2014 must classify as File, not Add.
        // Pinning this catches a swap of the if-arms.
        assert_eq!(classify_line("+++"), DiffLine::File);
    }

    #[wasm_bindgen_test]
    #[test]
    fn classify_file_priority_over_del() {
        assert_eq!(classify_line("---"), DiffLine::File);
    }

    #[wasm_bindgen_test]
    #[test]
    fn classify_single_plus_is_add() {
        assert_eq!(classify_line("+"), DiffLine::Add);
    }

    #[wasm_bindgen_test]
    #[test]
    fn classify_single_minus_is_del() {
        assert_eq!(classify_line("-"), DiffLine::Del);
    }

    #[wasm_bindgen_test]
    #[test]
    fn classify_at_sign_alone_is_context() {
        // Single `@` doesn't qualify as `@@` hunk header.
        assert_eq!(classify_line("@"), DiffLine::Ctx);
    }

    #[wasm_bindgen_test]
    #[test]
    fn classify_double_at_is_hunk() {
        assert_eq!(classify_line("@@"), DiffLine::Hunk);
    }

    // --- DiffLine::class: per-variant + pairwise unique -----------------

    #[wasm_bindgen_test]
    #[test]
    fn class_strings_match_solidjs() {
        // Pinned by SolidJS CSS + Playwright `.diff-*` selectors.
        assert_eq!(DiffLine::Add.class(), "diff-add");
        assert_eq!(DiffLine::Del.class(), "diff-del");
        assert_eq!(DiffLine::Hunk.class(), "diff-hunk");
        assert_eq!(DiffLine::File.class(), "diff-file");
        assert_eq!(DiffLine::Ctx.class(), "diff-ctx");
    }

    #[wasm_bindgen_test]
    #[test]
    fn class_strings_pairwise_unique() {
        let all = [
            DiffLine::Add,
            DiffLine::Del,
            DiffLine::Hunk,
            DiffLine::File,
            DiffLine::Ctx,
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(
                    a.class(),
                    b.class(),
                    "class({a:?}) collides with class({b:?})"
                );
            }
        }
    }

    // --- render_diff_html: fixture-level outputs -----------------------

    #[wasm_bindgen_test]
    #[test]
    fn render_diff_html_simple_added_line() {
        let out = render_diff_html("+ added\n");
        assert_eq!(out, "<span class=\"diff-add\">+ added</span>");
    }

    #[wasm_bindgen_test]
    #[test]
    fn render_diff_html_drops_trailing_empty() {
        // Trailing newline produces an empty final element which is
        // dropped (parity with App.tsx::renderDiff).
        let n = render_diff_html("+ a\n").matches("<span").count();
        assert_eq!(n, 1);
    }

    #[wasm_bindgen_test]
    #[test]
    fn render_diff_html_preserves_intermediate_empty() {
        // Intermediate empty lines stay as empty `diff-ctx` spans.
        let out = render_diff_html("+ a\n\n- b\n");
        assert_eq!(
            out,
            "<span class=\"diff-add\">+ a</span>\
             <span class=\"diff-ctx\"></span>\
             <span class=\"diff-del\">- b</span>"
        );
    }

    #[wasm_bindgen_test]
    #[test]
    fn render_diff_html_escapes_html_chars() {
        let out = render_diff_html("+ <script>alert(1)</script>\n");
        assert!(
            out.contains("&lt;script&gt;alert(1)&lt;/script&gt;"),
            "got: {out}"
        );
        assert!(!out.contains("<script>"), "got: {out}");
    }

    #[wasm_bindgen_test]
    #[test]
    fn render_diff_html_full_patch() {
        let src = "--- a/foo.rs\n+++ b/foo.rs\n@@ -1 +1 @@\n- old\n+ new\n  ctx\n";
        let out = render_diff_html(src);
        // Six spans, one per line (no trailing empty).
        assert_eq!(out.matches("<span").count(), 6);
        assert!(out.contains("<span class=\"diff-file\">--- a/foo.rs</span>"));
        assert!(out.contains("<span class=\"diff-file\">+++ b/foo.rs</span>"));
        assert!(out.contains("<span class=\"diff-hunk\">@@ -1 +1 @@</span>"));
        assert!(out.contains("<span class=\"diff-del\">- old</span>"));
        assert!(out.contains("<span class=\"diff-add\">+ new</span>"));
        assert!(out.contains("<span class=\"diff-ctx\">  ctx</span>"));
    }

    #[wasm_bindgen_test]
    #[test]
    fn render_diff_html_no_separator_between_spans() {
        // SolidJS uses `display: block` so adjacent spans visually
        // wrap; pin the literal absence of separators (a future
        // refactor that adds a `\n` would break the SolidJS CSS).
        let out = render_diff_html("+ a\n+ b\n");
        assert_eq!(
            out,
            "<span class=\"diff-add\">+ a</span><span class=\"diff-add\">+ b</span>"
        );
    }

    #[wasm_bindgen_test]
    #[test]
    fn render_diff_html_empty_input() {
        assert_eq!(render_diff_html(""), "");
    }

    #[wasm_bindgen_test]
    #[test]
    fn render_diff_html_just_newline() {
        // Single `\n` \u2014 `split` gives `["", ""]`; trailing empty
        // dropped \u2014 leaves one empty ctx span.
        assert_eq!(
            render_diff_html("\n"),
            "<span class=\"diff-ctx\"></span>"
        );
    }
}
