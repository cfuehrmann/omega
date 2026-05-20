# omega-store — mutation testing audit ✅

**Status:** 0 survivors — 100% kill rate (61 caught, 5 timeout, 25 unviable, 91 total).  
**Completed:** 2026-05-19 (Session 3 — e2e inline-test audit).

---

## What was done

### STEP 1 — Inline test block audit ✅

Two `#[cfg(test)]` blocks were audited.

#### `src/session_dir.rs`

| Tests | Decision | Reason |
|---|---|---|
| `session_dir_name_matches_re` | Keep inline | Direct test of public pure function; not duplicated in integration suite |
| `session_dir_re_matches_legacy_formats` | Keep inline | Direct regex coverage; not duplicated in integration suite |
| `strip_jsonc_*` (10 tests) | **Migrate** | Private function; all scenarios expressible via `read_session_metadata` + real files |

All 10 `strip_jsonc_comments` tests were deleted from the inline block and
re-expressed as 8 async tokio tests in `tests/session_dir.rs` that write
`session.jsonc` files and call `read_session_metadata`.  The 2 basic cases
(`strip_jsonc_single_line_comment`, `strip_jsonc_block_comment`) were already
covered by existing integration tests and needed no new test.

New integration tests added:

| Test | Mutant targeted |
|---|---|
| `lone_slash_at_end_of_file_does_not_corrupt_output` | outer `i + 1 < len` guard (`< → <=`) |
| `single_line_comment_at_position_zero_is_stripped` | `i += 2` skip (`+= → -=`, underflows usize 0) |
| `single_line_comment_no_trailing_newline_is_stripped` | inner `while i < len` (`< → <=`) |
| `single_line_comment_past_midpoint_is_stripped` | `i += 2` skip (`+= → *=`) |
| `block_comment_at_position_zero_is_stripped` | block `i += 2` (`+= → -=`, underflows usize 0) |
| `block_comment_past_midpoint_is_stripped` | block `i += 2` (`+= → *=`; short comment `/* */` so `2p > len`) |
| `block_comment_star_not_followed_by_slash_is_stripped` | inner `&&` (`&& → \|\|`) |
| `unclosed_block_comment_at_eof_does_not_panic` | inner block loop bounds (`< → <=`) |

#### `src/context_hash.rs`

| Tests | Decision | Reason |
|---|---|---|
| `hash_from_str_rejects_12_char` | **Delete** | Duplicate of `hash_from_str_rejects_legacy_12_char` in `tests/context_store.rs` |
| `hash_from_str_accepts_valid_16` | **Delete** | Duplicate of `hash_from_str_accepts_valid_16_hex` |
| `hash_from_str_rejects_uppercase` | **Delete** | Duplicate of `hash_from_str_rejects_uppercase_letters` |
| `hash_from_str_rejects_short` | **Delete** | Duplicate of `hash_from_str_rejects_too_short` |
| `hash_from_str_rejects_long` | **Delete** | Duplicate of `hash_from_str_rejects_too_long` |
| `hash_from_str_rejects_non_hex` | **Delete** | Duplicate of `hash_from_str_rejects_non_hex_chars` |
| `into_string_works` | Keep inline | Not duplicated in integration suite |
| `content_hash_is_16_lower_hex` | Keep inline | Pure function property; not duplicated |
| `content_hash_is_deterministic` | Keep inline | Determinism spec at function level |
| `dist_*` / `pairwise` tests | Keep inline | Distinctness properties; not duplicated |
| LOCK-1 through LOCK-7 | Keep inline | Pin exact hash values; must be precise |

### STEP 2 — No `#[mutants::skip]` annotations ✅

```
grep -rn "mutants::skip" crates/omega-store/
```

Result: **zero results** — no annotations to review.

### STEP 3 — Measure and kill ✅

Final run after Step 1 changes:

| Mutants | Caught | Missed | Timeout | Unviable | Kill rate |
|---------|--------|--------|---------|----------|-----------|
| 91 | 61 | **0** | 5 | 25 | **100% ✅** |

One mutant that was missed in the pre-audit run (`session_dir.rs:222:15
replace += with *=` — the `i += 2` guard that skips over `/*`) was killed by
shortening the block comment in `block_comment_past_midpoint_is_stripped` to
`/* */` so that `2 × position > len`, making the mutant truncate the output.

#### Timeout mutants (caught by hang-detection)

| Location | Mutation | Root cause |
|---|---|---|
| `session_dir.rs:216:19` | `replace += with -=` | Single-line comment inner loop steps backward forever |
| `session_dir.rs:216:19` | `replace += with *=` | Inner loop spins on the same byte forever |
| `session_dir.rs:224:19` | `replace += with *=` | Block-comment outer `i += 2 → i *= 2` lands wrong; inner scan spins |
| `session_dir.rs:238:15` | `replace += with -=` | Character-advance loop reverses forever |
| `session_dir.rs:238:15` | `replace += with *=` | Character-advance loop spins forever |
