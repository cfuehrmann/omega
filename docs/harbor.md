# Harbor Benchmarking

Running Omega against Terminal-Bench 2.0 to produce apples-to-apples numbers
against Claude Code, Terminus-2, Mini-SWE-Agent, and OpenHands on the same model.

**Entry point:** [Roadmap](#roadmap) — the first non-DONE item is always the next thing to do.

## Status

| Model | Effort | Trials | Pass rate (unique tasks) | Spend |
|---|---|---|---|---|
| `claude-sonnet-4-6` | medium | 106 (best-of-N) | 50/76 = **66 %** | $28.90 |
| `claude-opus-4-7` | xhigh | 76 (1-shot) | 50/76 = **66 %** | $29.23 |
| `claude-opus-4-7` | high | +11 retry trials | **56/76 = 74 %** | TBD |

Single-shot comparison: Opus 4.7 xhigh **66 %** vs Sonnet 4.6 medium **55 %** (42/76).
Opus 4.7 wins by ~11 pp in a fair per-trial comparison; the headline tie is an
artefact of Sonnet having multiple attempts on some tasks.

> **Reporting note:** `bench-summary.ts` previously counted *total trial passes / 76*
> instead of *unique task passes / 76*, inflating Sonnet from 50 → 54. Fixed.

- **Results data:** `benchmark-results/results.jsonl`, `docs/results.md`
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

#### Fix C — Recovery loop (thinking-budget exhaustion) — **DONE**

When `stop_reason=max_tokens` with no output, inject a synthetic user message
and retry. Capped at 1 recovery per turn. In `src/agent.ts`.
**Yield:** 1 task flipped (`winning-avg-corewars`).

#### Fix E — Submission-state verification prompt — **DONE** (yield below expectation)

Added to system prompt: before declaring done, re-read task output requirements
and verify the submission directory matches exactly. Judgment-based, not rule-based.
**Yield:** `distribution-search` flipped (Fix D+E combined). The three primary Shape 3
targets (extract-elf, polyglot-rust-c, polyglot-c-py) did not flip — extract-elf
needed the structural CWD fix (Fix F); polyglot-rust-c binary cleanup still eludes
the prompt; polyglot-c-py is a persistent infra DNS flake.

#### Fix F — CWD fix in omega_agent.py — **DONE**

`omega_agent.py` was running Omega with `cd /home/agent/omega`, so `process.cwd()`
was the install directory. Files written to relative paths landed there instead of
`/app/`. Fix: run `cd /app && bun run /home/agent/omega/src/cli.ts`. Also removed
`.omega/system-prompt-append.md` noise (Omega dev conventions) from task context.
**Yield:** `extract-elf` flipped 0→1 (v0.1.2 smoke test, job `fix-f-smoke-test`).
`polyglot-rust-c` still leaves compiled binaries; `polyglot-c-py` infra DNS flake.

**Sonnet leaderboard after item 6:** 50/76 = 66 % (corrected; was mis-reported as 54/76 = 71 %).

### 7 — Opus 4.7 full run — **DONE** (job: `opus-4-7-xhigh-76`, 3h 22m, $29.23)

**Result:** 50/76 = 66 % (1-shot). Matches Sonnet best-of-N; beats Sonnet 1-shot by 11 pp.

**Opus-only passes (10):** `count-dataset-tokens`, `dna-insert`, `feal-linear-cryptanalysis`,
`gpt2-codegolf`, `mteb-retrieve`, `path-tracing-reverse`, `protein-assembly`, `regex-chess`,
`sanitize-git-repo`, `sqlite-with-gcov`.

**Sonnet-only passes, Opus fails (10) — by cause:**

| Task | Opus outcome | Category |
|---|---|---|
| `mailman` | AgentSetupTimeoutError | Infra |
| `prove-plus-comm` | bun.sh DNS fail (82 s) | Infra |
| `chess-best-move` | AgentTimeoutError (987 s / 900 s limit) | xhigh too slow |
| `tune-mjcf` | AgentTimeoutError (1006 s / 900 s limit) | xhigh too slow |
| `winning-avg-corewars` | server error after 1541 s | High variance |
| `distribution-search` | reward=0.0 | High variance (Sonnet needed retry) |
| `qemu-startup` | test_version failed | Model quality |
| `configure-git-webserver` | test_hello_html_exists failed | Model quality |
| `headless-terminal` | 1 of 7 tests failed | Near-miss |
| `openssl-selfsigned-cert` | 1 of 6 tests failed | Near-miss |

**xhigh effort is too slow for 900 s tasks.** Extended thinking consumes 2–4× more
time per call; five tasks timed out that Sonnet completed on medium. Re-running those
at `high` effort should recover 2–4 tasks.

**Adjusted Opus estimate:** 52/76 (68 %) excluding infra; 54/76 (71 %) also excluding
xhigh timeouts.

**Reference:** Claude Code + Sonnet 4.5 ≈ 50 % on TB 2.0. Omega clears that by ~16 pp.

### 8 — Opus targeted re-run — **DONE** (job: `opus-4-7-high-retry`, 37m, 5/9 pass)

Re-ran 9 tasks with `claude-opus-4-7` at `high` effort.

| Task | xhigh outcome | high outcome | Flipped? |
|---|---|---|---|
| `chess-best-move` | AgentTimeoutError | reward=1.0 | ✅ |
| `tune-mjcf` | AgentTimeoutError | reward=1.0 | ✅ |
| `gcode-to-text` | AgentTimeoutError | reward=1.0 (despite timeout) | ✅ |
| `raman-fitting` | AgentTimeoutError | reward=0.0 (still times out) | ✗ |
| `dna-assembly` | NonZeroAgentExitCodeError | reward=1.0 | ✅ |
| `mailman` | AgentSetupTimeoutError | reward=1.0 | ✅ |
| `prove-plus-comm` | NonZeroAgentExitCodeError | NonZeroAgentExitCodeError | ✗ — Fix F regression |
| `winning-avg-corewars` | NonZeroAgentExitCodeError | NonZeroAgentExitCodeError | ✗ — OOM/resource kill |
| `filter-js-from-html` | NonZeroAgentExitCodeError | reward=0.0 (bun lightningcss fail) | ✗ |

**Opus leaderboard after item 8: 55/76 = 72 %**

**Fix F regression (`prove-plus-comm`):** `cd /app &&` fails immediately with
`cd: /app: No such file or directory` for tasks whose Docker image has no `/app`
mount. Fix G (below) changes the prefix to `cd /app 2>/dev/null || true`.

### 9 — Fix G + targeted re-run — **DONE** (job: `opus-4-7-fixg-retry`, 5m 55s)

v0.1.3 tagged with Fix G. Re-ran `prove-plus-comm` and `winning-avg-corewars`.

| Task | Outcome | Flipped? |
|---|---|---|
| `prove-plus-comm` | reward=1.0 | ✅ Fix G confirmed |
| `winning-avg-corewars` | reward=0.0 (5 min, no exception) | ✗ genuinely hard |

**Opus leaderboard after item 9: 56/76 = 74 %**

### 10 — Next steps — **next**

Remaining Opus failures worth investigating:
- `openssl-selfsigned-cert`, `headless-terminal`, `configure-git-webserver`, `qemu-startup`
  — Sonnet-only passes; model quality or near-miss
- `polyglot-rust-c` — binary cleanup (Shape 3); prompt still insufficient
- `raman-fitting`, `path-tracing`, `gcode-to-text` — Shape 2 (genuine timeouts)
- `filter-js-from-html` — bun lightningcss tarball failure (infra; not effort-related)

Candidate: run a Sonnet 4.6 full re-run at v0.1.3 to measure Fix G's impact there,
or push to SWE-Bench Verified.

### 11 — SWE-Bench Verified — **later**

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
- `benchmark-results/oracle-tasks.json` — per-task oracle status
- `benchmark-results/.skip-trials` — trial UUIDs permanently ignored by ingest
- `docs/results.md` — per-task table and category breakdown
- `docs/failure-analysis.md` — failure-shape taxonomy and cheap-fix plan
- `jobs/<phase>/<task>/agent/{events,context}.jsonl` — raw session per trial
- `omega_agent.py` — Harbor wrapper
- `scripts/bench-ingest.ts`, `scripts/bench-summary.ts` — results tooling

### Terminology

| Term | Meaning |
|---|---|
| **Terminal-Bench (TB)** | The benchmark — tasks + tests. We use version 2.0. |
| **Harbor** | The harness that runs containerised agent benchmarks. |
| **Oracle** | Built-in Harbor agent that replays each task's `solution.sh`. 76/89 TB 2.0 tasks pass the oracle. |

### Model choice and cost

Omega is Anthropic-only: `claude-sonnet-4-6`, `claude-opus-4-6`, `claude-opus-4-7`.
Pricing: Sonnet 4.6 $3/$15 per MTok (input/output); Opus 4.7 $5/$25.
