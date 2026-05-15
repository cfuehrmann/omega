#!/usr/bin/env python3
"""Audit Omega tool outputs across two scopes:

  1. Local dev sessions (.omega/sessions/*/events.jsonl)
  2. Benchmark trials   (bench/jobs/*/<task>/agent/events.jsonl)

Both scopes use the same event schema. They are reported separately so
optimisations are not overfit to "Omega working on its own repo".

Writes a markdown report to test-output/token-audit.md and also prints
a short summary to stdout.

A token ≈ 4 chars (rough).
"""
from __future__ import annotations

import argparse
import json
import re
import shlex
import statistics
import sys
from collections import Counter, defaultdict
from pathlib import Path

# --- heuristics ---------------------------------------------------------

TRUNC_MARKERS = (
    "[Output truncated at",
    "[Truncated at",
    "[truncated:",
    "postprocess output truncated",
)

ANSI_RE = re.compile(r"\x1b\[[0-9;?]*[A-Za-z]")
CARRIAGE = "\r"


def argv0(cmd: str) -> str:
    """Coarse program name for grouping; skips past `cd X &&` and env prefixes."""
    try:
        toks = shlex.split(cmd)
    except ValueError:
        toks = cmd.split()
    i = 0
    while i < len(toks):
        if re.match(r"^[A-Z_][A-Z0-9_]*=", toks[i]):
            i += 1
            continue
        if toks[i] == "cd" and i + 2 < len(toks) and toks[i + 2] in ("&&", ";"):
            i += 3
            continue
        break
    if i >= len(toks):
        return "(empty)"
    head = toks[i].split("/")[-1]
    if head in ("just", "cargo", "git", "npm", "pnpm", "yarn", "rg", "uv",
                "pip", "python3", "python", "docker", "kubectl", "gh",
                "bun", "bunx", "npx", "go", "make"):
        if i + 1 < len(toks) and not toks[i + 1].startswith("-"):
            return f"{head} {toks[i + 1]}"
    return head


def symptom_labels(out: str) -> list[str]:
    labels = []
    if not out:
        return labels
    lines = out.splitlines()
    n = len(lines)

    compiling = sum(1 for l in lines if l.lstrip().startswith("Compiling "))
    if compiling >= 10:
        labels.append("compile-spam")

    test_ok = sum(1 for l in lines if re.match(r"^test .+ \.\.\. (ok|ignored)$", l))
    if test_ok >= 20:
        labels.append("test-enum")

    if ANSI_RE.search(out) or out.count(CARRIAGE) >= 5:
        labels.append("ansi/progress")

    git_keys = ("Enumerating objects", "Counting objects",
                "Compressing objects", "Receiving objects",
                "Resolving deltas", "remote: Counting")
    if sum(1 for k in git_keys if k in out) >= 2:
        labels.append("git-progress")

    if n >= 30:
        nonblank = [l for l in lines if l.strip()]
        if nonblank:
            ratio = len(set(nonblank)) / len(nonblank)
            if ratio < 0.5 and len(nonblank) >= 30:
                labels.append("repetition")

    if any(m in out for m in TRUNC_MARKERS):
        labels.append("truncated")

    if n >= 500 and sum(1 for l in lines if l.startswith(" ") or l.startswith("\t")) > n * 0.4:
        labels.append("big-dump")

    return labels


def percentile(xs, p):
    if not xs: return 0
    xs = sorted(xs)
    k = max(0, min(len(xs) - 1, int(round((p / 100) * (len(xs) - 1)))))
    return xs[k]


def fmt_bytes(n):
    if n < 1024: return f"{n}B"
    if n < 1024 * 1024: return f"{n / 1024:.1f}K"
    return f"{n / 1024 / 1024:.1f}M"


def tok(n): return n // 4


# --- core audit ---------------------------------------------------------

def audit(event_paths: list[Path], scope_label: str, min_bytes=2000, top=25):
    """Return a markdown report string for the given list of events.jsonl files."""
    tool_bytes: Counter = Counter()
    tool_count: Counter = Counter()
    tool_sizes: dict[str, list[int]] = defaultdict(list)
    rc_bytes: Counter = Counter()
    rc_count: Counter = Counter()
    rc_sizes: dict[str, list[int]] = defaultdict(list)
    symptom_bytes: Counter = Counter()
    symptom_count: Counter = Counter()
    truncation_hits = 0
    total_bytes = 0
    total_results = 0
    unused_bytes = 0
    unused_eligible_bytes = 0
    big_outputs: list[tuple[int, str, str, str]] = []
    per_session_bytes: list[tuple[int, str]] = []

    scanned = 0
    for ev_path in event_paths:
        if not ev_path.exists():
            continue
        scanned += 1
        calls: dict[str, dict] = {}
        results: dict[str, dict] = {}
        seq: list[tuple[str, dict]] = []
        try:
            with ev_path.open() as f:
                for line in f:
                    try:
                        e = json.loads(line)
                    except json.JSONDecodeError:
                        continue
                    t = e.get("type")
                    if t == "tool_call":
                        calls[e["id"]] = e
                        seq.append(("call", e))
                    elif t == "tool_result":
                        results[e["id"]] = e
                        seq.append(("result", e))
                    elif t == "text_block":
                        seq.append(("text", e))
                    elif t == "user_message":
                        seq.append(("user", e))
        except OSError:
            continue

        # follow-up text per result id
        result_followup: dict[str, str] = {}
        for i, (kind, ev) in enumerate(seq):
            if kind != "result":
                continue
            buf = []
            for k2, e2 in seq[i + 1:]:
                if k2 == "user":
                    break
                if k2 == "text":
                    buf.append(e2.get("text", "") or e2.get("content", ""))
            result_followup[ev["id"]] = "\n".join(buf)

        session_bytes = 0
        sid = str(ev_path.parent.relative_to(ev_path.parents[2]) if len(ev_path.parents) >= 3 else ev_path.parent.name)

        for rid, ev in results.items():
            name = ev.get("name", "?")
            out = ev.get("output", "") or ""
            if isinstance(out, list):
                out = json.dumps(out)
            b = len(out)
            total_bytes += b
            total_results += 1
            session_bytes += b
            tool_bytes[name] += b
            tool_count[name] += 1
            tool_sizes[name].append(b)

            if name == "run_command":
                cmd = (calls.get(rid, {}).get("input") or {}).get("command", "")
                key = argv0(cmd) if cmd else "(?)"
                rc_bytes[key] += b
                rc_count[key] += 1
                rc_sizes[key].append(b)

            labs = symptom_labels(out)
            for lab in labs:
                symptom_bytes[lab] += b
                symptom_count[lab] += 1
            if "truncated" in labs:
                truncation_hits += 1

            if b >= 2000:
                unused_eligible_bytes += b
                follow = result_followup.get(rid, "")
                referenced = False
                if follow:
                    step = max(1, len(out) // 200)
                    for k in range(0, max(1, len(out) - 24), step):
                        w = out[k:k + 24]
                        if w and w in follow:
                            referenced = True
                            break
                if not referenced:
                    unused_bytes += b

            if b >= min_bytes:
                preview = out[:120].replace("\n", "\\n")
                big_outputs.append((b, sid, name, preview))

        per_session_bytes.append((session_bytes, sid))

    # ---- render markdown ----
    L = []
    L.append(f"## Scope: {scope_label}\n")
    L.append(f"- Sessions scanned: **{scanned}**")
    L.append(f"- Total tool_results: **{total_results}**")
    L.append(f"- Total output bytes: **{fmt_bytes(total_bytes)}** (~{tok(total_bytes):,} tokens)")
    L.append(f"- Truncation hits: **{truncation_hits}**\n")

    L.append("### Per-tool totals\n")
    L.append("| tool | calls | bytes | ~tokens | p50 | p95 | max | avg |")
    L.append("|---|---:|---:|---:|---:|---:|---:|---:|")
    for name, b in tool_bytes.most_common():
        s = tool_sizes[name]
        L.append(f"| {name} | {tool_count[name]} | {fmt_bytes(b)} | {tok(b):,} | "
                 f"{fmt_bytes(percentile(s,50))} | {fmt_bytes(percentile(s,95))} | "
                 f"{fmt_bytes(max(s))} | {fmt_bytes(int(statistics.mean(s)))} |")
    L.append("")

    L.append("### run_command by program (argv[0])\n")
    L.append("| program | calls | bytes | ~tokens | p50 | p95 | max |")
    L.append("|---|---:|---:|---:|---:|---:|---:|")
    for key, b in rc_bytes.most_common(30):
        s = rc_sizes[key]
        L.append(f"| `{key}` | {rc_count[key]} | {fmt_bytes(b)} | {tok(b):,} | "
                 f"{fmt_bytes(percentile(s,50))} | {fmt_bytes(percentile(s,95))} | "
                 f"{fmt_bytes(max(s))} |")
    L.append("")

    L.append("### Symptom classes\n")
    L.append("| symptom | count | bytes | ~tokens |")
    L.append("|---|---:|---:|---:|")
    for sym, c in symptom_count.most_common():
        L.append(f"| {sym} | {c} | {fmt_bytes(symptom_bytes[sym])} | {tok(symptom_bytes[sym]):,} |")
    L.append("")

    L.append(f"### Top {top} largest individual outputs\n")
    L.append("| bytes | tool | preview |")
    L.append("|---:|---|---|")
    big_outputs.sort(reverse=True)
    for b, sid, name, preview in big_outputs[:top]:
        prev = preview.replace("|", "\\|")[:110]
        L.append(f"| {fmt_bytes(b)} | {name} | `{prev}` |")
    L.append("")

    L.append("### Top 10 sessions by total tool-output bytes\n")
    L.append("| bytes | ~tokens | session |")
    L.append("|---:|---:|---|")
    per_session_bytes.sort(reverse=True)
    for b, sid in per_session_bytes[:10]:
        L.append(f"| {fmt_bytes(b)} | {tok(b):,} | `{sid}` |")
    L.append("")

    if unused_eligible_bytes:
        ratio = unused_bytes / unused_eligible_bytes
        L.append("### Unused-output heuristic (outputs ≥ 2 KB)\n")
        L.append(f"- Eligible: {fmt_bytes(unused_eligible_bytes)} (~{tok(unused_eligible_bytes):,} tokens)")
        L.append(f"- Apparently unused (no 24-char window quoted in next assistant turn): "
                 f"**{fmt_bytes(unused_bytes)}** (~{tok(unused_bytes):,} tokens, {ratio:.0%})")
        L.append("- Cheap heuristic; the model often uses content without quoting it verbatim.\n")

    return "\n".join(L), {
        "scanned": scanned,
        "results": total_results,
        "bytes": total_bytes,
        "truncation_hits": truncation_hits,
    }


# --- main ---------------------------------------------------------------

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--top", type=int, default=25)
    ap.add_argument("--out", default="test-output/token-audit.md")
    args = ap.parse_args()

    local_dir = Path(".omega/sessions")
    local_paths = sorted(local_dir.glob("*/events.jsonl")) if local_dir.exists() else []

    bench_root = Path("bench/jobs")
    bench_paths = sorted(bench_root.glob("*/*/agent/events.jsonl")) if bench_root.exists() else []

    local_md, local_sum = audit(local_paths, "local interactive sessions (.omega/sessions)",
                                top=args.top)
    bench_md, bench_sum = audit(bench_paths, "benchmark trials (bench/jobs)",
                                top=args.top)

    parts = [
        "# Omega tool-output audit\n",
        "Two independent analyses follow. They are kept fully separate so that ",
        "optimisation decisions for one scope are not contaminated by the other. ",
        "The local scope is Omega working on its own repo (high risk of ",
        "overfitting). The benchmark scope is Terminal-Bench 2.0 trials, which ",
        "is the better signal for generalisation.\n",
        "---\n",
        "# Analysis 1 — Local interactive sessions\n",
        local_md,
        "\n---\n",
        "# Analysis 2 — Benchmark trials (Terminal-Bench 2.0)\n",
        bench_md,
    ]

    out_path = Path(args.out)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text("\n".join(parts))
    print(f"Wrote {out_path}")
    print(f"local : {local_sum['scanned']:>4d} sessions  {fmt_bytes(local_sum['bytes']):>7s}  "
          f"~{tok(local_sum['bytes']):>10,} tok  trunc={local_sum['truncation_hits']}")
    print(f"bench : {bench_sum['scanned']:>4d} sessions  {fmt_bytes(bench_sum['bytes']):>7s}  "
          f"~{tok(bench_sum['bytes']):>10,} tok  trunc={bench_sum['truncation_hits']}")


if __name__ == "__main__":
    main()
