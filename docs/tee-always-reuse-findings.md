# Tee-always cache reuse — findings

**Question.** When `run_command`, `wait_for_output`, and `fetch_url` write their
*full* output to a session-cache log file (commit `e26817c`, 2026-05-15), does
the LLM ever follow up by reading that cache path? Is "tee always" worth the
disk I/O relative to "tee on truncate"?

**Method.** `scripts/analyze_tee_reuse.py` scans every session
`events.jsonl` since the rollout, finds tool results whose output footer
surfaced a real cache path (`[full output: …]` or `[truncated; … Full
output: …]`), and counts subsequent `tool_call` events in the same session
whose `input` (JSON-serialised) contains that path as a substring.

A path is "real" if it contains `/` and ends in `.log` — this rejects the
literal `<path>` placeholder that appears in the system prompt and tool
descriptions as documentation.

**Data.** 35 sessions, 2026-05-15 → 2026-05-17, 468 cache-emitting tool
results (the absolute majority `run_command`).

## Results

| status | tool | calls | reused calls | reuse % | follow-up calls |
|---|---|---:|---:|---:|---:|
| full       | run_command     | 448 | 3 | 0.7%  | 4 |
| full       | fetch_url       |   9 | 1 | 11.1% | 2 |
| full       | wait_for_output |   3 | 0 | 0.0%  | 0 |
| truncated  | run_command     |   5 | 0 | 0.0%  | 0 |
| truncated  | fetch_url       |   2 | 1 | 50.0% | 2 |
| truncated  | wait_for_output |   1 | 0 | 0.0%  | 0 |

Combined:

| status | calls | reused calls | reuse % | follow-up calls | avg follow-ups / call |
|---|---:|---:|---:|---:|---:|
| **full**       | **460** | **4** | **0.9%**  | **6** | **0.01** |
| **truncated**  |   **8** | **1** | **12.5%** | **2** | **0.25** |

Follow-up tools (which tool dipped into the cache):
- After **full** result: `run_command` ×5, `grep_files` ×1.
- After **truncated** result: `run_command` ×2.

## Interpretation

The whole point of "tee always" over "tee on truncate" is that even when the
LLM got the complete output inline it might still want to grep/read the
cache later (e.g. after context compaction, or to avoid re-running an
expensive command). The data does not support this.

- **Full-output reuse rate is 0.9% (4/460).** Effectively noise. The 5
  `run_command` and 1 `grep_files` follow-ups are scattered across 3
  sessions and could equally have been served by re-running.
- **Truncated reuse rate is 12.5% (1/8).** Low absolute volume but order
  of magnitude higher than full. This is where the cache actually earns
  its keep.
- The truncated sample is small (n=8) over a short observation window;
  the directional gap (full ≈ 0%, truncated > 10%) is nonetheless clear.

## Recommendation

Revert to **tee on truncate**. The full-output case (98% of all
tee-emitting tool calls) produces almost no follow-up reuse, so we are
paying disk I/O and surfacing extra footer noise to the LLM for ~1% of
calls. Keeping the cache for the truncated case preserves the only
regime where reuse actually happens.

If we want to keep tee-always for other reasons (post-hoc debugging by
humans, session export, etc.), document that motivation explicitly — the
LLM-reuse rationale alone does not justify it.

## Reproducing

```
python3 scripts/analyze_tee_reuse.py [--since YYYY-MM-DD] [--verbose]
```
