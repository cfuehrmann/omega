# omega-agent — survivors resolved ✅

**Status:** 0 survivors — 100% kill rate (64 caught, 108 unviable, 172 total).  
**Completed:** 2026-05-19 (Session 2).

---

## What was done

### Session 2 — inline test audit + 7 survivors killed

#### STEP 1 — Inline test block audit

All 7 inline `#[cfg(test)]` blocks were reviewed and given a
`//! Justification for carve-out:` comment explaining why each stays inline
rather than being migrated to `MockProvider` integration tests.

| Block | Decision | Reason |
|---|---|---|
| `elide_request_tests` (agent.rs) | Keep | Private pure function; output not assertable via MockProvider |
| `abandonment_closer_tests` (agent.rs) | Keep | Private; per-slot emission decisions not e2e-visible |
| `session_resume.rs` tests | Keep | Pure helpers over event slices; not reachable via Agent surface |
| `system_prompt.rs` tests | Keep | Public pure functions but simpler inline |
| `controls.rs` tests | Keep | Requires `lock_state()` (`pub(crate)`) — inaccessible from integration tests |
| `error_classify.rs` tests | Keep | Pattern-matches `LlmError` variants not constructible via MockProvider |
| `config.rs` tests | Keep | Pure constant lookups by model-name string |

#### STEP 2 — Survivors killed

**1. `gen_call_id` (agent.rs)**

Added `gen_call_id_tests` module:
- `gen_call_id_returns_8_hex_chars`: asserts `result.len() == 8` and all chars are
  ASCII hex digits.
- `gen_call_id_successive_calls_differ`: two calls produce different values.

**2. `LlmResponseEnded` in-loop guard (session_resume.rs)**

Added to the existing inline block:
- `project_turn_llm_response_ended_without_text_no_agent_line`
- `project_turn_llm_response_ended_with_text_emits_agent_line`
- `project_turn_two_llm_responses_produce_separate_agent_lines` — the critical
  test: a multi-round turn must emit two *separate* Agent lines.  The
  `delete match arm` mutant is killed here because without the arm,
  `pending_text` is never cleared between rounds and the chunks concatenate.

**3. Post-loop flush guard (session_resume.rs)**

Added:
- `project_turn_interrupted_text_appears_via_flush`
- `project_turn_empty_sequence_no_blank_agent_line`

**4. `global_agents_md_path` (system_prompt.rs)**

Added `global_agents_md_path_is_some_in_real_env`: calls the real function
(not the `_from_env` variant) and asserts `is_some()`.  `$HOME` is always
set in CI.

#### STEP 3 — `#[mutants::skip]` annotations reviewed

All 4 annotations kept as genuine equivalent mutants:

| Location | Function | Why equivalent |
|---|---|---|
| `controls.rs` | `pending_continue_ready` | `-> true` skips the wait; WS tests can't tell "skipped" from "woken" |
| `controls.rs` | `exit_suspend` | `TurnGuard` restores the flag on Drop; single-pause tests see identical output |
| `controls.rs` | `now_iso()` | Timestamps are universally redacted in snapshot assertions |
| `session_resume.rs` | `slice_start_after` | `i + 1 → i * 1`: `session_resumed` events are transparent to `group_into_turns` |
