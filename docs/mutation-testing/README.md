# Mutation Testing

**Tool:** cargo-mutants 26.0.0 · **Flags:** `-j1 --no-shuffle`  
**Last sweep:** 2026-05-18 (≈ 20 min, 7 production crates)

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
| `omega-types` | 5 | 4 | **1** | 0 | 0 | 80% |
| `omega-cli` | 20 | 13 | 0 | 0 | 7 | 100% ✅ |
| `omega-store` | 65 | 39 | 0 | 1 | 25 | 98% ✅ |
| `omega-core` | 108 | 65 | 0 | 2 | 41 | 97% ✅ |
| `omega-server` | 110 | 36 | **3** | 0 | 71 | 92% |
| `omega-agent` | 172 | 64 | 0 | 0 | 108 | 100% ✅ |
| `omega-tools` | 267 | 145 | 0 | 5 | 117 | 100% ✅ |
| **Total** | **747** | **366** | **4** | **8** | **369** | **99%** |

Survivor details live in each crate's `survivors.md`. Crates marked ✅ have no survivors.

---

## Work plan

| Session | Crate(s) | Survivors | Status |
|---------|----------|-----------|--------|
| 1 | `omega-tools` | 16 | ✅ Done — 0 missed, 145 caught (267 total). Also fixed a latent `utf8_boundary_forward` bug discovered during migration. |
| 2 | `omega-agent` | 7 | ✅ Done — 0 missed, 64 caught (172 total). Inline test audit; 7 survivors killed; 4 `#[mutants::skip]` annotations confirmed equivalent. |
| 3 | `omega-server` + `omega-types` | 3 + 1 | Pending — details in `omega-server/survivors.md` |

---

## Timeout mutants (not survivors — already caught, but slow)

These timed out rather than being caught cleanly. Worth noting in case they become flaky.

| Crate | Location | Mutation |
|-------|----------|----------|
| `omega-store` | `session_dir.rs:216:19` | `strip_jsonc_comments` — `*=` |
| `omega-core` | `retry.rs:134:46` | `retry_loop` — `*` |
| `omega-core` | `retry.rs:135:40` | `retry_loop` — `&&` |
| `omega-tools` | `output_cleaner.rs:72:15` | `crlf_normalize` — `-=` |
| `omega-tools` | `output_cleaner.rs:75:15` | `crlf_normalize` — `*=` |
| `omega-tools` | `cap_and_tee.rs:201:15` | `utf8_boundary_backward` — `*=` |
| `omega-tools` | `tools/edit_file.rs:113:15` | `count_occurrences` — `*=` |
| `omega-tools` | `tools/read_file.rs:67:13` | `char_boundary_at_or_before` — `/=` |

---

## `#[mutants::skip]` annotations

8 annotations in the codebase. All reviewed; none require immediate action.

| Function | File | Verdict |
|----------|------|---------|
| (see source comment) | Rationale is co-located with each annotation | All ✅ or ⚠️ re-evaluate if surrounding code changes |

Full annotation review: run `python3 scripts/mutation-analysis.py --out-dir docs/mutation-testing --repo-root .` to regenerate the detailed report.

---

## `exclude_re` in `.cargo/mutants.toml`

| Pattern | Rationale |
|---------|-----------|
| `Message::Close` match arm in `handle_socket` | Dropping the `break` makes the next `reader.next()` return `None`, exiting the loop identically — documented equivalent mutant |
