# Omega — Terminal-Bench 2.0

Terminal-Bench 2.0 is a 89-task containerised coding benchmark run via [Harbor](https://github.com/the-harbor-project/harbor).

## Results

| Sweep | Model | Effort | Substrate | Score | Notes |
|---|---|---|---|---|---|
| [`S3`](#sweep-s3) | `claude-opus-4-7` | high | `standard` | **62 / 89 = 69.7 %** | omega v0.1.2, parallel. xhigh effort for tasks ≥ 900 s budget. |
| [`S2`](#sweep-s2) | `claude-sonnet-4-6` | medium | `repl-centric` | **52 / 89 = 58.4 %** | omega v0.1.15 + v0.1.16 correction. As-run: 50/89; 2 tasks re-run after infra bug fixes. [Session I + error analysis](../docs/repl-and-substrates.html#session-i-error-analysis). |
| [`S1`](#sweep-s1) | `claude-sonnet-4-6` | medium | `standard` | **53 / 89 = 59.6 %** | omega ≤ v0.1.2, parallel, April 2026. **Score not reliably reconstructible** — see S1 registry entry. |
| — | Claude Opus 4.7 / Adaptive (official) | adaptive | n/a | **69.4 %** | tbench.ai leaderboard |

Sweep IDs (`S1`–`S3`) resolve to entries in [`bench/results/sweeps.json`](results/sweeps.json), which records every component job directory, omega version, date window, and the selection rule for picking the canonical per-task result.

Omega + Opus 4.7 at 69.7 % matches the official leaderboard's Opus 4.7 Adaptive entry (69.4 %) — same model, different agent harness.

The Sonnet-medium `repl-centric` vs `standard` comparison is **−1.1 pp on n = 89 single trials** (S2: 52/89 vs S1: 53/89), which is well within plausible sampling noise. Additionally, S1 is not reliably reconstructible (see registry), so this gap may not be meaningful. An n ≥ 3 replication on a representative subset is what Phase 2.2.2 still needs to distinguish substrate signal from variance.

Run `python bench/scripts/bench-summary.py` for a live breakdown from `bench/results/results.jsonl` (regenerate this script in Python if it does not exist yet).

---

## Sweep registry

`bench/results/sweeps.json` is the authoritative record of every logical sweep. It is the single place that answers:

- Which job directories belong to this sweep?
- Which omega version ran each component?
- When did each component run (start / end timestamps)?
- When a sweep has corrections or gap-fill re-runs, which task uses which component's result?

### Convention

Each sweep entry carries a stable `sweep_id` (`S1`, `S2`, …), a `selection_rule`, and an ordered `components` list. The two supported selection rules are:

| Rule | Meaning |
|---|---|
| `latest_per_task` | For each canonical task, use the trial with the latest `started_at` across all component job dirs. |
| `correction_override` | Components are ordered (seq 1, 2, …). For each task, use the highest-seq component that has a result for it. Later components act as corrections to earlier ones. |

### Registering a new sweep

After any sweep (or correction re-run) completes:

1. Add an entry to `sweeps.json` with a new `sweep_id`.
2. List every component job directory glob, its omega version, and its date range.
3. State the `selection_rule` and a `reconstruction_note` summarising how to get the canonical score.
4. If the sweep corrects a previous sweep (same model/effort/substrate), add the new components to the existing sweep entry (increment `seq`) rather than creating a new top-level sweep.
5. Update the results table above with the canonical score and a link to the sweep ID anchor.

### Sweep summaries

<a id="sweep-s1"></a>
**S1 — Standard Sonnet medium, parallel (April 2026), `reconstructible: false`**  
Omega ≤ v0.1.2, run in parallel across ~10 job directories (`phaseA-batch1`, `phaseD-remaining-42`, `sonnet-missing-13`, and others). Score 53/89 was recorded at the time but exact trial-to-task mapping is not preserved; `latest_per_task` over all known components yields a different number. ~4 tasks hit setup-timeouts due to parallel Docker resource contention. **A fresh n = 1 replication is required before comparing S1 against S2.**

<a id="sweep-s2"></a>
**S2 — Sequential repl-centric Sonnet medium (May 2026), `reconstructible: true`**  
Omega v0.1.15 main sweep (`v0115-seq-*-sonnet-medium-repl-centric`, all 89 tasks, 2026-05-30 – 2026-05-31) scored **50/89** as-run. Two tasks failed with omega-cli infra bugs (not agent failures): `fix-code-vulnerability` and `pytorch-model-recovery`. Both re-run with v0.1.16 after the bugs were fixed, both scored 1.000. Canonical score: **52/89**. Selection rule: `correction_override` — for `fix-code-vulnerability` use `v0116-fix-code-vulnerability-*`; for `pytorch-model-recovery` use `v0116-pytorch-model-recovery-*`; for all other tasks use the v0.1.15 job dir.

<a id="sweep-s3"></a>
**S3 — Standard Opus high, parallel (April 2026), `reconstructible: true`**  
Omega v0.1.2, four job directories (`opus-4-7-xhigh-76`, `opus-4-7-high-retry`, `opus-4-7-fixg-retry`, `opus-missing-13`), all 89 tasks covered. Selection rule: `latest_per_task`. Canonical score: **62/89**.

## Running benchmarks

**Always run Harbor from the `bench/` directory** — Harbor writes `jobs/` relative to CWD, so running from `bench/` is what keeps job output inside `bench/jobs/`.

> **Harbor version requirement.** Use harbor ≥ **v0.9.0**.  Older clients
> (≤ v0.8.0) issue a task-version-resolution query that exceeds the Supabase
> row cap and fails with a server-side `statement_timeout` (Postgres code
> `57014`).  The fix landed in harbor v0.9.0 (PRs #1719 paginated queries +
> #1736 RPC-based resolution).  Upgrade via `uv tool upgrade harbor`.

```bash
cd bench

# one specific task
harbor run -d terminal-bench@2.0 \
  --agent-import-path omega_agent:OmegaRustAgent \
  -m anthropic/claude-sonnet-4-6 \
  --ae ANTHROPIC_API_KEY=$ANTHROPIC_API_KEY \
  -t terminal-bench/fix-git -n 1

# explicit list of tasks — use -i (repeatable) for multi-task runs
harbor run -d terminal-bench@2.0 \
  --agent-import-path omega_agent:OmegaRustAgent \
  -m anthropic/claude-sonnet-4-6 \
  --ae ANTHROPIC_API_KEY=$ANTHROPIC_API_KEY \
  -i taskA -i taskB \
  --job-name my-run
```

Results land in `bench/jobs/<job-name>/`. Each trial directory contains
`agent/events.jsonl`, `agent/context.jsonl`, Harbor's `result.json`, and `trial.log`.

> **Flag note — `-t` (singular) vs `-i` / `--include-task-name` (repeatable):**
> `-t <name>` sets the **one** task for a single-task run and cannot be repeated.
> For multi-task runs use `-i` (or `--include-task-name`) — it is repeatable and
> supports glob patterns: `-i fix-git -i crack-7z-hash -i 'build-*'`.
> The sequential sweep wrapper (`run_sequential_sweep.py`) always uses `-t` with one
> task per invocation — that is intentional (see Phase 2.2.2(c) rationale in
> `docs/repl-and-substrates.html`).

**Harbor buffers all stdout until the run completes.** Use `run_background` +
`wait_for_output` sized to the batch (~30 min per task upper bound), pattern
`"Mean:|Results written to|Exception"`.

> **Note on effort:** `xhigh` is too slow for tasks with ≤ 900 s budgets — extended
> thinking consumes 2–4× the token budget per call. Use `high` for tasks under ~15 min.

## Tool selection sweeps

Tool selection is exposed via `--preset` on the omega-cli binary, surfaced to
Harbor as a regular `--agent-kwarg` flag.  Three presets, defined once in
`crates/omega-tools/src/schemas.rs::PRESETS`:

| Preset | Tools | Use case |
|---|---|---|
| `standard` (default) | 12 — file ops + shell + web | Baseline; matches the unflagged default |
| `all` | 13 — standard plus `python_repl` | REPL alongside the rest |
| `repl-centric` | 3 — `python_repl`, `web_search`, `fetch_url` | REPL-only: shell + file I/O must happen inside Python |

The resolved selection lands in `SessionStartedEvent.tool_selection` for every
session; post-mortem can grep `events.jsonl` to recover the exact tool set a
run used.

Treatment arms:

| Arm | `--agent-kwarg` |
|---|---|
| Baseline | _(none, or `preset=standard`)_ |
| Additive REPL | `--agent-kwarg preset=all` |
| REPL-centric | `--agent-kwarg preset=repl-centric` |

Example:

```bash
harbor run -d terminal-bench@2.0 \
  --agent-import-path omega_agent:OmegaRustAgent \
  -m anthropic/claude-sonnet-4-6 \
  --ae ANTHROPIC_API_KEY=$ANTHROPIC_API_KEY \
  --agent-kwarg preset=repl-centric \
  --job-name v0114-repl-centric-sonnet-medium
```

Unknown preset names fail loudly at clap-parse time — no silent fallback.

> **Historical note.**  Earlier rounds (v0.1.8–v0.1.13) drove the same
> treatments via three `OMEGA_FEATURE_REPL*` env vars passed with `--ae`.
> Those env vars were removed in Phase 1.2 (commit `568e7af`) and replaced
> with `tool_selection`; the CLI surface above landed in Phase 2.1.  The
> mapping is: old `REPL=1` → `preset=all`; old `REPL=1 REPL_REPLACES_SHELL=1`
> → `preset=repl-centric`.  The old `REPL_REPLACES_FILEOPS=1` arm (file ops
> stripped but shell retained) is no longer a named preset — it can still be
> reached via a custom `--tools` list once the UI ships that escape hatch, or
> by editing `PRESETS` directly.

---

## What we've learned from REPL experiments (v0.1.8 – v0.1.13)

Between v0.1.8 and v0.1.13 we ran a sequence of controlled experiments on the two
locally-cached tasks (fix-git and crack-7z-hash).  Key findings:

- **Additive REPL (v0.1.8) does not engage the LLM.** When `python_repl` was offered
  alongside the full standard toolset, the LLM never called it.  Both tasks still
  passed via their normal routes.  The additive design cannot measure REPL value.

- **Tier 1 (replaces_fileops only, v0.1.9) engages REPL on file-centric tasks.**
  fix-git passed with exactly one `python_repl` call (file write via Python string
  literals).  crack-7z-hash timed out — file tools were inert for a password-cracking
  task.  N=1 each.

- **The first Tier 2 pass (v0.1.10) was a false positive** — the LLM used
  `fetch_url.postprocess` as a shell backdoor.  Closed in v0.1.11 by stripping
  `postprocess` when shell tools are gated.

- **Three rounds of `python_repl` hardening were needed** (v0.1.11–v0.1.13):
  bootstrap-on-missing (apt-get python3), per-call 60 s timeout with SIGINT/kill-group
  escalation, tee forensics + tail-bias output, and system-prompt update.

- **v0.1.13 Tier 2 (full mode) on crack-7z-hash passed in 10 min 44 s.**
  26/26 tool calls were `python_repl` (100 % REPL usage), genuine cross-call state
  (variables `proc`, `passwords`, `hash_val`, `found_password` persisting across 2–4
  calls each), Python threading for parallelism, 3 subprocess-level timeout
  recoveries with REPL state preserved.  N=1.

The **original v0.1.8 baseline** runs in `bench/jobs/v018-smoke-*` are superseded for
cross-version comparison by the v0.1.13-era results.  Use v0.1.13 Tier 2 full mode as
the reference treatment arm.  Full analysis in
`docs/repl-and-substrates.html` (Findings section).

---

### REPL benchmark plan (v0.1.14+, first sweep on the `--preset` surface)

Run both legs at the **same Omega tag** so the comparison is apples-to-apples.

```bash
cd bench

# Baseline (12 standard tools, no REPL).
harbor run -d terminal-bench@2.0 \
  --agent-import-path omega_agent:OmegaRustAgent \
  -m anthropic/claude-sonnet-4-6 \
  --ae ANTHROPIC_API_KEY=$ANTHROPIC_API_KEY \
  --job-name v0114-baseline-sonnet-medium

# Treatment: same everything, REPL-centric preset (REPL + web only).
harbor run -d terminal-bench@2.0 \
  --agent-import-path omega_agent:OmegaRustAgent \
  -m anthropic/claude-sonnet-4-6 \
  --ae ANTHROPIC_API_KEY=$ANTHROPIC_API_KEY \
  --agent-kwarg preset=repl-centric \
  --job-name v0114-repl-centric-sonnet-medium
```

The headline number: **delta between baseline and repl-centric**.

Start with sonnet-medium only (cheaper, faster to iterate; Kim et al.'s
results suggest small-model effects are where multi-agent / scaffold changes
matter most). Opus-high can be a follow-up sweep if sonnet results are
interesting in either direction.

See `docs/repl-and-substrates.html` for the broader context.

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
| `bench/results/sweeps.json` | **Sweep registry** — maps each sweep ID to its component job dirs, versions, date windows, and selection rule |
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

## Running the sequential sweep

The sequential sweep runner (`bench/run_sequential_sweep.py`) runs one Harbor
trial per task, sequentially, with Docker cleanup between trials.  This avoids
the parallel resource contention that caused install timeouts in earlier sweep
attempts (3–6 min per install solo, 10–20 min under parallel load).

**Step 1 — build the portable host binary (one-time, ~5 min):**

```bash
cd /home/carsten/omega/dev
bench/build_release_binary.sh
```

> **Why a container build?**  A host-native `cargo build` links against the
> host's glibc (currently 2.43 on this machine).  Four of the ten TB2 task
> images carry older base images (glibc < 2.38), so the host binary refuses
> to load there with a `GLIBC_2.38 not found` error.  Building inside
> `ubuntu:20.04` (glibc 2.31) produces a binary that runs on every TB2 image.
> The script mounts the repo into the container and uses a persisted
> `target-builder/` directory, so subsequent runs reuse cargo's incremental
> cache and are fast.

The script prints a one-line summary including the max GLIBC version.
Output binary lands in `target-builder/release/omega` (separate from the
native `target/` directory used by `cargo test` and dev builds).

Verify the version matches the pin in `omega_agent.py`:

```bash
./target-builder/release/omega --version
# Expected output: omega 0.1.16  (matches OMEGA_VERSION = "v0.1.16")
```

If the version doesn't match, re-run the build script and/or check `crates/omega-cli/Cargo.toml`.

**Step 2 — run the sweep:**

```bash
cd /home/carsten/omega/dev/bench

# Full 87-task sweep (~3–4 h wall-clock, ~$25 Sonnet budget)
python run_sequential_sweep.py

# Resume after interruption (skips tasks with valid result.json)
python run_sequential_sweep.py --resume

# Smoke-test on a single known-passing task
python run_sequential_sweep.py --tasks-from <(echo fix-git) --max-tasks 1

# Dry-run: print planned commands without executing
python run_sequential_sweep.py --dry-run
```

Each task runs a single Harbor trial.  After Harbor exits, the wrapper
automatically prunes stopped containers and dangling images (`docker container
prune -f && docker image prune -f`).  Operators do not need to monitor disk
usage between trials.

Results land in `bench/jobs/v0115-seq-<task>-sonnet-medium-repl-centric/`.
The aggregate summary is written to
`bench/jobs/v0115-seq-sonnet-medium-repl-centric-summary.json` and also
printed to stdout at the end of the sweep.

**Expected wall-clock:** ~3–4 h for 87 tasks (setup now < 60 s per task
because the binary is pre-built on the host; the old approach took 5+ min
per task for `cargo build`).

## Next steps

1. **Confirmation runs** — run 3–5 trials of repl-centric mode on fix-git and
   crack-7z-hash to quantify single-trial variance on the v0.1.13 result.

2. **Broader REPL sweep** — sweep repl-centric mode across all 89 TB2 tasks
   (unblocked since harbor v0.9.0 — see the version requirement above).
   Sonnet-medium full-sweep budget ≈ $25; expected wall-clock ≈ 1–2 h at
   `-n 10` concurrency.

3. **SWE-Bench Verified** (planned): same Harbor wrapper, one flag change;
   ~500 tasks, ~$300 budget.
