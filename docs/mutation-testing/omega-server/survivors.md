# omega-server — 3 surviving mutants

**Session 3 target** (alongside omega-types).

Verify kills with: `cargo mutants -p omega-server -j1`

---

## `router.rs` — 2 survivors

Both are the same mutation in symmetric functions.

### `handle_reset` (line 862) — 1 mutant

```rust
async fn handle_reset(...) -> Result<(), String> {
    if !allow_dirty {                          // mutant: drop `!`
        let cwd = std::env::current_dir()...;
        if git_has_pending_changes(&cwd) {
            let _ = tx.send(WsMessage::PendingChangesWarning {
                intent: PendingChangesIntent::Reset { model, effort },
            });
            return Ok(());
        }
    }
    // ... proceed with reset
```

**What's missing:** no test calls `handle_reset` with `allow_dirty = false`
in a dirty working tree and asserts that a `PendingChangesWarning` is sent
instead of proceeding. Dropping the `!` means the guard fires when the tree
is *clean* — but no test checks which WS messages are sent in the dirty case.  
**Fix:** a unit test that calls `handle_reset` with `allow_dirty = false` and
a fake dirty-tree state (or via `git_has_pending_changes` returning true);
assert the channel receives `WsMessage::PendingChangesWarning` and nothing else
(no `ResetDone`, no `History`, no `Ready`).

---

### `handle_resume_session` (line 924) — 1 mutant

Identical pattern to `handle_reset`:

```rust
async fn handle_resume_session(...) -> Result<(), String> {
    // ...
    if !allow_dirty {                          // mutant: drop `!`
        if git_has_pending_changes(&cwd) {
            let _ = tx.send(WsMessage::PendingChangesWarning {
                intent: PendingChangesIntent::ResumeSession { session_dir },
            });
            return Ok(());
        }
    }
```

**Fix:** same pattern — test `allow_dirty = false` with a dirty tree; assert
`PendingChangesWarning { intent: ResumeSession { .. } }` is sent and the
session is not replaced.

---

## `ws_message.rs` — 1 survivor

### `PendingChangesIntent::to_json` (line 110) — 1 mutant

```rust
impl PendingChangesIntent {
    fn to_json(&self) -> serde_json::Value {
        match self {                           // mutant: entire body → Default::default()
            Self::Reset { model, effort } => {
                // builds {"kind":"reset", "model":..., "effort":...}
            }
            Self::ResumeSession { session_dir } => {
                // builds {"kind":"resumeSession", "sessionDir":...}
            }
        }
    }
}
```

**What's missing:** `to_json` is only called when serialising a
`PendingChangesWarning` WS message; no test inspects the JSON payload
of that message.  
**Fix:** unit tests directly on `PendingChangesIntent`:
1. `Reset { model: Some("m"), effort: Some("e") }` → assert JSON has
   `"kind":"reset"`, `"model":"m"`, `"effort":"e"`.
2. `Reset { model: None, effort: None }` → assert `"kind":"reset"` present,
   no `"model"` or `"effort"` keys.
3. `ResumeSession { session_dir: "2025-01-01T00-00-00-000-abc" }` → assert
   `"kind":"resumeSession"`, `"sessionDir"` matches.

> `to_json` is `fn` (not `pub fn`), so tests must live in a `#[cfg(test)]`
> module inside `ws_message.rs` with `use super::*`.
