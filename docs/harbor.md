# Harbor Benchmarking

Running Omega against Terminal-Bench 2.0 to produce apples-to-apples numbers
against Claude Code, Terminus-2, Mini-SWE-Agent, and OpenHands on the same model.

**Entry point:** [Roadmap](#roadmap) — the first non-DONE item is always the next thing to do.

## Status

- **Model under evaluation:** `claude-sonnet-4-6`
- **Tasks attempted:** 76 / 76 oracle-passing TB 2.0 tasks (complete)
- **Pass rate:** 54 / 76 = **71 %** (leaderboard-comparable number)
- **API spend to date:** ≈ $28.30
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
The recovery loop (Fix C) addresses this.

## Roadmap

### 1–5 — **DONE**

- **1** Prompt hypothesis validated (2 tasks flipped: circuit-fibsqrt, overfull-hbox).
- **2** winning-avg-corewars timeout mismatch fixed (removed hard-coded 1800 s cap).
- **3** Deadline injection added to omega_agent.py (1 task flipped: write-compressor).
- **4** Fresh 12-task run: 9/12 (75 %). Leaderboard metric established at 67 %.
- **5a** Failure-mode investigation complete. Seven shapes found. See `docs/failure-analysis.md`.

### 6 — Implement and validate cheap fixes — **in progress**

#### Fix C — Recovery loop after thinking-budget exhaustion — **DONE**

When `stop_reason=max_tokens` arrives with no text and no tool_use blocks, the
agent injects a synthetic user message asking the model to write a short plan
and call a tool, then retries. Capped at 1 recovery per turn. Implemented in
`src/agent.ts`; two tests in `src/agent-rate-limit.test.ts`.

**Validated yield:** 1 task (`winning-avg-corewars` flipped 0→1). `dna-assembly`
still times out at 1800 s — Fix C was not sufficient for that task.

---

#### Fix E — Submission-state verification — **DONE** (prompt landed; yield below expectation)

**Affected tasks:** `polyglot-c-py`, `polyglot-rust-c`, `extract-elf` (Shape 3)

**Root cause (two variants, same underlying gap):**

- *Wrong directory:* The agent's working directory is `/home/agent/omega/`. When
  a task says "write to `/app/extract.js`", the agent writes there, tests it
  successfully against that path — then declares done. The verifier checks `/app/`
  and finds nothing. Confirmed for `extract-elf`; likely for `distribution-search`.

- *Dirty submission directory:* The agent compiles its polyglot source to verify
  it, which is correct. But it never cleans up the binaries. The verifier does an
  exact directory listing and fails — `['cmain', 'main.rs', 'rmain']` instead of
  `['main.rs']`.

**Why not a rule-based fix.** A rule like "delete compiled binaries" would be
benchmark overfitting and could be harmful in general use. The real principle is
simpler and more agnostic: **the task description specifies the required final
state of the submission; the agent should verify against that spec, not against
what it happens to have produced.** The model already has the task description;
it just needs to be prompted to re-read it at the end and check.

**Scope note.** This only applies to the task's designated submission directory
(typically `/app/` or a subdirectory of it). The agent's own session artifacts —
`events.jsonl`, `context.jsonl`, working files in `/home/agent/omega/` — are in
a different location and are never in scope.

**Implementation.** Add to the Task-completion section of `.omega/system-prompt-append.md`
(which is injected into the Harbor agent's system prompt via `omega_agent.py`):

> **Before declaring done, verify the submission state:**
> Re-read the task description's output requirements. Check that the submission
> directory contains exactly what the task asks for — no more, no less. In
> particular: if the task names a specific output path, confirm the file exists
> at that exact absolute path; if the task specifies which files should be
> present, list the directory and compare.

This is deliberately judgment-based. The model reads the task spec and decides
what "correct final state" means for that task — no hardcoded rules about file
types or directories.

**Validated yield:** 0 of the 3 primary targets flipped:

| Task | Outcome | Root cause |
|---|---|---|
| `polyglot-c-py` | `NonZeroAgentExitCodeError` | bun.sh DNS failure in container (infra, not Fix E) |
| `polyglot-rust-c` | reward=0.0 | Agent compiled binaries and left `cmain`/`rmain` in `/app/polyglot/`; prompt not strong enough to trigger cleanup |
| `extract-elf` | reward=0.0 | Agent wrote `extract.js` to `/home/agent/omega/` instead of `/app/`; task description doesn't name the destination explicitly so the model didn't detect the mismatch |

Fix E also contributed to `distribution-search` flipping (see Fix D below).

---

#### Fix D — distribution-search false negative — **RESOLVED**

Re-run with Fix E landed: `distribution-search` flipped 0→1. The network
failure was transient. Fix E helped the agent write to the correct path.

---

#### Validation run — fix-e-validation — **DONE**

Run job `fix-e-validation` (2026-04-25). Results:

| Task | Before | After | Flip? |
|---|---|---|---|
| `winning-avg-corewars` | 0.0 | 1.0 | ✅ Fix C |
| `distribution-search` | 0.0 | 1.0 | ✅ Fix D+E |
| `extract-elf` | 0.0 | 0.0 | ✗ |
| `polyglot-rust-c` | 0.0 | 0.0 | ✗ |
| `polyglot-c-py` | 0.0 | n/a | ✗ (infra) |
| `dna-assembly` | 0.0 | 0.0 | ✗ (timeout) |

**2 tasks flipped.** Pass criterion (≥ 3) not met. New leaderboard metric: **53/76 = 70 %**.

Remaining Shape 3 failures carry forward to Fix F below.

---

#### Fix F — CWD fix in omega_agent.py — **DONE**

**Root cause.** `omega_agent.py` ran Omega with `cd /home/agent/omega`, making
`process.cwd()` the install directory. This caused two problems simultaneously:

1. *Wrong working directory:* The system prompt told the agent its CWD was
   `/home/agent/omega`, so files written to relative or CWD-relative paths
   landed there instead of `/app/`. This is the structural cause of all Shape 3
   wrong-directory failures.
2. *Polluted system prompt:* `.omega/system-prompt-append.md` was loaded from the
   cloned repo, injecting Omega development conventions (bun test, just gate,
   SolidJS, branch policy) into the context of every benchmark task — noise with
   no relevance to the task.

**Fix.** `omega_agent.py` now runs `cd /app && bun run /home/agent/omega/src/cli.ts`.
With `process.cwd()` = `/app/`: the system prompt names the right directory, no
`.omega/system-prompt-append.md` exists there (so nothing is appended), and the
agent defaults to writing files in the task directory.

**Additional prompt improvements landed simultaneously (v0.1.2):**
- `## Bug fixes` (red-green testing) moved from `system-prompt-append.md` to
  `core.ts` — it is universal good practice, not Omega-project-specific.
- `## Task completion` in `core.ts` extended: conditional submission-state
  verification + relative-path-assumptions warning.

**Smoke test result (job: fix-f-smoke-test, 2026-04-25):**

| Task | Before | After | Flip? | Root cause |
|---|---|---|---|---|
| `extract-elf` | 0.0 | 1.0 | ✅ | CWD fix resolved wrong-directory issue |
| `polyglot-rust-c` | 0.0 | 0.0 | ✗ | Agent writes to `/app/polyglot/` correctly now, but still leaves `cmain`/`rmain` after compilation; verifier asserts `['main.rs']` only |
| `polyglot-c-py` | 0.0 | 0.0 | ✗ | DNS failure in verifier container (uv install from GitHub fails) — infra, not agent |

**1 task flipped.** New leaderboard metric: **54/76 = 71 %**.

`polyglot-rust-c` still blocked by binary cleanup: the submission-state check
prompt is not strong enough on Sonnet 4.6 to consistently trigger cleanup of
compiled artifacts before submission. This may resolve under Opus 4.7 (item 7)
given its stronger instruction-following, or may need a dedicated Fix G targeting
the binary-cleanup pattern explicitly.

`polyglot-c-py` is a persistent infra flake (bun.sh DNS and GitHub DNS both
transient-fail in some containers). Likely resolves with a retry; not an agent issue.

---

---

**Item 6 complete.** Fix F validated (extract-elf flipped). Proceeding to item 7.

### 7 — Opus 4.7 run — **in progress** (job: `opus-4-7-xhigh-76`)

Full 76 tasks with `claude-opus-4-7` at `xhigh` effort. Compare against Sonnet
4.6 (71 %) to isolate scaffolding contribution from model contribution.

Shape 1 (thinking-budget exhaustion) largely disappears on Opus (128k ceiling).
Shape 2 may partially improve (Opus more capable per turn, fewer turns needed).
Shape 3 should be resolved by Fix F (CWD fix).

Estimated budget: ≈ $100–150 at Opus 4.7 pricing ($5/$25 per MTok).

**Reference baseline.** Claude Code + Sonnet 4.5 ≈ 50 % on TB 2.0. Omega +
Sonnet 4.6 at 70 % clears that by ~20 pp.

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
