# omega-tools — 16 surviving mutants

**Session 1 target.**

## Testing approach

All survivors live in the output-processing pipeline of the `run_command` tool.
The right test surface is `execute_tool("run_command", input, None, None)` from
`crates/omega-tools/src/lib.rs` — the same entry point the agent uses.
Tests go in `lib.rs`'s existing `#[cfg(test)]` block (or a new one there).

Do **not** call the private helpers (`is_utf8_continuation`,
`utf8_boundary_forward/backward`, `crlf_normalize`) directly — they are
implementation details of the pipeline.

Verify kills with: `cargo mutants -p omega-tools -j1`

---

## Survivors and what test each needs

### 1. `is_utf8_continuation` (cap_and_tee.rs:206) — 2 mutants

```rust
fn is_utf8_continuation(b: u8) -> bool {
    (b & 0xC0) == 0x80   // & → |   and   & → ^
}
```

These survive because no test produces output where a multi-byte UTF-8
character straddles the truncation boundary, so the loop calling this
function never iterates.

**Test:** Run a command that emits a large block of multi-byte text (e.g.
`printf '%.0sé' {1..60000}` — `é` is 2 bytes, so 60 000 repetitions = 120 KB,
over the 100 KB `LLM_CAP`). With default `Head` bias, assert the returned
string is valid UTF-8 (no `\u{FFFD}` replacement characters) and contains the
truncation footer.

---

### 2. `utf8_boundary_forward` (cap_and_tee.rs:183–184) — 3 mutants

```rust
while end > 0 && is_utf8_continuation(data[end - 1]) {  // > → ==,  - → /
    end -= 1;                                             // -= → +=
}
```

Same root cause as above — the loop body never executes in existing tests.

**Test:** Same test as §1 covers these automatically, because with `Head` bias
`utf8_boundary_forward` is the function that snaps the head window to a valid
boundary. If the result is valid UTF-8, the loop ran correctly.

---

### 3. `utf8_boundary_backward` (cap_and_tee.rs:197–198) — 5 mutants

```rust
while start < len && is_utf8_continuation(data[start]) {  // < → ==, >, <=
    start += 1;                                            // += → -=, *=
}
```

Same root cause; `utf8_boundary_backward` is called for `Tail` bias.

**Test:** Same command as §1 but with `"truncation_bias": "tail"` in the input
JSON. Assert valid UTF-8 in the result. The `Middle` bias calls both forward
and backward — add a third variant with `"truncation_bias": "middle"` to cover
both directions in one shot.

---

### 4. `cap_and_tee` with `TruncationBias::Middle` (cap_and_tee.rs:127, 130) — 2 mutants

```rust
let half = cap / 2;                      // / → *
let tail_start = total_bytes - tail_len; // - → /
```

These are in the `Middle` branch, which splits the cap into head and tail
halves. No existing test exercises Middle bias.

**Test:** The `"truncation_bias": "middle"` variant from §3 covers these.
Additionally assert the body contains both a fragment from the start of the
output **and** a fragment from the end, with the `"... N bytes omitted ..."`
marker in between. That assertion distinguishes `cap / 2` from `cap * 2`
(which would yield an oversized window that may not truncate at all).

---

### 5. `crlf_normalize` (output_cleaner.rs:70) — 2 mutants

```rust
if data[i] == b'\r' && i + 1 < data.len() && data[i + 1] == b'\n' {
//                      ^^^^^^^^^^^^^^^^^ < → <=       + → *
```

The `< → <=` mutation would panic when `\r` is the last byte of the buffer
(out-of-bounds read). The `+ → *` mutation would always check `data[0]`
instead of `data[i+1]`, silently misidentifying lone `\r` bytes as CRLF.

**Test:** Run a command that produces a `\r` as its final byte with no
following `\n` — e.g. `printf 'foo\r'`. Assert no panic and the output
contains the `\r` unchanged. Also run a command that produces `\r\n` sequences
and assert they are collapsed to `\n` in the result.

---

### 6. `execute` — truncation bias selection (run_command.rs:187) — 2 mutants

```rust
let bias = bias_override.unwrap_or_else(|| match &outcome {
    Outcome::Finished(Some(s)) if s.success() => TruncationBias::Head,  // guard → true / false
    _ => TruncationBias::Tail,
});
```

With `guard → true` every command uses Head bias regardless of exit code.
With `guard → false` every command uses Tail bias regardless of exit code.
No test currently runs `execute_tool("run_command", ...)` without a
`truncation_bias` override and then checks which bias was applied.

**Test:** Two tests, both with output long enough to trigger truncation and
no `truncation_bias` field in the input:
1. Command exits 0 (e.g. `yes | head -n 20000`) — assert footer says
   `"showed first"` (Head bias).
2. Command exits non-zero (e.g. `yes | head -n 20000; exit 1`) — assert
   footer says `"showed last"` (Tail bias).
