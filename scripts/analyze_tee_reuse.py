#!/usr/bin/env python3
"""
Analyse "tee always" cache reuse across sessions.

For tee-emitting tools (run_command, wait_for_output, fetch_url) and also
fetch_url's special "Cached:" path, find each tool_result that surfaced a
cache path, classify it as truncated vs full, and count how many *later*
tool calls in the same session referenced that path in their input.

Usage:
    scripts/analyze_tee_reuse.py [SESSIONS_DIR] [--since YYYY-MM-DD]
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from collections import defaultdict
from pathlib import Path

TEE_TOOLS = {"run_command", "wait_for_output", "fetch_url"}

FOOTER_FULL = re.compile(r"\[full output: ([^\]]+)\]")
FOOTER_TRUNC = re.compile(r"\[truncated; showed [^\]]*Full output: ([^\]]+)\]")
# fetch_url also exposes the *content-addressed full download* path via
# `Cached: <path>`. This is NOT the cap_and_tee mechanism under evaluation
# — it predates tee-always and is the URL-keyed dedupe layer for the
# network request. We track it separately for context.
FETCH_CACHED = re.compile(r"^Cached: (\S+)", re.M)


def _is_real_path(p: str) -> bool:
    # Reject documentation placeholders like '<path>'.
    return "/" in p and not p.startswith("<") and p.endswith(".log")


def classify(output: str) -> tuple[str | None, str | None]:
    """Return (status, path) where status is 'truncated' | 'full' | None."""
    m = FOOTER_TRUNC.search(output)
    if m and _is_real_path(m.group(1).strip()):
        return "truncated", m.group(1).strip()
    m = FOOTER_FULL.search(output)
    if m and _is_real_path(m.group(1).strip()):
        return "full", m.group(1).strip()
    return None, None


def iter_events(jsonl: Path):
    with jsonl.open() as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            try:
                yield json.loads(line)
            except json.JSONDecodeError:
                continue


def analyse_session(events_path: Path) -> list[dict]:
    """Return one record per tee tool_result with a surfaced cache path."""
    # First pass: collect tool_results in order, and all subsequent inputs.
    events = list(iter_events(events_path))

    # Build list of (idx, name, input_text) for every tool_call.
    tool_calls: list[tuple[int, str, str]] = []
    for i, ev in enumerate(events):
        if ev.get("type") == "tool_call":
            inp = ev.get("input", {})
            tool_calls.append((i, ev.get("name", ""), json.dumps(inp)))

    records: list[dict] = []
    for i, ev in enumerate(events):
        if ev.get("type") != "tool_result":
            continue
        name = ev.get("name", "")
        if name not in TEE_TOOLS:
            continue
        out = ev.get("output", "") or ""
        status, path = classify(out)
        if path is None:
            # Some tools (esp. wait_for_output, fetch_url) may surface the
            # path differently. Skip those for now — we only credit reuse
            # when the path was actually visible to the LLM.
            continue

        # Count subsequent tool_calls referencing this path.
        followups: list[tuple[str, int]] = []
        for j, nm, inp_text in tool_calls:
            if j <= i:
                continue
            if path in inp_text:
                followups.append((nm, j))

        records.append(
            {
                "session": events_path.parent.name,
                "tool": name,
                "status": status,
                "path": path,
                "followup_count": len(followups),
                "followup_tools": [t for t, _ in followups],
            }
        )
    return records


def analyse_fetch_raw_cache(events_path: Path) -> list[dict]:
    """Track reuse of `fetch_url`'s content-addressed full download path
    (`Cached: <hash>.txt`) — separate from cap_and_tee."""
    events = list(iter_events(events_path))
    tool_calls: list[tuple[int, str, str]] = []
    for i, ev in enumerate(events):
        if ev.get("type") == "tool_call":
            tool_calls.append((i, ev.get("name", ""), json.dumps(ev.get("input", {}))))

    records: list[dict] = []
    for i, ev in enumerate(events):
        if ev.get("type") != "tool_result" or ev.get("name") != "fetch_url":
            continue
        out = ev.get("output", "") or ""
        m = FETCH_CACHED.search(out)
        if not m:
            continue
        path = m.group(1).strip()
        followups = [(nm, j) for (j, nm, inp) in tool_calls if j > i and path in inp]
        records.append(
            {
                "session": events_path.parent.name,
                "path": path,
                "followup_count": len(followups),
                "followup_tools": [t for t, _ in followups],
            }
        )
    return records


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("sessions_dir", nargs="?", default=".omega/sessions")
    ap.add_argument(
        "--since",
        default="2026-05-15",
        help="Only sessions whose directory name sorts >= this prefix.",
    )
    ap.add_argument("--verbose", action="store_true")
    args = ap.parse_args()

    root = Path(args.sessions_dir)
    sessions = sorted(d for d in root.iterdir() if d.is_dir() and d.name >= args.since)

    all_records: list[dict] = []
    fetch_raw_records: list[dict] = []
    for s in sessions:
        ev = s / "events.jsonl"
        if not ev.exists():
            continue
        all_records.extend(analyse_session(ev))
        fetch_raw_records.extend(analyse_fetch_raw_cache(ev))

    if not all_records:
        print("No tee tool_results with surfaced cache paths found.")
        return 0

    # Aggregate
    by_status_tool: dict[tuple[str, str], dict] = defaultdict(
        lambda: {"calls": 0, "reused_calls": 0, "followup_total": 0}
    )
    overall: dict[str, dict] = defaultdict(
        lambda: {"calls": 0, "reused_calls": 0, "followup_total": 0}
    )

    for r in all_records:
        key = (r["tool"], r["status"])
        b = by_status_tool[key]
        b["calls"] += 1
        b["followup_total"] += r["followup_count"]
        if r["followup_count"] > 0:
            b["reused_calls"] += 1
        ob = overall[r["status"]]
        ob["calls"] += 1
        ob["followup_total"] += r["followup_count"]
        if r["followup_count"] > 0:
            ob["reused_calls"] += 1

    def fmt(b: dict) -> str:
        n = b["calls"]
        total = b["followup_total"]
        reused = b["reused_calls"]
        ratio = (total / n) if n else 0
        pct = (100 * reused / n) if n else 0
        return (
            f"calls={n:5d}  followups={total:4d}  "
            f"followups/call={ratio:.3f}  reuse={pct:5.1f}% ({reused}/{n})"
        )

    print(f"Sessions scanned (>= {args.since}): {len(sessions)}")
    print(f"Tee tool_results with cache path: {len(all_records)}")
    print()
    print("By status × tool:")
    for (tool, status), b in sorted(by_status_tool.items()):
        print(f"  {status:10s} {tool:18s}  {fmt(b)}")
    print()
    print("By status (all tee tools combined):")
    for status, b in sorted(overall.items()):
        print(f"  {status:10s}  {fmt(b)}")

    # Followup tool distribution
    follow_dist_by_status: dict[str, dict[str, int]] = defaultdict(lambda: defaultdict(int))
    for r in all_records:
        for ft in r["followup_tools"]:
            follow_dist_by_status[r["status"]][ft] += 1
    print()
    print("Which tools reuse cache paths (count of follow-up calls):")
    for status, dist in sorted(follow_dist_by_status.items()):
        print(f"  {status}:")
        for tool, n in sorted(dist.items(), key=lambda kv: -kv[1]):
            print(f"    {tool:18s} {n}")

    # fetch_url raw-download cache (not the tee-always mechanism, but the
    # URL-keyed content cache that surfaces a `Cached:` line).
    if fetch_raw_records:
        n = len(fetch_raw_records)
        total = sum(r["followup_count"] for r in fetch_raw_records)
        reused = sum(1 for r in fetch_raw_records if r["followup_count"])
        ratio = total / n if n else 0
        dist: dict[str, int] = defaultdict(int)
        for r in fetch_raw_records:
            for ft in r["followup_tools"]:
                dist[ft] += 1
        print()
        print("fetch_url raw-download cache (`Cached: <hash>.txt`, content-addressed):")
        pct = 100 * reused / n if n else 0
        print(
            f"  calls={n}  followups={total}  followups/call={ratio:.3f}  "
            f"reuse={pct:.1f}% ({reused}/{n})"
        )
        print(
            "  follow-up tools: "
            + ", ".join(f"{t}={c}" for t, c in sorted(dist.items(), key=lambda kv: -kv[1]))
        )
        print(
            "  Note: this cache predates tee-always — it is fetch_url's URL-keyed\n"
            "        dedupe layer, not the cap_and_tee postprocess log."
        )

    if args.verbose:
        print()
        print("Reused entries:")
        for r in all_records:
            if r["followup_count"]:
                print(
                    f"  {r['session']}  {r['tool']:14s} {r['status']:10s} "
                    f"follow={r['followup_count']:2d} ({','.join(r['followup_tools'])})"
                )

    return 0


if __name__ == "__main__":
    sys.exit(main())
