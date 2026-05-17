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

## Where this leaves us

The data does not justify a blanket revert. It does suggest:

1. **Keep tee-always for `fetch_url`** unambiguously.
2. **Keep tee-always for `run_command`** by default — what little reuse
   exists is on full outputs, and the cost of tee'ing text we already
   sent inline is one short `write_all`.
3. **`wait_for_output`** is undecidable on n=4. Default-keep alongside
   `run_command` for consistency.
4. Re-run this analysis after another ~100 sessions, especially with an
   eye on truncated-bucket `run_command` (currently n=5 — too small to
   draw conclusions). If a larger sample still shows the full-vs-
   truncated asymmetry inverted for `run_command`, we have a robust
   finding; if not, revisit.

## Reproducing

```
python3 scripts/analyze_tee_reuse.py [--since YYYY-MM-DD] [--verbose]
```
