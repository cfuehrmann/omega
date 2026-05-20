# Mutation Testing

**Tool:** cargo-mutants 26.0.0 · **Flags:** `-j1 --no-shuffle`  
**Last sweep:** 2026-05-20 · All crates at 100% kill rate · No survivors.
`omega-store` re-run 2026-05-19 after e2e test migration (91 mutants, was 65).

## Excluded crates

| Crate | Reason |
|-------|--------|
| `omega-test-fixtures` | Test infrastructure — no production callers; kills would be circular |
| `omega-mock-server` | Playwright fixture binary — not shipped to users |
| `omega-e2e` | Browser tests — requires live Chromium; excluded via `.cargo/mutants.toml` |

---

## Summary

| Crate | Mutants | Caught | Missed | Timeout | Unviable | Kill rate |
|-------|---------|--------|--------|---------|----------|-----------|
| `omega-types` | 5 | 4 | 0 | 0 | 0 | 100% ✅ |
| `omega-cli` | 20 | 13 | 0 | 0 | 7 | 100% ✅ |
| `omega-store` | 91 | 61 | 0 | 5 | 25 | 100% ✅ |
| `omega-core` | 108 | 65 | 0 | 2 | 41 | 100% ✅ |
| `omega-server` | 112 | 25 | 0 | 17 | 71 | 100% ✅ |
| `omega-agent` | 172 | 64 | 0 | 0 | 108 | 100% ✅ |
| `omega-tools` | 267 | 145 | 0 | 5 | 117 | 100% ✅ |
| **Total** | **775** | **377** | **0** | **29** | **369** | **100% ✅** |

> `omega-server` and `omega-types` were re-run together on 2026-05-19 (117 combined); the per-crate split above is estimated from that combined result.  
> `omega-store` was re-run on 2026-05-19 after migrating `strip_jsonc_comments` inline tests to e2e integration tests.

---

## Timeout mutants (not survivors — caught by hang-detection, but slow)

These timed out rather than being caught cleanly. Worth noting in case they become flaky.

| Crate | Location | Mutation |
|-------|----------|----------|
| `omega-store` | `session_dir.rs:216:19` | `strip_jsonc_comments` — `-=` |
| `omega-store` | `session_dir.rs:216:19` | `strip_jsonc_comments` — `*=` |
| `omega-store` | `session_dir.rs:224:19` | `strip_jsonc_comments` — `*=` |
| `omega-store` | `session_dir.rs:238:15` | `strip_jsonc_comments` — `-=` |
| `omega-store` | `session_dir.rs:238:15` | `strip_jsonc_comments` — `*=` |
| `omega-core` | `retry.rs:134:46` | `retry_loop` — `*` |
| `omega-core` | `retry.rs:135:40` | `retry_loop` — `&&` |
| `omega-tools` | `output_cleaner.rs:72:15` | `crlf_normalize` — `-=` |
| `omega-tools` | `output_cleaner.rs:75:15` | `crlf_normalize` — `*=` |
| `omega-tools` | `cap_and_tee.rs:201:15` | `utf8_boundary_backward` — `*=` |
| `omega-tools` | `tools/edit_file.rs:113:15` | `count_occurrences` — `*=` |
| `omega-tools` | `tools/read_file.rs:67:13` | `char_boundary_at_or_before` — `/=` |

---

## Inline-test e2e audit

Tracks which crates have had their `#[cfg(test)]` blocks audited for e2e
quality (private-function tests migrated to integration tests, duplicates
removed).

| Crate | Status | Session | Notes |
|-------|--------|---------|-------|
| `omega-store` | ✅ done | Session 3 — 2026-05-19 | `strip_jsonc_comments` tests migrated; 6 duplicate `hash_from_str` tests removed |
| `omega-agent` | ✅ done | Session 2 — 2026-05-19 | All 7 inline blocks reviewed; justified carve-outs retained |
| `omega-types` | ✅ done | Session 1 — 2026-05-16 | No inline tests exist |
| `omega-cli` | ✅ done | Session 2 — 2026-05-20 | 3 private `git_has_pending_changes` tests migrated to integration tests; 3 new tests added (`clean_repo_not_dirty`, `allow_dirty` flag, `OMEGA_ALLOW_DIRTY` env) |
| `omega-server` | ✅ done | Session 3 — 2026-05-19 | No inline tests exist |
| `omega-core` | ⬜ pending | — | Inline blocks not yet reviewed |
| `omega-tools` | ⬜ pending | — | Inline blocks not yet reviewed |

---

## `#[mutants::skip]` annotations

8 annotations in the codebase. All reviewed; rationale is co-located with each annotation in source.

---

## `exclude_re` in `.cargo/mutants.toml`

| Pattern | Rationale |
|---------|-----------|
| `Message::Close` match arm in `handle_socket` | Dropping the `break` makes the next `reader.next()` return `None`, exiting the loop identically — documented equivalent mutant |
