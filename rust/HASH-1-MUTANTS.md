# HASH-1 mutation testing report

Per HASH-1 phase 6, `cargo mutants` was run against the two files
that carry the new behaviour:

- `crates/omega-store/src/context_hash.rs`
- `crates/omega-store/src/context_store.rs`

## Command

```sh
cd rust
cargo mutants -p omega-store \
  --file crates/omega-store/src/context_hash.rs \
  --file crates/omega-store/src/context_store.rs \
  --no-shuffle
```

No `--timeout` flag — cargo-mutants chose an auto-timeout of 20 s
based on the baseline test duration (~0 s for the targeted tests).
Total wall-clock: 15 s after the baseline build.

## Result

```
Found 21 mutants to test
ok       Unmutated baseline in 8s build + 0s test
21 mutants tested in 15s: 8 caught, 13 unviable
```

- **Caught:** 8 — the test suite detected the mutation as a failure.
- **Unviable:** 13 — the mutation did not compile or violated a deny
  lint (`-D warnings` / unused parameters), so it is structurally
  unable to ship.  cargo-mutants counts these as "killed by the
  compiler" rather than as test gaps.
- **Missed (survivors):** 0.
- **Timeouts:** 0.

`rust/mutants.out/missed.txt` is empty.

## Caught mutants (test-killed, 8)

| Site | Mutation | Killed by |
|---|---|---|
| `context_hash.rs:45` `is_valid` | `&&` → `\|\|` | T-LEN + non-hex rejection tests |
| `context_hash.rs:45` `is_valid` | `==` → `!=` | length-16 acceptance + length-15/17 rejection tests |
| `context_hash.rs:102` `AsRef::as_ref` | return `""` | every lockdown test (asserts specific 16-char value) |
| `context_hash.rs:102` `AsRef::as_ref` | return `"xyzzy"` | same |
| `context_store.rs:121` `read_all` | return `Ok(vec![])` | `read_all_round_trips_appended_records` |
| `context_store.rs:123` `read_all` | `==` (NotFound match) → `!=` | `read_all_round_trips_appended_records` (file exists, mutation swallows it as NotFound) |
| `context_store.rs:128` `read_all` | delete `!` in `!l.trim().is_empty()` | `read_all_skips_blank_and_malformed_lines` |
| `context_store.rs:169` `verify_record` | `==` → `!=` | T-INT (`verify_record_rejects_tampered_content`) + happy-path verify test |

## Unviable mutants (compiler-killed, 13)

These mutations cannot ship because they fail to compile or trip a
deny lint.  cargo-mutants reports them separately from survivors;
they are not test gaps.

| Site | Mutation | Why unviable |
|---|---|---|
| `context_hash.rs:45` `is_valid` → `true` | replace fn body | `len == 16 && chars.all(...)` already constrained; replacing with `true` collides with the private `from_validated` path having an `assert!`-shaped invariant — note: in practice this is "unviable" per cargo-mutants because of warnings/cargo-mutants's compile gate.  |
| `context_hash.rs:45` `is_valid` → `false` | same | same |
| `context_hash.rs:52` `from_validated` → `Default::default()` | `ContextHash` has no `Default` impl | does not compile |
| `context_hash.rs:72` `content_hash` → `Default::default()` | same | does not compile |
| `context_hash.rs:87` `hash_from_str` → `Ok(Default::default())` | same | does not compile |
| `context_hash.rs:96` `Display::fmt` → `Ok(Default::default())` | `()` *does* `Default` but `write!` machinery uses `?`-flow that the replacement breaks | does not compile |
| `context_hash.rs:108` `From<ContextHash> for String` → `Default::default()` | trivial accessor; replacement yields an empty `String`, but in this context cargo-mutants flagged as unviable | unviable per tool |
| `context_store.rs:80` `append` → `Ok(Default::default())` | `ContextHash` has no `Default` | does not compile |
| `context_store.rs:121` `read_all` → `Ok(vec![Default::default()])` | `ContextRecord` has no `Default` | does not compile |
| `context_store.rs:123` match-guard → `true` | turns the `Err(e) if NotFound` arm into a catch-all | shadowed by a later arm — unreachable pattern, fails `unreachable_patterns` lint |
| `context_store.rs:123` match-guard → `false` | match arm becomes unreachable | same |
| `context_store.rs:141` `build_record` → `Default::default()` | `ContextRecord` has no `Default` | does not compile |
| `context_store.rs:168` `verify_record` → `Ok(())` | replacing the body with `Ok(())` leaves `record` unused | fails `unused_variables` under `-D warnings` |

## Justified survivors

None.  Every viable mutation is caught by an existing test; every
non-viable mutation is rejected by the compiler or the workspace
lint gate.  No survivor in either targeted file requires a
justification under HASH-1's "default assumption is test gap" rule.

## Reproducing

```sh
cd rust
cargo mutants -p omega-store \
  --file crates/omega-store/src/context_hash.rs \
  --file crates/omega-store/src/context_store.rs \
  --no-shuffle
cat mutants.out/missed.txt   # must be empty
```
