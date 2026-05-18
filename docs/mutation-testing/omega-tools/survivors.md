# omega-tools — 16 surviving mutants

**Session 1 target.** All survivors are in `src/cap_and_tee.rs`,
`src/output_cleaner.rs`, and `src/tools/run_command.rs`.
Tests belong in the existing `#[cfg(test)]` blocks in each file
(`cap_and_tee.rs:213`, `output_cleaner.rs:142`; `run_command.rs` needs a new block).

Verify kills with: `cargo mutants -p omega-tools -j1`

---

## `cap_and_tee.rs` — 12 survivors

### `is_utf8_continuation` (line 206) — 2 mutants

```rust
fn is_utf8_continuation(b: u8) -> bool {
    (b & 0xC0) == 0x80   // & → | and & → ^ both survive
}
```

**What's missing:** no test calls this function with bytes that distinguish
`&` from `|` or `^`.  
**Fix:** test every byte class — ASCII (0x00–0x7F), continuation (0x80–0xBF),
leading 2-byte (0xC0–0xDF), leading 3-byte (0xE0–0xEF), leading 4-byte (0xF0+).

---

### `utf8_boundary_forward` (lines 183–184) — 3 mutants

```rust
fn utf8_boundary_forward(data: &[u8], max: usize) -> usize {
    let mut end = max.min(data.len());
    while end > 0 && is_utf8_continuation(data[end - 1]) {  // > → ==, - → /
        end -= 1;                                            // -= → +=
    }
    end
}
```

**What's missing:** no test passes a `max` that lands mid-multibyte-character,
so the while-loop body never executes — all three operator mutations are invisible.  
**Fix:** test with a 3-byte UTF-8 sequence (e.g. `"héllo"`) where `max` cuts
into the second byte of `é`; assert the result backs up to the boundary.
Also test `max = 0` (exercises the `end > 0` guard) and `max > data.len()`.

---

### `utf8_boundary_backward` (lines 197–198) — 5 mutants

```rust
fn utf8_boundary_backward(data: &[u8], max: usize) -> usize {
    let len = data.len();
    let raw_start = len.saturating_sub(max);
    let mut start = raw_start;
    while start < len && is_utf8_continuation(data[start]) {  // < → ==, >, <=
        start += 1;                                            // += → -=, *=
    }
    len - start
}
```

**What's missing:** same as `utf8_boundary_forward` — no test places `raw_start`
mid-character, so the loop body never runs.  
**Fix:** test with a trailing multi-byte character (e.g. `"hello€"`, where `€`
is 3 bytes) where `max` cuts into the continuation bytes; assert the returned
window length steps forward to the next valid start.

---

### `cap_and_tee` with `TruncationBias::Middle` (lines 127, 130) — 2 mutants

```rust
TruncationBias::Middle => {
    let half = cap / 2;                        // / → *
    // ...
    let tail_start = total_bytes - tail_len;   // - → /
```

**What's missing:** `TruncationBias::Middle` is not tested with multi-byte
characters in either the head or tail window.  Existing tests only cover
`Head` and `Tail` bias.  
**Fix:** add a `Middle` bias test with data long enough to truncate; assert
the body contains both the first and last segments and the `"... N bytes omitted ..."` marker.
A follow-up test with a multi-byte character straddling the midpoint exercises
both arithmetic mutations.

---

## `output_cleaner.rs` — 2 survivors

### `crlf_normalize` (line 70) — 2 mutants

```rust
while i < data.len() {
    if data[i] == b'\r' && i + 1 < data.len() && data[i + 1] == b'\n' {
        //                   ^^^^^^^^^^^^^^^^^ < → <=  and  + → *
```

**What's missing:** no test ends with a lone `\r` as the **last byte** of the
buffer. When `\r` is at index `data.len() - 1`, the bounds check `i + 1 < data.len()`
is the only thing preventing an out-of-bounds read; replacing `<` with `<=`
would panic.  
**Fix:** test `b"\r"` (lone carriage return at end of buffer) and `b"foo\r"`
(lone CR after content); assert they pass through unchanged and no panic occurs.
Also test `b"\r\n"` (CRLF as the entire input) to exercise the `+` → `*` mutation
(which would compute index 0 instead of 2, causing wrong branch).

---

## `tools/run_command.rs` — 2 survivors

### `execute` — truncation bias selection (line 187) — 2 mutants

```rust
let bias = bias_override.unwrap_or_else(|| match &outcome {
    Outcome::Finished(Some(s)) if s.success() => TruncationBias::Head,  // guard → true / false
    _ => TruncationBias::Tail,
});
```

**What's missing:** no test calls `execute` with `bias_override: None` and
then checks *which* bias was actually applied based on exit code.  
**Fix:** two tests without a `bias_override`:
1. Command that exits 0 — assert the output footer says `"first"` (Head bias).
2. Command that exits non-zero — assert the footer says `"last"` (Tail bias).

Use `echo ok` / `false` (or `exit 1`) as the command. Both tests need output
long enough to trigger truncation so the footer is visible.

> **Note:** these two mutants are the only ones in this session requiring a
> real subprocess. Write them last; if time runs short they can move to Session 2.
