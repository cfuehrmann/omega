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
- **Failure analysis:** `docs/failure-analysis.md` (roadmap item 5a — complete)

### Failure-shape taxonomy (post item 5a analysis)

Seven distinct failure shapes found across the 29 failing tasks / 40 failing trials.
The "capability ceiling" label used during the live run conflated three structurally
different classes; the revised taxonomy surfaces separately-fixable sub-problems.

| # | Shape | Tasks (n) | Distinguishing signal |
|---|---|---|---|
| 1 | **Thinking-budget exhaustion** | 5 | `stop_reason: max_tokens`, `output_tokens: 64000`, `text: ""`, no tool calls |
| 2 | **Wall-clock timeout** | 10 | Agent mid-loop when container killed; output file never written |
| 3 | **Artifact in wrong location** | 3 | Agent writes to `/home/agent/omega/` or leaves compiled binaries; verifier checks `/app/` |
| 4 | **Wrong numerical answer** | 5 | Agent finishes and self-verifies; verifier disagrees on value or frame numbers |
| 5 | **Verifier infrastructure failure** | 3–4 | `uvx`/`uv` install fails inside container (DNS/network); 1 confirmed false negative |
| 6 | **Near-miss / edge case** | 3 | Passes most verifier tests; misses one specific case (asyncio cancel, git object store, matrix size) |
| 7 | **Turn exhaustion** | 3 | 50-turn limit hit or interrupted; output file never written |

#### Shape 1 — Thinking-budget exhaustion in detail

This is the most surprising shape and worth understanding precisely because it is
**a Sonnet 4.6-specific scaffolding issue**, not a model capability limit.

Sonnet 4.6's hard API maximum is **64 000 output tokens per call**. This single
budget covers thinking tokens + response text + tool call arguments, in that
generation order. In adaptive thinking mode, Claude evaluates the task's difficulty
and allocates thinking tokens accordingly. On hard planning tasks it can
consume the entire 64 000-token budget for internal reasoning alone, leaving zero
tokens for any text or tool call.

Every Shape 1 failure in our logs is identical:

```
output_tokens: 64000   thinking_chars: 76k–93k   text: ""   tool_use_blocks: []
```

| Task | Thinking chars at cutoff |
|---|---|
| circuit-fibsqrt (failing trial) | 76 696 |
| dna-assembly | 76 119 |
| feal-linear-cryptanalysis | 82 832 |
| regex-chess (both trials) | 87 329 / 89 113 |
| winning-avg-corewars | 93 276 |

The agent code handles `(toolUseBlocks.length > 0 && max_tokens)` — the
truncated-tool-call case — by injecting synthetic error results so the context stays
well-formed. But `(toolUseBlocks.length === 0 && max_tokens)` — the
pure-thinking-exhaustion case — falls through silently: `continueLoop` is never set
to `true`, the turn ends, and the verifier finds an empty `/app` directory.

**64k is a hard API ceiling.** We already send the maximum; it cannot be raised on
the synchronous Messages API for Sonnet 4.6. Opus 4.6 and Opus 4.7 both have 128k —
Shape 1 would largely disappear on an Opus run. The `output-300k-2026-03-24` beta
header raises the limit to 300k but only on the Batches API (async), not on
real-time calls, so it does not help here. `display: "omitted"` is a latency
optimisation only; it does not change the token ceiling.

**circuit-fibsqrt correction.** The failing trial for circuit-fibsqrt hits max_tokens
(Shape 1), not a near-miss edge case as initially classified. The verifier shows some
tests failing because the agent wrote a partial implementation before the cutoff, not
because it nearly-but-not-quite solved the problem. The passing trial (after the
prompt fix) ran 14 turns without hitting max_tokens and passes all 32 tests.

#### Shape 5 — Confirmed false negative

`distribution-search` is a confirmed false negative: the agent found the correct
mathematical solution (KL divergence = 10.000, error = 1.74×10⁻¹²), but the verifier
container couldn't install `uvx` (`curl: (7) Failed to connect to astral.sh`) and so
never checked the output. `reward = 0.0` for infrastructure reasons, not agent failure.
Two other tasks (`gpt2-codegolf`, `raman-fitting`) also have verifier DNS failures, but
those agents timed out independently, so the correct outcome is still `reward = 0.0`.

## Roadmap

Ordered. First non-DONE item is the next thing to do.

### 1 — Validate the prompt hypothesis — **DONE** (2026-04-24)

Rephrased the design-discipline clause; added an explicit Task-completion section
requiring verification before declaring done; moved Carsten-specific habits to
`.omega/system-prompt-append.md`. **Result: 2/7 tasks flipped** (circuit-fibsqrt ✓,
overfull-hbox ✓). Confirmed the causal mechanism for early-stopping failures; revealed
the remaining 5 had deeper issues (wrong approach, not premature exit).

### 2 — winning-avg-corewars timeout-mismatch — **DONE** (2026-04-24)

Hard-coded `RUN_TIMEOUT_SEC = 1800` in `omega_agent.py` fired at 30 min on a task
with a 3600-second deadline. Removed; Harbor's per-task `agent_timeout_sec` is now
the sole controlling mechanism.

### 3 — Deadline injection — **DONE** (2026-04-24)

`_get_agent_timeout_sec()` in `omega_agent.py` reads `config.json` and the task's
`task.toml`, then prepends `"Time budget: N seconds (M minutes).\n\n"` to the
instruction. **Result: 1/4 tasks flipped** (`write-compressor` ✓ — committed a working
solution before the clock ran out). `gcode-to-text` and `largest-eigenval` are
genuinely hard at 45 and 21 turns; they need more than a deadline prompt.

### 4 — Fresh 12-task exploratory run — **DONE** (2026-04-24)

9/12 passed (75 %). Confirmed interventions generalise to unseen tasks and the
re-run cluster is harder than average. New leaderboard metric established: **67 %**.

### 5 — Failure-mode investigation — **DONE (5a)** (2026-04-25)

**5a (read-and-cluster pass) complete.** Full output: `docs/failure-analysis.md`.
Seven shapes identified (see taxonomy above). Key findings:
- Shape 1 (thinking-budget exhaustion) is new and explains 5 tasks previously
  mislabelled "capability ceiling".
- Shape 3 (artifact in wrong location) explains 3 tasks trivially fixable with a
  prompt addition.
- Shape 5 surfaces 1 confirmed false negative (distribution-search).
- Rough ceiling with all cheap fixes applied: **+5 tasks** → ~74 % on Sonnet 4.6.

**5b (per-trial deep-dive) — optional.** Seven trials flagged for deeper inspection if
needed before the Opus run: `distribution-search`, `largest-eigenval`,
`count-dataset-tokens`, `sanitize-git-repo`, `cancel-async-tasks`, `regex-chess`,
`protein-assembly`. Skip if cheap-fix validation (item 6) gives a clear-enough signal.

### 6 — Implement and validate cheap fixes — **next**

Five fixes identified in `docs/failure-analysis.md`, ordered by yield × cost:

#### Fix A — Delete compiled artifacts before submitting (Shapes 3, 6)

**Affected tasks:** `polyglot-c-py`, `polyglot-rust-c`
**Root cause.** Both polyglot tasks fail because the agent compiles the source to
verify it, then leaves the test binaries (`cmain`, `rmain`) alongside the source file.
The verifier expects exactly one file in the submission directory.

**Implementation.** Add to the Task-completion section of the system prompt:
> After testing compiled code, remove all compiled binaries, object files, and
> build artifacts from the submission directory. Before declaring done, list the
> submission directory to confirm only the expected output files remain.

**Expected yield:** 2 tasks flip immediately (polyglot-c-py, polyglot-rust-c).

---

#### Fix B — Verify output at the exact specified path (Shape 3)

**Affected tasks:** `extract-elf` (confirmed), `sqlite-with-gcov` (partial)
**Root cause.** The agent writes output to its working directory
(`/home/agent/omega/`) rather than `/app/`. It self-verifies against the wrong
location and declares success; the verifier checks `/app/` and finds nothing.

**Implementation.** Extend the Task-completion section:
> When the task specifies an output path (e.g. `/app/extract.js`), confirm
> the file exists at that exact absolute path — not relative to the working
> directory — before declaring done.

**Expected yield:** 1 task firm (`extract-elf`). `sqlite-with-gcov` requires the
agent to understand where the verifier will search for `.gcda` files, which is
task-specific; may need a second look.

---

#### Fix C — Recovery loop after thinking-budget exhaustion (Shape 1)

**Affected tasks:** `winning-avg-corewars`, `dna-assembly`, and possibly
`feal-linear-cryptanalysis`; `regex-chess` unlikely (may be computationally infeasible).
**Root cause.** When `stop_reason === "max_tokens"` with no tool_use blocks and no
text, `agent.ts` silently ends the turn. The model had useful partial context and a
full plan, but no chance to act on it.

**Implementation.** In `agent.ts`, add a branch for the pure-thinking-exhaustion case:

```typescript
if (
  toolUseBlocks.length === 0 &&
  assembledText.length === 0 &&
  response.stop_reason === "max_tokens"
) {
  // Increment a per-turn recovery counter; cap at 2 to prevent infinite loops.
  // Inject a corrective user message and set continueLoop = true.
}
```

Injected message:
> Your extended thinking ran over the 64 000-token output limit and produced no
> action. Please continue — write a short plan (≤ 5 lines) and immediately call
> a tool. Do not re-explore the problem from scratch.

**Expected yield:** 1–2 tasks. `winning-avg-corewars` had completed 20 tool calls of
research before the cutoff; with recovery it likely writes a warrior. `regex-chess` is
probably unreachable on Sonnet 4.6 regardless. **Note:** this shape disappears on
Opus 4.7 (128k ceiling), so Fix C matters only for the Sonnet score.

---

#### Fix D — Skip the distribution-search false negative (Shape 5)

**Root cause.** Verifier container can't reach `astral.sh`; `uvx` never installs;
the verifier never checks the agent's (correct) output. `reward = 0.0` for
infrastructure reasons.

**Implementation.**
```bash
# Get the trial UUID, then:
echo "<trial-uuid>" >> benchmark-results/.skip-trials
```
Also note in `docs/results.md` that the 67 % rate may include 1 confirmed verifier-infra
false negative.

**Expected yield:** 1 task eligible for re-run. Distribution-search also has the
wrong-path bug (agent wrote to `/home/agent/omega/` rather than `/app/`) — Fix B must
also land before the re-run registers correctly.

---

#### Fix E — Pre-completion checklist (general, subsumes A + B)

Combine Fixes A and B into a single structured verification step at the end of the
Task-completion section. Makes the intent explicit and gives the model a concrete
sequence to follow before it closes out any task:

> **Before declaring done:**
> 1. List all files in the output/submission directory and confirm only the
>    required outputs are present (no compiled binaries, no temporaries).
> 2. Confirm each output file exists at the exact absolute path specified in
>    the task description.
> 3. Run the code or command that produces the output one final time in a clean
>    state to confirm reproducibility.

**Expected yield:** Prevents future recurrences of Shape 3 failures across any task,
not just the currently known ones.

---

#### Validation plan

After landing Fixes A/B/C/D/E:

1. Re-run `polyglot-c-py`, `polyglot-rust-c`, `extract-elf` — expect all 3 to flip.
2. Re-run `distribution-search` — expect to flip (if Fix B also applied).
3. Re-run `winning-avg-corewars`, `dna-assembly` — expect 1–2 to flip.

**Pass criterion for item 6:** ≥ 3 tasks flip to pass. If ≥ 3 flip, proceed to item 7;
if < 3 flip, investigate before spending the Opus budget.

### 7 — Opus 4.7 run — **planned**

Run the full 76 tasks with `claude-opus-4-7` at `xhigh` effort (Anthropic's
recommended starting point for agentic coding). Compare pass rate against Sonnet 4.6
(67 %) to isolate the scaffolding contribution from the model contribution.

**Expected outcomes of the Opus run:**
- Shape 1 (thinking-budget exhaustion) disappears entirely (128k ceiling).
- Shape 2 (wall-clock timeout) partially improves (Opus is more capable per turn,
  may finish in fewer turns).
- Shapes 3, 5, 6 should already be addressed by item 6; if not, they will surface
  as persistent scaffolding gaps.

Estimated budget: ≈ $100–150 at Opus 4.7 pricing ($5 / $25 per MTok).

**Reference baseline.** Claude Code + Sonnet 4.5 ≈ 50 % on TB 2.0 (tbench.ai
leaderboard). Omega + Sonnet 4.6 at 67 % clears that bar by ~17 pp. To isolate
the scaffolding premium: compare Omega + Opus 4.7 against published Opus numbers.

### 8 — SWE-Bench Verified — **later**

Same Harbor wrapper, one flag change. 500 tasks, plan a few hundred dollars of API
budget. Only after item 7.

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

Results land in `jobs/<timestamp>/` (or `jobs/<job-name>/` with `--job-name`). Each
trial directory contains `agent/events.jsonl`, `agent/context.jsonl`, Harbor's
`result.json`, and `trial.log`.

**harbor buffers all stdout until the run completes.** The log file is written in
one shot at the end. From an Omega session use `run_background` + a single
`wait_for_output` with `timeoutMs` sized to the batch (30 min per task upper bound)
and pattern `"Mean:|Results written to|Exception"`.

### Skipping contaminated trials

`benchmark-results/.skip-trials` — plain list of trial UUIDs the ingest script
permanently ignores. Populate it for trials that fail for reasons unrelated to agent
behaviour (API quota hit, container setup race, verifier infrastructure failure).
Legitimate fail/timeout trials stay in the results.

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

### Terminology

| Term | Meaning |
|---|---|
| **Terminal-Bench (TB)** | The benchmark — tasks + tests. We use version 2.0. |
| **Harbor** | The harness that runs containerised agent benchmarks. |
| **Oracle** | Built-in Harbor agent that replays each task's `solution.sh`. 76/89 TB 2.0 tasks pass the oracle; 13 need GPU/huge downloads and are excluded. See `benchmark-results/oracle-tasks.json`. |

### Model choice and cost

Omega is Anthropic-only: `claude-sonnet-4-6`, `claude-opus-4-6`, `claude-opus-4-7`.
Pricing: Sonnet 4.6 $3 / $15 per MTok (input / output); Opus 4.7 $5 / $25.

## Archive

Historical notes kept for reference; no longer action items.

**Oracle sweep** (2026-04-23). 76/89 TB 2.0 tasks pass the oracle. List in
`benchmark-results/oracle-tasks.json`.

**Omega CLI** (`src/cli.ts`, tagged `v0.1.0`). Headless entry point: `--instruction`,
`--model`, `--effort`, `--session-dir`, `--max-turns`. LLM text → stdout, structured
logs → stderr. Exit 0 on `turn_end`, 1 on interrupt/error.

**`omega_agent.py`** (Harbor wrapper, repo root). Bring-up fixes: `unzip` added to
apt deps, `--agent-import-path` without `./` prefix, `RUN_TIMEOUT_SEC = 1800` removed
(Harbor's per-task timeout is now the sole mechanism), deadline injection added.

**Omega bugs surfaced during bring-up.** `wait_for_output` used `String.includes`
instead of `RegExp`; `wait_for_output` ignored the abort signal. Both fixed.

**Phase A prompt-validation run** (2026-04-24, job `phaseA-prompt-validation`).
Rephrased design-discipline clause; added Task-completion section; moved
Carsten-specific habits to `.omega/system-prompt-append.md`. 2/7 tasks flipped
(circuit-fibsqrt, overfull-hbox). Phase A batch 1 (15 tasks, 7 pass / 8 fail) run
before the prompt fix; 2 infra issues handled (monthly quota hit, `curl | bash` bun
install failure). See commit `f4320cd`.

**`OMEGA_HEADLESS` prompt-gating idea, rejected.** Would make the agent behave
differently in benchmark runs. Rejected as "teaching to the test."

**Phase B deadline-validation run** (2026-04-24, job `phaseB-deadline-validation`).
1/4 tasks flipped (`write-compressor`). Confirmed deadline injection works when the
agent commits early; `gcode-to-text` and `largest-eigenval` are genuinely hard.

**Phase C fresh-12 run** (2026-04-24, job `phaseC-fresh-12`). 9/12 passed (75 %).
Confirmed interventions generalise. Leaderboard metric established at 67 %.

**Phase D full-76 run** (2026-04-24, jobs `phaseD-remaining-42` + `phaseD-infra-retry`).
51/76 = 67 %. $24.67 cumulative. 5 Phase D trials failed on first attempt due to
transient network errors; retry recovered 2 (`merge-diff-arc-agi-task`, `chess-best-move`).
Full per-task breakdown: `docs/results.md`.

**Phase A — failure-mode investigation 5a** (2026-04-25). Full output in
`docs/failure-analysis.md`. Seven failure shapes found; five cheap fixes ranked.
Revised taxonomy corrects the "capability ceiling" label that conflated thinking-budget
exhaustion, verifier infra failures, and genuine capability limits.

## References

- `benchmark-results/results.jsonl` — accumulated trial data
- `benchmark-results/oracle-tasks.json` — per-task oracle status
- `benchmark-results/.skip-trials` — trial UUIDs permanently ignored by ingest
- `docs/results.md` — per-task table and category breakdown
- `docs/failure-analysis.md` — failure-shape taxonomy and cheap-fix plan
- `jobs/<phase>/<task>/agent/{events,context}.jsonl` — raw session per trial
- `omega_agent.py` — Harbor wrapper
- `scripts/bench-ingest.ts`, `scripts/bench-summary.ts` — results tooling
