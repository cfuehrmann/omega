#!/usr/bin/env python3
"""
Targeted analysis: directory-listing orientation patterns.

Questions answered:
  1. What fraction of sessions open with some form of directory listing
     (list_files, ls, find, du, tree, …)?
  2. Are those listings for /app, /., /, a home dir, or something else?
  3. How many *distinct* listing calls does the agent make in the first 8
     tool calls — i.e., how deep does it drill?
  4. If we pre-loaded a listing, would it plausibly replace those calls?
  5. What is the "root path" the agent is oriented against, and how big is it
     (directory depth proxy)?
"""

from __future__ import annotations

import json
import re
import sys
from collections import Counter, defaultdict
from pathlib import Path
from textwrap import shorten

REPO_ROOT = Path(__file__).parent.parent.parent
OMEGA_SESSIONS_DIR = REPO_ROOT / ".omega" / "sessions"
HARBOR_JOBS_DIR    = REPO_ROOT / "bench" / "jobs"

_RUST_SESSION_ID_RE = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}-\d{3}-[0-9a-f]+$")
_REAL_MSG_ID_RE     = re.compile(r"^msg_01[A-Za-z0-9]+$")
_REAL_TOOL_ID_RE    = re.compile(r"^(toolu|srvtoolu)_01[A-Za-z0-9]+$")

def _is_rust_clean(events):
    started = next((e for e in events if e.get("type") in ("session_started","session_start")), None)
    if not started: return False
    if not _RUST_SESSION_ID_RE.match(started.get("sessionId","")): return False
    for ev in events:
        t = ev.get("type","")
        if t == "llm_call":
            url = ev.get("url","")
            if "127.0.0.1" in url or "localhost" in url: return False
        elif t == "llm_response":
            mid = (ev.get("responseSummary",{}) or {}).get("id","")
            if mid and not _REAL_MSG_ID_RE.match(mid): return False
        elif t == "tool_call":
            tid = ev.get("id","")
            if tid and not _REAL_TOOL_ID_RE.match(tid): return False
    return True

def _events(path):
    evs = []
    try:
        for line in path.read_text(errors="replace").splitlines():
            line = line.strip()
            if line:
                try: evs.append(json.loads(line))
                except json.JSONDecodeError: pass
    except OSError: pass
    return evs

def _first_prompt(events):
    for ev in events:
        if ev.get("type") == "user_message":
            c = ev.get("content","")
            if isinstance(c, str): return c.strip()
            if isinstance(c, list):
                return " ".join(b.get("text","") for b in c if isinstance(b,dict)).strip()
    return None

def _tool_calls(events, n=10):
    calls = []
    for ev in events:
        if ev.get("type") == "tool_call":
            calls.append({"name": ev.get("name",""), "input": ev.get("input",{})})
            if len(calls) >= n: break
    return calls

# ── listing detection ──────────────────────────────────────────────────────

LS_COMMANDS = re.compile(r"\b(ls|find|tree|du|dir)\b")

def is_listing_call(call):
    name = call["name"]
    inp  = call["input"]
    if name == "list_files":
        return True
    if name in ("run_command", "run_background"):
        cmd = inp.get("command","")
        return bool(LS_COMMANDS.search(cmd))
    if name == "find_files":
        # find_files is mostly for locating a specific file, not orientation
        # treat it as a listing only if pattern is very broad
        pat = inp.get("pattern","")
        return pat in ("*", "**", ".")
    return False

def listing_root(call):
    """Best-effort root path for a listing call."""
    name = call["name"]
    inp  = call["input"]
    if name == "list_files":
        return inp.get("path","?")
    if name in ("run_command","run_background"):
        cmd = inp.get("command","")
        # pull the path argument from ls/find/du/tree
        m = re.search(r"\b(?:ls|find|du|tree)\s+(-\S+\s+)*([/~\w.][/\w.-]*)", cmd)
        if m: return m.group(2)
        return "(shell)"
    return "?"

def _classify_root(root):
    """Bucket the root path."""
    if root in ("/.","/./","."): return "project-root"
    if root == "/":              return "container-root"
    if re.match(r"^/app",root): return "/app"
    if re.match(r"^/home/",root): return "home-dir"
    if re.match(r"^/tmp/",root): return "/tmp"
    return "other"

# ── depth analysis ─────────────────────────────────────────────────────────

def listing_depth_profile(calls):
    """
    For the listing calls in a session's opening, return:
      - roots: set of distinct roots listed
      - max_depth: how many unique listing calls (proxy for drill-down)
      - is_recursive: any recursive list_files?
    """
    listing = [c for c in calls if is_listing_call(c)]
    roots   = {listing_root(c) for c in listing}
    recursive = any(
        c["name"] == "list_files" and c["input"].get("recursive")
        for c in listing
    )
    return {
        "count":     len(listing),
        "roots":     roots,
        "recursive": recursive,
    }

# ── "would pre-loading help?" heuristic ────────────────────────────────────

def would_help(profile, source):
    """
    Returns one of:
      "yes"       – pre-loading a shallow listing of the project root would
                    plausibly have replaced these calls
      "partial"   – pre-loading would cover the first call but not the drill-down
      "no"        – pre-loading wouldn't help (no listing, or wrong root)
    """
    if profile["count"] == 0:
        return "no-listing"
    roots_classified = {_classify_root(r) for r in profile["roots"]}

    # Omega almost always lists project-root or /
    # Harbor lists / or /app
    relevant_roots = {"project-root","container-root","/app"}
    if not roots_classified & relevant_roots:
        return "no-wrong-root"

    # If only one shallow listing with no drill-down → pre-loading fully replaces it
    if profile["count"] == 1 and not profile["recursive"]:
        return "yes-full"

    # Multiple listings or recursive → partial coverage
    return "partial"

# ── load sessions ──────────────────────────────────────────────────────────

def load_omega():
    rows = []
    if not OMEGA_SESSIONS_DIR.exists(): return rows
    for d in sorted(OMEGA_SESSIONS_DIR.iterdir()):
        if not d.is_dir(): continue
        ef = d / "events.jsonl"
        if not ef.exists(): continue
        evs = _events(ef)
        if not _is_rust_clean(evs): continue
        prompt = _first_prompt(evs)
        if not prompt: continue
        calls = _tool_calls(evs, 10)
        profile = listing_depth_profile(calls)
        rows.append({
            "source": "omega",
            "id":     d.name,
            "prompt": prompt,
            "calls":  calls,
            "profile":profile,
        })
    return rows

def load_harbor():
    rows = []
    if not HARBOR_JOBS_DIR.exists(): return rows
    for ef in sorted(HARBOR_JOBS_DIR.rglob("events.jsonl")):
        parts = ef.relative_to(HARBOR_JOBS_DIR).parts
        task  = re.sub(r"__[A-Za-z0-9]+$","", parts[1] if len(parts)>1 else "?")
        evs   = _events(ef)
        prompt= _first_prompt(evs)
        if not prompt: continue
        calls = _tool_calls(evs, 10)
        profile = listing_depth_profile(calls)
        rows.append({
            "source": "harbor",
            "id":     str(ef.relative_to(HARBOR_JOBS_DIR)),
            "task":   task,
            "prompt": prompt,
            "calls":  calls,
            "profile":profile,
        })
    return rows

# ── report ─────────────────────────────────────────────────────────────────

def report(omega, harbor):
    all_s = omega + harbor

    def pct(n, total): return f"{100*n/max(total,1):.1f}%"

    def section(title):
        print()
        print("="*72)
        print(f"  {title}")
        print("="*72)

    # ── 1. Overall: how many sessions open with any listing call? ──────────
    section("1. Sessions opening with ≥1 directory listing call")
    for label, rows in [("Omega", omega), ("Harbor", harbor), ("Total", all_s)]:
        has_listing = [r for r in rows if r["profile"]["count"] > 0]
        print(f"  {label:8s}  {len(has_listing):3d}/{len(rows)}  ({pct(len(has_listing),len(rows))})")

    # ── 2. What root paths are being listed? ──────────────────────────────
    section("2. Root paths listed in opening 10 calls (all sessions)")
    root_counter = Counter()
    for r in all_s:
        for call in r["calls"]:
            if is_listing_call(call):
                root = listing_root(call)
                root_counter[_classify_root(root) + "  " + root] += 1
    for k, v in root_counter.most_common(30):
        print(f"  {v:4d}  {k}")

    # ── 3. Drill-down depth (how many listing calls per session)  ──────────
    section("3. Number of listing calls in opening 10 calls (per-session histogram)")
    for label, rows in [("Omega", omega), ("Harbor", harbor)]:
        counts = Counter(r["profile"]["count"] for r in rows)
        print(f"\n  {label}:")
        for k in sorted(counts):
            bar = "█" * counts[k]
            print(f"    {k} listing(s): {counts[k]:3d} sessions  {bar}")

    # ── 4. Would pre-loading a project-root listing have helped? ──────────
    section("4. Would pre-loading a shallow root listing have helped?")
    for label, rows in [("Omega", omega), ("Harbor", harbor)]:
        help_counter = Counter(would_help(r["profile"], r["source"]) for r in rows)
        total = len(rows)
        print(f"\n  {label} (n={total}):")
        for outcome in ["yes-full","partial","no-listing","no-wrong-root"]:
            n = help_counter[outcome]
            print(f"    {outcome:20s}  {n:3d}  ({pct(n, total)})")

    # ── 5. Omega: what does the agent do *after* list_files(/.)?  ──────────
    section("5. Omega: what immediately follows list_files(/.)?")
    follow_counter = Counter()
    for r in omega:
        calls = r["calls"]
        for i, c in enumerate(calls):
            if c["name"] == "list_files" and c["input"].get("path","") in ("/.", "."):
                # grab next call
                if i+1 < len(calls):
                    nxt = calls[i+1]
                    follow_counter[nxt["name"] + " → " + (nxt["input"].get("path","") or nxt["input"].get("pattern","") or nxt["input"].get("command","?")[:40])] += 1
                break
    for k,v in follow_counter.most_common(20):
        print(f"  {v:3d}  {k}")

    # ── 6. Harbor variants of listing (ls vs list_files vs find) ──────────
    section("6. Harbor: listing call variants in first 10 calls")
    variant_counter = Counter()
    for r in harbor:
        for c in r["calls"]:
            if is_listing_call(c):
                name = c["name"]
                if name in ("run_command","run_background"):
                    cmd = c["input"].get("command","")
                    m = re.match(r"\s*(\w+)", cmd)
                    variant = m.group(1) if m else "run_command"
                else:
                    variant = name
                variant_counter[variant] += 1
    for k,v in variant_counter.most_common(15):
        print(f"  {v:4d}  {k}")

    # ── 7. Recursive listings ─────────────────────────────────────────────
    section("7. Sessions using recursive listing in first 10 calls")
    for label, rows in [("Omega", omega), ("Harbor", harbor)]:
        rec = [r for r in rows if r["profile"]["recursive"]]
        print(f"  {label}: {len(rec)}/{len(rows)} ({pct(len(rec),len(rows))}) use recursive list_files")
        for r in rec[:5]:
            prompt_short = shorten(r["prompt"].replace("\n"," "), 80)
            print(f"    {r['id'][:50]}  [{prompt_short}]")

    # ── 8. Sessions where root is a user home dir (big-dir problem) ────────
    section("8. Sessions listing a home directory (big-dir risk)")
    for label, rows in [("Omega", omega), ("Harbor", harbor)]:
        home_sessions = []
        for r in rows:
            for c in r["calls"]:
                if is_listing_call(c):
                    root = listing_root(c)
                    if _classify_root(root) == "home-dir":
                        home_sessions.append(r)
                        break
        print(f"\n  {label}: {len(home_sessions)} session(s) list a home dir")
        for r in home_sessions[:8]:
            prompt_short = shorten(r["prompt"].replace("\n"," "), 90)
            print(f"    [{prompt_short}]")

    # ── 9. Sessions where agent jumps to a specific file without listing ───
    section("9. Sessions that skip listing and jump directly to a named file")
    for label, rows in [("Omega", omega), ("Harbor", harbor)]:
        skip_listing = [r for r in rows
                        if r["calls"] and not is_listing_call(r["calls"][0])
                        and r["calls"][0]["name"] == "read_file"]
        print(f"\n  {label}: {len(skip_listing)}/{len(rows)} start with read_file (no listing)")
        for r in skip_listing[:5]:
            first = r["calls"][0]
            path  = first["input"].get("path","?")
            prompt_short = shorten(r["prompt"].replace("\n"," "), 70)
            print(f"    read_file({path})  <- [{prompt_short}]")

    print()

if __name__ == "__main__":
    print("Loading…", file=sys.stderr)
    omega  = load_omega()
    harbor = load_harbor()
    print(f"  Omega={len(omega)}  Harbor={len(harbor)}", file=sys.stderr)
    report(omega, harbor)
