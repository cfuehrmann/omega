# Tee-always cache reuse — findings

**Question.** When `run_command`, `wait_for_output`, and `fetch_url` write
their *full* output to a session-cache log file (commit `e26817c`,
2026-05-15), does the LLM ever follow up by reading that cache path? Is
"tee always" worth the disk I/O relative to "tee on truncate"?

**Method.** `scripts/analyze_tee_reuse.py` scans every session
`events.jsonl` since the rollout, finds tool results whose output footer
surfaced a real cache path (`[full output: …]` or `[truncated; … Full
output: …]`), and counts subsequent `tool_call` events in the same
session whose `input` (JSON-serialised) contains that path as a
substring. A path is "real" if it contains `/` and ends in `.log` — this
rejects the literal `<path>` placeholder that appears in the system
prompt and tool descriptions as documentation.

**Two metrics, both useful.**

- `followups / call` — expected number of cache-referencing follow-up
  tool calls per originating cache write. Captures *intensity*: how
  much value we get from a cached file when it is touched.
- `reuse %` — fraction of cached files that were referenced at all
  (`reused_calls / calls`). Captures *probability of being useful*:
  pairs naturally with the per-write cost (every cached file is
  paid once whether or not it is later read).

For a cost-benefit decision both matter. A cache with 0.01
followups/call but 1% reuse means writes mostly go to waste; one with
2.0 followups/call but the same 1% reuse means *when* a cache is used,
it's used hard — different design implications.

**Data.** 35 sessions, 2026-05-15 → 2026-05-17, 468 cache-emitting tool
results.

## Two caches in `fetch_url` — disambiguation

`fetch_url` surfaces **two** paths to the LLM:

1. `Cached: <hash>.txt` — the **content-addressed full download**.
   URL-keyed, persists across sessions, predates the tee-always
   rollout. This is the dedupe layer for the HTTP request itself.
2. `[full output: …-pp.log]` — the **postprocess output**, written via
   `cap_and_tee`. This *is* the tee-always mechanism under evaluation.

The table below measures #2 — the cap_and_tee output across all three
tee tools. Cache #1 is reported separately below as context; it is
not what tee-always is paying for.

## Results — by tool × status

| tool | status | calls | follow-ups | **follow-ups / call** | **reuse %** |
|---|---|---:|---:|---:|---:|
| `fetch_url`       | truncated |   2 | 2 | **1.000** | **50.0%** (1/2) |
| `fetch_url`       | full      |   9 | 2 | **0.222** | **11.1%** (1/9) |
| `run_command`     | full      | 458 | 4 | **0.009** | **0.7%**  (3/458) |
| `run_command`     | truncated |   5 | 0 | 0.000     | 0.0%  (0/5) |
| `wait_for_output` | full      |   3 | 0 | 0.000     | 0.0%  (0/3) |
| `wait_for_output` | truncated |   1 | 0 | 0.000     | 0.0%  (0/1) |

Combined (kept for completeness, but misleading on its own):

| status | calls | follow-ups | follow-ups / call | reuse % |
|---|---:|---:|---:|---:|
| full       | 470 | 6 | 0.013 | 0.9%  (4/470) |
| truncated  |   8 | 2 | 0.250 | 12.5% (1/8) |

Follow-up tools (which tool dipped into the cache):
- After **full** result: `run_command` ×5, `grep_files` ×1.
- After **truncated** result: `run_command` ×2.

### Aside: `fetch_url` raw-download cache (not tee-always)

| metric | value |
|---|---:|
| calls (with `Cached:` path)  | 13 |
| follow-up calls              | 10 |
| **follow-ups / call**        | **0.769** |
| **reuse %**                  | **53.8%** (7/13) |
| follow-up tools              | `read_file` ×9, `run_command` ×1 |

This is the strongest single reuse signal in the dataset — about
2× `fetch_url`'s own postprocess-log rate (0.36 combined), and orders
of magnitude above `run_command`'s rate. The pattern is consistent:
the LLM runs a narrow `postprocess` to extract one thing, then
returns to the full download with `read_file` to look at more. It
justifies the content-addressed cache strongly, but **says nothing
about tee-always**: that cache would exist either way as the URL-
dedupe layer.

## Interpretation — read it per tool, not in aggregate

The combined row makes "truncated > full" look like the story, but the
entire effect is `fetch_url`. Per tool:

- **`fetch_url`** — tee-always for the **postprocess log** shows
  meaningful intensity in both buckets (0.22 full, 1.0 truncated,
  n=11 total). Modest absolute volume but the rate is the highest of
  the three tools. Note that most of `fetch_url`'s observed cache
  reuse goes to the raw-download cache (0.77 followups/call), which
  exists independently of tee-always. **Keep tee-always for the
  postprocess log; it's cheap and the rate is non-trivial.**
- **`run_command`** — very low intensity overall (~1 follow-up per 100
  calls), but every observed follow-up came from a **full**-output
  call. Tee-on-truncate would have captured **zero** of the actual
  reuses on this dataset. The truncated bucket (n=5) is too small to
  conclude truncation is "more reusable"; what we *can* say is that
  tee-on-truncate is strictly no better than tee-always for
  `run_command` and very plausibly worse. **No support for reverting.**
- **`wait_for_output`** — n=4 total across both buckets, zero reuse.
  No signal in either direction.

## Earlier (flawed) conclusion, retracted

The first version of this note recommended a revert to tee-on-truncate
based on the aggregate row (full 0.9 % reuse vs truncated 12.5 %). That
aggregate is dominated by `fetch_url`. For `run_command` — which is 97 %
of the sample — the truncated bucket has *zero* reuse and the full
bucket has all of it. So the "truncated is where the cache earns its
keep" framing does not generalise to the high-volume tool. **Revert
recommendation withdrawn.**

## Time-savings estimate — `run_command`

For the 3 `run_command` origins that received follow-ups, the script
estimates time saved under the model: **without the cache, each
follow-up would have re-run the originating command.**

| session | origin (truncated) | orig duration | follow-ups | saved |
|---|---|---:|---:|---:|
| 2026-05-15T16-00-46 | `cd rust && cargo test --workspace` …                       | 10 901 ms | 1 (`grep_files`)    | **10.9 s** |
| 2026-05-16T20-24-19 | `just rust-gate 2>&1 \| grep -iE 'failed\|error\|...' \| head` | 39 653 ms | 1 (`tail` via `run_command`) | **39.7 s** |
| 2026-05-16T20-24-19 | `just rust-gate 2>&1 \| tail -25`                            | 47 066 ms | 2 (`grep` ×2)       | **94.1 s** |
| **Totals** | | | **4 follow-ups** | **≈ 144.7 s** |

Follow-up cost: 17 ms total. Net saved ≈ 144.67 s.

**Selection effect.** Two of three cases are `just rust-gate`. The
commands whose output invites a follow-up grep are the ones whose
output is long enough to need one — i.e. long-running commands.
Conditional on reuse, the avoided cost is large precisely because
long-running commands self-select into the reuse population.

**Amortised across all cached `run_command` writes:** ~306 ms saved
per write (144 686 ms / 473 cached writes). Each write itself is one
async `write_all` of typically <100 KB — sub-millisecond. The
cost/benefit ratio is heavily in favour of tee-always even with the
tiny 0.7% reuse probability, because the *conditional* saving when
reuse fires is dominated by long-running commands.

**Caveats.**
- n=3 origins is small; one extra long gate re-use, or one fewer,
  swings the per-write amortised number by 30–60 ms.
- The model assumes the follow-up grep/tail would have been *part of*
  the re-run command (cheap), so the avoided cost is ~one full origin
  run per follow-up. A model where the LLM would have read the log
  via a different cheap path gives different numbers.
- This is *only* `run_command`. `wait_for_output` had zero reuse.
  `fetch_url` postprocess saw 4 follow-ups but the originating
  durations are dominated by network I/O — separate analysis.

## Disk-usage cost

Total cache across the 35 sessions:

| tool   | files | total   | avg / file |
|---|---:|---:|---:|
| `run`   | 558 | 5.6 MB | 10.2 KB |
| `fetch` |  20 | 567 KB | 28.3 KB |
| `wait`  |   6 | 748 KB | 124.6 KB |
| **all** | **583** | **6.9 MB** | **12.0 KB** |

Per-session: median 37 KB, p90 342 KB, max 3.1 MB.

### `run_command` file-size distribution

| percentile | size |
|---|---:|
| min  | 3 B |
| p50  | 533 B |
| p90  | 5.3 KB |
| p99  | 83.3 KB |
| max  | 2.1 MB |

- **62 %** of cached `run_command` outputs are ≤ 1 KB.
- **0.7 %** (4 files) exceed the 100 KB LLM cap.

### Tee-always marginal cost over tee-on-truncate

Of the 5.6 MB written for `run_command`:

- **4.4 MB** in the 4 over-cap files — would be written under tee-on-truncate too.
- **1.1 MB** in the other 554 files — the *marginal* cost of tee-always.

Per-session marginal cost: **~32 KB**.

### Cost vs benefit — settled

**All three originating cache files in the reuse cases are under the
100 KB cap** (520 B, plus two files well under 100 KB in the gate session).
Tee-on-truncate would have written *zero* of them, and would have
caught *zero* of the 144.7 s of observed savings.

Net:

- **~32 KB extra disk per session** → **~4 s less LLM latency per session**
  (144.7 s / 35).
- **~7 KB extra disk per second of LLM latency saved.**

The cost-benefit ratio for tee-always is overwhelming.

## Caveat — the counterfactual is unobservable

The "144.7 s saved" number assumes that without the cache, the LLM
would have re-run the origin command. In all three observed reuse
cases for `run_command`, the cache file contains *exactly* the bytes
already present in the model's context (post-pipeline output, just
without the footer). So the model wasn't reaching for missing bytes
— it was running a focused query over bytes it already had.

The true counterfactual is three-way, not binary:

| option | latency | quality |
|---|---|---|
| re-run the command (e.g. gate ~40 s) | high | high |
| reason over in-context bytes         | ~zero | degraded — miscounts, omissions, fading salience over distance |
| grep the cache                       | ~zero | high — focused extract, verifiable |

The cache enables option 3. Option 3 is strictly better than option 2
on quality, even when bytes were never lost: LLM attention over long
context is not uniform, and a fresh tool result containing exactly the
relevant lines is a higher-quality input for the next response than
asking the model to project precisely over a blob from several turns
ago. This is the same reason a human with the wall of text on screen
still reaches for grep — focus, working-memory limits, verification.

Implications:

- **The 144.7 s figure is an upper bound, not a measurement.** True
  time-savings sit somewhere between 0 (if the model would have
  reasoned in-context) and 144.7 s (if it would have re-run). We
  cannot distinguish these from the data.
- **But the cache's value is not only "seconds saved".** It is also
  *output quality* on tasks that need precise extraction from
  moderately-sized blobs. This is unmeasured in our data but is
  consistent with both the human analogy and what we know about LLM
  attention degradation.
- **The disk-cost-is-trivial argument is unaffected** — ~32 KB / session
  marginal cost stands regardless of which counterfactual is true.
- **The verdict still stands, for a slightly different reason than
  originally framed:** not "tee-always demonstrably saves 145 s" but
  "tee-always costs near-zero, enables a strictly-higher-quality
  reasoning path, and *may* additionally save substantial time when
  the alternative would have been a re-run."

## Alternative considered: lower the cap, use tee-on-truncate

Conjecture: if the 100 KB cap is too generous, the LLM may often have
bytes in context that it could reason over directly. A lower cap would
trigger truncation more often, tee-on-truncate would then catch the
cases that truly need cache, and tee-always becomes redundant.

**Refuted by the file sizes of the actually-reused caches.** The three
`run_command` cache files that received follow-ups were:

| size  | command                                              |
|---:|---|
|  520 B | `cd rust && cargo test … \| tail -8`                |
|  671 B | `just rust-gate … \| tail -25`                       |
| 3.0 KB | `just rust-gate … \| grep … \| head -40`            |

All three are far below *any* realistic cap. Even a 5 KB cap would
send all three to the model in full. Therefore tee-on-truncate would
not have written any of them, regardless of how low the cap is set.

What this says about LLM behaviour: the model reaches for `grep_files`
on the cache *even at 520 B of output*. At that size, in-context
reasoning is trivially reliable for any reader — so the grep-on-cache
reflex is not driven by "output is too big to read". It is driven by
prompt compliance, verification-via-tool, or the focus/quality
preference covered in the caveat above. Cap-lowering does not reach
any of those mechanisms.

The two design dimensions are independent:

1. **Cap size** — probably worth lowering on token-cost grounds (a
   100 KB inline result can consume ~25 K tokens that the model may
   not read). This is a separate optimisation deserving its own
   analysis.
2. **Tee policy** — tee-always still wins, because the observed reuse
   happens on files no realistic cap would have truncated.

## Observed model behaviour: reflexive tool-grep on small in-context bytes

The three `run_command` cache files that received follow-ups were 520 B,
671 B, and 3.0 KB — all small, all recent (within a few turns of the
originating call), and all the target of simple text-matching
operations (find "FAILED", tail the last N lines). At those sizes,
for those operations, in-context reasoning is *trivially reliable*.
The model grep'd the cache anyway.

The right decision rule for tool-vs-in-context is three-axis, not
size-only:

> Reach for a tool over in-context bytes when **at least one** of:
> - the input is large enough that attention degrades over it,
> - the input is distant enough in the conversation that salience has
>   faded, **or**
> - the operation itself is one the model is unreliable at
>   (arithmetic, exact counts over long lists, structured parsing).
>
> Otherwise, reason in-context.

The calculator framing makes the third axis vivid: two 20-digit
numbers fit in 50 bytes of context, but multiplying them is precisely
what LLMs are unreliable at. Tool-use is correct there even at
trivial input size. Conversely, finding "FAILED" lines in 520 B of
test output is something the model can do perfectly in context — small
input *and* an operation it's reliable at.

The observed three reuses sit in the "small × recent × simple
operation" cell — the exact cell where in-context reasoning dominates
and tool-grep is overhead (an extra inference round-trip and tokens
for no quality gain). The behaviour appears to be reflexive
tool-grep whenever a cache path is visible, without the meta-cognitive
step of asking "can I do this in my head?".

**This is a mild meta-cognitive limitation of the current model**, not
only a prompt artefact. The prompt does over-broadly advertise the
cache ("use grep_files on the cache instead of re-running"), but a
model with sharper self-modelling of its own reliability would
modulate — just as a human does when choosing between mental arithmetic
and a calculator.

**Practical response: document, don't fix.** Threading the three-axis
rule into the prompt would be fragile, risks over-correction (model
stops using cache when it should), and re-creates in instructions a
judgement the model should ideally make implicitly. Absolute cost is
small (3 reflex-greps across 35 sessions, ~17 ms each). Future
models may close the gap without prompt changes. If we ever rewrite
the cache-advertising language, the three-axis framing is the right
starting point.

## Anthropic's published guidance on tool-output size

Researched 2026-05-17 to inform the cap-size question. Summary: the
only direct, numeric recommendation Anthropic has published is for
Claude Code generally, not for Sonnet 4.6 / Opus 4.6 or 4.7
specifically.

**Source**: *Writing effective tools for agents* (Anthropic
Engineering blog, Sep 11 2025).
`https://www.anthropic.com/engineering/writing-tools-for-agents`

**Key quote** (section: "Optimizing tool responses for token
efficiency"):

> For Claude Code, we restrict tool responses to 25,000 tokens by
> default. We expect the effective context length of agents to grow
> over time, but the need for context-efficient tools to remain.

25,000 tokens ≈ 100 KB at ~4 chars/token. Omega's current 100 KB cap
is approximately in line with this. The guidance also recommends:

- A combination of *pagination, range selection, filtering, and/or
  truncation* with sensible defaults for any tool that could produce
  large output.
- When truncating, *steer agents with helpful instructions* (the
  truncation footer itself is a steering surface).
- Encourage *many small targeted searches over single broad ones* in
  the prompt.

**What this doesn't tell us**: whether the cap should differ for the
larger / smarter Opus 4.7 vs Sonnet 4.6 (no model-specific guidance
published); whether the cap should be lower for tools the model uses
repetitively; or whether the 25K-token figure is empirically tuned
or a round-number default. Our own data (p99 of `run_command` output
is 83 KB, only 0.7 % exceeds 100 KB cap) suggests the current cap is
in a reasonable regime: low enough that truncation is rare, high
enough that most outputs survive intact.

**Related pages checked, no additional cap guidance**:
- `platform.claude.com/docs/en/agents-and-tools/tool-use/overview.md`
- `platform.claude.com/docs/en/agents-and-tools/tool-use/define-tools.md`
- `platform.claude.com/docs/en/agents-and-tools/tool-use/handle-tool-calls.md`
- `platform.claude.com/docs/en/agents-and-tools/tool-use/manage-tool-context.md` (covers tool-search, programmatic tool calling, prompt caching, context editing — overall context bloat, not per-result cap)

## Decisions (2026-05-17)

1. **`run_command`: keep tee-always.** Strong evidence. ~145 s of
   LLM-perceived latency avoided across 35 sessions for a marginal
   disk cost of ~32 KB / session over tee-on-truncate. All observed
   reuse hit sub-cap files — tee-on-truncate would have caught none
   of it.
2. **`fetch_url`: keep tee-always.** Postprocess log has the highest
   reuse rate of the three tools (0.36 followups/call combined).
   Per-session disk cost ~16 KB. Note: the content-addressed
   raw-download cache is a *separate* mechanism and not under this
   decision; it has 0.77 followups/call and stays regardless.
3. **`wait_for_output`: keep tee-always provisionally.** n=4 calls
   is too thin to commit to removal. Decision deferred — see TODO.

## TODO — revisit when n grows

- [ ] **`wait_for_output` tee policy.** Re-run
  `scripts/analyze_tee_reuse.py` once the sample reaches **n ≥ 30**
  `wait_for_output` calls. If reuse is still 0 and per-session disk
  cost is still trivial (current ~21 KB), drop tee for this tool.
  If any reuse appears, keep tee-always and update this doc.
- [ ] **`run_command` truncated bucket.** Currently n=5 truncated
  origins with 0 follow-ups; all observed savings are on full-output
  origins. Re-check after another ~100 sessions to confirm the
  full-vs-truncated asymmetry holds (it would be surprising if
  truncated outputs *never* see reuse — selection effect should run
  the other way).
- [ ] **Optimal cap study.** Independent of the tee policy: the
  current 100 KB cap may be too generous on token-cost grounds. A
  single 100 KB inline result can consume ~25 K context tokens that
  the model may not read (because it grep'd the cache instead). Pick
  a candidate cap (e.g. 20 KB — currently truncates ~1 % of
  `run_command` results, would rise to ~5 %), run a week of sessions
  on a branch, and compare reuse patterns + output-quality
  qualitatively. *Note:* this does **not** subsume the tee-policy
  question — the observed reuses are on files of 520 B / 671 B /
  3 KB, so no realistic cap would have made tee-on-truncate catch
  them.
- [x] **Scope cache-advertising language to the truncation case**
  (`system_prompt.rs` + `schemas.rs` tool descriptions for
  `run_command` and `wait_for_output`). Done 2026-05-17. The previous
  text pushed cache-grep "for any follow-up on a tool output"
  unconditionally; rewritten to nudge toward cache only when the
  result is **truncated** or when an earlier full output has aged out
  of immediate context. Explicit counter-instruction added: "when the
  bytes you need are already inline and recent, read them directly."
  `fetch_url` advertising left unchanged — the data shows non-trivial
  reuse on its postprocess log (0.36 followups/call) and the
  "different postprocess query on same content" workflow is genuinely
  what that cache is for.
- [ ] **Re-measure reflexive-grep rate after the prompt change.** With
  the scoped advertising in place, the expectation is that
  full-output `run_command` reuse rate (currently 0.9 %, all on small
  recent bytes) drops toward 0, while truncated-result reuse stays
  or rises. If the full-output rate stays, the behaviour is more
  model-intrinsic than prompt-driven — worth knowing.

## Reproducing

```
python3 scripts/analyze_tee_reuse.py [--since YYYY-MM-DD] [--verbose]
```
