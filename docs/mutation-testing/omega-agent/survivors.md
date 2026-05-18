# omega-agent — 7 surviving mutants

**Session 2 target.** Survivors span three files.

Verify kills with: `cargo mutants -p omega-agent -j1`

---

## `agent.rs` — 1 survivor

### `gen_call_id` (line 2089) — 1 mutant

```rust
fn gen_call_id() -> String {
    let bytes: [u8; 4] = rand::random();
    bytes.iter().fold(String::with_capacity(8), |mut s, b| {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
        s
    })
}
```

**Mutant:** entire function body replaced with `"xyzzy".into()`.

**What's missing:** no test asserts structural properties of the returned ID.  
**Fix:** test that the result is exactly 8 characters, contains only `[0-9a-f]`,
and that two successive calls produce different values (probabilistic but
sufficient — collision probability is 1/2³² ≈ 0).

---

## `session_resume.rs` — 4 survivors

All four are in `project_turn`, which builds the plain-text summary of one
turn from a slice of `OmegaEvent`s.

### Guard `!pending_text.is_empty()` (line 226) — 3 mutants

```rust
OmegaEvent::LlmResponseEnded(_) if !pending_text.is_empty() => {
    //                              ^^^^^^^^^^^^^^^^^^^^^^^^^
    //  mutants: replace guard with `true`, `false`, drop `!`
    let joined = pending_text.join("");
```

**What's missing:** no test exercises the case where `LlmResponseEnded` arrives
with an *empty* `pending_text` (guard = false path). The `true`/`false`/`!`
mutations all survive because the existing tests always have text before `LlmResponseEnded`.  
**Fix:** one test where a `LlmResponseEnded` event arrives without any preceding
`TextBlock`; assert the output does **not** contain a spurious `"Agent:"` line.
A second test where it arrives *with* text; assert the `"Agent:"` line appears.

---

### Flush guard `!pending_text.is_empty()` (line 267) — 1 mutant

```rust
// Flush any text not followed by LlmResponseEnded (e.g. interrupted turns).
if !pending_text.is_empty() {
    //  mutant: drop `!`
```

**What's missing:** no test covers an interrupted turn where text accumulates
but `LlmResponseEnded` never fires.  
**Fix:** test a sequence ending with `TextBlock` events but no `LlmResponseEnded`;
assert the text is still included in the output (flush path taken).
Also test an empty sequence; assert the flush block does not emit a blank `"Agent:"` line.

---

## `system_prompt.rs` — 2 survivors

### `global_agents_md_path` (line 133) — 2 mutants

```rust
pub fn global_agents_md_path() -> Option<PathBuf> {
    global_agents_md_path_from_env(           // whole body → None
        std::env::var_os("XDG_CONFIG_HOME").as_deref(),  // or → Some(Default::default())
        std::env::var_os("HOME").as_deref(),
    )
}
```

**What's missing:** `global_agents_md_path` (the public wrapper) is never
called in tests — only the `_from_env` variant is tested directly.
Replacing the body with `None` or `Some(PathBuf::default())` is invisible
because no test calls the real function and checks what it returns.  
**Fix:** one test that calls `global_agents_md_path()` directly in an
environment where `HOME` is set (which it always is in CI) and asserts the
result is `Some(_)` containing a non-empty path.  The test does not need to
check the exact path — `is_some()` is sufficient to kill both mutants.

> **Note:** do not use `#[serial_test]` or mutate `HOME` — just assert the
> value is `Some` given the real environment. This is safe because `HOME` is
> always set in the CI environment.
