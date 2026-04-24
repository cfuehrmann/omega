# Harbor Benchmarking

Running Omega against Terminal-Bench 2.0 (and, later, SWE-Bench Verified via
Harbor's registry) to produce apples-to-apples numbers against Claude Code,
Terminus-2, Mini-SWE-Agent, OpenHands on the same model.

**Entry point:** [Roadmap](#roadmap) — the first item is always the next thing
to do.

## Status

- **Model under evaluation:** `claude-sonnet-4-6`
- **Tasks scanned:** 10 of 76 oracle-passing TB 2.0 tasks (plus a smoke test,
  not counted here)
- **Pass rate:** 7 / 10 (70 %)
- **Results data:** `benchmark-results/results.jsonl`
- **Per-trial logs:** `jobs/<timestamp>/<task>/agent/{events,context}.jsonl`

| Result | Count | Tasks |
|---|---|---|
| Pass | 7 | prove-plus-comm, fix-git, log-summary-date-ranges, cobol-modernization, vulnerable-secret, regex-log, sqlite-db-truncate |
| Fail (agent) | 3 | overfull-hbox, extract-elf, largest-eigenval |

All three failures are variations of **missing goal-check** — see
[Known weaknesses](#known-weaknesses). n=3 is too thin to commit to a specific
intervention shape; the first roadmap item broadens the sample before we
prototype anything.

**Flakiness flag.** `crack-7z-hash` passed in the original oracle-era smoke
test (908 s) but hit `AgentTimeoutError` at 1800 s when re-run on 2026-04-24
to validate Phase 1 of the events-analysis plan. One data point — watch for
non-determinism when the full Phase A scan runs.

## Roadmap

Ordered. First item is the next thing to do.

### 1. Broaden the failure sample (Phase A) — **next**

**Scope of this session.** One batch of ~15 previously-unrun tasks from the
76 oracle-passing set. Ingest, post a short summary, stop. Do not start
item 2. Do not change Omega's agent behaviour.

**Why.** Avoid overfitting a single "goal-check" hypothesis to n=3. Phase A's
aggregate goal is ≥ 10 failure trials spanning ≥ 4 categories — may span
more than one batch, but one batch is the unit per session.

**How.**

1. **Dedup against already-run tasks.**
   ```bash
   jq -r '.task_name' benchmark-results/results.jsonl | sort -u
   ```
   Construct an explicit task list for harbor that excludes those — see
   `harbor run --help` for the flag shape.
2. **Run the scan.** See [Running benchmarks](#running-benchmarks) below —
   **harbor buffers all stdout until the run completes**. From an Omega
   session, use `run_background` + a single `wait_for_output` with pattern
   `"Results written to|Exception"` and `timeoutMs: 7200000` (2 h). Do NOT
   re-run or kill just because output is silent.
3. **Ingest and summarise.**
   ```bash
   bun scripts/bench-ingest.ts
   bun scripts/bench-summary.ts
   ```
4. **Post a short summary and stop.** Report: pass/fail split for this
   batch, whether any new failure shape doesn't match the three existing
   weakness patterns, any infrastructure issues handled. Then stop.

**Autonomy envelope.**

In scope if they arise:
- Retry a crashed harbor invocation.
- Fix an `omega_agent.py` infrastructure bug (missing apt dep, path glitch,
  timeout setting, etc.). Because the container installs Omega from
  `git clone --branch v0.1.0 --depth 1`, any fix must also be pushed and
  the `v0.1.0` tag re-pointed (`git tag -f v0.1.0 && git push --force origin v0.1.0`)
  so the next harbor run actually gets the fix.
- Re-run a single timed-out or crashed task.

Out of scope (stop and ask):
- Changing Omega's agent behaviour (system prompts, tools, loop logic).
- Starting item 2 or later roadmap items.
- Revising the weakness hypothesis.

**Budget.** ≈ $4–6 API spend, ≈ 1–2 h wall-clock. Trials now persist both
`events.jsonl` and `context.jsonl` (context.jsonl added to `omega_agent.py`
on 2026-04-24) so the full session is replayable and LLM-diagnosable.

### 2. LLM-driven diagnosis script — **blocked on design review**

Feed each failure's `events.jsonl` + `context.jsonl` to a separate Claude
call with a categorisation prompt. Output: failure category (goal-check /
rabbit-hole / convergence / model-layer / other) + evidence pointers.
Replaces manual trial browsing and scales to hundreds of trials.

**Blocked on:** Carsten to review the design before implementation. Do not
start this item without explicit go-ahead.

### 3. Categorise failures against hypothesis buckets (Phase B)

Run item 2 across all failure trials. If > 50 % of agent-layer failures
fall into one bucket, that is the first affordance to prototype. If the
distribution is flat, prioritise the broadest affordance.

### 4. Prototype one affordance, A/B test (Phase C)

Implement behind a feature flag. Re-run the *same* failed tasks, compare
before/after pass rate.

- **Success criterion:** ≥ 2 net passes on the held-out failures, zero
  regressions on the passing tasks.
- **Hard constraint:** the affordance must be generic — no task-specific
  prompting. If we find ourselves writing "when you see a LaTeX task…", the
  design is wrong.

### 5. Full 76-task run with the winner (Phase D)

Leaderboard-comparable number on Sonnet 4.6. Optionally repeat on Opus 4.7
to separate scaffolding effects from model strength.

### 6. SWE-Bench Verified (later)

Same Harbor wrapper, one flag change. 500 tasks, plan a few hundred dollars
of API budget. Only after Phase D.

## Known weaknesses

Observed patterns in Omega's failures. Each is a candidate affordance for
Phase C. Evidence is in the referenced trial directories under `jobs/`.

### No goal-check against stated success criterion

The model produces plausible output, runs it once, and exits without
verifying the stated success criterion was actually met.

| Trial | Task | What happened |
|---|---|---|
| `d27ba77a-…` | `overfull-hbox` | Edited twice, re-ran `pdflatex`, warnings still present, ended without further edits. |
| `376cd0ab-…` | `extract-elf` | Produced valid JSON, never measured coverage against the task's explicit ≥ 75 % threshold. |

**Candidate affordance.** Success-criterion reminder — when the instruction
contains a concrete threshold ("≥ 75 %", "no overfull hbox warnings", "all
tests pass"), surface that threshold in the context near the end of each
turn and prompt self-evaluation against it.

### No time-budget awareness / rabbit-hole

The model pursues optimisation without ever having delivered a working
baseline. No stop signal on "X min elapsed, nothing shipped."

| Trial | Task | What happened |
|---|---|---|
| `18106395-…` | `largest-eigenval` | 30+ tool calls into `ctypes` / OpenBLAS `dgeev`; never wrote `/app/eigen.py`; hit the 900 s task timeout. A `scipy.linalg.eig` solution would have passed in ~60 s. |

**Candidate affordances.**
- Inject `elapsed: X min / deadline: Y min` into the context at turn end.
  Omega currently has turn budgets but no wall-clock awareness.
- Depth limit: after N consecutive tool calls with no file write to a target
  path, inject "step back — have you delivered a solution?"
- System-prompt nudge: "commit a working solution before optimising."

### Common thread

All three failures are variations of **"kept going without a meta-check that
the stated goal was reached."** Whether this is the dominant weakness
Omega-wide, or an artefact of n=3, is the question Phase A will answer.

## Running benchmarks

### Run one or more tasks

```bash
# one specific task
harbor run -d terminal-bench@2.0 \
  --agent-import-path omega_agent:OmegaAgent \
  -m anthropic/claude-sonnet-4-6 \
  --ae ANTHROPIC_API_KEY=$ANTHROPIC_API_KEY \
  -t terminal-bench/fix-git -n 1

# N random tasks
harbor run -d terminal-bench@2.0 \
  --agent-import-path omega_agent:OmegaAgent \
  -m anthropic/claude-sonnet-4-6 \
  --ae ANTHROPIC_API_KEY=$ANTHROPIC_API_KEY \
  -n 1 --n-tasks 15
```

Results land in `jobs/<timestamp>/`. Each trial directory contains
`agent/events.jsonl`, `agent/context.jsonl`, Harbor's `result.json`, and
`trial.log`.

**harbor buffers all stdout until the run completes.** The log file is
written in one shot at the end — don't expect it to grow while tasks run.
From an Omega session, use `run_background` + a single `wait_for_output`
with `timeoutMs` ≥ 1800000 (30 min for a 1-task run; 7200000 for a 15-task
batch) and pattern `"Mean:|Results written to|Exception"`. Check the
result once at the timeout; do not issue follow-up waits or kill the
process just because output is silent.

### Ingest results

```bash
bun scripts/bench-ingest.ts                              # scan all of jobs/
bun scripts/bench-ingest.ts jobs/2026-05-01__10-00-00    # specific job
```

Idempotent (dedup key: `trial_id`). Appends one line per trial to
`benchmark-results/results.jsonl`.

### View current state

```bash
bun scripts/bench-summary.ts           # all models
bun scripts/bench-summary.ts sonnet    # filter by model-name substring
```

### `results.jsonl` schema

| Field | Description |
|---|---|
| `trial_id` | Harbor trial UUID — dedup key |
| `job_id` | Harbor job UUID (multi-task runs share one) |
| `task_name` | e.g. `terminal-bench/fix-git` |
| `ingested_at`, `started_at`, `finished_at` | ISO-8601 |
| `runtime_sec` | Wall-clock seconds for the trial |
| `model` | e.g. `claude-sonnet-4-6` |
| `reward` | 0.0 or 1.0; null if verifier never ran |
| `n_input_tokens`, `n_output_tokens`, `n_cache_tokens` | From `turn_end` event |
| `exception` | `AgentTimeoutError` etc., or null |

## Reference

### Terminology

| Term | Meaning |
|---|---|
| **Terminal-Bench (TB)** | The benchmark — tasks + tests. Versions 1.x, 2.0, 3.0 (dev). |
| **Harbor** | The harness that runs containerised agent benchmarks. General-purpose. |
| **`harbor` CLI** | Current tool. |
| **`tb` CLI** | **Legacy** v1. Ignore. |
| **Oracle** | Built-in Harbor agent that replays each task's `solution.sh`. No LLM, no cost. 76 / 89 TB 2.0 tasks pass the oracle; the other 13 need GPU or huge downloads and are excluded from agent comparisons — see `benchmark-results/oracle-tasks.json`. |

**Analogy.** Harbor : Terminal-Bench ≈ pytest : a test suite.

### Model choice

Omega is Anthropic-only: `claude-sonnet-4-6`, `claude-opus-4-6`,
`claude-opus-4-7`. Cross-provider comparison isn't a goal; meaningful
benchmarks are model-matched (Omega + Sonnet vs Claude Code + Sonnet). Plan
to benchmark on both Sonnet 4.6 and Opus 4.7 so scaffolding effects can be
separated from model strength.

### Cost pointers

- Sonnet 4.6 pricing: $3 / $15 / $0.30 per MTok (input / output / cache-read)
- A passing `crack-7z-hash` trial costs ≈ $0.25
- Extrapolated full 76-task pass: ≈ $19

## Archive

Completed or superseded work, kept for historical pointers.

- **Oracle sweep** (2026-04-23). 76 / 89 TB 2.0 tasks pass the oracle. List
  in `benchmark-results/oracle-tasks.json`.
- **Omega CLI** (`src/cli.ts`, tagged `v0.1.0`). Headless entry point:
  `--instruction`, `--model`, `--effort`, `--session-dir`, `--max-turns`.
  LLM text → stdout, structured logs → stderr. Exit 0 on `turn_end`, 1 on
  interrupt/error.
- **`omega_agent.py`** (Harbor wrapper, repo root). Installed-agent adapter.
  Bring-up fixes: `unzip` added to apt deps, `--agent-import-path` without
  `./` prefix, `RUN_TIMEOUT_SEC = 1800`.
- **Omega bugs surfaced during bring-up.** `wait_for_output` used
  `String.includes` instead of `RegExp`; `wait_for_output` ignored the abort
  signal. Both fixed.
- **Phase 1 — persist `context.jsonl`** (`omega_agent.py`, 2026-04-24).
  Trials now preserve the full session, not just events. Smoke-tested with
  a re-run of `crack-7z-hash`.
- **Phase 2A — web-UI replay script.** Originally planned as glue to load
  trials in Omega's web UI. Judged ballast given the LLM-driven diagnosis
  goal and deleted before landing.

## References

- `benchmark-results/results.jsonl` — accumulated trial data
- `benchmark-results/oracle-tasks.json` — per-task oracle status
- `jobs/<timestamp>/<task>/agent/{events,context}.jsonl` — raw session per trial
- `omega_agent.py` — Harbor wrapper
- `scripts/bench-ingest.ts`, `scripts/bench-summary.ts` — results tooling
