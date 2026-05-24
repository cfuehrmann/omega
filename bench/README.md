# Omega — Terminal-Bench 2.0

Terminal-Bench 2.0 is a 89-task containerised coding benchmark run via [Harbor](https://github.com/the-harbor-project/harbor).

## Results

| Model | Effort | Score | Notes |
|---|---|---|---|
| `claude-sonnet-4-6` | medium | **53/89 = 59.6 %** | |
| `claude-opus-4-7` | high | **62/89 = 69.7 %** | xhigh used for tasks ≥ 900 s budget |
| Claude Opus 4.7 / Adaptive (official) | adaptive | **69.4 %** | tbench.ai leaderboard |

Omega + Opus 4.7 at 69.7 % matches the official leaderboard's Opus 4.7 Adaptive entry (69.4 %) — same model, different agent harness.

Run `python bench/scripts/bench-summary.py` for a live breakdown from `bench/results/results.jsonl` (regenerate this script in Python if it does not exist yet).

## Running benchmarks

**Always run Harbor from the `bench/` directory** — Harbor writes `jobs/` relative to CWD, so running from `bench/` is what keeps job output inside `bench/jobs/`.

```bash
cd bench

# one specific task
harbor run -d terminal-bench@2.0 \
  --agent-import-path omega_agent:OmegaRustAgent \
  -m anthropic/claude-sonnet-4-6 \
  --ae ANTHROPIC_API_KEY=$ANTHROPIC_API_KEY \
  -t terminal-bench/fix-git -n 1

# explicit list of tasks (recommended for targeted re-runs)
harbor run -d terminal-bench@2.0 \
  --agent-import-path omega_agent:OmegaRustAgent \
  -m anthropic/claude-sonnet-4-6 \
  --ae ANTHROPIC_API_KEY=$ANTHROPIC_API_KEY \
  -i taskA -i taskB \
  --job-name my-run
```

Results land in `bench/jobs/<job-name>/`. Each trial directory contains
`agent/events.jsonl`, `agent/context.jsonl`, Harbor's `result.json`, and `trial.log`.

**Harbor buffers all stdout until the run completes.** Use `run_background` +
`wait_for_output` sized to the batch (~30 min per task upper bound), pattern
`"Mean:|Results written to|Exception"`.

> **Note on effort:** `xhigh` is too slow for tasks with ≤ 900 s budgets — extended
> thinking consumes 2–4× the token budget per call. Use `high` for tasks under ~15 min.

## Feature flag sweeps

Omega's research-mode feature flags (`[b0bc06]`) are runtime-switchable env
vars surfaced via Harbor's `--ae KEY=VAL`. No `omega_agent.py` change required
to enable them — pass `--ae OMEGA_FEATURE_REPL=1` and/or
`--ae OMEGA_FEATURE_SUBAGENTS=1` at run time.

The resolved combination lands in `SessionStartedEvent.features` for every
session; post-mortem can grep `events.jsonl` to know which combination a run
used.

### Limit-mode treatment arm

To run the **limit-mode** treatment (removes the six file-op tools so the LLM
must choose between `python_repl` and `run_command` for file work), pass both
flags together:

```bash
--ae OMEGA_FEATURE_REPL=1 --ae OMEGA_FEATURE_REPL_REPLACES_FILEOPS=1
```

Setting `OMEGA_FEATURE_REPL_REPLACES_FILEOPS=1` without `OMEGA_FEATURE_REPL=1`
is a configuration error and the agent will exit with a clear message at
startup.

### Shell-replacement treatment arm (Tier 2)

To run the **shell-replacement** treatment (removes `run_command`,
`run_background`, `wait_for_output`, `write_stdin` — forcing shell work through
`subprocess` inside `python_repl`), use:

```bash
--ae OMEGA_FEATURE_REPL=1 --ae OMEGA_FEATURE_REPL_REPLACES_SHELL=1
```

Setting `OMEGA_FEATURE_REPL_REPLACES_SHELL=1` without `OMEGA_FEATURE_REPL=1`
is a configuration error (same loud-failure policy as the fileops flag).

The two `REPL_REPLACES_*` flags are **orthogonal** — all four combinations are
valid when `OMEGA_FEATURE_REPL=1`.  For the full Tier 2 (REPL-only, no direct
file or shell access) use:

```bash
--ae OMEGA_FEATURE_REPL=1 \
--ae OMEGA_FEATURE_REPL_REPLACES_FILEOPS=1 \
--ae OMEGA_FEATURE_REPL_REPLACES_SHELL=1
```

The five-mode sweep matrix for the REPL benchmark is:

| Treatment arm | `--ae` flags |
|---|---|
| OFF (baseline) | _(none)_ |
| Additive REPL | `--ae OMEGA_FEATURE_REPL=1` |
| Limit-mode REPL (fileops removed) | `--ae OMEGA_FEATURE_REPL=1 --ae OMEGA_FEATURE_REPL_REPLACES_FILEOPS=1` |
| Shell-replacement REPL (Tier 2) | `--ae OMEGA_FEATURE_REPL=1 --ae OMEGA_FEATURE_REPL_REPLACES_SHELL=1` |
| Full Tier 2 (fileops + shell removed) | `--ae OMEGA_FEATURE_REPL=1 --ae OMEGA_FEATURE_REPL_REPLACES_FILEOPS=1 --ae OMEGA_FEATURE_REPL_REPLACES_SHELL=1` |

### REPL benchmark plan (v0.1.8, first benchmark data on the REPL MVP)

Run both legs at the **same Omega tag** so the comparison is apples-to-apples.
The legacy `v0.1.7` baseline (53/89 sonnet-medium, 62/89 opus-high) is not
comparable directly — `v0.1.8` includes intervening scaffolding work
(`FeatureFlags`, `DomainSnapshot`, `fold_features`, system-prompt round-trip)
that may shift the baseline by a percentage point or two.

```bash
cd bench

# Baseline at v0.1.8 (REPL off).
harbor run -d terminal-bench@2.0 \
  --agent-import-path omega_agent:OmegaRustAgent \
  -m anthropic/claude-sonnet-4-6 \
  --ae ANTHROPIC_API_KEY=$ANTHROPIC_API_KEY \
  --job-name v018-baseline-sonnet-medium

# Treatment: same everything, OMEGA_FEATURE_REPL=1.
harbor run -d terminal-bench@2.0 \
  --agent-import-path omega_agent:OmegaRustAgent \
  -m anthropic/claude-sonnet-4-6 \
  --ae ANTHROPIC_API_KEY=$ANTHROPIC_API_KEY \
  --ae OMEGA_FEATURE_REPL=1 \
  --job-name v018-repl-on-sonnet-medium
```

The headline number: **delta between v018-baseline and v018-repl-on**.

Start with sonnet-medium only (cheaper, faster to iterate; Kim et al.'s
results suggest small-model effects are where multi-agent / scaffold changes
matter most). Opus-high can be a follow-up sweep if sonnet results are
interesting in either direction.

See `docs/repl-and-subagents-research.html` §1.6 for the broader context.

## Ingest and view results

The ingest and summary scripts are lightweight Python utilities. If they are
not present in `bench/scripts/`, ask the LLM to write them — the schema below
and the existing `results.jsonl` are sufficient context.

Expected interface (regenerate to taste):

```bash
python bench/scripts/bench-ingest.py                      # scan all of bench/jobs/
python bench/scripts/bench-ingest.py bench/jobs/my-run    # specific job
python bench/scripts/bench-summary.py                     # all models
python bench/scripts/bench-summary.py sonnet              # filter by model substring
```

Ingestion is idempotent: re-running after the same job adds nothing.

To permanently exclude a contaminated or infra-failed trial, add its UUID to
`bench/results/.skip-trials` (one UUID per line, `#` for comments).

## File layout

| Path | Contents |
|---|---|
| `bench/omega_agent.py` | Harbor agent adapter |
| `bench/results/results.jsonl` | Accumulated trial data (one JSON record per line) |
| `bench/results/oracle-tasks.json` | Per-task metadata for all 89 tasks |
| `bench/results/.skip-trials` | Trial UUIDs permanently excluded from ingest |
| `bench/jobs/` | Raw Harbor job output — gitignored, local only |
| `bench/scripts/bench-ingest.py` | Scan `bench/jobs/` → append new trials to `results.jsonl` (regenerate if absent) |
| `bench/scripts/bench-summary.py` | Print results summary table (regenerate if absent) |
| `bench/scripts/analyze-failures2.py` | Failure-log analysis |

## `results.jsonl` schema

| Field | Description |
|---|---|
| `trial_id` | Harbor trial UUID — dedup key |
| `job_id` | Harbor job UUID (multi-task runs share one) |
| `task_name` | e.g. `terminal-bench/fix-git` |
| `ingested_at`, `started_at`, `finished_at` | ISO-8601 timestamps |
| `runtime_sec` | Wall-clock seconds for the trial |
| `agent` | Agent name, e.g. `omega` |
| `model` | Model name, e.g. `claude-sonnet-4-6` |
| `reward` | `0.0` or `1.0`; `null` if the verifier never ran |
| `n_input_tokens`, `n_output_tokens`, `n_cache_tokens` | From the final `turn_end` event |
| `exception` | e.g. `AgentTimeoutError`, or `null` |

## Failure taxonomy (Sonnet 4.6 baseline)

Analysis of the 89-task run surfaced seven structurally distinct failure shapes:

| # | Shape | Tasks (n) | Mechanism |
|---|---|---|---|
| 1 | **Thinking-budget exhaustion** | 4–5 | Sonnet 4.6 hits the 64 k output-token limit mid-thinking, producing no tool call; the turn ends silently. Unaffected on Opus 4.7 (128 k limit). |
| 2 | **Wall-clock timeout** | 10 | Agent is mid-loop when the container is killed; the output file was never committed. |
| 3 | **Artifact in wrong location** | 3 | Agent writes to its CWD (`/home/agent/omega/`) or leaves compiled test binaries in the submission directory; verifier checks `/app/`. |
| 4 | **Wrong numerical answer** | 5 | Agent's approach is correct in structure but produces a wrong value (off-by-one, wrong dataset slice, wrong algorithm parameter). |
| 5 | **Verifier infrastructure failure** | 3–4 | `uvx`/`uv` DNS failure inside the container; verifier never checks the agent's output. One confirmed false negative (`distribution-search`). |
| 6 | **Near-miss / edge case** | 3 | Agent passes most verifier tests but misses one specific case (e.g. asyncio cancellation above semaphore limit). |
| 7 | **Turn exhaustion** | 3 | 50-turn limit reached; output never written. Includes `make-mips-interpreter` where container setup takes 6+ min. |

## Terminology

| Term | Meaning |
|---|---|
| **Terminal-Bench (TB)** | The benchmark — 89 tasks + automated verifiers. We use version 2.0. |
| **Harbor** | The harness that runs containerised agent benchmarks. |
| **Oracle** | Harbor's built-in agent that executes each task's `solution.sh` verbatim. Its pass/fail reflects whether the *reference script* works in that container, not which tasks a real agent should attempt. All 89 tasks are in scope for every agent. |

## Model costs (Anthropic)

| Model | Input | Output | Cache read |
|---|---|---|---|
| `claude-sonnet-4-6` | $3 / MTok | $15 / MTok | $0.30 / MTok |
| `claude-opus-4-7` | $5 / MTok | $25 / MTok | $0.50 / MTok |

A full 89-task Sonnet run costs ≈ $25; Opus ≈ $30.

## Next steps

**SWE-Bench Verified** (planned): same Harbor wrapper, one flag change; ~500 tasks, ~$300 budget.
