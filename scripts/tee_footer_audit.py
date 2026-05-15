#!/usr/bin/env python3
"""Measure whether 'tee-always + footer-always' would pay for itself.

CORRECTED: tool_call uses field `input`, not `arguments`.

Five numbers per corpus:

  A. Footer cost (deterministic): N(tool_results) * ~160 B / 4 chars/tok
  B. fetch_url cache reuse: surfaced paths -> later referenced?
  C. Re-derivation potential: outputs >=1 KB whose content reappears
  D. ANY cache-path reuse (broad net: cache/, .cache/omega/, .omega/...)
  E. Project-side tee reuse: gate-logs, test-output/, gate-latest
"""
from __future__ import annotations

import json
import re
from pathlib import Path
from collections import defaultdict, Counter

CHARS_PER_TOKEN = 4
FOOTER_TEMPLATE = ("[truncated; showed first 100 KB of 12.3 MB. "
                   "Full output: .omega/sessions/2026-05-15T13-55-30-967-12fd676b/"
                   "cache/run/2026-05-15T13-55-30-967-f6f6b41c-cargo.log]")
FOOTER_BYTES = len(FOOTER_TEMPLATE)

FETCH_CACHE_RE = re.compile(r"(?:cache(?:-\d+)?|cache/fetch)/[0-9a-f]{16,}\.txt")

# Broad "cap_and_tee or session-cache" paths the LLM might reuse
CAP_CACHE_RES = [
    re.compile(r"\.cache/omega/[^\s\"'`)]+"),
    re.compile(r"cache/(?:fetch|run|wait)/[^\s\"'`)]+"),
    re.compile(r"\.omega/sessions/[^/\s\"']+/cache/[^\s\"'`)]+"),
    re.compile(r"cache-\d+/[0-9a-f]{16,}\.txt"),
]

# Project-side tee surfaces (the gate)
PROJECT_TEE_RES = [
    re.compile(r"\.omega/gate-logs/[^\s\"'`)]+"),
    re.compile(r"test-output/gate-latest\.log"),
    re.compile(r"test-output/[A-Za-z0-9._/-]+\.(?:log|txt)"),
]

WINDOW = 200
WINDOW_STRIDE = 4000


def iter_events(path: Path):
    try:
        with open(path) as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                try:
                    yield json.loads(line)
                except json.JSONDecodeError:
                    continue
    except OSError:
        return


def call_input_str(e: dict) -> str:
    """Stringify a tool_call's input for substring search.
    Schema: field is 'input', not 'arguments'."""
    return json.dumps(e.get("input", {}))


def analyse_session(path: Path) -> dict:
    n_tool_results = 0
    total_output_bytes = 0
    fetch_surfaced = []
    fetch_reused = 0
    cap_cache_surfaced = []          # any cap_and_tee cache path surfaced
    cap_cache_reused = 0
    project_tee_surfaced = []        # gate-logs / test-output/*
    project_tee_reused = 0
    rederive_candidates = 0
    rederive_hits = 0
    truncated_results = 0

    events = list(iter_events(path))
    call_args = []
    for i, e in enumerate(events):
        if e.get("type") == "tool_call":
            call_args.append((i, call_input_str(e)))

    bodies_seq = []
    for i, e in enumerate(events):
        if e.get("type") != "tool_result":
            continue
        n_tool_results += 1
        body = e.get("output") or ""
        total_output_bytes += len(body)
        if "[truncated" in body or "[Truncated" in body or "[Output truncated" in body:
            truncated_results += 1
        bodies_seq.append((i, body))

        if e.get("name") == "fetch_url":
            for m in FETCH_CACHE_RE.finditer(body):
                fetch_surfaced.append((i, m.group(0)))

        for rgx in CAP_CACHE_RES:
            for m in rgx.finditer(body):
                cap_cache_surfaced.append((i, m.group(0)))
        for rgx in PROJECT_TEE_RES:
            for m in rgx.finditer(body):
                project_tee_surfaced.append((i, m.group(0)))

    # reuse checks
    def count_reuse(surfaced):
        n = 0
        seen = set()
        for idx, p in surfaced:
            if (idx, p) in seen:
                continue
            seen.add((idx, p))
            for cidx, args in call_args:
                if cidx > idx and p in args:
                    n += 1
                    break
        return n

    fetch_reused = count_reuse(fetch_surfaced)
    cap_cache_reused = count_reuse(cap_cache_surfaced)
    project_tee_reused = count_reuse(project_tee_surfaced)

    # re-derivation probe
    bodies = [b for _, b in bodies_seq]
    for k, (idx, body) in enumerate(bodies_seq):
        if len(body) < 1024:
            continue
        rederive_candidates += 1
        fps = []
        for off in range(0, max(1, len(body) - WINDOW), WINDOW_STRIDE):
            w = body[off:off + WINDOW]
            if len(w.strip()) < 50:
                continue
            fps.append(w)
            if len(fps) >= 25:
                break
        if not fps:
            continue
        later = "\n".join(bodies[k + 1:])
        if not later:
            continue
        for w in fps:
            if w in later:
                rederive_hits += 1
                break

    return dict(
        n_tool_results=n_tool_results,
        total_output_bytes=total_output_bytes,
        truncated_results=truncated_results,
        fetch_surfaced=len(set(p for _, p in fetch_surfaced)),
        fetch_reused=fetch_reused,
        cap_cache_surfaced=len(set(p for _, p in cap_cache_surfaced)),
        cap_cache_reused=cap_cache_reused,
        project_tee_surfaced=len(set(p for _, p in project_tee_surfaced)),
        project_tee_reused=project_tee_reused,
        rederive_candidates=rederive_candidates,
        rederive_hits=rederive_hits,
    )


def aggregate(paths, label):
    agg = defaultdict(int)
    n_sessions = 0
    for p in paths:
        s = analyse_session(p)
        if s["n_tool_results"] == 0:
            continue
        n_sessions += 1
        for k, v in s.items():
            agg[k] += v
    footer_bytes = agg["n_tool_results"] * FOOTER_BYTES
    footer_tokens = footer_bytes // CHARS_PER_TOKEN
    total_tokens = agg["total_output_bytes"] // CHARS_PER_TOKEN
    print(f"\n=== {label} ===")
    print(f"sessions          : {n_sessions}")
    print(f"tool_results      : {agg['n_tool_results']:,}")
    print(f"  truncated       : {agg['truncated_results']:,}  "
          f"({100*agg['truncated_results']/max(1,agg['n_tool_results']):.1f}%)")
    print(f"output tokens     : ~{total_tokens:,}")
    print()
    print(f"(A) FOOTER-ALWAYS cost:")
    print(f"    {FOOTER_BYTES} B * {agg['n_tool_results']:,} = "
          f"~{footer_tokens:,} tokens "
          f"({100*footer_tokens/max(1,total_tokens):.2f}% of output)")
    print()
    print(f"(B) fetch_url:        surfaced={agg['fetch_surfaced']}  "
          f"reused={agg['fetch_reused']}  "
          f"({100*agg['fetch_reused']/max(1,agg['fetch_surfaced']):.1f}%)")
    print(f"(D) any cap_and_tee:  surfaced={agg['cap_cache_surfaced']}  "
          f"reused={agg['cap_cache_reused']}  "
          f"({100*agg['cap_cache_reused']/max(1,agg['cap_cache_surfaced']):.1f}%)")
    print(f"(E) project-tee:      surfaced={agg['project_tee_surfaced']}  "
          f"reused={agg['project_tee_reused']}  "
          f"({100*agg['project_tee_reused']/max(1,agg['project_tee_surfaced']):.1f}%)")
    print()
    print(f"(C) Re-derive probe:  candidates={agg['rederive_candidates']}  "
          f"hits={agg['rederive_hits']}  "
          f"({100*agg['rederive_hits']/max(1,agg['rederive_candidates']):.1f}%)")


def main():
    local = sorted(Path(".omega/sessions").glob("*/events.jsonl"))
    bench = sorted(Path("bench/jobs").glob("**/events.jsonl"))
    print(f"local sessions: {len(local)}    bench trials: {len(bench)}")
    aggregate(local, "LOCAL (.omega/sessions)")
    aggregate(bench, "BENCH (bench/jobs)")


if __name__ == "__main__":
    main()
