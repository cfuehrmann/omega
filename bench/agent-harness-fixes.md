# Agent harness fixes — surfaced by TB2 Sonnet medium 2026-05-03

Five backlog items for the **agent harness** (not the LLM). All five were
identified by analysing the 2026-05-03 89-task Sonnet-4.6/medium run
(`bench/jobs/2026-05-03__01-50-33/`, 49/89 = 55.1 %, ~5 h wall-clock).

The selection criterion was strict: only failures where the *harness* could
have done something better, regardless of the LLM's reasoning quality.
Wrong-answer / hallucination / capability-gap failures are excluded — those
move with the model, not with the harness.

Ranked by estimated **tasks recovered ÷ implementation effort**.

| # | Item | Recovered (est.) | Effort |
|---|---|---|---|
| 1 | [AGENT-1](#agent-1--retry-on-transient-network-errors-during-agent-setup) — setup-command retry | +2 to +3 | tiny |
| 2 | [AGENT-2](#agent-2--per-llm-call-timeout-with-recovery) — per-LLM-call timeout | +2 to +4 | small |
| 3 | [AGENT-3](#agent-3--draft-commit-reminder-at-half-budget) — draft-commit reminder | +2 to +5 | small |
| 4 | [AGENT-4](#agent-4--pre-submission-output-path-validation) — pre-submission output validation | +1 to +2 | medium |
| 5 | [AGENT-5](#agent-5--live-wall-clock-awareness-in-system-prompt) — live wall-clock in prompt | +1 to +3 | small |

Combined: AGENT-1 + AGENT-2 alone should reclaim 4–7 tasks, moving Sonnet
medium from 49/89 (55 %) back to ~54–56/89 (61–63 %) without changing the
LLM. The full set targets ~66–70/89 (74–79 %).

---

## AGENT-1 — Retry on transient network errors during agent setup

**Status:** P1 — open. Highest ROI; trivial fix.

### Evidence

`bench/omega_agent.py` (lines 71–80, mirrored at 269–280) executes setup
commands with **zero retries**:

```python
await self.exec_as_agent(environment,
    command="apt-get update -qq && apt-get install -y ...")
await self.exec_as_agent(environment,
    command="curl -fsSL https://bun.sh/install | bash")
await self.exec_as_agent(environment,
    command="git clone --branch v0.1.4 ... && bun install --frozen-lockfile")
```

A single transient DNS or connection failure during any of these aborts the
trial as `NonZeroAgentExitCodeError`, before the LLM ever sees the task.
In the 2026-05-03 run:

| Task | Failure point | Error |
|---|---|---|
| `merge-diff-arc-agi-task` | `apt-get update` | Connection timed out fetching `archive.ubuntu.com` |
| `pypi-server` | `curl bun.sh/install` | Failed to connect to `bun.sh` port 443 (curl exit 7) |
| `mteb-leaderboard` | (later, while running) | Indirect — `bun run` exited 1 |

All three tasks **passed previously** on Sonnet medium and/or Opus
(`pypi-server` 1/1 prior, `merge-diff-arc-agi-task` 1/2 prior), so these are
pure infrastructure regressions, not capability regressions.

### Proposed fix

Wrap each setup `exec_as_agent` call in a 3-attempt retry with exponential
backoff (e.g. 5 s, 15 s, 45 s). On the third failure, surface the original
error so debug visibility is preserved. Limit retry to the three known
network-bound steps: `apt-get`, `curl`, `git clone`.

### Risks / non-goals

- Don't retry the LLM-driven instruction step itself — that's
  AGENT-2's territory.
- Backoff total (≤ 65 s) must stay well under the shortest task budget
  (900 s for many tasks).

---

## AGENT-2 — Per-LLM-call timeout with recovery

**Status:** P1 — open. High ROI; fixes a worst-case failure mode where a
single hung HTTP request consumes the entire trial.

### Evidence

Two trials in the 2026-05-03 run logged an `llm_call` event that **never
produced an `llm_response`** before the wall-clock timeout fired:

| Task | Total events | Last event | Time elapsed | Outcome |
|---|---|---|---|---|
| `polyglot-rust-c` | 6 | `llm_call` | ~13 min hang | AgentTimeoutError, no output |
| `write-compressor` | 12 | `llm_call` | ~15 min hang | AgentTimeoutError, no output |

In `write-compressor` the agent had completed two `read_file` calls
(`/app/decomp.c`, `/app/data.txt`), then issued the next LLM call, which
never returned. The trial died at the 900 s ceiling having produced no
output file. Both tasks have prior passes (`write-compressor` 1/2 Sonnet,
1/1 Opus).

There is currently no per-call ceiling shorter than the wall-clock; the
streaming response can hang indefinitely. The `recovery 1/1` mechanism for
`stop_reason=max_tokens` (which works correctly — see notes below) does
*not* cover this case because no response is received at all.

### Proposed fix

1. Add a per-LLM-call timeout (suggested: 5 minutes) on top of the existing
   streaming infrastructure.
2. On timeout, abort the in-flight HTTP request, emit a synthetic
   `llm_error` event for diagnosis, and retry once. After the second
   timeout, surface the failure as an `agent_error` so the wall-clock
   doesn't get swallowed silently.
3. Treat the retry budget as separate from the existing thinking-budget
   `recovery 1/1` counter; both concerns are real and orthogonal.

### Notes

The thinking-budget recovery (`recovery 1/1` after `stop_reason=max_tokens`)
**already works correctly**. Five tasks hit it in the 2026-05-03 run; all
five recovered on the next call. One (`schemelike-metacircular-eval`) even
passed (reward=1.0). No task hit a second exhaustion that would defeat the
1/1 cap. So AGENT-2 is purely about *no response at all*, not about
truncated thinking.

### Risks / non-goals

- 5-minute cap must not interrupt legitimate long extended-thinking calls.
  Sonnet at `medium` has `max_tokens=64000`; even at slow streaming rates
  this fits comfortably under 5 min.
- Don't add this on top of the existing per-stream-chunk timeout if there
  already is one — verify in `rust/crates/omega-core/` first.

---

## AGENT-3 — Draft-commit reminder at half-budget

**Status:** P2 — open. Medium ROI, small effort.

### Evidence

Several trials reached the wall-clock or turn limit having **never written
to the expected output path** despite the instruction explicitly naming it.

| Task | LLM calls | `write_file` to answer path? | Tools used |
|---|---|---|---|
| `mteb-leaderboard` | 100 | ❌ never | 48× `fetch_url`, 58× `run_command`, 0× `write_file`/`edit_file` |
| `path-tracing` | ~32 | ❌ never | inspected `/app/image.ppm`, never wrote `/app/output.png` |
| `path-tracing-reverse` | ~50+ | ❌ never | same pattern |
| `gcode-to-text` | ~40 | ❌ never (wrote 5 *script* files but no answer file) | |

`mteb-leaderboard` is the clearest case: 100 turns, 0 writes to
`/app/result.txt`, exhausted the turn cap. The agent went down a rabbit
hole exploring the `mteb` Python API and fetching URLs, never committing
an answer.

The system prompt already contains "*commit a working solution before
refining*" but it is read once and forgotten. There is no mid-run
enforcement.

### Proposed fix

Inject a **system reminder** at fixed checkpoints during a long agentic
loop:

- At turn ⌈N/2⌉ (50 by default): "*N/2 turns used. Have you written your
  best-effort answer to `{output_path_from_instruction}`? If not, commit
  a draft now and refine afterwards.*"
- At turn ⌈0.9·N⌉ (90 by default): "*Only ⌈0.1·N⌉ turns remaining. Stop
  exploring; commit your best answer now.*"

The path can be regex-extracted from the instruction (look for the first
`/app/...` mentioned) or, as a fallback, the reminder can omit the path
and just say "the required output."

### Risks / non-goals

- Reminders that fire too often dilute their effect. Two checkpoints is
  the proposed ceiling.
- Some tasks have no single "answer file" (e.g. multi-file refactors).
  The reminder text must be generic enough to apply.

---

## AGENT-4 — Pre-submission output-path validation

**Status:** P3 — open. Small impact, harder to generalise.

### Evidence

Two failures in the 2026-05-03 run were caught by trivial
post-hoc structural checks:

| Task | What the verifier saw | What the agent did |
|---|---|---|
| `polyglot-c-py` | `os.listdir("/app/polyglot") == ["main.py.c"]` failed: also contained compiled `cmain` | Compiled the binary while testing, never cleaned up |
| `sam-cell-seg` | `IsADirectoryError` reading `/app/output.csv` | Created a directory at the file path |

Both could be caught by a one-line pre-submission check parsed from the
instruction's stated expectations.

### Proposed fix

Before the agent's final turn, run a structural sanity sweep:

1. Extract paths mentioned in the instruction (regex `/app/[^\s]+`).
2. For each path, classify expected kind (file vs. directory) from
   surrounding context: "*write a file*", "*create a directory*", "*put
   the result in*", etc.
3. If a path's actual kind disagrees, inject a final reminder turn:
   "*`/app/output.csv` is a directory but the instruction expects a file —
   fix this before finishing.*"

### Risks / non-goals

- Heuristic extraction is brittle. False positives could nag the agent
  about non-issues.
- Lower priority than AGENT-1/2/3 — only ~2 tasks affected per run.

---

## AGENT-5 — Live wall-clock awareness in system prompt

**Status:** P3 — open. Modest impact, small effort.

### Evidence

The time budget is currently injected **once** into the user message
prefix:

```
Time budget: 900 seconds (15 minutes).
```

The LLM has no way to know how much has actually elapsed. Tasks that
iterate without converging (`largest-eigenval` made 55 LLM calls and 70
tool calls polishing an eigenvalue solver that never matched the
required speedup; `train-fasttext` 36 LLM calls iterating on a model
with 0.51 accuracy vs. required 0.62) have no signal that the deadline
is approaching.

### Proposed fix

Inject elapsed-time metadata into a per-turn system message reminder, e.g.

```
You have used 8m 15s of your 15m budget. 6m 45s remain.
```

This can be combined with AGENT-3's reminder at checkpoints rather than
firing every turn (every-turn injection invalidates the prompt cache).

### Risks / non-goals

- Per-turn injection invalidates the cache prefix and increases cost ~3×.
  Only emit at the same fixed checkpoints as AGENT-3.
- Don't use Anthropic's `task_budget` feature for this — see
  [`../backlog/task-budget.md`](../backlog/task-budget.md) for why
  (Opus 4.7 only, advisory not enforced, cache mutation issues).

---

## Out of scope — what this file does **not** cover

The following 2026-05-03 failures are LLM/capability issues, not harness
issues, and won't be fixed by anything in this file:

- **Wrong numerical answers** (off-by-one, wrong tokenizer version):
  `count-dataset-tokens` (79566 vs 79586), `dna-insert` (Tm 55.9 vs ≥58),
  `video-processing` (frame 227 vs 231–234).
- **Hallucination / wrong domain knowledge**: `mteb-retrieve` (wrote
  HumanEval description for MTEB), `protein-assembly` (wrong domain
  order), `dna-assembly` (missing primers).
- **Insufficient algorithmic depth**: `train-fasttext` (accuracy 0.51 vs
  0.62), `raman-fitting` (completely wrong fit), `largest-eigenval`
  (speedup ≈ 0).
- **Verifier-side infrastructure failures** (false negatives — neither
  the agent nor the harness can fix these): `overfull-hbox`,
  `distribution-search`, `extract-moves-from-video`, `caffe-cifar-10`,
  `pytorch-model-recovery`. All five blocked by the verifier's own
  inability to download `uv` or PyPI wheels inside the sandbox.

These belong in a future ticket about benchmark-environment robustness
(e.g. mirror `astral.sh` and PyPI inside the test infra), not in the
agent-harness backlog.
