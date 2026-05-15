# Token optimizations (non-tee follow-ups)

*Status: backlog. Not blocked, not started. Tee-on-truncate
(`backlog/tee-on-truncate.md`) is the first and largest win;
these are the smaller follow-ups identified by the same audit
(`backlog/tee-on-truncate-audit.md`).*

Ordered by recommended priority. Each item is independent —
they can be picked up one at a time, in any order, once
tee-on-truncate has landed (or in parallel if someone wants).

---

## 1. Investigate the `git add` 8.6 MB anomaly

**Symptom.** Local audit attributes 8.6 MB across 698 calls to
`git add` — p95 = 57 KB per call. `git add` produces near-zero
output on success. Almost certainly attributable to chained
commands like `git add -A && git commit -m "…"` where the
commit's pre-commit-hook output is attributed to argv[0] = `git add`.

**Why it matters.** Single biggest line item in the local audit.
~2 M tokens.

**Hypothesis.** The pre-commit hook's "tail -60 of gate log on
failure" runs the gate even on successful commits when (a) a
prior gate already wrote a stale log, or (b) some chatter
escapes the `> /dev/null` redirect. Need to read the actual
session outputs to confirm.

**Next step.** Grep the largest `git add` outputs in
`.omega/sessions/*/events.jsonl` for what they actually
contain. 30 minutes of work. Then either fix the hook or
adjust how `run_command` attributes argv[0].

**Not in benchmark scope** — bench trials don't run the gate.
Pure local-development win.

---

## 2. Strip `\r`-progress in `run_command` output

**Symptom.** Progress bars and carriage-return-redrawn status
lines (e.g. `\rRead 1M words\rRead 2M words\rRead 3M words…`)
leak through verbatim, hugely inflating the byte count for
zero information value — only the last frame matters.

**Bytes affected.** ~3.2 M tokens local (ansi/progress class),
~95 K bench. Tee-on-truncate alone won't fix this; the bytes
that reach the LLM are still bloated.

**Fix.** In the `run_command` output path, post-process:
collapse runs of `\r`-separated lines down to the last one
before each `\n`. Strip bare ANSI escape sequences too
(SGR colour codes, cursor moves).

**Risk.** Some commands use `\r` semantically. Mitigate by
making this a default-on flag with an opt-out, or scope it
to known-noisy commands.

**Effort.** Small — one regex pass on the output buffer.

---

## 3. Cap `wait_for_output` output

**Symptom.** `wait_for_output` is uncapped. Single calls
returned 1.1 MB locally and ~97.8 KB repeatedly in benchmark
(`cargo mutants`, `fasttext` training, `CompCert` proof builds).

**Fix.** Apply the same `cap_and_tee` helper from
tee-on-truncate to `wait_for_output`. **This is already
listed in the tee-on-truncate implementation order**, so it
will be handled there — listed here only so the audit
backlog is complete.

---

## 4. Prompt nudge against full reads of large planning docs

**Symptom.** Top-25 largest single outputs in local audit
include repeated full `read_file` of `rust-migration.md`
(~107 KB) across different sessions. Each costs ~27 K tokens.
Same pattern likely with other large markdown.

**Fix.** Add to the system prompt: "For large planning or
reference docs (>200 lines), prefer `grep_files` for the
relevant section, or `read_file` with `offset`/`limit`, over
a full read."

**Effort.** One-line prompt change. No code.

**Caveat.** Soft constraint; effectiveness will need
measurement after the change (re-run the audit).

---

## 5. Reduce shell-util usage in favour of native tools

**Symptom.** Benchmark trials show heavy use of `cat`
(818 KB), `grep` (327 KB), `ls` (253 KB) via `run_command`,
even though Omega has native `read_file` / `grep_files` /
`list_files` / `find_files` that are already capped and
structured.

**Why this is mostly OK.** In bench trials, the model often
needs to inspect arbitrary unknown files where it doesn't
know the path in advance — `cat`/`grep` via shell is
sometimes the only ergonomic option. So this is partially
unavoidable.

**Fix.** Strengthen the existing system-prompt nudge toward
native tools, particularly for `cat` of files whose paths
are already known. Possibly auto-rewrite `cat <path>` →
`read_file(path)` in `run_command` as a courtesy. Risky:
shell `cat` has different output semantics (no offset/limit
metadata).

**Probably do this last, or not at all.** Tee-on-truncate
absorbs most of the damage already.

---

## 6. Consider RTK shell-out wrapper (still optional)

**Recap.** RTK (Rust Token Killer) is a CLI proxy that
applies per-command filters to known-noisy programs
(`cargo test`, `pytest`, `jest`, `git`, …). See the earlier
RTK discussion summarised at the top of
`backlog/tee-on-truncate-audit.md`.

**Bench data verdict.** RTK's supported-command set
(test runners, package managers) is **barely used in
benchmark trials**. Bench bytes are dominated by one-off
custom scripts that no off-the-shelf filter understands.

**Local data verdict.** RTK could help with the `cargo
test` / `just gate` mass (~5 MB local), but tee-on-truncate
+ `\r`-stripping + the gate fix above target the same mass
more directly and stay in-tree.

**Conclusion.** Skip unless post-implementation re-audit
shows residual `cargo test`-style noise that the in-tree
fixes didn't catch. Even then, integrate as a runtime
shell-out wrapper (detect `rtk` on PATH, prepend it for
argv[0] ∈ {known commands}, config-gated), **not** as a
crate dependency.

---

## How to revisit

After tee-on-truncate ships, **re-run the audit**:

```
python3 scripts/token_audit.py
```

Then compare against `backlog/tee-on-truncate-audit.md`. The
delta tells us:

- Whether tee-on-truncate moved the needle as expected.
- Which of items 1-5 above are still material.
- Whether item 6 is needed at all.

This is the single most important follow-up step. The audit
script is the measurement instrument; everything else is
hypothesis.
