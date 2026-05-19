# omega-types — 0 surviving mutants ✅

**Session 3** (alongside omega-server) — all survivors killed.

Final run: 117 mutants tested across both crates — 29 caught, 17 timeout, 71 unviable, **0 missed**.

---

## History (resolved)

### `OmegaEvent::time` — killed

Added two unit tests in `crates/omega-types/src/events.rs`:
- `SessionStarted` with a known timestamp → `.time()` returns that timestamp
- `TurnEnd` with a known timestamp → same assertion

Two variants suffice because the exhaustive match means the compiler enforces
all arms are present, and the mutant replaces the entire function body (not
per-arm).
