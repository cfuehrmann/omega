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


def _ids(e: dict) -> list[str]:
    """Return all identifier-like fields on an event.

    Older sessions used `id` (LLM tool_use_id) or `callId` (Omega).
    Current schema uses `toolCallId`. Returning all of them lets us link
    a `tool_call` to its `tool_result` regardless of schema vintage.
    """
    return [v for v in (e.get("toolCallId"), e.get("callId"), e.get("id")) if v]


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
    """Return one record per tee tool_result with a surfaced cache path.

    Each record carries the originating tool's duration and the durations
    of any cache-referencing follow-ups, so the caller can estimate time
    saved under the model \"without the cache, each follow-up would have
    re-run the origin command\".
    """
    events = list(iter_events(events_path))

    call_by_id: dict[str, dict] = {}
    result_by_id: dict[str, dict] = {}
    for ev in events:
        for cid in _ids(ev):
            if ev.get("type") == "tool_call":
                call_by_id[cid] = ev
            elif ev.get("type") == "tool_result":
                result_by_id[cid] = ev

    # Ordered tool_calls with their id list for follow-up lookup.
    tool_calls: list[tuple[int, list[str], str, str]] = []
    for i, ev in enumerate(events):
        if ev.get("type") == "tool_call":
            tool_calls.append(
                (i, _ids(ev), ev.get("name", ""), json.dumps(ev.get("input", {})))
            )

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
            continue

        orig_dur_ms = ev.get("durationMs", 0)
        orig_call = next((call_by_id[c] for c in _ids(ev) if c in call_by_id), None)

        followups: list[dict] = []
        for j, fids, nm, inp_text in tool_calls:
            if j <= i or path not in inp_text:
                continue
            fu_res = next((result_by_id[c] for c in fids if c in result_by_id), None)
            followups.append({
                "tool": nm,
                "duration_ms": fu_res.get("durationMs", 0) if fu_res else 0,
            })

        records.append(
            {
                "session": events_path.parent.name,
                "tool": name,
                "status": status,
                "path": path,
                "orig_dur_ms": orig_dur_ms,
                "orig_cmd": (orig_call or {}).get("input", {}).get("command", "")
                if name == "run_command" else "",
                "followup_count": len(followups),
                "followup_tools": [f["tool"] for f in followups],
                "followups": followups,
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

    # Disk-usage summary across all session cache directories.
    def _hum(b: float) -> str:
        for u in ["B", "KB", "MB", "GB"]:
            if b < 1024:
                return f"{b:.1f} {u}"
            b /= 1024
        return f"{b:.1f} TB"

    tool_bytes: dict[str, int] = defaultdict(int)
    tool_files: dict[str, int] = defaultdict(int)
    run_sizes: list[int] = []
    per_session_bytes: list[int] = []
    for s in sessions:
        cache = s / "cache"
        if not cache.exists():
            continue
        sess_bytes = 0
        for sub in cache.iterdir():
            if not sub.is_dir():
                continue
            for f in sub.rglob("*"):
                if not f.is_file():
                    continue
                sz = f.stat().st_size
                tool_bytes[sub.name] += sz
                tool_files[sub.name] += 1
                sess_bytes += sz
                if sub.name == "run":
                    run_sizes.append(sz)
        per_session_bytes.append(sess_bytes)

    if tool_bytes:
        print()
        print("Cache disk usage (bytes on disk):")
        total_b = sum(tool_bytes.values())
        total_f = sum(tool_files.values())
        for tool in sorted(tool_bytes):
            b, f = tool_bytes[tool], tool_files[tool]
            print(
                f"  {tool:8s} files={f:5d}  total={_hum(b):>10s}  avg/file={_hum(b/f):>10s}"
            )
        print(
            f"  {'all':8s} files={total_f:5d}  total={_hum(total_b):>10s}  avg/file={_hum(total_b/total_f):>10s}"
        )
        if per_session_bytes:
            ps = sorted(per_session_bytes)
            n = len(ps)
            print(
                f"  per-session: median={_hum(ps[n//2])}  p90={_hum(ps[int(n*0.9)])}  max={_hum(ps[-1])}"
            )

    # Marginal cost of tee-always over tee-on-truncate for run_command:
    # bytes in files <= 100 KB cap.
    if run_sizes:
        run_total = sum(run_sizes)
        over_cap = sum(s for s in run_sizes if s > 100 * 1024)
        under_cap = run_total - over_cap
        n_over = sum(1 for s in run_sizes if s > 100 * 1024)
        print()
        print("run_command file-size distribution:")
        rs = sorted(run_sizes)
        n = len(rs)
        print(
            f"  p50={_hum(rs[n//2])}  p90={_hum(rs[int(n*0.9)])}  "
            f"p99={_hum(rs[int(n*0.99)])}  max={_hum(rs[-1])}"
        )
        tiny = sum(1 for s in rs if s <= 1024)
        print(f"  <= 1 KB: {tiny} ({100*tiny/n:.1f}%)")
        print(f"  >  100 KB (LLM cap): {n_over} ({100*n_over/n:.1f}%)")
        print(
            f"  Tee-always marginal cost vs tee-on-truncate: "
            f"{_hum(under_cap)} in {n-n_over} files "
            f"({_hum(under_cap/len(sessions))} per session)"
        )

    # Time-savings estimate for run_command (the only tool with both
    # non-trivial origin durations and observable follow-up reuse).
    rc = [r for r in all_records if r["tool"] == "run_command" and r["followup_count"]]
    if rc:
        total_saved = sum(r["followup_count"] * r["orig_dur_ms"] for r in rc)
        total_fu_cost = sum(f["duration_ms"] for r in rc for f in r["followups"])
        all_rc_writes = sum(
            1 for r in all_records if r["tool"] == "run_command"
        )
        print()
        print("run_command time-savings estimate")
        print("  Model: without the cache, each follow-up would have re-run the origin.")
        for r in rc:
            n = r["followup_count"]
            saved = n * r["orig_dur_ms"]
            print(
                f"  [{r['session']}] orig={r['orig_dur_ms']} ms  followups={n}  "
                f"saved={saved} ms  cmd={r['orig_cmd'][:90]!r}"
            )
        print(
            f"  Total naive saved:  {total_saved} ms ({total_saved/1000:.2f} s)"
        )
        print(f"  Follow-up cost:     {total_fu_cost} ms")
        print(
            f"  Net saved:          {total_saved - total_fu_cost} ms "
            f"({(total_saved - total_fu_cost)/1000:.2f} s)"
        )
        print(
            f"  Amortised per cached run_command write: "
            f"{total_saved / all_rc_writes:.1f} ms"
            f"  ({total_saved} ms / {all_rc_writes} writes)"
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
