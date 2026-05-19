# omega-store — Mutation Testing Results

**Tool:** cargo-mutants 26.0.0  
**Flags:** `-p omega-store -j1`  
**Date:** 2026-05-19  

## Summary

| Mutants | Caught | Missed | Timeout | Unviable | Kill rate |
|---------|--------|--------|---------|----------|-----------|
| 91 | 61 | 0 | 5 | 25 | **100% ✅** |

## Survivors

**None.** Every live mutant was either caught by a failing test or timed out
(infinite-loop mutations caught by hang-detection).

## Timeout mutants (not survivors — caught by hang-detection)

These mutations replace a `+=` increment with `-=` or `*=` inside the
`strip_jsonc_comments` inner loops, causing infinite loops.  They are counted
as caught (by timeout) rather than missed.

| Location | Mutation | Root cause |
|----------|----------|------------|
| `session_dir.rs:216:19` | `replace += with -=` | Single-line comment inner loop skips backward forever |
| `session_dir.rs:216:19` | `replace += with *=` | Single-line comment inner loop spins on same byte forever |
| `session_dir.rs:224:19` | `replace += with *=` | Block comment outer `i += 2` → `i *= 2` skips to wrong position; inner scan then spins |
| `session_dir.rs:238:15` | `replace += with -=` | Character-advance loop skips backward forever |
| `session_dir.rs:238:15` | `replace += with *=` | Character-advance loop spins on same character forever |

## Audit notes

### Inline test blocks

`src/session_dir.rs` — the `strip_jsonc_comments` boundary tests were
migrated to `tests/session_dir.rs` as async tokio tests exercising
`read_session_metadata` with real files.  The two `make_session_dir_name` /
`session_dir_re` inline tests were retained (pure public functions, not
duplicated in the integration suite).

`src/context_hash.rs` — six `hash_from_str_*` tests that were verbatim
duplicates of tests in `tests/context_store.rs` were removed.  The lockdown
fixtures (LOCK-1 through LOCK-7), distinctness, and pairwise-collision tests
are kept inline because they test pure cryptographic properties at the
function level.

### `#[mutants::skip]` annotations

Zero annotations found in `crates/omega-store/`.
