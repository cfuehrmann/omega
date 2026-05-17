# Design: unified tee-on-truncate for tool outputs

*Status: **implemented, expanded to footer-always**. See “Decision
log” below. Audit-driven; see `scripts/tee_footer_audit.py` for the
repeatable measurement.*

## Decision log

### 2026-05-15 — ship footer-always (not just tee-on-truncate)

**What we shipped.** `cap_and_tee` now appends a footer to **every**
result, not only when the cap fires:

- Non-truncated: `\n[full output: <path>]`
- Truncated: `\n[truncated; showed first 100 KB of 487 KB. Full output: <path>]`

Empty data still skips the footer (pointing the LLM at zero bytes is
just noise). Tool descriptions (`schemas.rs`) and the agent system
prompt (`system_prompt.rs`) were updated to tell the model it can
`read_file`/`grep_files` the cache path for follow-up queries instead
of re-running.

**Why we went past tee-on-truncate.** Initial reasoning argued that
surfacing the path on every result was a token tax for unmeasured
benefit. Re-measurement with the corrected schema
(`scripts/tee_footer_audit.py`, tool_call field is `input`, not
`arguments` — see process lesson below) gave the opposite picture:

| Surface (local corpus, n=662 sessions)        | Surfaced | Reused | Rate |
|------------------------------------------------|---------:|-------:|-----:|
| `fetch_url` cache (`~/.cache` / `cache/fetch`) |      253 |     51 | **20.2 %** |
| Any cap_and_tee cache path                     |      316 |     86 | **27.2 %** |
| Project `just gate` log (`test-output/...`)    |      250 |    143 | **57.2 %** |

Reuse means: the LLM later issued a `read_file` / `grep_files` /
`run_command` whose arguments contain that exact path. The gate-log
number is the empirical existence proof — in session
`2026-04-03T14-51-15-957-e7849c8d` the model grepped
`test-output/gate-latest.log` eight times to investigate a failure.
Prompt-side mention of the path is what unlocks this reuse, which is
why we paired the code change with system-prompt guidance.

**Cost / benefit napkin math (local corpus).**

- Footer cost: 28,426 tool_results × ~40 tokens ≈ **1.1 M tokens**.
- Estimated savings: 27 % cap-cache reuse × ~28k results × ~800-token avg output
  ≈ **5.9 M tokens** avoided in re-derivation.
- Net: ~5× ROI, with headroom growing once the prompt nudge takes effect.

**Validation plan.** Run `scripts/tee_footer_audit.py` after ~20 new
local sessions (post-rollout). The audit separates:

- **Truncated results** — baseline both schemes share.
- **Non-truncated results** — only footer-always surfaces a path here.
  Reuse rate × avg savings on this subset *is* the delta that
  justifies (or kills) footer-always.

If the non-truncated subset shows <break-even reuse, revert to
tee-on-truncate (one-line change in `cap_and_tee.rs`).

### Process lesson

My first audit reported 0 % reuse across all corpora and recommended
tee-on-truncate. The bug: the events.jsonl schema uses `input` on
`tool_call`, but the script read `arguments`. Every tool-call scan
returned an empty dict. Lesson: **dump one event before parsing 1 000**.
The correction inverted the recommendation.

---

## Original design notes (pre-decision)


## Background

Audit of 842 local sessions and 305 benchmark trials
(`scripts/token_audit.py` → `test-output/token-audit.md`) found:

- **1 073** truncation-cap hits in local sessions (3.8 % of all
  tool_results); **55** in benchmark trials. Today `run_command`
  silently truncates at 100 KB — bytes beyond the cap are gone, and the
  LLM often re-runs the command with `| tail -200` to recover what it
  needed.
- `run_command` is the dominant byte producer in benchmark trials
  (54 %); `read_file` dominates locally (44 %). The pattern is
  consistent across both scopes: a small number of large outputs
  account for most of the waste.
- Top symptoms by tokens burned (local): truncated (4.3 M), big-dump
  (3.7 M), ansi/progress (3.2 M).
- Wait_for_output is uncapped — a single benchmark call returned 1.1 MB.

## Goal

One uniform pattern across every tool that emits arbitrary-length
output: **always stream the full output to disk; return a capped
window to the LLM with a footer pointing at the full log**.

Make this the explicit contract — not three slightly different
ad-hoc implementations.

## Current state per tool

| Surface | Cap | Tee to disk? | Notes |
|---|---|---|---|
| `fetch_url` | 8 000 chars on **postprocess output** | **Yes** — `~/.cache/omega/fetch_url/<url-hash>.txt` | Already the gold standard. Cache is content-addressed by URL hash; tool tells LLM the path. |
| Pre-commit gate | tails last 60 lines on failure | **Yes** — `test-output/gate-latest.log` (`just gate` tees internally) | Works but truncation budget (60 lines) is undersized for some failures. |
| `run_command` | 100 KB in-memory | **No** | The big gap. 1 073 silent-truncation events. Head-bias today regardless of exit code. |
| `wait_for_output` | none in code | **No** | Returned 1.1 MB in one bench call. |
| `read_file` | none (offset/limit) | n/a | Up to the LLM to paginate. |
| `grep_files` / `find_files` | match count | n/a | Semantic cap, not byte-based. Fine as is. |

## Design

### Single helper

`omega-tools` exposes one helper roughly like:

```rust
pub struct CappedOutput {
    pub body: String,        // capped, ready to return to LLM
    pub truncated: bool,
    pub total_bytes: usize,
    pub log_path: PathBuf,   // always set — tee-always
}

pub async fn cap_and_tee(
    stream: impl AsyncRead,
    cap: usize,
    bias: TruncationBias,    // Head | Tail | HeadAndTail
    log_path: PathBuf,
) -> io::Result<CappedOutput>
```

All four sites (`run_command`, `wait_for_output`, `fetch_url`, gate)
end up using or being consistent with this. The LLM-facing footer is
a single canonical string the model learns once.

### Tee-always (decided)

Always stream to disk, not only when the cap fires. Rationale:

- Disk is cheap; tokens are not.
- The LLM can `grep`/`read_file` the log later instead of re-running.
- Symmetric with `fetch_url`, which already caches every download.

### Head vs tail bias

Per-tool, not global. Default rule for `run_command`:

- `exit code == 0` → **head-bias** (keep first N KB)
- `exit code != 0` → **tail-bias** (keep last N KB)

Rationale: build/test failures put the actionable bit at the end
(`error[E…]`, panic messages, `FAILED` summaries) after thousands of
"Compiling foo" / "test … ok" lines. Successful output usually has
the headline at the top.

Optional `truncation_bias: "head" | "tail" | "middle"` parameter on
`run_command` lets the model override when it knows better.

### Footer format

```
[truncated; showed last 100 KB of 487 KB. Full output: test-output/run/2026-05-14T19-01-33-cargo-test.log]
```

Three pieces of information the model needs:
1. **What was kept** (head/tail/middle) so it knows what direction to look in.
2. **Size ratio** so it knows the magnitude of what it didn't see.
3. **Path** for the recovery.

### Cache layout — DECIDED

**Colocate the cache with the session, never auto-delete.**

```
.omega/sessions/<session-id>/
    events.jsonl
    context.jsonl
    session.jsonc
    cache/                                      # NEW
        run/    <ts>-<argv0>.log
        wait/   <ts>-pid<N>-<argv0>.log
        gate/   <ts>.log
        fetch/  <url-hash>.txt                  # was ~/.cache/omega/fetch_url/
```

Rationale (user's insight, 2026-05-14):

1. **Cross-session retention of shell-output caches doesn't help** —
   an LLM in a different session has no knowledge of the paths
   referenced in another session's log footers. So a global
   cross-session cache is wasted disk for that case.
2. **But session-bound long retention IS useful**: when a session is
   resumed (strict resumption is a planned feature), the cached
   files referenced by old `tool_result` footers still resolve.
3. **Self-contained sessions**: tarball `.omega/sessions/<id>/` and
   you get the log AND the bytes it references. No orphaned cache.
4. **No separate GC**: cache lifetime = session lifetime, governed
   by whatever session-retention policy exists.
5. **Off `test-output/`**: that path is co-occupied by test artifacts
   and gets cleaned by `just clean`.

Naming convention:
- `<ISO-timestamp>-<short-tag>.log`
- For `run_command`: tag = sanitised `argv[0]` (same logic
  `scripts/token_audit.py` uses): `cargo-test`, `just-gate`,
  `git-status`. Timestamp first so `ls` sorts chronologically.
- For `wait_for_output`: tag = `pid<N>-<argv[0]-of-bg-process>`.

### `fetch_url` cache — also moves into the session (revised 2026-05-14)

Initial draft kept `fetch_url` at `~/.cache/omega/fetch_url/` for
cross-session dedup. Pushback from user; agreed: cross-session hit
rate is realistically low (different sessions, different URLs),
bandwidth/latency savings are trivial, and token savings are zero
(LLM only sees the postprocess output regardless). Not worth a
second storage class with its own lifetime policy.

What we keep:
- **Download-buffer role**: postprocess runs against the file on disk.
  Architectural, unchanged.
- **Within-session re-grep**: footer tells LLM the cache path; LLM
  can `grep_files` / `read_file` it with a different query instead
  of re-fetching. Same content-addressed naming (`<url-hash>.txt`)
  preserves within-session dedup.

What we drop:
- Cross-session dedup. `~/.cache/omega/fetch_url/` goes away.
- The second lifetime policy (LRU + size cap). One lifetime rule for
  everything: lives with the session, dies with the session.

Net effect: one cache root, one lifetime policy, one footer format.

### Gate log consolidation

- Move per-invocation log to `.omega/sessions/<id>/cache/gate/<ts>.log`.
- Keep `test-output/gate-latest.log` as a **symlink** to the most
  recent one, for backwards compatibility with the pre-commit hook's
  `tail -60` and README references — zero breakage.
- Bonus: per-invocation history (today's log is overwritten every commit).

## Implementation order

1. Land the `cap_and_tee` helper and unit tests.
2. Migrate `run_command` to use it (head/tail bias by exit code).
3. Migrate `wait_for_output` to use it (cap added).
4. Decide cache layout + lifetime; migrate `fetch_url` naming and the
   gate log to the new convention.
5. Update system prompt: document the footer and the
   `truncation_bias` parameter.

## Invariant: paths must be relative to `sessions_root`

All tee output paths MUST be resolved relative to the configured
`sessions_root`, never to `cwd` or a hardcoded `.omega/...`.

Why this matters: in Harbor/Terminal-Bench trials, sessions are
routed to `/logs/agent/omega-session/` (a bind-mounted directory)
while the agent's `cwd` is `/app` (where benchmark verifiers inspect
files). If tee output ever leaks into `cwd`-relative paths, it
would pollute `/app` and corrupt verifier results.

Reference: `bench/omega_agent.py`,
`OMEGA_RUST_SESSION_ROOT = "/logs/agent/omega-session"`. The CLI
flag/env that controls `sessions_root` is the single point of
truth; `cap_and_tee` must take a `sessions_root`-derived path,
not construct one itself.

## Open questions to discuss

- **Plumbing the session ID into tool context.** `omega-tools` today
  doesn't know which session it's running for. We need a small API
  addition so `cap_and_tee` can resolve
  `.omega/sessions/<id>/cache/<tool>/`. Likely a `ToolCtx` parameter
  threaded from the agent loop.
- **`read_file` over a tee log.** When the model wants to re-read a
  capped output, should it use `read_file` (which has no cap) or a
  dedicated `tail`/`grep` path? Probably `read_file` — one less tool.
- **Cache size sanity per session.** A pathological session could
  fill `.omega/sessions/<id>/cache/` with gigabytes. Soft warning
  threshold? Per-file size cap?
- **Migration of existing `test-output/gate-latest.log` consumers.**
  Find all references; the symlink approach should cover them, but
  verify before changing layout.

## Related work

Other token optimizations identified by the same audit but
*not* covered by tee-on-truncate are tracked in
`backlog/token-optimizations.md`. Pick those up after this lands
and the audit has been re-run to measure the delta.

## References

- Audit data and methodology: `backlog/tee-on-truncate-audit.md`
  (copy of `test-output/token-audit.md`, durable across `just clean`)
- Audit script: `scripts/token_audit.py` (re-runnable, two-scope:
  local sessions + benchmark trials)
- Current `run_command` cap: `crates/omega-tools/src/tools/run_command.rs` (`OUTPUT_CAP = 100_000`)
- Current `fetch_url` tee (the model to emulate): `crates/omega-tools/src/tools/fetch_url.rs` (`cache_file`)
- Gate tee: `scripts/pre-commit` + `just gate` writing to `test-output/gate-latest.log`
