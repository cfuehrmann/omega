# Harbor Benchmarking

Running Omega against Terminal-Bench 2.0 to produce apples-to-apples numbers
against Claude Code, Terminus-2, Mini-SWE-Agent, and OpenHands on the same model.

**Entry point:** [Roadmap](#roadmap) — the first non-DONE item is always the next thing to do.

## Status

| Model | Effort | Pass rate | Note |
|---|---|---|---|
| `claude-sonnet-4-6` | medium | **50/76** on wrong task set | corrected /89 pending item 10 |
| `claude-opus-4-7` | xhigh+high | **56/76** on wrong task set | corrected /89 pending item 11 |
| Anthropic / Terminus-2 | thinking off | **69.4 % (62/89)** | official self-reported |

> **Wrong task set discovered (item 10).** We filtered to 76 "oracle-passing" tasks
> based on a misunderstanding — see below. All leaderboard agents run all 89 tasks.
> Our numbers are not yet directly comparable to the leaderboard.

Single-shot comparison (within our wrong-76): Opus 4.7 **66 %** vs Sonnet 4.6 **55 %**
(42/76 first-trial). Opus leads by ~11 pp per trial.

> **Reporting note:** `bench-summary.ts` previously counted *total trial passes / 76*
> instead of *unique task passes / 76*, inflating Sonnet from 50 → 54. Fixed.

- **Results data:** `benchmark-results/results.jsonl`
- **Failure analysis:** `docs/failure-analysis.md`

### Failure-shape taxonomy (Sonnet 4.6 baseline)

| # | Shape | Tasks (n) | Distinguishing signal |
|---|---|---|---|
| 1 | **Thinking-budget exhaustion** | 5 | `stop_reason: max_tokens`, no output, no tool calls |
| 2 | **Wall-clock timeout** | 10 | Agent mid-loop when container killed |
| 3 | **Artifact in wrong location** | 3 | Writes to wrong dir or leaves compiled binaries |
| 4 | **Wrong numerical answer** | 5 | Agent self-verifies OK; verifier disagrees |
| 5 | **Verifier infrastructure failure** | 3–4 | `uvx`/`uv` DNS failure; 1 confirmed false negative |
| 6 | **Near-miss / edge case** | 3 | Passes most verifier tests; misses one |
| 7 | **Turn exhaustion** | 3 | 50-turn limit hit; output never written |

## Roadmap

### 1–5 — **DONE**

- **1** Prompt hypothesis validated (2 tasks: circuit-fibsqrt, overfull-hbox).
- **2** winning-avg-corewars timeout mismatch fixed (removed hard-coded 1800 s cap).
- **3** Deadline injection added to omega_agent.py (1 task: write-compressor).
- **4** Fresh 12-task run: 9/12 (75 %). Leaderboard metric established.
- **5** Failure-mode investigation complete. Seven shapes. See `docs/failure-analysis.md`.

### 6 — Cheap fixes — **DONE**

**Fix C** (recovery loop): When `stop_reason=max_tokens` with no output, inject a
synthetic user message and retry. Yield: `winning-avg-corewars` flipped.

**Fix E** (submission-state prompt): Before declaring done, re-read output requirements
and verify submission directory matches. Yield: `distribution-search` flipped.
Shape 3 targets didn't flip — extract-elf needed Fix F; polyglot-rust-c binary cleanup
still eludes the prompt; polyglot-c-py is a persistent infra DNS flake.

**Fix F** (CWD fix, v0.1.2): `omega_agent.py` ran Omega with cwd=/home/agent/omega,
so relative writes landed there instead of `/app/`. Fix: `cd /app && bun run ...`.
Also removed `.omega/system-prompt-append.md` dev-conventions noise from task context.
Yield: `extract-elf` flipped 0→1.

**Sonnet after item 6:** 50/76 = 66 % (corrected; was mis-reported as 54/76 = 71 %).

### 7 — Opus 4.7 full run — **DONE** (job: `opus-4-7-xhigh-76`, 3h 22m, $29.23)

Result: 50/76 = 66 % (1-shot). Matches Sonnet best-of-N; beats Sonnet 1-shot by 11 pp.

**Opus-only passes (10):** `count-dataset-tokens`, `dna-insert`, `feal-linear-cryptanalysis`,
`gpt2-codegolf`, `mteb-retrieve`, `path-tracing-reverse`, `protein-assembly`, `regex-chess`,
`sanitize-git-repo`, `sqlite-with-gcov`.

**Sonnet-only passes, Opus fails (10) — by cause:**

| Task | Opus outcome | Category |
|---|---|---|
| `mailman` | AgentSetupTimeoutError | Infra |
| `prove-plus-comm` | bun.sh DNS fail (82 s) | Infra / Fix F regression |
| `chess-best-move` | AgentTimeoutError (987 s) | xhigh too slow |
| `tune-mjcf` | AgentTimeoutError (1006 s) | xhigh too slow |
| `winning-avg-corewars` | server error after 1541 s | High variance |
| `distribution-search` | reward=0.0 | High variance (Sonnet needed retry) |
| `qemu-startup` | test_version failed | Model quality |
| `configure-git-webserver` | test_hello_html_exists failed | Model quality |
| `headless-terminal` | 1 of 7 tests failed | Near-miss |
| `openssl-selfsigned-cert` | 1 of 6 tests failed | Near-miss |

xhigh effort is too slow for 900 s tasks — extended thinking consumes 2–4× per call.

### 8 — Opus targeted re-run — **DONE** (job: `opus-4-7-high-retry`, 37m)

Re-ran 9 tasks at `high` effort. 5 flipped:

| Task | Flipped? |
|---|---|
| `chess-best-move`, `tune-mjcf`, `gcode-to-text`, `dna-assembly`, `mailman` | ✅ |
| `raman-fitting` | ✗ still times out |
| `prove-plus-comm` | ✗ Fix F regression: `cd /app` fails when `/app` absent |
| `winning-avg-corewars` | ✗ OOM/resource kill |
| `filter-js-from-html` | ✗ bun lightningcss tarball failure |

**Opus after item 8: 55/76 = 72 %**

### 9 — Fix G + targeted re-run — **DONE** (job: `opus-4-7-fixg-retry`, 5m 55s)

v0.1.3: changed `cd /app &&` to `cd /app 2>/dev/null || true` to handle tasks
without `/app`. `prove-plus-comm` flipped ✅. `winning-avg-corewars` ✗ (genuinely hard).

**Opus after item 9: 56/76 = 74 %**

---

### ⚠️ Wrong task set — discovered after item 9

We built `oracle-tasks.json` by assuming that "oracle fails = broken task, skip it".
That was wrong on two counts:

1. **The oracle is just a script runner** (`solution.sh`). Its pass/fail says nothing
   about whether the verifier works or whether a real agent can solve the task.
2. **All leaderboard agents run all 89 tasks** — there is no oracle-based filter in
   the benchmark rules. Anthropic's 69.4% is over 89 tasks, as is every entry on
   tbench.ai.

**Result:** we ran on the wrong 76 tasks. Our actual 76 differed from the correct
oracle-passing 76 in both directions:

| Direction | Tasks |
|---|---|
| **Wrongly excluded** (oracle passes, we skipped) | `build-pov-ray`, `compile-compcert`, `hf-model-inference`, `install-windows-3.11`, `llm-inference-batching-scheduler`, `pytorch-model-cli`, `reshard-c4-data`, `sam-cell-seg` |
| **Wrongly included** (oracle fails, we ran anyway) | `build-pmars`, `count-dataset-tokens`, `custom-memory-heap-crash`, `make-doom-for-mips`, `mteb-retrieve`, `protein-assembly`, `pypi-server`, `rstan-to-pystan` |

The wrongly-included tasks are fine to have run — the verifier works for them and
agents can legitimately solve them (Opus solved 3: count-dataset-tokens, mteb-retrieve,
protein-assembly). They stay in our results. The wrongly-excluded 8 tasks need to be run.

**Actual 13 oracle failures** (the tasks everyone scores ~0 on):
`caffe-cifar-10`, `torch-pipeline-parallelism`, `torch-tensor-parallelism`,
`train-fasttext`, `pytorch-model-recovery`, plus the 8 wrongly-included tasks above.

No task in TB 2.0 requires a GPU — the oracle runs on the benchmark machine
(no GPU) and passes all 8 wrongly-excluded tasks. The "GPU" and "long build" labels
in our original oracle-tasks.json were guesses from task descriptions, not from
actual trial results.

---

### 10 — Fix oracle-tasks.json + run all 13 missing tasks (Sonnet) — **next**

1. **Correct `benchmark-results/oracle-tasks.json`.**
   Ground truth is in `jobs/oracle-sweep-baseline/` — each task subdirectory
   has a `result.json`; read `verifier_result.rewards.reward` for each of the
   13 tasks we labelled non-oracle. The actual oracle failures differ from what
   the file currently says (see `⚠️ Wrong task set` section above).

2. **Run all 13 never-attempted tasks with Sonnet 4.6 at medium effort.**
   The oracle failing a task does NOT mean an agent can't solve it — Opus solved
   three oracle-failing tasks (`count-dataset-tokens`, `mteb-retrieve`,
   `protein-assembly`). All leaderboard agents run all 89 tasks.

```bash
harbor run -d terminal-bench@2.0 \
  --agent-import-path omega_agent:OmegaAgent \
  -m anthropic/claude-sonnet-4-6 \
  --ae ANTHROPIC_API_KEY=$ANTHROPIC_API_KEY \
  -i build-pov-ray -i caffe-cifar-10 -i compile-compcert \
  -i hf-model-inference -i install-windows-3.11 \
  -i llm-inference-batching-scheduler -i pytorch-model-cli \
  -i pytorch-model-recovery -i reshard-c4-data -i sam-cell-seg \
  -i torch-pipeline-parallelism -i torch-tensor-parallelism \
  -i train-fasttext \
  --job-name sonnet-missing-13
```

   Use `run_background` + `wait_for_output` with ~5 h timeout.
   Expect most tasks <20 min; `sam-cell-seg` up to 30 min;
   `compile-compcert` up to 25 min; `pytorch-model-recovery` may time out.

3. **Ingest and report.**

```bash
bun scripts/bench-ingest.ts jobs/sonnet-missing-13
bun scripts/bench-summary.ts sonnet
```

4. **Update this file:** mark item 10 DONE, record passes on the 13 new tasks,
   update the Status table with the corrected Sonnet score as `n/89`.

### 11 — Run all 13 missing tasks (Opus) — **next after 10**

Same 13 tasks with Opus 4.7 at `high` effort:

```bash
harbor run -d terminal-bench@2.0 \
  --agent-import-path omega_agent:OmegaAgent \
  -m anthropic/claude-opus-4-7 \
  --ae ANTHROPIC_API_KEY=$ANTHROPIC_API_KEY \
  -i build-pov-ray -i caffe-cifar-10 -i compile-compcert \
  -i hf-model-inference -i install-windows-3.11 \
  -i llm-inference-batching-scheduler -i pytorch-model-cli \
  -i pytorch-model-recovery -i reshard-c4-data -i sam-cell-seg \
  -i torch-pipeline-parallelism -i torch-tensor-parallelism \
  -i train-fasttext \
  --agent-kwargs effort=high \
  --job-name opus-missing-13
```

After ingesting, the final comparable scores (both /89) will be ready for a
direct apples-to-apples comparison with Terminus-2's 69.4 %.

### 12 — SWE-Bench Verified — **later**

Same Harbor wrapper, one flag change. 500 tasks, ~$300 budget.

## Running benchmarks

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
```

Results land in `jobs/<timestamp>/` (or `jobs/<job-name>/`). Each trial directory
contains `agent/events.jsonl`, `agent/context.jsonl`, Harbor's `result.json`, and
`trial.log`.

**harbor buffers all stdout until the run completes.** Use `run_background` +
`wait_for_output` with `timeoutMs` sized to the batch (30 min per task upper bound)
and pattern `"Mean:|Results written to|Exception"`.

### Skipping contaminated trials

`benchmark-results/.skip-trials` — plain list of trial UUIDs the ingest script
permanently ignores.

### Ingest and view results

```bash
bun scripts/bench-ingest.ts                              # scan all of jobs/
bun scripts/bench-ingest.ts jobs/2026-05-01__10-00-00    # specific job
bun scripts/bench-summary.ts                             # all models
bun scripts/bench-summary.ts sonnet                      # filter by substring
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

- `benchmark-results/results.jsonl` — accumulated trial data
- `benchmark-results/oracle-tasks.json` — per-task metadata (corrected in item 10)
- `benchmark-results/.skip-trials` — trial UUIDs permanently ignored by ingest
- `docs/failure-analysis.md` — failure-shape taxonomy and cheap-fix plan
- `jobs/<phase>/<task>/agent/{events,context}.jsonl` — raw session per trial
- `omega_agent.py` — Harbor wrapper
- `scripts/bench-ingest.ts`, `scripts/bench-summary.ts` — results tooling

### Terminology

| Term | Meaning |
|---|---|
| **Terminal-Bench (TB)** | The benchmark — 89 tasks + verifiers. We use version 2.0. |
| **Harbor** | The harness that runs containerised agent benchmarks. |
| **Oracle** | Built-in Harbor agent that executes each task's `solution.sh` verbatim. Tells you whether the *reference script* works — not which tasks agents should attempt. All 89 tasks are in scope for every agent. |

### Model choice and cost

Omega is Anthropic-only: `claude-sonnet-4-6`, `claude-opus-4-6`, `claude-opus-4-7`.
Pricing: Sonnet 4.6 $3/$15 per MTok (input/output); Opus 4.7 $5/$25.
