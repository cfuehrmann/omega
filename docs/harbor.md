# Harbor Benchmarking

Running Omega against Terminal-Bench 2.0 to produce apples-to-apples numbers
against Claude Code, Terminus-2, Mini-SWE-Agent, and OpenHands on the same model.

**Entry point:** [Roadmap](#roadmap) — the first non-DONE item is always the next thing to do.

## Status

- **Model under evaluation:** `claude-sonnet-4-6`
- **Tasks attempted:** 76 / 76 oracle-passing TB 2.0 tasks (complete)
- **Pass rate:** 51 / 76 = **67 %** (leaderboard-comparable number)
- **API spend to date:** ≈ $24.67
- **Results data:** `benchmark-results/results.jsonl`, `docs/results.md`
- **Failure analysis:** `docs/failure-analysis.md`

### Failure-shape taxonomy

Seven shapes across 29 failing tasks / 40 failing trials:

| # | Shape | Tasks (n) | Distinguishing signal |
|---|---|---|---|
| 1 | **Thinking-budget exhaustion** | 5 | `stop_reason: max_tokens`, `output_tokens: 64000`, `text: ""`, no tool calls |
| 2 | **Wall-clock timeout** | 10 | Agent mid-loop when container killed; output file never written |
| 3 | **Artifact in wrong location** | 3 | Agent writes to `/home/agent/omega/` or leaves compiled binaries; verifier checks `/app/` |
| 4 | **Wrong numerical answer** | 5 | Agent finishes and self-verifies; verifier disagrees on value or frame numbers |
| 5 | **Verifier infrastructure failure** | 3–4 | `uvx`/`uv` install fails inside container (DNS/network); 1 confirmed false negative |
| 6 | **Near-miss / edge case** | 3 | Passes most verifier tests; misses one specific case |
| 7 | **Turn exhaustion** | 3 | 50-turn limit hit; output file never written |

Shape 1 is a Sonnet 4.6-specific scaffolding issue: the 64k output budget covers
thinking + text + tool arguments in generation order. On hard planning tasks Claude
can exhaust the full budget on thinking alone, leaving zero tokens for any output.
The recovery loop (Fix C below) addresses this.

## Roadmap

### 1–5 — **DONE**

- **1** Prompt hypothesis validated (2 tasks flipped: circuit-fibsqrt, overfull-hbox).
- **2** winning-avg-corewars timeout mismatch fixed (removed hard-coded 1800 s cap).
- **3** Deadline injection added to omega_agent.py (1 task flipped: write-compressor).
- **4** Fresh 12-task run: 9/12 (75 %). Leaderboard metric established at 67 %.
- **5a** Failure-mode investigation complete. Seven shapes found. See `docs/failure-analysis.md`.

### 6 — Implement and validate cheap fixes — **in progress**

Five fixes, ordered by yield × cost. Expected total: **≥ 3 tasks flip** → proceed to item 7.

#### Fix A — Delete compiled artifacts before submitting — **pending**

**Affected tasks:** `polyglot-c-py`, `polyglot-rust-c` (Shape 3)

Both tasks fail because the agent compiles to verify, then leaves test binaries
alongside the source. The verifier expects exactly one file.

**Implementation.** Add to the Task-completion section of the system prompt
(`.omega/system-prompt-append.md`):
> After testing compiled code, remove all compiled binaries, object files, and
> build artifacts from the submission directory. Before declaring done, list the
> submission directory to confirm only the expected output files remain.

**Expected yield:** 2 tasks flip (polyglot-c-py, polyglot-rust-c).

---

#### Fix B — Verify output at the exact specified path — **pending**

**Affected tasks:** `extract-elf` (confirmed), `sqlite-with-gcov` (partial) (Shape 3)

Agent writes to `/home/agent/omega/` instead of `/app/`, self-verifies against
the wrong location, and declares success.

**Implementation.** Extend Task-completion section:
> When the task specifies an output path (e.g. `/app/extract.js`), confirm
> the file exists at that exact absolute path — not relative to the working
> directory — before declaring done.

**Expected yield:** 1 task firm (`extract-elf`).

---

#### Fix C — Recovery loop after thinking-budget exhaustion — **DONE** (2026-05-xx)

**Affected tasks:** `winning-avg-corewars`, `dna-assembly`, possibly
`feal-linear-cryptanalysis` (Shape 1)

When `stop_reason=max_tokens` arrives with no text and no tool_use blocks, the
agent now injects a synthetic user message asking the model to write a short plan
and call a tool, then retries. Capped at 1 recovery per turn. Implemented in
`src/agent.ts`; two tests added to `src/agent-rate-limit.test.ts`.

**Expected yield:** 1–2 tasks.

---

#### Fix D — Skip the distribution-search false negative — **pending**

**Root cause.** Verifier container can't reach `astral.sh`; `uvx` never installs;
reward = 0.0 for infrastructure reasons, not agent failure.

**Implementation.**
```bash
echo "<trial-uuid>" >> benchmark-results/.skip-trials
```
Requires Fix B to also land before re-run registers correctly (agent wrote to
wrong path too).

**Expected yield:** 1 task eligible for re-run.

---

#### Fix E — Pre-completion checklist (subsumes A + B) — **pending**

Replace the individual A/B prompt additions with a single structured checklist
in the Task-completion section:

> **Before declaring done:**
> 1. List all files in the output/submission directory and confirm only the
>    required outputs are present (no compiled binaries, no temporaries).
> 2. Confirm each output file exists at the exact absolute path specified in
>    the task description.
> 3. Run the code or command that produces the output one final time in a
>    clean state to confirm reproducibility.

**Expected yield:** Prevents future Shape 3 recurrences across any task.

---

#### Validation plan

After landing Fixes A/B/D/E:

1. Re-run `polyglot-c-py`, `polyglot-rust-c`, `extract-elf` — expect all 3 to flip.
2. Re-run `distribution-search` — expect to flip (Fix B must also be applied).
3. Re-run `winning-avg-corewars`, `dna-assembly` — expect 1–2 to flip.

**Pass criterion:** ≥ 3 tasks flip → proceed to item 7.

### 7 — Opus 4.7 run — **planned**

Full 76 tasks with `claude-opus-4-7` at `xhigh` effort. Compare against Sonnet
4.6 (67 %) to isolate scaffolding contribution from model contribution.

Shape 1 (thinking-budget exhaustion) largely disappears on Opus (128k ceiling).
Shape 2 may partially improve (Opus more capable per turn, fewer turns needed).
Shapes 3, 5, 6 should be resolved by item 6.

Estimated budget: ≈ $100–150 at Opus 4.7 pricing ($5/$25 per MTok).

**Reference baseline.** Claude Code + Sonnet 4.5 ≈ 50 % on TB 2.0. Omega +
Sonnet 4.6 at 67 % clears that by ~17 pp.

### 8 — SWE-Bench Verified — **later**

Same Harbor wrapper, one flag change. 500 tasks, ~$300 budget. Only after item 7.

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
