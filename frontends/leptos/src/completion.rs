//! Pure helpers for the composer's `@`-path file-completion dropdown
//! (Phase 3.4).
//!
//! Three concerns, all pure / DOM-free / mutation-tested:
//!
//! 1. [`at_token_at_cursor`] — given the textarea contents and the
//!    cursor position, decide whether the cursor sits inside an
//!    unbroken `@`-path token. If yes, return its start index and the
//!    path-prefix following the `@`. Mirrors the SolidJS UI's
//!    `getAtToken` (see `src/web/client/App.tsx` around the InputRow).
//!
//! 2. [`accept_completion`] — derive the new textarea state on accept:
//!    `(new_text, new_cursor, drill_in)`. Replaces the at-token (from
//!    the `@` through the cursor) with `@` + the chosen completion;
//!    if the completion ends with `/` the popup should stay open
//!    (drill into the directory).
//!
//! 3. [`next_highlight`] — wrap-around index move with `-1` meaning
//!    "no item highlighted". `delta = +1` for ArrowDown / Tab,
//!    `delta = -1` for ArrowUp / Shift-Tab.
//!
//! ## Mutation-test carve-out
//!
//! The component glue in `composer.rs` (textarea events, fetch
//! firing, popup show/hide, focus restore, NodeRef DOM reads) is the
//! JS-interop edge — same gap pattern as 3.1's `ws.rs`, 3.2's
//! `picker.rs` / `http.rs`, and 3.3's `feed.rs`. Pure logic landed
//! here is what `cargo mutants` is expected to lock down.

// ---------------------------------------------------------------------------
// At-token detection
// ---------------------------------------------------------------------------

/// One `@`-path token detected at a given cursor position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtToken {
    /// Byte offset of the `@` character in the original text.
    pub start: usize,
    /// Path-prefix following the `@` (everything between `@` and the
    /// cursor; never contains whitespace).
    pub prefix: String,
}

/// If the text immediately preceding `cursor` contains an unbroken
/// `@`-path token (no whitespace between the `@` and the cursor),
/// return its start index and the path-prefix string after the `@`.
///
/// `cursor` is a **byte** offset into `text`; values past
/// `text.len()` are clamped, values that fall mid-codepoint return
/// `None`. The returned `start` is a byte offset into the original
/// text suitable for `text[..start]` slicing.
///
/// Mirrors the SolidJS regex `/@(\S*)$/` against `text.slice(0,
/// cursor)`.
#[must_use]
pub fn at_token_at_cursor(text: &str, cursor: usize) -> Option<AtToken> {
    let cursor = cursor.min(text.len());
    if !text.is_char_boundary(cursor) {
        return None;
    }
    let before = &text[..cursor];
    // Walk back from the cursor: the token starts at the last `@`
    // that has no whitespace between itself and the cursor.
    let mut at_index: Option<usize> = None;
    for (i, ch) in before.char_indices().rev() {
        if ch == '@' {
            at_index = Some(i);
            break;
        }
        if ch.is_whitespace() {
            return None;
        }
    }
    let start = at_index?;
    // Prefix is everything after the `@` up to the cursor.
    let prefix = before[start + 1..].to_string();
    Some(AtToken { start, prefix })
}

// ---------------------------------------------------------------------------
// Accept-completion projection
// ---------------------------------------------------------------------------

/// Outcome of accepting a file-completion item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptOutcome {
    /// Replacement text for the textarea.
    pub new_text: String,
    /// New cursor position (byte offset into `new_text`).
    pub new_cursor: usize,
    /// `true` if the popup should stay open and drill into the
    /// selected directory (i.e. `item.ends_with('/')`); `false` if
    /// the popup should close (file completion accepted).
    pub drill_in: bool,
}

/// Replace the at-token under the cursor with `@` + `item`.
///
/// Returns `None` if the cursor isn't inside an at-token (caller
/// should leave the textarea untouched and just close the popup).
/// Otherwise the new text, new cursor, and the drill-in flag are
/// returned together so the caller can hand them to the textarea
/// state and the popup-open flag in one shot.
///
/// Cursor invariant: the new cursor lands immediately *after* the
/// inserted item (i.e. `start + 1 + item.len()`), inside the rebuilt
/// `new_text`.
#[must_use]
pub fn accept_completion(text: &str, cursor: usize, item: &str) -> Option<AcceptOutcome> {
    let cursor = cursor.min(text.len());
    let token = at_token_at_cursor(text, cursor)?;
    // Build "before@token" + "@" + item + "after-cursor".
    let mut new_text = String::with_capacity(text.len() + item.len());
    new_text.push_str(&text[..token.start]);
    new_text.push('@');
    new_text.push_str(item);
    let new_cursor = token.start + 1 + item.len();
    new_text.push_str(&text[cursor..]);
    Some(AcceptOutcome {
        new_text,
        new_cursor,
        drill_in: item.ends_with('/'),
    })
}

// ---------------------------------------------------------------------------
// Picker-insert projection
// ---------------------------------------------------------------------------

/// Compute the new textarea state after the session-picker's `@path` button
/// is clicked with `item` as the path to insert.
///
/// **Always appends** `@item` at the end of the existing text:
///
/// - Empty text → `@item`.
/// - Text ending in whitespace → `<text>@item` (no extra space).
/// - Anything else → `<text> @item` (one separator space).
///
/// We deliberately do **not** splice into a trailing `@`-token the way
/// the keyboard `@`-completion popup does. The picker is conceptually
/// "add a session reference", not "finish the half-typed token". Two
/// `@path` clicks in a row on an empty textarea must keep both paths
/// — and the only way to guarantee that is to never replace existing
/// content. The cursor lands immediately after the inserted `@item`.
#[must_use]
pub fn insert_item_text(text: &str, item: &str) -> (String, usize) {
    let space = if text.is_empty() || text.ends_with(char::is_whitespace) {
        ""
    } else {
        " "
    };
    let insertion = format!("{}@{}", space, item);
    let new_cursor = text.len() + insertion.len();
    (format!("{}{}", text, insertion), new_cursor)
}

// ---------------------------------------------------------------------------
// Highlight-index navigation
// ---------------------------------------------------------------------------

/// Move the popup-highlight index by `delta` with wrap-around.
///
/// Conventions:
/// - `len == 0` always returns `-1` (no item selectable).
/// - `current == -1` means "no item highlighted yet"; the first
///   `delta = +1` lands on `0`, the first `delta = -1` lands on
///   `len - 1`.
/// - `delta` of any non-zero magnitude is normalised to its sign;
///   only `+1` and `-1` are exercised by the composer today, but
///   the function is total over `i32`.
/// - `delta = 0` is a no-op (returns `current` unchanged, clamped
///   to `[-1, len)`).
///
/// Returns the new index in the range `[-1, len)`.
#[must_use]
pub fn next_highlight(current: i32, len: usize, delta: i32) -> i32 {
    if len == 0 {
        return -1;
    }
    #[allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]
    let len_i = len as i32;
    if delta == 0 {
        return current.clamp(-1, len_i - 1);
    }
    // `signum()` returns -1, 0, or 1; we've already filtered out 0,
    // so `direction` is always exactly +1 or -1 below. We compare
    // against `1` directly (rather than `> 0`) so cargo-mutants
    // can't slip through with a `>=` mutation — the two are
    // observationally equivalent on the reachable subset, but
    // `== 1` flipping to `!= 1` is caught by the up/down tests.
    let down = delta.signum() == 1;
    if current < 0 {
        // Cold-start: down → first; up → last.
        return if down { 0 } else { len_i - 1 };
    }
    if down {
        // Down with wrap.
        if current >= len_i - 1 { 0 } else { current + 1 }
    } else {
        // Up with wrap.
        if current <= 0 { len_i - 1 } else { current - 1 }
    }
}

/// Borrow the highlighted item from `items`, or `None` if the
/// highlight index is out of range. Convenience for the composer's
/// "Enter accepts highlighted item" handler.
#[must_use]
pub fn selected_item(items: &[String], highlight: i32) -> Option<&str> {
    if highlight < 0 {
        return None;
    }
    #[allow(clippy::cast_sign_loss)]
    let idx = highlight as usize;
    items.get(idx).map(String::as_str)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]

    use super::*;
    use wasm_bindgen_test::wasm_bindgen_test;

    // ---- at_token_at_cursor -------------------------------------------------

    #[wasm_bindgen_test]
    fn at_token_after_bare_at_returns_empty_prefix() {
        let r = at_token_at_cursor("@", 1).expect("token at @");
        assert_eq!(r.start, 0);
        assert_eq!(r.prefix, "");
    }

    #[wasm_bindgen_test]
    fn at_token_returns_prefix_between_at_and_cursor() {
        let r = at_token_at_cursor("hello @src", 10).expect("token");
        assert_eq!(r.start, 6);
        assert_eq!(r.prefix, "src");
    }

    #[wasm_bindgen_test]
    fn at_token_none_when_cursor_is_before_at() {
        // Cursor sits before the `@` — no token.
        let r = at_token_at_cursor("hi @src", 2);
        assert!(r.is_none());
    }

    #[wasm_bindgen_test]
    fn at_token_none_when_no_at_in_text() {
        assert!(at_token_at_cursor("hello world", 5).is_none());
    }

    #[wasm_bindgen_test]
    fn at_token_none_when_whitespace_between_at_and_cursor() {
        // SolidJS `\S*` regex stops at whitespace.
        assert!(at_token_at_cursor("@src foo", 8).is_none());
    }

    #[wasm_bindgen_test]
    fn at_token_picks_last_at_when_multiple_present() {
        let r = at_token_at_cursor("@a @b/c", 7).expect("token");
        assert_eq!(r.start, 3);
        assert_eq!(r.prefix, "b/c");
    }

    #[wasm_bindgen_test]
    fn at_token_handles_at_at_cursor_boundary() {
        // `@`-symbol immediately before cursor → empty prefix.
        let r = at_token_at_cursor("hi @", 4).expect("token");
        assert_eq!(r.start, 3);
        assert_eq!(r.prefix, "");
    }

    #[wasm_bindgen_test]
    fn at_token_clamps_out_of_range_cursor() {
        // cursor past end is clamped — same result as cursor at end.
        let a = at_token_at_cursor("@src", 999);
        let b = at_token_at_cursor("@src", 4);
        assert_eq!(a, b);
    }

    #[wasm_bindgen_test]
    fn at_token_returns_none_for_mid_codepoint_cursor() {
        // Greek letter α is two bytes (0xCE 0xB1). Cursor at byte 1
        // sits mid-codepoint and the function rejects it (rather than
        // panicking on the slice).
        assert!(at_token_at_cursor("α", 1).is_none());
    }

    #[wasm_bindgen_test]
    fn at_token_handles_multibyte_path_prefix() {
        // Non-ASCII inside the prefix is fine — chars are kept verbatim.
        let r = at_token_at_cursor("@αβ", 5).expect("token");
        assert_eq!(r.start, 0);
        assert_eq!(r.prefix, "αβ");
    }

    #[wasm_bindgen_test]
    fn at_token_at_token_with_path_separators() {
        let r = at_token_at_cursor("hi @src/web/client/", 19).expect("token");
        assert_eq!(r.start, 3);
        assert_eq!(r.prefix, "src/web/client/");
    }

    // ---- accept_completion --------------------------------------------------

    #[wasm_bindgen_test]
    fn accept_replaces_at_token_with_at_plus_item() {
        let out = accept_completion("hi @sr", 6, "src/").expect("accept");
        assert_eq!(out.new_text, "hi @src/");
        // cursor lands after the inserted item.
        assert_eq!(out.new_cursor, 8);
        assert!(out.drill_in, "trailing slash drills in");
    }

    #[wasm_bindgen_test]
    fn accept_inserts_at_top_of_text() {
        // No leading prefix — at-token starts at offset 0.
        let out = accept_completion("@", 1, "App.tsx").expect("accept");
        assert_eq!(out.new_text, "@App.tsx");
        assert_eq!(out.new_cursor, 8);
        assert!(!out.drill_in, "file does not drill in");
    }

    #[wasm_bindgen_test]
    fn accept_preserves_text_after_cursor() {
        // Trailing text after the cursor is preserved verbatim.
        let out = accept_completion("hi @sr after", 6, "src/").expect("accept");
        assert_eq!(out.new_text, "hi @src/ after");
        assert_eq!(out.new_cursor, 8);
    }

    #[wasm_bindgen_test]
    fn accept_drill_in_flag_tracks_trailing_slash() {
        let dir = accept_completion("@", 1, "src/").expect("accept");
        let file = accept_completion("@", 1, "src.txt").expect("accept");
        assert!(dir.drill_in);
        assert!(!file.drill_in);
    }

    #[wasm_bindgen_test]
    fn accept_returns_none_when_cursor_is_not_in_at_token() {
        // Cursor sits before any `@` — nothing to replace.
        assert!(accept_completion("hello", 3, "anything").is_none());
    }

    #[wasm_bindgen_test]
    fn accept_clamps_out_of_range_cursor() {
        let a = accept_completion("@s", 999, "src/").expect("accept");
        let b = accept_completion("@s", 2, "src/").expect("accept");
        assert_eq!(a, b);
    }

    #[wasm_bindgen_test]
    fn accept_handles_multiple_at_tokens_picks_the_one_under_cursor() {
        // Two `@`-tokens, cursor inside the second → only that one
        // is replaced; the first is preserved verbatim. The text is
        // `first @one second @tw end`; cursor=21 sits *after* `w`, so
        // the token captured is `@tw` (start=18, prefix="tw"). The
        // replacement runs from start through cursor; everything from
        // cursor onward (` end`) is preserved.
        let out =
            accept_completion("first @one second @tw end", 21, "two.txt").expect("accept");
        assert_eq!(out.new_text, "first @one second @two.txt end");
        assert!(!out.drill_in);
    }

    // ---- insert_item_text -------------------------------------------------
    //
    // The picker's "@ path" button always APPENDS — it never replaces
    // anything. The keyboard `@`-completion popup is a separate flow
    // that does prefix-replacement, but the picker is just "add a
    // session reference at the end of whatever is already there".
    //
    // The critical regression these tests guard against: two `@path`
    // clicks in a row on a textarea that started empty must keep both
    // paths. The full DOM-level repro is in
    // `omega-e2e/tests/02_picker.rs::picker_at_path_twice_preserves_both_paths`;
    // the unit case below pins the same invariant in fast-feedback form.

    #[wasm_bindgen_test]
    fn insert_item_appends_to_non_empty_text_with_space() {
        let (text, cursor) = insert_item_text("analyze this", "src/");
        assert_eq!(text, "analyze this @src/");
        assert_eq!(cursor, text.len());
    }

    #[wasm_bindgen_test]
    fn insert_item_into_empty_text_has_no_leading_space() {
        let (text, cursor) = insert_item_text("", "src/");
        assert_eq!(text, "@src/");
        assert_eq!(cursor, text.len());
    }

    #[wasm_bindgen_test]
    fn insert_item_after_trailing_whitespace_has_no_extra_space() {
        let (text, cursor) = insert_item_text("analyze this ", "src/");
        assert_eq!(text, "analyze this @src/");
        assert_eq!(cursor, text.len());
    }

    #[wasm_bindgen_test]
    fn insert_item_after_existing_at_path_appends_both() {
        // Regression for the overwrite bug: when the textarea already
        // contains exactly `@<path>` (no leading prose, no whitespace),
        // a second click must NOT treat that as a half-typed @-token
        // and replace it. Both paths must survive.
        let (text, cursor) = insert_item_text("@.omega/sessions/a/", ".omega/sessions/b/");
        assert_eq!(text, "@.omega/sessions/a/ @.omega/sessions/b/");
        assert_eq!(cursor, text.len());
    }

    #[wasm_bindgen_test]
    fn insert_item_cursor_lands_after_inserted_item() {
        let item = ".omega/sessions/xyz/";
        let (text, cursor) = insert_item_text("prompt", item);
        assert_eq!(&text[..cursor], &format!("prompt @{}", item));
        assert_eq!(cursor, text.len());
    }

    // ---- next_highlight -----------------------------------------------------

    #[wasm_bindgen_test]
    fn next_highlight_zero_len_always_returns_minus_one() {
        assert_eq!(next_highlight(-1, 0, 1), -1);
        assert_eq!(next_highlight(0, 0, 1), -1);
        assert_eq!(next_highlight(5, 0, -1), -1);
    }

    #[wasm_bindgen_test]
    fn next_highlight_cold_start_down_lands_on_first() {
        assert_eq!(next_highlight(-1, 5, 1), 0);
    }

    #[wasm_bindgen_test]
    fn next_highlight_cold_start_up_lands_on_last() {
        assert_eq!(next_highlight(-1, 5, -1), 4);
    }

    #[wasm_bindgen_test]
    fn next_highlight_down_increments() {
        assert_eq!(next_highlight(0, 5, 1), 1);
        assert_eq!(next_highlight(3, 5, 1), 4);
    }

    #[wasm_bindgen_test]
    fn next_highlight_down_wraps_at_end() {
        // last → first.
        assert_eq!(next_highlight(4, 5, 1), 0);
    }

    #[wasm_bindgen_test]
    fn next_highlight_up_decrements() {
        assert_eq!(next_highlight(3, 5, -1), 2);
        assert_eq!(next_highlight(1, 5, -1), 0);
    }

    #[wasm_bindgen_test]
    fn next_highlight_up_wraps_at_start() {
        // first → last.
        assert_eq!(next_highlight(0, 5, -1), 4);
    }

    #[wasm_bindgen_test]
    fn next_highlight_zero_delta_is_noop() {
        assert_eq!(next_highlight(2, 5, 0), 2);
        assert_eq!(next_highlight(-1, 5, 0), -1);
    }

    #[wasm_bindgen_test]
    fn next_highlight_normalises_large_delta() {
        // Only direction matters — magnitude is normalised.
        assert_eq!(next_highlight(0, 5, 99), 1);
        assert_eq!(next_highlight(0, 5, -99), 4);
    }

    #[wasm_bindgen_test]
    fn next_highlight_clamps_out_of_range_current_with_zero_delta() {
        // current = 99 with len = 5 and delta = 0 → clamp to 4.
        assert_eq!(next_highlight(99, 5, 0), 4);
        // current = -99 with len = 5 and delta = 0 → clamp to -1.
        assert_eq!(next_highlight(-99, 5, 0), -1);
    }

    // ---- selected_item ------------------------------------------------------

    #[wasm_bindgen_test]
    fn selected_item_returns_some_for_valid_index() {
        let items = vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];
        assert_eq!(selected_item(&items, 0), Some("a"));
        assert_eq!(selected_item(&items, 2), Some("c"));
    }

    #[wasm_bindgen_test]
    fn selected_item_returns_none_for_negative_highlight() {
        let items = vec!["a".to_owned()];
        assert_eq!(selected_item(&items, -1), None);
    }

    #[wasm_bindgen_test]
    fn selected_item_returns_none_for_out_of_range_index() {
        let items = vec!["a".to_owned()];
        assert_eq!(selected_item(&items, 1), None);
        assert_eq!(selected_item(&items, 99), None);
    }

    #[wasm_bindgen_test]
    fn selected_item_returns_none_for_empty_list() {
        let items: Vec<String> = vec![];
        assert_eq!(selected_item(&items, 0), None);
    }
}
