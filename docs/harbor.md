# Harbor Benchmarking

Running Omega against Terminal-Bench 2.0 (and, later, SWE-Bench Verified via
Harbor's registry) to produce apples-to-apples numbers against Claude Code,
Terminus-2, Mini-SWE-Agent, OpenHands on the same model.

**Entry point:** [Roadmap](#roadmap) — the first item is always the next thing
to do.

## Status

- **Model under evaluation:** `claude-sonnet-4-6`
- **Tasks attempted:** 76 / 76 oracle-passing TB 2.0 tasks (complete)
- **Pass rate:** 51 / 97 trials (53 %) on tried; **51 / 76 (67 %)** on the
  full oracle-passing set — the leaderboard-comparable number
- **API spend to date:** ≈ $24.67
- **Results data:** `benchmark-results/results.jsonl`
- **Per-trial logs:** `jobs/<timestamp>/<task>/agent/{events,context}.jsonl`

### Failure shape — after Phase C (fresh 12-task run)

The Phase C run (9/12 = 75 % pass rate on fresh tasks) confirms interventions
generalise and the re-run cluster is harder than average. Three new failures
fit cleanly into existing shapes:

| Category | Tasks (n) | Trial signature |
|---|---|---|
| **Wrong answer despite verification** | count-dataset-tokens, dna-insert, extract-elf, filter-js-from-html, query-optimize (5) | 6–13 LLM turns; real work, wrong result |
| **Genuinely hard / time-limited** | largest-eigenval, gcode-to-text, winning-avg-corewars (3) | wall-clock timeout even with deadline; 21–45 turns |
| **Output token limit** | regex-chess, winning-avg-corewars (2) | `max_tokens` stop; agent tries to emit huge output in one shot |
| **Fast capability failure** | cancel-async-tasks, polyglot-c-py (2) | 2–4 min; agent produces wrong answer quickly |

*`winning-avg-corewars` spans two categories (time-limited and max_tokens).
No new failure shape introduced by Phase C.*

**Prompt-fix outcome (2026-04-24, item 1 complete):**  
2 of 7 re-run tasks flipped to pass — below the ≥ 4 threshold.  
`circuit-fibsqrt` (was 2 turns → now 14 turns + all 32 tests pass) and
`overfull-hbox` (was seed failure → now 15 turns + zero warnings) both
benefited directly from the design-discipline / task-completion change.  
For the 5 remaining failures the root cause is not early stopping but
wrong approach or capability limits — see analysis under Mechanism 1 below.

**Flakiness flag.** `crack-7z-hash` passed in the oracle-era smoke test
(908 s) but hit `AgentTimeoutError` at 1800 s when re-run on 2026-04-24.
Two data points; watch for more non-determinism in Phase D.

## Agent-layer gap

Evidence pointing at two fixable gaps in Omega's core setup:

### Mechanism 1 — system prompt pushed the agent toward "stop and wait"

The original `core.ts` Design-discipline clause read:

> *"Discuss design with the user before implementing non-trivial changes.
> If the user raises a design question mid-implementation, stop and discuss
> before continuing."*

In headless runs this reduced to "do the smallest thing, then stop" — which
matches what the event logs show: goal-check fails had **2–13 LLM turns**
and always ended with a clean `turn_end` (no tool calls in the final
response = "I think I'm done"). No trial hit the 50-turn budget.

Additionally, the prompt had no explicit *task-completion* rule telling the
agent to verify the stated success criterion before declaring done.

**Validation result (Phase A item 1, 2026-04-24).** 2 of 7 re-run tasks
flipped. The two that passed (`circuit-fibsqrt`, `overfull-hbox`) confirm
the hypothesis: the prompt change caused the agent to run 14–15 turns
instead of 2 and actually verify its work. The 5 that still failed reveal
that the original "goal-check missing" label was partially wrong for those
tasks — the agent was not stopping early due to the prompt; it was running
6–13 turns and producing wrong answers. `count-dataset-tokens` ran 13 turns
in *both* runs (before and after the prompt fix), which makes it clear that
early stopping was never the issue there. `regex-chess` switched failure mode:
it now hits `max_tokens` on turn 2 because the agent tries to emit the entire
regex JSON in a single response, which exceeds the model's output token limit.
The 3-shape picture above reflects this revised understanding.

**Fix (landed 2026-04-24, commit `f4320cd`, `v0.1.0` tag re-pointed):**

- Design discipline: *"state your chosen approach and the alternatives you
  considered, then proceed. If the user raises a design question — before,
  during, or after — stop and discuss."* No implicit wait; user interrupts
  still halt.
- New Task-completion section: *"verify the stated success criterion before
  declaring done … run the check and confirm the measured value."* Also a
  half-budget rule for time-bounded tasks.
- Carsten-specific habits ("run git status before new work", general testing
  guidance) moved out of core into `.omega/system-prompt-append.md` so the
  core prompt stays behaviour-oriented and repo-neutral.

Design principle held throughout: **one prompt, both modes**. No
`OMEGA_HEADLESS` gating — what we want the benchmark agent to do is what we
want the daily agent to do. See the pass criterion in roadmap item 1.

### Mechanism 2 — zero wall-clock awareness

Rabbit-hole failures (4 of 12) all ran to a wall-clock cap (harbor's
`agent_timeout_sec = 900`, or our Python-side 1800 s). The agent never
knew a deadline existed. Harbor knows each task's `agent_timeout_sec`
and could prepend it to the instruction.

This is benchmark-supplied information, not a benchmark-specific agent
behaviour — equivalent to a user typing "I need this in 15 minutes" in
chat. The core prompt already has the matching rule ("if the instruction
names a time budget, commit a working solution before refining"); what's
missing is the wrapper plumbing. That's roadmap item 3.

### Wrong-answer category — capability floor, no scaffolding fix planned

4 tasks (count-dataset-tokens, dna-insert, extract-elf, filter-js-from-html)
failed in both runs despite the agent doing real work (6–13 LLM turns). No
single scaffolding intervention addresses this: the agent's approach or
knowledge is wrong, not its verification discipline. Three narrow levers
exist — higher effort (`--effort xhigh`), an infra check on network access
inside containers (count-dataset-tokens ran identically both times, which is
suspicious), and model upgrade — but the clean answer is the Opus 4.7 run
planned for Phase D. If these tasks still fail on Opus they are genuine
capability floors; if they flip, Sonnet was the ceiling and the scaffolding
is fine. No separate roadmap item; revisit after Phase D.

## Roadmap

Ordered. First item is the next thing to do.

### 1. Validate the prompt hypothesis — **DONE** (2026-04-24)

**Result:** 2 of 7 tasks flipped (circuit-fibsqrt ✓, overfull-hbox ✓).
Below the ≥ 4 threshold. Zero exceptions. Wall-clock 22 min 24 s, cost ≈ $3.28.
Job: `jobs/phaseA-prompt-validation/`.

**Interpretation.** The design-discipline / task-completion prompt change is
real and measurable: the two flips confirm the causal mechanism for tasks
where the agent had the right approach but quit early. However, it is not the
dominant cause of the original 7-failure cluster — 5 tasks had deeper issues
(wrong answers or capability limits) that extra verification turns cannot fix.
See the extended analysis under Mechanism 1 in the Agent-layer gap section.

**Autonomy envelope.** In scope: retry a crashed harbor invocation, re-run
a single timed-out task, fix an `omega_agent.py` infrastructure bug (and
re-point the `v0.1.0` tag). Out of scope: changing agent behaviour beyond
what's already committed, starting item 2.

### 2. `winning-avg-corewars` timeout-mismatch investigation — **DONE** (2026-04-24)

**Root cause.** `winning-avg-corewars` has `agent.timeout_sec = 3600` in its
`task.toml` (a 1-hour deadline, not the standard 900 s). Harbor's outer
`asyncio.wait_for` was correctly set to 3600 s. Our hard-coded
`RUN_TIMEOUT_SEC = 1800` in `omega_agent.py` created an inner timeout that
fired at 30 min, raising `RuntimeError` instead of `AgentTimeoutError` and
cutting the task's runtime in half. The `finally` downloads (events/context)
survived cancellation correctly in both cases.

**Fix.** Removed `timeout_sec=RUN_TIMEOUT_SEC` from the `exec_as_agent`
call and deleted the constant. Harbor's per-task timeout is now the sole
controlling mechanism, which is correct by design.

### 3. Rabbit-hole affordance — deadline injection — **DONE** (2026-04-24)

**Implementation.** `_get_agent_timeout_sec()` in `omega_agent.py` reads
`config.json` from the trial directory and looks up the task's `task.toml`
from the harbor host cache (`~/.cache/harbor/tasks/*/task_name/task.toml`).
Applies the same multiplier + cap logic harbor uses. Result prepended as
`"Time budget: N seconds (M minutes).\n\n"` before the instruction.
Fails gracefully (no prepend) if the cache entry is absent.

**Result:** 1 of 4 flipped. Below the ≥ 2 threshold. Wall-clock 28 min 41 s.
Job: `jobs/phaseB-deadline-validation/`.

| Task | Before | After | Turns | Notes |
|---|---|---|---|---|
| write-compressor | AgentTimeoutError, 0 reward | **reward=1.0**, AgentTimeoutError | 9 | committed early, timed out during final size-check — injection **worked** |
| gcode-to-text | AgentTimeoutError | AgentTimeoutError | 45 | ran full 900 s; 45 turns of real iteration; genuinely hard |
| largest-eigenval | AgentTimeoutError | AgentTimeoutError | 21 | 21 turns on eigenvalue algorithm; still timed out |
| winning-avg-corewars | RuntimeError 1800 s | reward=0.0, no timeout | 11 | RUN_TIMEOUT_SEC fix worked; but hit `max_tokens` on turn 2, wrong answer |

**Interpretation.** Deadline injection is real and causally confirmed on
`write-compressor`: the agent committed a working solution before running out
of time, and the verifier rewarded it. For `gcode-to-text` and
`largest-eigenval` the tasks are genuinely hard — 45 and 21 turns of real work
still couldn’t deliver in 900 s. `winning-avg-corewars` changed failure shape:
now completes without timeout (the RUN_TIMEOUT_SEC fix) but hits `max_tokens`
and submits a wrong answer — a separate capability-floor issue.

### 4. Fresh ~12-task exploratory run — **DONE** (2026-04-24)

**Tasks:** fix-git, cobol-modernization, crack-7z-hash, nginx-request-logging,
openssl-selfsigned-cert, kv-store-grpc, query-optimize, git-multibranch,
log-summary-date-ranges, polyglot-c-py, cancel-async-tasks, fix-code-vulnerability.

**Result:** 9/12 passed (75 %). Zero exceptions. 13 min 2 s.
Job: `jobs/phaseC-fresh-12/`. New leaderboard metric: **27/76 = 36 %**.

**Interpretation.** 75 % pass rate on fresh tasks vs 54 % overall confirms
interventions generalise — the re-run cluster is genuinely harder than
average, not a sign of overfitting. Three new failures mapped to existing
shapes (wrong-answer × 1, fast capability failure × 2). No new failure
category emerged; roadmap holds.

### 5. Failure-mode investigation across the 76 tried tasks — **next**

We now have 29 failing tasks (and 47 passing) with full event logs at
`jobs/<phase>/<task>/agent/{events,context}.jsonl`. The headline failure-shape
taxonomy in this doc was built incrementally during the runs themselves — a
clean read-through of the actual logs may reveal patterns that the
incremental view missed.

**Goals.**
- Find recurring patterns in *how* trials fail (not just *that* they fail) —
  e.g. shared tool-misuse, shared dead-end approaches, shared blind spots.
- Identify candidates for cheap fixes (prompt or wrapper level) before
  spending on the Opus run.
- Sanity-check `docs/results.md`'s shape taxonomy against actual log content.
- Surface anything *surprising* in the passing trials too (e.g. tasks that
  passed by accident, near-misses, suspicious shortcuts).

**Approach — two sub-steps.**

**5a. Read-and-cluster pass** (Sonnet 4.6, medium effort, agentic). Read
event logs across the 29 failures (and a sample of passes for contrast).
Produce `docs/failure-analysis.md` containing: (i) a refined shape taxonomy
with examples, (ii) a list of candidate cheap fixes ranked by expected
yield, (iii) a list of "interesting" trials worth deeper inspection.

**5b. Per-trial deep-dive** (optional, Opus 4.7, batch script). For trials
flagged interesting in 5a, run a non-interactive script that sends each
trial's full event excerpt to Opus 4.7 with a structured diagnosis prompt.
Opus's reasoning depth pays off when looking at one failure deeply, without
the context bloat of an agentic loop. Skip if 5a already gave a clear
picture.

**Stays inside autonomy envelope:** read-only investigation, retries fine,
`omega_agent.py` infra fixes fine, no agent-behaviour changes.

**Pass criterion.** `docs/failure-analysis.md` exists and either (a) names
≥ 1 cheap fix worth implementing before the Opus run, or (b) confirms the
existing taxonomy is sound and nothing cheap is left on the table.

### 6. Full 76-task run — Phase D — **DONE** (2026-04-24)

**Result: 51 / 76 = 67 %** on Sonnet 4.6. $24.67 cumulative spend.
Jobs: `jobs/phaseD-remaining-42/` + `jobs/phaseD-infra-retry/`.
Full per-task breakdown and category analysis: **`docs/results.md`**.

**Category breakdown:**

| Category | n | Pass | Rate |
|---|---|---|---|
| data-processing | 4 | 4 | 100 % |
| data-querying | 1 | 1 | 100 % |
| system-administration | 7 | 6 | 86 % |
| security | 8 | 7 | 64 % |
| debugging | 5 | 5 | 100 % |
| software-engineering | 23 | 15 | 65 % |
| data-science | 5 | 2 | 40 % |
| mathematics | 4 | 2 | 50 % |
| scientific-computing | 8 | 3 | 38 % |
| file-operations | 5 | 2 | 40 % |
| machine-learning / model-training / video-processing | 3 | 0 | 0 % |

**Reference baseline:** Claude Code + Sonnet 4.5 scores ≈ 50 % on TB 2.0
(tbench.ai leaderboard). Omega + Sonnet 4.6 at **67 %** clears that bar by
∼ 17 pp — attributable to a combination of model version (4.6 > 4.5) and
scaffolding quality. To isolate the scaffolding contribution: run Opus 4.7
(item 7) and compare with published Opus numbers.

**Infra-retry note.** 5 of 42 Phase D trials failed on first attempt due to
transient network errors (Docker TLS timeout, curl/git install failures). A
retry batch recovered 2 of 5 (`merge-diff-arc-agi-task` ✓, `chess-best-move`
✓). `make-doom-for-mips` and `adaptive-rejection-sampler` hit
`AgentTimeoutError` on retry (genuine task failures). `make-mips-interpreter`
still fails setup (omega install > 360 s); likely a large MIPS compiler
download inside the container.

### 7. SWE-Bench Verified — later

Same Harbor wrapper, one flag change. 500 tasks, plan a few hundred
dollars of API budget. Only after Phase D.

## Running benchmarks

### Run one or more tasks

```bash
# one specific task
harbor run -d terminal-bench@2.0 \
  --agent-import-path omega_agent:OmegaAgent \
  -m anthropic/claude-sonnet-4-6 \
  --ae ANTHROPIC_API_KEY=$ANTHROPIC_API_KEY \
  -t terminal-bench/fix-git -n 1

# explicit list of tasks (recommended for targeted re-runs)
harbor run -d terminal-bench@2.0 \
  --agent-import-path omega_agent:OmegaAgent \
  -m anthropic/claude-sonnet-4-6 \
  --ae ANTHROPIC_API_KEY=$ANTHROPIC_API_KEY \
  -i taskA -i taskB -i taskC \
  --job-name my-validation-run

# N random tasks (bring-up only; prefer explicit lists for data collection)
harbor run -d terminal-bench@2.0 \
  --agent-import-path omega_agent:OmegaAgent \
  -m anthropic/claude-sonnet-4-6 \
  --ae ANTHROPIC_API_KEY=$ANTHROPIC_API_KEY \
  -n 1 --n-tasks 15
```

Results land in `jobs/<timestamp>/` (or `jobs/<job-name>/` if `--job-name` is
passed). Each trial directory contains `agent/events.jsonl`,
`agent/context.jsonl`, Harbor's `result.json`, and `trial.log`.

**harbor buffers all stdout until the run completes.** The log file is
written in one shot at the end — don't expect it to grow while tasks run.
From an Omega session, use `run_background` + a single `wait_for_output`
with `timeoutMs` sized to the batch (30 min per task is a reasonable upper
bound) and pattern `"Mean:|Results written to|Exception"`. Check the result
once at the timeout; do not issue follow-up waits or kill the process just
because output is silent.

### Skipping contaminated trials

`benchmark-results/.skip-trials` is a plain list of trial UUIDs that the
ingest script permanently ignores. Populate it when a trial fails for
reasons unrelated to agent behaviour — API quota hit mid-run, container
setup race, etc. Legitimate fail/timeout trials stay in the results.

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
- **Phase A batch 1** (2026-04-24). 15 tasks, 7 pass / 8 fail; 2 infra
  issues handled (Anthropic monthly quota hit mid-run, `curl | bash` bun
  install failure under concurrent bring-up). See commit `7ff87cb`. All
  8 new failures fit the existing two shapes — no third category emerged.
- **Prompt refinement** (2026-04-24, commit `f4320cd`). Design-discipline
  rephrased from "discuss before implementing" to "state, then proceed",
  new Task-completion section added, Carsten-specific habits moved to
  `.omega/system-prompt-append.md`. `v0.1.0` tag re-pointed from `657a647`
  to `f4320cd`.
- **Phase A prompt-validation run** (2026-04-24, job `phaseA-prompt-validation`).
  Re-ran the 7 goal-check-fail tasks with the revised prompt. 2 of 7 flipped
  (circuit-fibsqrt, overfull-hbox). Below the ≥ 4 threshold; hypothesis
  partially confirmed but not dominant. Updated failure shape: 3 categories
  (wrong-answer ×4, rabbit-hole ×4, max-tokens ×1). See Mechanism 1 analysis.
- **`OMEGA_HEADLESS` prompt-gating idea, rejected.** Was briefly
  considered as a way to make the agent behave differently in benchmark
  runs. Rejected as "teaching to the test" — the fix belongs in the
  single shared prompt, not behind a benchmark-only gate.

## References

- `benchmark-results/results.jsonl` — accumulated trial data
- `benchmark-results/oracle-tasks.json` — per-task oracle status
- `benchmark-results/.skip-trials` — trial UUIDs permanently ignored by ingest
- `jobs/<timestamp>/<task>/agent/{events,context}.jsonl` — raw session per trial
- `omega_agent.py` — Harbor wrapper
- `scripts/bench-ingest.ts`, `scripts/bench-summary.ts` — results tooling
