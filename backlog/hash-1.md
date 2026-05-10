# HASH-1 — Deterministic content-derived `ContextHash`

**Status:** open. Hard cutover; no old-session compatibility. Prerequisite for SCHEMA-8.

## Why

`ContextHash` is currently a 6-byte random ID generated at
`ContextStore::append()` time. Despite the name, it has nothing to do
with the content of the record. The original rationale was perf — avoid
hashing each record's bytes — but the cost of `sha256` over a few KB of
JSON is microseconds, dwarfed by literally everything else the agent
does. The optimisation was misguided.

Going content-derived recovers properties the random ID quietly threw
away:

| Property | Random ID (today) | Content hash (after HASH-1) |
|---|---|---|
| Replay determinism | ✗ | ✓ |
| Hash verifies content on read | ✗ | ✓ |
| Same content debug-spot-checkable | ✗ | ✓ |
| Byte-equal goldens for `context.jsonl` | ✗ | ✓ |
| Cross-session dedup possible (future) | ✗ | ✓ |

The byte-equal-goldens property is what makes HASH-1 a useful prerequisite
for SCHEMA-8: the SCHEMA-8 refactor's defensive harness becomes a strict
file-bytes comparison rather than a `(role, content)` projection.

## Design

### Hash function

```rust
pub fn content_hash(role: &Role, content: &[ContentBlock]) -> ContextHash {
    let canonical = serde_json::to_vec(&(role, content))
        .expect("Role and ContentBlock are infallible to serialise");
    let digest = sha2::Sha256::digest(&canonical);
    let prefix = &digest[..8];                    // 8 bytes = 64 bits
    let hex = hex_lower(prefix);                  // 16 lowercase hex chars
    ContextHash::from_validated(hex)
}
```

- Input: only `role` and `content` — *not* `time`, *not* the hash itself.
  The hash must be stable across replays and machines, so wall-clock
  and recursive inputs are excluded.
- Canonical form: `serde_json::to_vec` of the tuple `(role, content)`.
  This relies on serde's documented behaviour of serialising struct
  fields in declaration order. Documented as part of the hash ABI: any
  change to `Role` or `ContentBlock` field order, variant order, or
  `serde` rename attributes is a breaking change.
- Truncation: take the first 8 bytes of the sha256 digest. Birthday-bound
  collision probability ~50% at 4 billion records — comfortably above any
  realistic Omega usage scale.
- Output: 16 lowercase hex chars.

### Length validation

`is_valid` and `hash_from_str` accept **only** length 16 lowercase hex.
The previous length-12 path is removed entirely. No old-session
compatibility.

### Collision handling

Two `ContextStore::append()` calls with identical `(role, content)`
produce the same hash. This is fine semantically — both references point
to "the message that said X" — but breaks the assumption "each append
returns a new unique key". Audit before merging:

- `LlmCallEvent.context_hashes`: `Vec<ContextHash>`. Duplicates are
  meaningful ("we sent these N records, in this order, and these two
  happen to be identical"). No code change needed.
- `ToolCallEvent.context_hash`: foreign key to the assistant message
  containing the tool_use. Tool_use blocks always have unique `id`s, so
  any assistant message containing one is structurally unique. No
  ambiguity in practice.
- Loading by hash: any consumer that does `find_record_by_hash` returns
  the first match. Two records with the same hash are by definition
  identical, so this is correct.

The "ContextStore returns a unique handle for each append" invariant
becomes "ContextStore returns the content hash; two appends with
identical content return the same handle". Document.

### Naming

`ContextHash` becomes accurate — it is a real hash now. Keep the type
name. Update the doc-comment to reflect the new semantics.

`random_hash()` is removed if no remaining call sites, or kept with a
warning if used elsewhere (audit Phase 1).

## Implementation phases

### Phase 1 — Hash function and tests (no behavioural change yet)

**File: `rust/crates/omega-store/src/context_hash.rs`**

1. Add `pub fn content_hash(role: &Role, content: &[ContentBlock]) -> ContextHash`.
2. Internal helper `from_validated(s: String) -> ContextHash` — assumes
   the string was produced by our own hashing path; private.
3. Keep `random_hash` for now (Phase 1 is purely additive).
4. Keep `is_valid` accepting length 12 for now (Phase 2 narrows it).

Tests in the same module — see Testing Concept below.

### Phase 2 — Switch `ContextStore::append`, narrow validation

**File: `rust/crates/omega-store/src/context_store.rs`**

5. `ContextStore::append`: replace `random_hash()` with
   `content_hash(&role, &content)`.
6. `ContextStore::build_record`: same change.

**File: `rust/crates/omega-store/src/context_hash.rs`**

7. `is_valid`: require length 16 only.
8. `hash_from_str`: error on length 12 input (now invalid).
9. Update existing tests (e.g. `random_hash_is_12_hex_chars` → `_is_16_`).

### Phase 3 — Remove `random_hash`

10. Audit workspace for `random_hash` callers. Expected callers: only
    tests creating synthetic hashes.
11. Replace synthetic-hash test usages with either `content_hash(...)`
    over the test's content, or a hard-coded 16-char hex literal where
    the test only needs *a* valid hash.
12. Delete `random_hash`. The `rand` dependency on `omega-store` may
    become removable; verify and clean up.

### Phase 4 — Read-side integrity (optional, recommended)

13. Add `ContextStore::verify_record(record: &ContextRecord) -> bool`
    that recomputes `content_hash(&record.role, &record.content)` and
    compares against `record.hash`. Returns `false` on mismatch.
14. Use this in a debug-only `cargo test` helper that scans a session
    directory and asserts every record verifies. Wire into integration
    tests for ContextStore.
15. Do **not** make this a hard runtime check on every read — perf is
    fine but the failure mode (panic on a bit-flipped file) is too
    hostile for production. Keep it test-only.

### Phase 5 — Workspace fixture/test sweep

16. Search for hard-coded 12-char hex hashes in tests, fixtures,
    snapshots:
    ```
    rg '"[0-9a-f]{12}"' --type rust
    ```
17. For each match, decide:
    - Is the test asserting on a *specific* hash that came from the old
      random path? → recompute via `content_hash` over the test's
      content, replace the literal.
    - Is it just *any* valid hash for shape testing? → replace with a
      16-char placeholder like `"0000000000000000"` and update validators.
18. Update mock-server fixtures, leptos SSR snapshots, e2e expectations.
19. Run the full workspace test suite. Should be green.

### Phase 6 — Mutation testing on `omega-store`

Mutation testing is the final validation that the test suite for HASH-1
actually catches bugs, not just exercises code. `cargo-mutants` is
already established in this workspace (see `rust/PHASE-1d.0-NOTES.md`
for the `omega-agent` precedent). HASH-1 is a particularly good
candidate because the surface area is small (one hash function, one
store module) and the failure modes are subtle (off-by-one in the
byte-prefix slice, swapped arguments, wrong digest variant).

**Steps:**

20. After Phase 5 is green, run:
    ```
    cd rust && cargo mutants -p omega-store --timeout 60
    ```
21. Triage every survivor. For each one, decide:
    - **Real gap**: write a test that catches it, re-run.
    - **Acceptable miss**: documented in a notes file with explicit
      justification (e.g., a timestamp helper whose exact format is
      not part of the contract). Acceptable misses must be the
      exception, not the rule.
22. Aim for **zero unjustified survivors** in `context_hash.rs` and
    `context_store.rs`. Mutations elsewhere in `omega-store` (e.g.,
    in `event_store.rs` or `session_dir.rs`) are out of scope for
    HASH-1 — note them but do not chase them as part of this item.
23. Record results in a `rust/HASH-1-MUTANTS.md` notes file analogous
    to `PHASE-1d.0-NOTES.md`: total mutants, caught, unviable, missed,
    plus per-miss justification.

**What good coverage looks like:**

For the lockdown tests, mutations like "replace `&digest[..8]` with
`&digest[..7]`" should fail every lockdown test (different bytes
produce different prefixes). Mutations like "replace `Sha256::digest`
with `Sha512::digest`" should likewise blow up every lockdown.

For the determinism tests, mutations like "replace `serde_json::to_vec`
with a function that re-serializes with a salt" would survive T-DET
(determinism is per-call) but be caught by T-LOCK (the locked-in value
depends on a salt-free serialisation). T-LOCK is the canary; T-DET is
the baseline.

For `ContextStore::append`, mutations like "swap role and content in
the `content_hash` call" must fail T-LOCK or T-RT. If they survive,
the lockdown set is too narrow — add a fixture where role and content
combined would produce a different hash than swapped role and content.

## Testing Concept

Test rigor is the heart of this change. We are committing to a hash
function for the lifetime of every future session. A bug in the canonical
form silently invalidates session chains; a poorly-scoped lockdown test
fails to catch ABI drift. The test suite must exercise:

### T-DET — Determinism

```rust
#[test]
fn content_hash_is_deterministic() {
    let role = Role::Assistant;
    let content = vec![ContentBlock::Text { text: "hello".into() }];
    assert_eq!(content_hash(&role, &content), content_hash(&role, &content));
}
```

Multiple calls with the same input produce the same hash. Trivial but
foundational.

### T-DIST — Distinctness across meaningful changes

One test per dimension:

- Different `text` → different hash.
- Different `Role` → different hash.
- Different block kinds (`Text` vs `Thinking`) with same text → different hash.
- Different block order (`[A, B]` vs `[B, A]`) → different hash.
- Different signature on a `Thinking` block → different hash.
- Same signature, different thinking text → different hash.
- Different `tool_use_id` → different hash.
- Empty `content` for different `Role`s → different hashes.

These tests pin down "the hash is sensitive to every meaningful field of
`(role, content)`". A future refactor that accidentally collapses two
fields would break exactly the right test.

### T-SHAPE — Output shape

```rust
#[test]
fn content_hash_is_16_lower_hex() {
    let h = content_hash(&Role::User, &[]);
    assert_eq!(h.as_ref().len(), 16);
    assert!(h.as_ref().bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')));
}
```

### T-LOCK — Lockdown values (the canary)

This is the most important class. For each of a small set of curated
fixtures, hard-code the expected hash as a string literal in the test.
If the canonical form drifts (e.g., someone reorders fields in
`ContentBlock`, adds a field with `#[serde(default)]`, renames a
variant), these tests scream.

Curated fixtures (each with the hash recomputed once during HASH-1 and
locked):

- `LOCK-1`: `Role::User`, `[Text { text: "hello" }]`.
- `LOCK-2`: `Role::Assistant`, `[Text { text: "ok" }]`.
- `LOCK-3`: `Role::Assistant`, `[Thinking { thinking: "let me see", signature: Some("sig-abc") }]`.
- `LOCK-4`: `Role::Assistant`, `[Thinking { thinking: "let me see", signature: None }]`.
- `LOCK-5`: `Role::Assistant`, `[Thinking{...}, Text{...}, ToolUse{id:"tu_1",name:"read_file",input:{...}}]` — full multi-block.
- `LOCK-6`: `Role::User`, `[ToolResult { tool_use_id: "tu_1", content: "result body", is_error: false }]`.
- `LOCK-7`: empty content vector with `Role::User`.

Each test:

```rust
#[test]
fn lockdown_user_hello() {
    let role = Role::User;
    let content = vec![ContentBlock::Text { text: "hello".into() }];
    let h = content_hash(&role, &content);
    // Locked HASH-1, 2024-01-XX. If this test fails, the canonical hash
    // form has drifted — any session referencing the old hash will
    // fail integrity checks. STOP and discuss before updating this
    // string. Bumping a lockdown value is a session-invalidation event.
    assert_eq!(h.as_ref(), "<insert computed value during impl>");
}
```

The implementation agent computes each lockdown value during Phase 1
and inserts it into the test. The user reviews the resulting commit but
cannot independently verify the values without running the code.
Mitigation: the commit message lists each lockdown's `(role, content)`
in plain English so the reviewer can spot-check the *fixture*, even if
the hash itself is opaque.

### T-PAIRWISE — Pairwise distinctness over generated samples

```rust
#[test]
fn content_hashes_distinct_across_synthetic_samples() {
    let mut seen = std::collections::HashSet::new();
    for spec in synthetic_record_specs(1000) {
        let h = content_hash(&spec.role, &spec.content);
        assert!(seen.insert(h.clone()), "collision on spec: {spec:?}");
    }
}
```

`synthetic_record_specs` is a small generator that varies role, block
kinds, text content, signatures, and tool inputs. 1000 samples gives a
collision probability of ~10⁻¹⁴ at 64-bit truncation — passing this test
is essentially a proof that the truncation didn't introduce a naive
prefix-collision pattern.

### T-RT — Round-trip via ContextStore

```rust
#[tokio::test]
async fn append_then_verify_roundtrip() {
    let dir = tempdir().unwrap();
    let store = ContextStore::new(dir.path().join("context.jsonl"));

    let role = Role::Assistant;
    let content = vec![ContentBlock::Text { text: "ok".into() }];
    let returned_hash = store.append(role.clone(), content.clone()).await.unwrap();

    let recomputed = content_hash(&role, &content);
    assert_eq!(returned_hash, recomputed);

    // Read back from disk and verify too
    let line = std::fs::read_to_string(dir.path().join("context.jsonl")).unwrap();
    let record: ContextRecord = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(record.hash, recomputed);
    assert_eq!(content_hash(&record.role, &record.content), record.hash);
}
```

This proves the hash on disk equals the hash computed from the
deserialised on-disk content. If a future change introduces a
serialisation asymmetry (e.g., adds a `#[serde(skip_serializing_if)]`
that omits a field on write but defaults it on read), this test catches
it.

### T-CONT — Same content, same hash (collision-by-design)

```rust
#[tokio::test]
async fn duplicate_content_yields_same_hash() {
    let store = ContextStore::new(tmpfile());
    let role = Role::User;
    let content = vec![ContentBlock::Text { text: "hi".into() }];
    let h1 = store.append(role.clone(), content.clone()).await.unwrap();
    let h2 = store.append(role.clone(), content.clone()).await.unwrap();
    assert_eq!(h1, h2);
}
```

Documents the intentional behaviour: identical `(role, content)` →
identical hash, even across appends. Forces any future code that
assumes uniqueness-per-append to grow a different idiom.

### T-LEN — Reject 12-char hashes (post-Phase-2)

```rust
#[test]
fn hash_from_str_rejects_12_char() {
    assert!(hash_from_str("0123456789ab").is_err());
}
```

Ensures the legacy length is unambiguously rejected.

### T-INT — Read-side integrity (Phase 4)

```rust
#[tokio::test]
async fn verify_record_detects_tampering() {
    let dir = tempdir().unwrap();
    let store = ContextStore::new(dir.path().join("context.jsonl"));

    store.append(Role::User, vec![ContentBlock::Text { text: "a".into() }]).await.unwrap();

    // Tamper: replace "a" with "b" but leave the original hash in place.
    let path = dir.path().join("context.jsonl");
    let raw = std::fs::read_to_string(&path).unwrap();
    let tampered = raw.replace("\"a\"", "\"b\"");
    std::fs::write(&path, tampered).unwrap();

    let records = store.load_all().await.unwrap();
    assert_eq!(records.len(), 1);
    assert!(!ContextStore::verify_record(&records[0]));
}
```

Detects content tampering when the hash is left untouched.

### Test placement

- T-DET, T-DIST, T-SHAPE, T-LOCK, T-PAIRWISE, T-LEN: in
  `omega-store/src/context_hash.rs` `#[cfg(test)] mod tests`.
- T-RT, T-CONT, T-INT: in `omega-store/src/context_store.rs`
  `#[cfg(test)] mod tests`.
- Optionally also a workspace-level integration test that round-trips
  through `ContextStore` and verifies via the `verify_record` API
  across a multi-record session.

## Acceptance criteria

- All HASH-1 tests pass: T-DET, T-DIST, T-SHAPE, T-LOCK, T-PAIRWISE,
  T-RT, T-CONT, T-LEN, T-INT.
- `cargo mutants -p omega-store` reports zero unjustified survivors
  in `context_hash.rs` and `context_store.rs`. Results recorded in
  `rust/HASH-1-MUTANTS.md`.
- All workspace tests pass after Phase 5 sweep.
- `random_hash` is deleted from the codebase (or, if a non-context use
  is found, retained with explicit justification).
- `ContextStore::append` returns a deterministic hash given fixed
  `(role, content)`, verified by `T-RT` and `T-CONT`.
- No 12-char hex hashes appear in tests, snapshots, fixtures, or
  documented examples.
- The `notes on lockdown values` paragraph in the commit message
  spells out each lockdown's `(role, content)` in English so a reviewer
  can spot-check fixtures without running the code.

## Out of scope (deferred to later items)

- Cross-session deduplication using the new content-addressed hashes.
  Possible future cache: "have we already summarised this assistant
  message?" — keyed by hash.
- Switching to a non-JSON canonical form (bincode, postcard) for
  stricter byte-stability guarantees. The serde-json declaration-order
  guarantee is sufficient for now.
- Lengthening beyond 16 hex chars. Birthday safety at 16 hex is
  comfortable for any plausible scale. Revisit only if a concrete
  collision-sensitive feature lands.

## Relationship to SCHEMA-8

HASH-1 ships first. SCHEMA-8 then builds on deterministic hashes:

- SCHEMA-8 Phase 0 golden tests revert to **byte-equal comparison** on
  `context.jsonl` files (no `(role, content)` projection needed).
- SCHEMA-8's `ContextStore`-touching code is unchanged behaviourally;
  the new event grammar produces the same `(role, content)` for the
  same fixture, hence the same hash, hence byte-identical files.
- The lockdown values from HASH-1 keep working through SCHEMA-8 unless
  SCHEMA-8 changes `Role` or `ContentBlock` definitions — which it does
  not.
