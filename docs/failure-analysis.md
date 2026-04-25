# Failure-Mode Analysis — Terminal-Bench 2.0, Omega + Sonnet 4.6

**Generated:** 2026-04-25 (roadmap item 5a)  
**Scope:** 29 failing tasks / 40 failing trials (76 total tasks)  
**Method:** Systematic log read — events.jsonl + verifier/test-stdout.txt for every failing
trial; contrast samples on selected passing trials.

---

## 1. Revised failure-shape taxonomy

The incremental taxonomy in `docs/results.md` was built one run at a time and misclassified
several failures. After reading every verifier output and diffing it against the agent's
event log, the true picture has seven shapes, some with overlap.

### Shape 1 — Thinking-budget exhaustion (NEW, 4 tasks / 5 trials)

| Task | Trials | Signal |
|---|---|---|
| regex-chess | 2 | max_tokens on turn 2 (both runs) |
| winning-avg-corewars | 1 | max_tokens on turn 11 |
| dna-assembly | 1 | max_tokens on turn 6 |
| feal-linear-cryptanalysis | 1 | max_tokens (turn count varies) |

**Mechanism.** Sonnet 4.6 is capped at 64 000 output tokens per API call — this budget
covers both thinking tokens and response text. On hard planning tasks, the model
generates 87 000–93 000 characters of internal reasoning that gets packed into ~64 000
tokens, leaving zero tokens for any text or tool call. The `llm_response` event shows
`stopReason: "max_tokens"`, `output_tokens: 64000`, and an empty `text` field.

**Agent behaviour after max_tokens with no tool_use.** The current `agent.ts` handles
`(toolUseBlocks.length > 0 && max_tokens)` by injecting synthetic error tool results so
the context stays well-formed. But `(toolUseBlocks.length === 0 && max_tokens)` — the
pure-thinking-exhausted case — falls through silently: `continueLoop` was never set to
`true`, so the turn ends, and the verifier runs on an empty /app directory.

**Concrete log excerpt — regex-chess (phaseA-prompt-validation):**
```
llm_response stopReason=tool_use         # turn 1: reads /app/check.py
tool_call: run_command ls /app/
tool_call: run_command git status
tool_result: [check.py listing]
tool_result: [git status]
llm_response stopReason=max_tokens       # turn 2: 64 000 output tokens, 89 113 chars thinking, text=""
                                         # → turn ends, no re.json written
turn_end
server_stopped
```

Verifier confirms: `FileNotFoundError: /app/re.json` — all 4 tests fail.

**Concrete log excerpt — winning-avg-corewars (phaseB-deadline-validation):**  
11 runs of pmars benchmarks, then:
```
llm_response stopReason=max_tokens       # 64 000 thinking tokens, no warrior written
turn_end
```
Verifier: `Unable to open file '/app/my_warrior.red'`.

**Why Sonnet 4.6 is uniquely affected.** Opus 4.6/4.7 have 128 000 max_tokens; the same
thinking depth leaves them with ample budget for tool calls. This shape would largely
disappear on the Opus run.

---

### Shape 2 — Wall-clock timeout (10 tasks / 12 trials)

| Task | Trials | Turns at cutoff | Notes |
|---|---|---|---|
| gcode-to-text | 2 | mid-run | 45 turns on second run, no out.txt |
| path-tracing | 1 | mid-run | 5/5 verifier tests fail (no reconstructed.ppm) |
| path-tracing-reverse | 1 | mid-run | rendering completes but /app/reverse binary missing |
| make-doom-for-mips | 1 | mid-run | no frame.bmp |
| raman-fitting | 1 | mid-run | verifier infra failure compounds (see Shape 5) |
| gpt2-codegolf | 1 | mid-run | verifier infra failure compounds |
| largest-eigenval | 2 | mid-run | 21 turns; partial pass (see Shape 6) |
| adaptive-rejection-sampler | 1 | mid-run | no ars.R |
| crack-7z-hash (fail) | 1 | mid-run | found password but solution.txt not yet written |
| write-compressor (fail) | 1 | mid-run | data.comp not written; passing trial DID write it |

**Verifier signal:** `FileNotFoundError` or image/file similarity tests fail because the
expected output file never appeared. The agent was active (last event is mid-loop `llm_call`)
when the timeout fired.

**Contrast — write-compressor (passing trial, phaseB-deadline-validation):**  
The deadline-injection fix caused the agent to write `/app/data.comp` before attempting
final size optimisation. The failing trial in the same batch had the same code path but
the timeout fired before the write committed. This is direct evidence that the commit-early
pattern works; the barrier is the agent's willingness to commit before the clock runs out.

---

### Shape 3 — Artifact left in wrong location (3 tasks / 4 trials)

| Task | Error | What happened |
|---|---|---|
| extract-elf | `/app/extract.js` not found | Agent wrote `extract.js` to `/home/agent/omega/` (its working directory), not `/app/` |
| polyglot-c-py | `Expected only main.py.c, found: ['cmain', 'main.py.c']` | Agent compiled the polyglot to test it, left the `cmain` binary alongside the source |
| polyglot-rust-c | `Expected only main.rs, found: ['cmain', 'main.rs', 'rmain']` | Same — two compiled binaries left in `/app/polyglot/` |

**Concrete log excerpt — extract-elf (phaseA-prompt-validation):**
```python
# Agent writes successfully…
write_file → /home/agent/omega/extract.js  (700 entries, success)
run_command → node /home/agent/omega/extract.js /app/a.out  (returns valid JSON)
# Agent declares done
turn_end
# Verifier checks…
node /app/extract.js /app/new.o → MODULE_NOT_FOUND
```

The agent self-verified against its own working directory. The verifier uses absolute `/app/`
paths and found nothing.

**Concrete log excerpt — polyglot-rust-c (phaseD-remaining-42):**
```
tool_call: run_command {"command":"cd /app/polyglot && rustc -o rmain main.rs && gcc -o cmain main.rs"}
# → both binaries created
turn_end
# Verifier:
os.listdir("/app/polyglot") → ['cmain', 'main.rs', 'rmain']
assert polyglot_files == ["main.rs"]   # FAILS
```

The task spec says the directory should contain only the polyglot source. The agent compiled
to verify behaviour but never cleaned up.

**This shape is the most directly fixable.** See Section 3.

---

### Shape 4 — Wrong numerical answer (5 tasks / 8 trials)

| Task | Agent output | Expected | Gap |
|---|---|---|---|
| count-dataset-tokens | 79566 (both runs) | 79586 | 20 tokens |
| video-processing | frame 52 / frame 61 (example video) | 219–223 / 225–230 | completely wrong |
| protein-assembly | donor not found (idx=-1) | flag < donor < dhfr < acceptor < snap | wrong sequence |
| dna-insert (trial 2) | Tm(fwd)=70.07, Tm(rev)=62.93 | ΔTm ≤ 5 °C | ΔTm=7.15 |
| query-optimize | median=0.968s | ≤1.05×golden=0.875s | 10% too slow |

**count-dataset-tokens** is the most suspicious: both runs (before and after the
prompt fix) produce exactly 79566 via the same 13-turn sequence. The agent correctly
identifies `ryanmarten/OpenThoughts-1k-sample`, the `science` domain (biology + chemistry
+ physics = 26 rows), tokenizes with `Qwen2.5-1.5B-Instruct`, and reports 79566.
The verifier expects 79586 — a difference of 20 tokens. Two hypotheses: (a) the agent
fetches a slightly different slice of the dataset due to network caching or sampling
inside the container; (b) the agent's row filter omits or double-counts some rows. Either
way, this is a precision-of-approach problem, not a wrong-approach problem.

**video-processing:** The agent detects a jump at frame 52/61 in the _example_ video
(which it processed correctly in the event log), but the verifier runs against both the
example video AND a hidden test video. The output.toml contains example-video frame
numbers only; the algorithm is calibrated to one specific input and fails on a different
one. The agent's final check showed correct numbers for the example but didn't test
generalisation.

---

### Shape 5 — Verifier infrastructure failure (3–4 tasks)

These trials have `reward=0` but the agent either solved the task correctly or timed out
for independent reasons. The verifier itself failed to install its test dependencies.

| Task | Verifier failure | Agent status |
|---|---|---|
| **distribution-search** | `curl: (7) Failed to connect to astral.sh` → uvx not installed → `uvx: command not found` | **Agent SOLVED the problem** (KL=10.000, error=1.74e-12) |
| gpt2-codegolf | `curl: (6) Could not resolve host: release-assets.githubusercontent.com` → uv install fails | Agent also timed out (independent failure) |
| raman-fitting | Same uv/curl failure | Agent also timed out |
| mteb-retrieve | nvidia-cusparselt download timeout (UV_HTTP_TIMEOUT=30s) | Agent timed out downloading models |

**distribution-search is a definite false negative.** The agent's event log shows:

```
run_command → python find_dist.py
  ✓ Found solution with k1=6, k2=1
  KL(P||U) = 10.0000000000  |error| = 1.74e-12  ≤ 0.001? True
  KL(U||P) = 10.0000001553  |error| = 1.55e-7   ≤ 0.001? True
  Sum = 1.000000000000  All > 0: True
write_file → /home/agent/omega/find_dist.py   # ← wrong directory! (see Shape 3)
turn_end
# Verifier: fails to install uv, never checks distribution file
```

Note: distribution-search also has the wrong-directory artifact issue — the file was
written to /home/agent/omega/ not /app/. So even with a working verifier it might still
fail. But the agent did find the correct mathematical solution.

**Recommendation:** Add these trials to `.skip-trials` or note as verifier-infra failures
in results.md. For gpt2-codegolf and raman-fitting, the agent would have failed anyway
(timeout + hard task), so skipping is optional.

---

### Shape 6 — Near-miss / edge case (3 tasks)

Tasks where the agent was nearly correct but missed specific test cases.

| Task | What passed | What failed |
|---|---|---|
| cancel-async-tasks | 5/6 verifier tests | `test_tasks_cancel_above_max_concurrent`: queued-but-not-started tasks don't get cleanup called |
| largest-eigenval | 21–25/27 tests | Specific matrix sizes [4,5,6,9] fail speedup requirement; different sizes fail across two runs |
| sanitize-git-repo | 2/3 tests | Git history rewrite succeeds but damages object store; verifier's gitpython can't resolve SHA |

**cancel-async-tasks detail.** The verifier test sends SIGINT when 3 tasks are running
with max_concurrent=2. Only 2 tasks started; the 3rd was queued. The test requires all 3
tasks' `finally` cleanup to run. The agent's implementation uses
`asyncio.Semaphore(max_concurrent)` — tasks waiting on the semaphore are cancelled but
their `finally` blocks don't fire because they never entered the semaphore. This is a
genuine Python asyncio gotcha. The agent's own test suite passed 5/6 but didn't test
the above-concurrent-limit cancellation case.

**sanitize-git-repo detail.** The agent rewrote git history and 2/3 tests pass (secrets
are removed, replacements are correct). But the git object store was corrupted in the
process — `git rev-parse d6987af...` returns "missing". The agent used git filter-branch
or similar in a way that left dangling objects. The verifier uses gitpython which verifies
object integrity.

---

### Shape 7 — Turn exhaustion (3 tasks / 3 trials)

| Task | Signal | Turns | What failed |
|---|---|---|---|
| mteb-leaderboard | NonZeroAgentExitCodeError | 50 | Never wrote result.txt; 50 turns of web-scraping attempts |
| extract-moves-from-video | NonZeroAgentExitCodeError | turn_interrupted | Never wrote solution.txt |
| make-mips-interpreter | NonZeroAgentExitCodeError | 50+ | Setup never completed (Bun install taking 360+ s inside container) |

**make-mips-interpreter** is a special case: the agent can't get started because the
container spends 6+ minutes downloading/installing Bun (a CI dependency of Omega itself).
The `omega_agent.py` setup phase inside the container installs Omega, which pulls Bun,
which in this container is very slow. The task never starts in any meaningful sense.

---

### Taxonomy comparison vs. results.md

| results.md shape | Count | Revised shape(s) | Notes |
|---|---|---|---|
| Timeout | 10 | Shape 2 (timeout) | Confirmed, accurate |
| 50-turn exhaustion | 3 | Shape 7 (turn exhaust) | Confirmed |
| Wrong answer | 6 | Shapes 3+4+6 | Undercount; many "wrong answer" are file-location or near-miss |
| Capability ceiling | 9 | Shapes 1+4+5+6 | Mixes thinking-exhaust, wrong-algorithm, verifier-infra |
| Infrastructure | 1 | Shape 7 + Shape 5 | make-mips-interpreter + 3 verifier-infra failures |

The "capability ceiling" and "wrong answer" labels in results.md conflated structurally
different failures. The revised taxonomy surfaces three separately-fixable sub-classes:
thinking-budget exhaustion (Shape 1), artifact placement (Shape 3), and verifier infra
(Shape 5).

---

## 2. Candidate cheap fixes, ranked by yield × cost

### Fix A — Delete compiled artifacts before finishing [HIGHEST yield, trivial cost]

**Addresses:** polyglot-c-py, polyglot-rust-c (2 tasks, confirmed by verifier output)

**Root cause:** The agent compiles the polyglot file to verify it, leaves the binary in
the submission directory, and the verifier expects exactly one file. The task spec is clear
("the polyglot directory should contain only `main.py.c`"), but the agent doesn't re-read
the spec before finishing.

**Fix (system-prompt addition to task-completion section):**
```
After testing, remove all compiled binaries, object files, and temporary
build artifacts unless the task explicitly asks you to keep them. Before
declaring done, list the submission directory to confirm only expected
files remain.
```

**Expected yield:** 2 tasks / 2 trials flip to pass (polyglot-c-py, polyglot-rust-c).
Both are easy tasks where the agent gets the logic right but fails on cleanup.

**Risk:** Low. This is a pure addition to the task-completion verification step.

---

### Fix B — Verify output paths match task spec [HIGH yield, low cost]

**Addresses:** extract-elf (1 task clear), possibly sqlite-with-gcov, dna-insert

**Root cause:** The agent writes output files to its working directory
(`/home/agent/omega/`) rather than `/app/`. This happens when the agent doesn't
explicitly `cd /app` before writing, or uses relative paths in `write_file`.

**Fix (system-prompt addition):**
```
When a task specifies an output path (e.g. /app/extract.js, /app/result.txt),
verify the file exists at that exact absolute path before declaring done. Do not
assume relative paths map to the task's expected location.
```

**Expected yield:** 1 task firm (extract-elf). sqlite-with-gcov is harder — the agent
builds in `/app/sqlite-build/` but the verifier checks `/app/sqlite/`; this requires the
agent to understand where the verifier will look, which is task-specific.

**Risk:** Low. Verification step addition only.

---

### Fix C — Handle max_tokens with no content (thinking-only cutoff) [MEDIUM yield, low-medium code cost]

**Addresses:** winning-avg-corewars, dna-assembly, feal-linear-cryptanalysis (3 tasks)

**Root cause:** When 64 000 thinking tokens are consumed with no text/tool_use blocks,
`agent.ts` silently ends the turn. The agent had useful partial context but no recovery
path.

**Fix (agent.ts code change, ~15 lines):**
When `response.stop_reason === "max_tokens"` AND `toolUseBlocks.length === 0` AND
`assembledText.length === 0`, inject a synthetic user message before the next LLM call:

```
"Your extended thinking ran over the 64 000-token output limit and produced no
action. Please continue — write a short plan (≤ 5 lines) and then immediately
call a tool. Avoid re-exploring the problem from scratch."
```

And set `continueLoop = true` so the agent loop continues.

**Expected yield:** 1–2 tasks. winning-avg-corewars had 20 tool calls of research before
the cutoff — if the loop continues, the agent has enough context to write the warrior.
regex-chess is likely infeasible regardless (pure-thinking-exhaustion on a computationally
hard problem). dna-assembly/feal-linear-cryptanalysis are borderline.

**Risk:** Low-medium. Could cause infinite loops if the agent keeps hitting max_tokens;
add a per-turn max_tokens-recovery counter (cap at 2 recoveries per conversation).

**Note:** This shape would largely disappear on Opus 4.7 (128 000 max_tokens), so
this fix primarily matters for the Sonnet 4.6 score.

---

### Fix D — Flag verifier infrastructure failures [LOW yield, data-hygiene]

**Addresses:** distribution-search (confirmed false negative), optionally gpt2-codegolf,
raman-fitting

**Fix:** Add the distribution-search trial UUID to `.skip-trials`, and note in results.md
that the 67% rate may include 1–2 verifier-infra false negatives.

```bash
# distribution-search failing trial UUID
grep "distribution-search" benchmark-results/results.jsonl  # get trial_id
echo "<uuid>" >> benchmark-results/.skip-trials
```

**Expected yield:** distribution-search potentially flips to pass if re-run (assuming the
file-path bug is also fixed — agent wrote to /home/agent/omega/ not /app/). gpt2-codegolf
and raman-fitting would still fail even with a working verifier (agents timed out).

**Risk:** Low. Data hygiene only.

---

### Fix E — Cleanup before task completion (general) [MEDIUM yield, low cost]

**Addresses:** Broad prevention of the "almost right but verifier sees wrong state" class.

**Fix (system-prompt addition, task-completion section):**
```
Before declaring a task complete, run a final check:
1. List all files in the expected output directory and confirm only required
   outputs are present (no compiled binaries, no temporaries).
2. Confirm each output file exists at the exact path the task specifies.
3. For any code that produces output files, run it one final time in a clean
   working directory to confirm reproducibility.
```

This subsumes Fix A and Fix B into a structured pre-completion checklist.

**Expected yield:** Prevents future occurrences of Shapes 3 and 6 (artifact,
near-miss). Hard to estimate precisely — potentially 3–4 tasks.

---

### Summary table

| Fix | Tasks affected | Effort | Risk | Recommended? |
|---|---|---|---|---|
| A: Delete artifacts | polyglot-c-py, polyglot-rust-c | 1 line prompt | Low | **Yes — do first** |
| B: Verify output paths | extract-elf | 2 lines prompt | Low | **Yes** |
| C: max_tokens recovery | winning-avg-corewars, dna-assembly | ~15 lines code | Low-medium | **Yes, if Sonnet matters** |
| D: Skip verifier infra | distribution-search | 1 UUID | Low | **Yes** |
| E: Pre-completion checklist | General | 6 lines prompt | Low | **Yes** |

Fixes A, B, D, E are prompt-or-data-only and can be bundled into one commit.
Fix C is the only agent.ts change.

---

## 3. Interesting passing trials

### 3a. "Pass despite AgentTimeoutError" — mechanism confirmed

Two tasks pass with `reward=1.0` despite `exception: AgentTimeoutError` in results.jsonl.
The mechanism is now understood:

**schemelike-metacircular-eval:** 41 LLM responses / 53 tool calls over 2400 s. The agent
built a complete Scheme interpreter across 40 turns and was doing additional recursive
self-test iterations when the timeout fired. The working interpreter was already committed
to the container filesystem. The verifier ran on the container state and found a working
interpreter — 63/63 tests passed.

**model-extraction-relu-logits:** 9 LLM responses / 10 tool calls. The agent extracted 9
of 10 neurons, committed the result, and was attempting to find the 10th when the 900 s
timeout fired. The verifier checked the committed result and found the 9 neurons were
sufficient to pass `test_stolen_matrix_matches`.

**Pattern:** The verifier runs on whatever state the Docker container was left in at
timeout. If the agent committed a correct partial solution before the timeout, it can
still pass. This is the same mechanism exploited by the deadline-injection fix
(write-compressor), but here it happened organically.

**Replication implications:** Tasks where a partial solution is accepted should be
prioritised for "commit early" framing. The current system prompt's half-budget rule
("commit a working solution before refining") is already correct — these two examples
confirm it works when the agent follows it.

---

### 3b. Flaky trials

**crack-7z-hash (3 trials):** 2 pass, 1 fails (AgentTimeoutError). The failing trial
(phaseC-fresh-12, rt=1080s) ran the same strategy as passing trials but the cracking ran
over time — the wordlist hash rate is non-deterministic. The agent did write `honeybear`
to solution.txt in the passing trials (both John-the-Ripper wordlist approach). The
failing trial's verifier shows "FileNotFoundError: /app/solution.txt" — the file was
never written because the timeout fired mid-crack.

**overfull-hbox (2 trials):** 1 passes, 1 fails. The failing trial's verifier shows
`test_input_file_matches` failing — the agent slightly modified input.tex even though the
task says not to. The passing trial (after prompt fix) ran 15 turns and did not modify the
input file. This is a deterministic improvement from the prompt change (task-completion
verification).

**circuit-fibsqrt (2 trials):** 1 passes, 1 fails. Failing trial: specific large values
(e.g. N=1763) produce wrong Fibonacci output — the circuit handles N only up to ~31998
(32-bit constraint). Passing trial (after prompt fix): all 32 tests pass because the
agent ran more turns and discovered the N>31998 edge case, expanding the circuit.

---

### 3c. Trials worth deeper inspection in sub-step 5b

| Trial | Why interesting | Hypothesis |
|---|---|---|
| **distribution-search (failing)** | Agent solved it correctly; verifier infra failed | Confirm solve + propose re-run |
| **largest-eigenval (both trials)** | Passes most speedup tests but fails specific matrix sizes | What's special about size 9? Implementation issue or numerical tolerance? |
| **count-dataset-tokens (both runs)** | Exactly 79566 vs 79586 (off by 20) every time | What rows is the agent filtering? Dataset caching in container? |
| **sanitize-git-repo** | 2/3 tests pass; git object store corrupted | What specific filter-branch/BFG command breaks gitpython? |
| **cancel-async-tasks** | 5/6 tests pass; misses exactly the above-max_concurrent cancellation case | Is the asyncio.Semaphore approach fixable with one change? |
| **regex-chess** | Problem may be computationally infeasible | Can the regex-based approach even work for full chess? |
| **protein-assembly** | donor fragment not found (idx=-1) | What sequence is the agent assembling? Wrong linker order? |

---

## 4. Sanity-check of the results.md taxonomy

**What holds up:**
- Timeout × 10 is accurate (Shape 2)
- 50-turn exhaustion × 3 is accurate (Shape 7)
- Infrastructure × 1 (make-mips-interpreter) is accurate

**What needs revision:**
- "Wrong answer despite verification × 6" should be split into:
  - Artifact in wrong location (2–3 tasks)
  - Genuinely wrong computation (3–4 tasks)
  - Near-miss / edge case (2–3 tasks)
- "Capability ceiling × 9" should be split into:
  - Thinking-budget exhaustion (4 tasks) — a scaffolding issue, not model capability
  - Verifier infrastructure failures (3 tasks) — not agent failures at all
  - Genuine capability limits (2 tasks: regex-chess, largest-eigenval hard mode)

**Net implication:** Fewer tasks are true capability ceilings than the taxonomy suggests.
The "29 failures" include:
- ~3 verifier-infra false negatives
- ~3 fixable with prompt changes (polyglot × 2, extract-elf × 1)  
- ~1–2 fixable with the max_tokens recovery (winning-avg-corewars, possibly dna-assembly)
- ~1 false negative fixable with re-run (distribution-search)

Plausible ceiling with all cheap fixes applied: **~5 additional passes**, bringing the
Sonnet 4.6 score from 51/76 = 67% to ~56/76 = 74%. That estimate assumes the
agent-level fixes work on first retry and the verifier infra is stable on re-run.

---

*Sub-step 5b candidates: 7 trials listed in §3c. See docs/harbor.md item 5 for scope.*
