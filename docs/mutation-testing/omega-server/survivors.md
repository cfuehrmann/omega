# omega-server — 0 surviving mutants ✅

**Session 3** (alongside omega-types) — all survivors killed.

Final run: 117 mutants tested — 29 caught, 17 timeout, 71 unviable, **0 missed**.

---

## History (resolved)

### `handle_reset` / `handle_resume_session` dirty-tree guards — killed

Added `AppState.cwd: PathBuf` (captured once at startup) so integration tests
can inject a real dirty git repo as the working directory. Two new WS tests in
`tests/ws.rs` cover the `allow_dirty = false` + dirty-tree path for both
handlers, asserting `PendingChangesWarning` is sent and no further progress
messages follow.

### `PendingChangesIntent::to_json` — killed

Added three unit tests directly in `src/ws_message.rs`:
- `Reset { model: Some, effort: Some }` → verifies kind, model, effort fields
- `Reset { model: None, effort: None }` → verifies kind, no model/effort keys
- `ResumeSession` → verifies kind and sessionDir
