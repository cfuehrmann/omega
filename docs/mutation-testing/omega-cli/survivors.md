# omega-cli — Mutation Testing Results

**Run date:** 2026-05-20  
**Tool:** cargo-mutants · **Flags:** `-p omega-cli -j1`  
**TMPDIR override:** `/home/carsten/mutants-tmp` (tmpfs space constraint)

## Summary

| Mutants | Caught | Missed | Timeout | Unviable | Kill rate |
|---------|--------|--------|---------|----------|-----------|
| 20 | 13 | 0 | 0 | 7 | **100% ✅** |

## Survivors

**None.** Every viable mutant was caught by the integration tests in
`tests/cli.rs`.

## Caught mutants (13)

| Location | Mutation | Killed by |
|----------|----------|-----------|
| `src/main.rs:110:18` | `replace match guard !k.trim().is_empty() with true in run` | `empty_api_key_exits_with_error` |
| `src/main.rs:110:18` | `replace match guard !k.trim().is_empty() with false in run` | `happy_path_single_text_turn` |
| `src/main.rs:110:18` | `delete ! in run` (API-key guard) | `empty_api_key_exits_with_error` |
| `src/main.rs:129:21` | `replace && with \|\| in run` (pending-changes gate) | `allow_dirty_flag_bypasses_pending_changes_gate` |
| `src/main.rs:129:8` | `delete ! in run` (`!allow_dirty`) | `dirty_tree_without_allow_dirty_exits_with_error` |
| `src/main.rs:183:13` | `delete field max_attempts from struct RetryConfig expression in run` | `retry_exhaustion_emits_agent_error_and_turn_interrupted` |
| `src/main.rs:241:21` | `delete match arm OmegaEvent::TurnEnd(te) in run` | `happy_path_single_text_turn` |
| `src/main.rs:252:21` | `delete match arm OmegaEvent::TurnInterrupted(ti) in run` | `retry_exhaustion_emits_agent_error_and_turn_interrupted` |
| `src/main.rs:262:21` | `delete match arm OmegaEvent::AgentError(ae) in run` | `retry_exhaustion_emits_agent_error_and_turn_interrupted` |
| `src/main.rs:265:21` | `delete match arm OmegaEvent::ToolCall(tc) in run` | `tool_use_then_text` |
| `src/main.rs:268:21` | `delete match arm OmegaEvent::ToolResult(tr) in run` | `tool_use_then_text` |
| `src/main.rs:276:21` | `delete match arm OmegaEvent::LlmCall(_) in run` | `happy_path_single_text_turn` |
| `src/main.rs:302:24` | `delete ! in git_has_pending_changes` (stdout non-empty check) | `clean_repo_not_dirty_proceeds_past_git_check` |

## Unviable mutants (7)

These mutations produce code that fails to compile and are therefore not
testable. They are not survivors.

| Location | Mutation | Why unviable |
|----------|----------|--------------|
| `src/main.rs:70:5` | `replace main with ()` | `main` must return `()` — replacing the body trivially satisfies the return type, but async fn wiring breaks |
| `src/main.rs:109:5` | `replace run -> i32 with 0` | `initial_backoff` variable becomes unused → `unused_variables` error |
| `src/main.rs:109:5` | `replace run -> i32 with 1` | same as above |
| `src/main.rs:109:5` | `replace run -> i32 with -1` | same as above |
| `src/main.rs:184:13` | `delete field initial_backoff from struct RetryConfig expression in run` | `initial_backoff` local variable becomes unused → compile error |
| `src/main.rs:295:5` | `replace git_has_pending_changes -> bool with true` | function body uses `std::process::Command`; replacing with a literal produces unused-import errors |
| `src/main.rs:295:5` | `replace git_has_pending_changes -> bool with false` | same as above |

## Notes

This run followed the inline-test migration session (2026-05-20) which
replaced the 3 private `#[cfg(test)]` unit tests of `git_has_pending_changes`
(`clean_repo_not_dirty`, `untracked_file_is_dirty`, `non_git_dir_not_dirty`)
with integration-level tests driving the real `omega` binary:

- `clean_repo_not_dirty_proceeds_past_git_check`
- `allow_dirty_flag_bypasses_pending_changes_gate`
- `omega_allow_dirty_env_bypasses_pending_changes_gate`

The mutation at line 302 (`delete !` inside `git_has_pending_changes`) is now
killed by the new `clean_repo_not_dirty_proceeds_past_git_check` test.
