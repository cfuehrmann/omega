# omega-tools — Session 1 plan

## Testing philosophy for this crate

All tests must go through `execute_tool(name, input, None, ctx)` from
`crates/omega-tools/src/lib.rs` — the same entry point the agent uses.

The inline `#[cfg(test)]` blocks in source files are a problem:

| File | Inline tests | Action |
|------|-------------|--------|
| `src/cap_and_tee.rs` | Calls `cap_and_tee()` directly | Migrate to integration tests; keep only `format_size` and `parse_bias` unit tests |
| `src/output_cleaner.rs` | Calls `clean_output()` directly (private fn) | Migrate all to integration tests |
| `src/tools/read_file.rs` | Calls `char_boundary_at_or_before()` directly (private fn) | Delete — already covered by `file_tools.rs` integration test `read_file_multibyte_char_at_boundary_is_trimmed_cleanly` |

Integration tests live in `crates/omega-tools/tests/`. The `run_command`-related
ones go in `process_tools.rs`. File-tool ones go in `file_tools.rs`.

---

## Phase A — migrate existing inline tests

### `src/cap_and_tee.rs`

Every scenario in the existing `#[cfg(test)]` block maps cleanly to an
`execute_tool("run_command", ...)` call:

| Current inline test | Integration equivalent |
|---------------------|----------------------|
| `no_truncation_returns_full_data_with_footer` | `echo hello` with cap > output → assert footer |
| `exactly_cap_bytes_is_not_truncated` | `printf 'x%.0s' {1..100}` with cap=100 |
| `head_bias_returns_first_cap_bytes` | long command, assert result starts with head content |
| `tail_bias_returns_last_cap_bytes` | long command, assert result ends with tail content |
| `middle_bias_returns_head_and_tail` | `truncation_bias: "middle"`, assert both ends |
| `log_file_contains_full_data_even_when_truncated` | `exec_with_ctx`, check log size |
| `creates_nested_parent_directories` | covered by any `exec_with_ctx` call |

Tests to **keep** as unit tests (pure functions, no I/O):
- `format_size_bytes`, `format_size_kilobytes`, `format_size_megabytes`
- `from_str_parses_known_values`, `from_str_unknown_falls_back_to_head`

### `src/output_cleaner.rs`

All `clean_output` tests map to shell commands:

| Current inline test | Integration equivalent |
|---------------------|----------------------|
| `no_cr_no_ansi_returns_identical_bytes` | any plain `echo` |
| `crlf_converted_to_lf` | `printf 'foo\r\nbar\r\n'` |
| `mixed_crlf_and_lf_both_normalised` | `printf 'a\r\nb\nc\r\n'` |
| `apt_get_pattern_preserves_package_names` | `printf '...'` with the exact apt-get pattern |
| `progress_bar_collapses_to_last_frame` | `printf '\rRead 1M words\rRead 2M words\rRead 100M words\n'` |
| `multiple_lines_with_cr_each_collapsed_independently` | `printf 'step1\rSTEP1\nstep2\rSTEP2\n'` |
| `sgr_colour_codes_stripped` | `printf '\x1b[32mok\x1b[0m\n'` |
| `cursor_movement_stripped` | `printf 'line1\x1b[1A\x1b[Kline2\n'` |
| `osc_hyperlink_stripped` | `printf '\x1b]8;;https://example.com\x1b\\\\click here\x1b]8;;\x1b\\\\\n'` |
| `ffmpeg_style_cr_with_ansi` | `printf` with the ffmpeg pattern |
| `tqdm_progress_collapses_to_final_frame` | `printf` with the tqdm pattern |
| `curl_verbose_headers_preserved` | `printf` with the curl pattern |
| `empty_input_returns_empty` | `true` (no output) |

### `src/tools/read_file.rs`

Delete `char_boundary_at_or_before` inline tests — fully covered by the
existing `read_file_multibyte_char_at_boundary_is_trimmed_cleanly` integration
test in `file_tools.rs`.

---

## Phase B — kill the 16 survivors

All 16 survivors are reached through `execute_tool("run_command", ...)`.
Tests go in `process_tools.rs` unless noted. Add them alongside or after the
Phase A migration.

### Survivors 1–5: UTF-8 boundary logic
(`utf8_boundary_forward` lines 183–184 ×3, `utf8_boundary_backward` lines 197–198 ×5,
`is_utf8_continuation` line 206 ×2 — all in `cap_and_tee.rs`)

**Root cause:** No existing test produces multi-byte UTF-8 output large enough
to trigger truncation, so the loops that snap the window to a valid boundary
never iterate.

**Test A — Head bias, UTF-8 boundary:**
```
command: "printf '%.0sé' {1..60000}"   # é = 2 bytes → 120 KB > 100 KB LLM_CAP
```
Assert: result is valid UTF-8 (no `\u{FFFD}`), contains truncation footer,
says "first".

**Test B — Tail bias, UTF-8 boundary:**
Same command, add `"truncation_bias": "tail"`. Assert valid UTF-8, says "last".

**Test C — Middle bias, UTF-8 boundary:**
Same command, add `"truncation_bias": "middle"`. Assert valid UTF-8, contains
both a fragment from the start AND a fragment near the end, plus the
`"... N bytes omitted ..."` gap marker.

These three tests together kill all 10 boundary-logic survivors.

### Survivors 6–7: `cap_and_tee` Middle arithmetic
(`cap_and_tee.rs` lines 127, 130: `cap / 2` and `total_bytes - tail_len`)

Killed by **Test C** above. The additional assertion — that BOTH head and tail
content appear — distinguishes `cap / 2` (correct) from `cap * 2` (window so
large that truncation may not fire, or head and tail overlap).

### Survivors 8–9: `crlf_normalize` bounds check
(`output_cleaner.rs` line 70: `i + 1 < data.len()` → `<= ` and `+ → *`)

**Root cause:** The `< → <=` mutation causes an out-of-bounds read when `\r`
is the last byte of the buffer. The `+ → *` mutation would check `data[0]`
instead of `data[i+1]` for every `\r`, silently mis-detecting lone CRs as CRLF.

**Test D — lone CR as last byte:**
```
command: "printf 'foo\\r'"
```
Assert: no error (no panic), result contains `foo`, no spurious newline
conversion.

**Test E — CRLF sequence at end:**
```
command: "printf 'line\\r\\n'"
```
Assert: result contains `line` followed by `\n` (not `\r\n`).

These are already partially covered by migrated Phase A tests but the
survivors specifically need the lone-CR-at-end case.

### Survivors 10–11: `execute` truncation bias selection
(`run_command.rs` line 187: match guard `s.success()` → `true` / `false`)

**Root cause:** No existing test runs `execute_tool("run_command", ...)` without
`truncation_bias` in the input AND checks which bias was applied.

**Test F — exit-0 uses Head bias:**
```
command: "yes | head -n 20000"   # ~100 KB of 'y\n', exits 0
# no truncation_bias field
```
Assert: footer says "first" (Head).

**Test G — non-zero exit uses Tail bias:**
```
command: "yes | head -n 20000; exit 1"
# no truncation_bias field
```
Assert: footer says "last" (Tail), contains exit code notice.

---

## Verify

After both phases:

```
cargo test -p omega-tools          # all tests pass
cargo mutants -p omega-tools -j1   # target: 0 survivors
```
