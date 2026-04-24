# Investigating Omega's Benchmark Efficiency

## Purpose

Terminal-Bench 2 gives us per-task evidence of where Omega succeeds and fails.
This document collects findings, and more importantly **frames the investigation
so that it stays focused on what the Omega scaffold could realistically do
better** — as opposed to limitations of the underlying Sonnet model.

This is a living document. Start new sessions from here, append findings.

## Framing: Omega vs. the Model

Every benchmark failure can be attributed to one of two layers:

1. **Model layer (Sonnet)** — the LLM couldn't reason its way to the solution,
   didn't know an API, mis-counted bytes, hallucinated a function signature,
   etc. These are capability gaps in the model weights.
2. **Agent layer (Omega)** — the scaffold around the model: which tools it
   exposes, what context it injects, what self-checks it prompts, how it
   handles time/turn budgets, when it surfaces "are you done?" signals.

**This investigation is scoped to the agent layer.** If a failure is
fundamentally "Sonnet didn't know how to parse an ELF header", we note it but
don't pursue it — there's nothing Omega can do about that short of switching
models. If a failure is "Sonnet produced a half-correct ELF parser and never
measured its own coverage before declaring done", that **is** an agent-layer
opportunity — Omega could have prompted a coverage check.

### Heuristic for classification

Ask: *"Given the same model weights, could a different scaffold plausibly have
reached the right answer?"* If yes → agent-layer. If no → model-layer, skip.

## Current Findings

Data: 10-task overnight run on 2026-04-24 (Sonnet 4.6, `-n 1`), 8/10 pass.
Full results in `benchmark-results/results.jsonl`.

### 1. `overfull-hbox` — didn't loop to convergence

**Task.** Fix LaTeX "overfull hbox" warnings by swapping words in `input.tex`
for shorter synonyms listed in `synonyms.txt`. Can't edit `main.tex` or the
synonyms file.

**What happened.** Omega correctly identified the files, ran `pdflatex`, spotted
offending lines, grep'd for candidate words, made two rounds of edits. Final
tool call was a bare `pdflatex` run — warnings were still present — and the
session ended without further edits.

**Classification.** **Agent-layer.** Sonnet understood the task and made
plausible fixes. The scaffold failure was: no signal to the model that "the
verification you ran literally still shows warnings; you haven't finished".

**Hypothesis to investigate.** A "verification-aware" affordance — e.g. when
the instruction mentions a concrete success criterion ("no overfull hbox
warnings"), the agent surfaces a reminder of that criterion each turn, or
injects a success-check as a pseudo-tool.

### 2. `extract-elf` — incomplete coverage, no self-check

**Task.** Write `extract.js` that parses a C binary (ELF64) and emits a JSON
map of memory addresses → integer values. Must cover ≥75% of the reference
addresses; any included address with a wrong value fails the test.

**What happened.** Omega wrote a working Node.js ELF parser that produced valid
JSON output. Ran it, saw "Success", moved on. Never measured *how many*
addresses it was covering vs. the reference, and never re-read the task's
"≥75% coverage" requirement.

**Classification.** **Agent-layer, though with a model-layer component.** The
ELF parser itself may have been genuinely incomplete (Sonnet's bug), but the
agent never prompted "you produced output — does it meet the explicit coverage
threshold stated in the task?" Without that prompt, Sonnet exited on "it
runs".

**Hypothesis to investigate.** Same "verification-aware" affordance as (1),
generalised: when a task states a numeric success threshold ("≥75%", "under
5 ms", "all tests pass"), the agent should surface that threshold in the
context near the end of the turn.

### 3. `largest-eigenval` — rabbit-hole, never committed

**Task.** Implement a Python function that finds the dominant eigenvalue +
eigenvector of a small (≤10×10) real matrix faster than numpy's
`np.linalg.eig`.

**What happened.** Omega ran 30+ tool calls exploring optimisation paths:
scipy.linalg, numba JIT, then a deep excursion into ctypes/OpenBLAS trying to
call `dgeev` directly. **Never wrote anything to `/app/eigen.py`**. Hit the
900-second task timeout while still benchmarking ctypes variants.

**Classification.** **Purely agent-layer.** A working solution using
`scipy.linalg.eig` would have scored in the first 60 seconds. The model had
no sense that it had already spent too much time exploring without delivering.

**Hypothesis to investigate.** Most actionable of the three. Several
candidate affordances:

- **Time-budget awareness.** Inject "elapsed: X min / timeout: Y min" into the
  context at each turn. Omega currently has turn budgets but no wall-clock
  awareness.
- **Ship-then-refine pattern.** System prompt nudge: "before deep
  optimisation, commit a working solution and measure it." Debatable whether
  a prompt change alone is enough.
- **Depth-limiting.** After N consecutive tool calls with no file write (or no
  progress on the target file), inject a "step back — have you delivered a
  solution?" message.

## Common Thread

All three failures share a pattern: **the model kept going without a
meta-check that the stated goal had been reached.** Overfull-hbox kept
editing but never re-read "is pdflatex clean now?" as a stop condition.
Extract-elf produced output but never checked "is my coverage ≥75%?".
Largest-eigenval optimised without ever checking "have I produced *any*
working solution?".

This suggests a single agent-level intervention could move the needle across
many tasks: **a "goal-check" affordance** that periodically reminds the model
of the stated success criteria and prompts a self-evaluation against them.

## Investigation Plan

### Phase A — Gather more data (cheap)

Run another 15–20 tasks from the remaining 65 to broaden the sample before
pattern-matching too hard on n=3. Favour medium-difficulty tasks with
concrete success criteria (numeric thresholds, test suites, deterministic
verification), which are the ones most likely to exhibit the "didn't
self-check" pattern.

Already in-scope, runs via existing `harbor run -i …` + `bun
scripts/bench-ingest.ts` workflow. Budget: ≈$4–6 in API spend, ≈1–2 hours
wall-time.

**Success criterion for Phase A.** We have failure logs from at least 10
tasks, spanning at least 4 categories, with at least one failure from each of:
software-engineering, debugging, data-processing, security.

### Phase B — Categorise failures against the hypothesis

For each failure, classify:

- **Goal-check failure** (didn't verify stated success criterion)
- **Rabbit-hole failure** (no time/depth awareness)
- **Convergence failure** (verification ran but model didn't re-read output)
- **Model-layer failure** (genuine capability gap, skip)
- **Other** (surface new patterns)

This is much easier once the events-analysis work
(`docs/benchmark-events-analysis-plan.md`) is in place — categorisation on
raw `events.jsonl` is tedious because tool outputs aren't persisted.

**Decision point.** If >50% of agent-layer failures map to the "goal-check"
category, that's the first intervention to prototype. If the distribution is
flatter, we prioritise the broadest affordance.

### Phase C — Prototype one affordance, measure

Pick the highest-value affordance from Phase B. Implement it behind a feature
flag in the Omega agent. Re-run the **same** failed tasks from Phase A.
Compare before/after pass rate.

Key design constraint: any affordance we add must be **generic** — not tuned
to the specific tasks we used for evaluation. If we hand-craft prompts that
only work on Terminal-Bench, we're overfitting.

**Success criterion for Phase C.** Net pass-rate improvement on the held-out
failed tasks of ≥2 tasks, with no regressions on the passed tasks. If the
affordance helps one set and hurts another, we've found an interesting
tradeoff, not a win.

### Phase D — Re-run broader benchmark with the winner

Only after Phase C shows positive results: expand to the full 76-task
oracle-passing set and measure aggregate pass rate. This is the point at
which we care about leaderboard-comparable numbers.

## Out of Scope

- **Prompt engineering for specific tasks.** If we find ourselves writing
  "when you see a LaTeX task…", we've misunderstood the mission.
- **Switching models.** This investigation assumes claude-sonnet-4-6. We
  can re-evaluate with opus-4-6 or opus-4-7 separately; don't conflate.
- **Harbor/infrastructure bugs.** Tracked in `docs/benchmarking-notes.md`.
  Re-surface there, not here.

## References

- `docs/benchmarking-notes.md` — Harbor/TB2 setup + known infrastructure issues
- `docs/benchmark-events-analysis-plan.md` — tooling to make this
  investigation cheaper per task
- `benchmark-results/results.jsonl` — accumulated per-task data
- `jobs/<timestamp>/<task>/agent/events.jsonl` — raw session events per trial
