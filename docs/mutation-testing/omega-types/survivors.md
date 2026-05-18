# omega-types — 1 surviving mutant

**Session 3 target** (alongside omega-server; trivial, add it first).

Verify kill with: `cargo mutants -p omega-types -j1`

---

## `events.rs` — 1 survivor

### `OmegaEvent::time` (line 510) — 1 mutant

```rust
pub fn time(&self) -> &ISOTimestamp {
    match self {                               // mutant: entire body →
        Self::SessionStarted(e) => &e.time,   // Box::leak(Box::new(Default::default()))
        Self::ServerStarted(e) => &e.time,
        // ... all variants
    }
}
```

**What's missing:** `.time()` is never called in any test; the method's
return value is never asserted.  
**Fix:** one parameterised test (or a test per variant) that constructs an
`OmegaEvent`, calls `.time()`, and asserts the returned timestamp equals the
one embedded in the event. Two or three variants are enough — the exhaustive
match means the compiler catches missing arms, so covering any real variant
is sufficient to kill the mutant.

Example sketch:
```rust
let ts = ISOTimestamp::from("2025-01-01T00:00:00Z");
let event = OmegaEvent::SessionStarted(SessionStartedEvent {
    time: ts.clone(), ..Default::default()
});
assert_eq!(event.time(), &ts);
```
