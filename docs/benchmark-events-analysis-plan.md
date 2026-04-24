# Leveraging Persisted Events for Benchmark Analysis

## Purpose

Every benchmark trial runs a full Omega session, which already persists rich
structured data (`events.jsonl`, `context.jsonl`) during normal operation.
Right now we can't fully exploit that data for benchmark analysis, and fixing
this looks **cheap and high-leverage** — it makes the investigation work in
`docs/benchmark-investigation.md` dramatically easier per task.

This document is a standalone plan that can be executed in its own session.

## Motivation

Diagnosing a single benchmark failure today means reading a sparse
`events.jsonl` where tool *outputs* aren't present — only input arguments,
timing, and error flags. The agent's actual context (what Sonnet saw, what
each tool returned) is **hashed by reference** into a context store that
isn't preserved.

For the three failures in `docs/benchmark-investigation.md`, diagnosis
required:

- Reading `events.jsonl` tool-call inputs to guess what was happening
- Inferring tool outputs from subsequent tool calls
- Reading the task description separately
- Writing one-off Python scripts to summarise

This is fine for three tasks. It will not scale to the 76 tasks × 5 attempts
× multiple Omega revisions that serious evaluation requires.

## Current State

### What Omega persists in normal operation

Per `AGENT.md` ("Contract Authority — the most public contract wins"):

- **`events.jsonl`** — append-only log of session events (LLM calls, tool
  calls, tool results, user messages). **Tool results are referenced by
  `contextHash`; their content is not embedded.**
- **`context.jsonl`** — the content side of the store. Maps context hashes
  to actual text (tool outputs, system prompts, etc.).

Together these two files fully reconstruct a session. Either alone does not.

### What benchmark runs persist

`omega_agent.py` runs Omega inside the Harbor container at
`/tmp/omega-session/`, then in its `finally` block downloads **only
`events.jsonl`** to the host via `environment.download_file(...)`:

```python
await environment.download_file(
    f"{OMEGA_SESSION_DIR}/events.jsonl",
    self.logs_dir / "events.jsonl",
)
```

`context.jsonl` exists in the container at `/tmp/omega-session/context.jsonl`
but is **never downloaded**. When the container is torn down, it's lost.

### Consequence

Every benchmark trial's full session data is irretrievable after the run.
We can see what Omega *did* but not what it *saw*.

## The Obvious First Fix

Add a second `download_file` call in the `finally` block of `OmegaAgent.run`:

```python
await environment.download_file(
    f"{OMEGA_SESSION_DIR}/context.jsonl",
    self.logs_dir / "context.jsonl",
)
```

One line of code. Zero risk — it's a post-hoc file copy that can't affect the
agent run. From that point forward every benchmark trial directory contains a
**complete**, replayable Omega session.

**This should land before anything else in this plan.**

## Proposal

### Phase 1 — Persist `context.jsonl` (trivial, do first)

Update `omega_agent.py` to download `context.jsonl` alongside `events.jsonl`.
Wrap in its own try/except so one failing download doesn't prevent the other.

Run the existing crack-7z-hash smoke test afterwards, verify the file appears
in `jobs/<timestamp>/crack-7z-hash*/agent/context.jsonl`.

**Effort:** ~15 minutes including the smoke-test verification.

### Phase 2 — Decide: reuse Omega's viewer, or build a bench-specific one?

The Omega web UI already renders `events.jsonl` + `context.jsonl` into a
readable session view. Two paths:

#### 2A — Reuse Omega's existing UI

Copy each trial's `events.jsonl` + `context.jsonl` into a session directory
that Omega's web server can load. Point a browser at it.

- **Pros.** Zero new rendering code. WYSIWYG — we see the session exactly as
  Omega does.
- **Cons.** One-at-a-time viewing. No cross-session queries. Web UI not
  designed for bulk diffing.

#### 2B — Build a bench-specific viewer/analyser

A CLI (Bun/TypeScript) that reads a trial's `events.jsonl` + `context.jsonl`
and prints a markdown-rendered session summary to stdout. Optionally a second
mode that emits machine-readable metrics.

- **Pros.** Scriptable. Diffable with `diff`. Easy to feed into other tools
  (e.g. summarise 20 sessions, group by failure category).
- **Cons.** Duplicates some rendering logic.

**Recommendation.** Do **both**, in order: 2A first (no code — just a script
that copies trial files into the right shape for Omega's UI), then 2B once
we know what cross-session queries we actually need.

**Effort:** 2A ≈ half-hour; 2B ≈ half-day.

### Phase 3 — Metrics extraction

Once sessions are fully reconstructible, extract per-trial metrics beyond
what Harbor's `result.json` already gives us:

- Number of tool calls, by tool
- Wall-clock time to first file-write (proxy for "time-to-action")
- Wall-clock time between last file-write and session end (proxy for
  "polish/verification time")
- Count of `run_command` calls that errored vs succeeded
- Count of LLM turns, and turns spent on tool vs. on text output
- Presence/absence of task-stated success criteria in the final turn's
  context (proxy for "did the model re-read the goal?")

These feed directly into the categorisation step in
`docs/benchmark-investigation.md` (Phase B).

**Output format.** Append metrics to `benchmark-results/results.jsonl` as
additional fields, or a separate `metrics.jsonl` keyed by `trial_id`. The
latter is cleaner — keeps the core results file stable.

**Effort:** ~half-day.

### Phase 4 — Failure categorisation (optional, later)

Programmatic or LLM-assisted categorisation of failed trials against the
hypothesis buckets from `docs/benchmark-investigation.md`:
goal-check / rabbit-hole / convergence / model-layer.

One approach: for each failure, feed the full session transcript to a
separate Claude call with a categorisation prompt. Cheap ($0.01–0.05 per
trial, maybe $1–2 for the full 76-task set) and gives consistent labels.

Defer until we have ≥20 failures to categorise — below that, manual
classification is faster than building the tool.

## Open Design Questions

1. **Container vs. host paths.** Omega inside the container uses
   `/tmp/omega-session/`. The host receives files at
   `jobs/<ts>/<trial>/agent/`. Should we also persist Omega's session
   metadata (cwd, env vars, git revision) so runs are fully reproducible
   offline? Probably yes, but out of scope for Phase 1.
2. **Trial-file size.** Some tasks may produce large `context.jsonl` files
   (many tool outputs). Check after Phase 1 whether this becomes a disk
   pressure issue after 100+ trials. If so, add compression.
3. **Pass runs vs. fail runs.** Do we need `context.jsonl` for successful
   runs too? Probably yes — baseline sessions are as valuable as failures
   when designing affordances. Default: persist for all runs.

## Order of Operations (Recommended)

1. **Phase 1** — 15 min. Land the `download_file` change, smoke-test, commit.
2. **Phase 2A** — 30 min. Script that copies a trial's files into Omega's
   session format, so the existing UI can replay any trial.
3. Use 2A to manually re-review the three existing failures with full
   context. Refine the hypothesis buckets in `benchmark-investigation.md` if
   the richer data surfaces new patterns.
4. Only after that, decide whether Phase 2B/3/4 are worth doing, or whether
   the UI-replay alone is enough leverage.

## Success Criterion for This Plan

After Phase 1 + 2A: **a failed benchmark trial can be loaded in the Omega
web UI in under a minute, and the full session — including every tool
output — can be read directly.** That's the minimum useful outcome; anything
beyond is bonus.

## References

- `omega_agent.py` — the Harbor adapter that needs the one-line change
- `AGENT.md` — "Contract Authority" section on events.jsonl / context.jsonl
  as the most-public contracts
- `docs/benchmark-investigation.md` — the investigation this unblocks
- `jobs/<timestamp>/<trial>/agent/events.jsonl` — current trial persistence
