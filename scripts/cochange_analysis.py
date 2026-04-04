#!/usr/bin/env python3
"""
Co-change analysis for Omega architectural coherence.

Two signals:
  1. Git commits — which files changed together in the same commit
  2. Session events — which files were write_file/edit_file'd in the same session

For each signal we build a co-occurrence matrix, compute Jaccard similarity
for every pair, and flag cross-directory pairs with high coupling as potential
architectural dislocations.
"""

import os
import json
import subprocess
import re
from collections import defaultdict
from itertools import combinations
from pathlib import Path

# ---------------------------------------------------------------------------
# Config
# ---------------------------------------------------------------------------

PROJECT_ROOT = Path(__file__).parent.parent
SESSIONS_DIR = PROJECT_ROOT / ".omega" / "sessions"

# Paths to exclude from analysis (globs / prefixes)
EXCLUDE_PREFIXES = (
    "src/web/public/",   # built assets
    "test-output/",
    "test-results/",
    ".omega/",
    "diagnosis/",
    "bun.lock",
)

EXCLUDE_SUFFIXES = (
    ".log",
)

WRITE_TOOLS = {"write_file", "edit_file"}


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def normalize_path(raw: str) -> str | None:
    """Strip absolute prefix and project root; return None if excluded."""
    p = raw.strip()
    # Strip absolute paths pointing at project root
    prefix = "/home/carsten/omega/dev/"
    if p.startswith(prefix):
        p = p[len(prefix):]
    # Already relative
    if p.startswith("/"):
        return None  # some other absolute path — ignore
    for pfx in EXCLUDE_PREFIXES:
        if p.startswith(pfx):
            return None
    for sfx in EXCLUDE_SUFFIXES:
        if p.endswith(sfx):
            return None
    return p


def directory_of(path: str) -> str:
    """Immediate parent directory (or '.' for root files)."""
    return str(Path(path).parent)


def top_dir(path: str) -> str:
    """Top-level directory component."""
    parts = Path(path).parts
    return parts[0] if len(parts) > 1 else "."


def shared_prefix_depth(a: str, b: str) -> int:
    """
    How many path components do a and b share?
    e.g. src/web/server.ts and src/web/client/App.tsx -> 2  (src/web)
         src/agent.ts and src/web/server.ts           -> 1  (src)
         src/agent.ts and e2e/foo.spec.ts             -> 0
    """
    pa = Path(a).parts[:-1]  # drop filename
    pb = Path(b).parts[:-1]
    depth = 0
    for x, y in zip(pa, pb):
        if x == y:
            depth += 1
        else:
            break
    return depth


# ---------------------------------------------------------------------------
# Signal 1: git commits
# ---------------------------------------------------------------------------

def collect_git_changesets() -> list[frozenset[str]]:
    """Return one frozenset of changed src files per commit."""
    result = subprocess.run(
        ["git", "log", "--name-only", "--pretty=format:COMMIT"],
        cwd=PROJECT_ROOT,
        capture_output=True, text=True
    )
    changesets = []
    current: set[str] = set()
    for line in result.stdout.splitlines():
        if line == "COMMIT":
            if current:
                changesets.append(frozenset(current))
            current = set()
        else:
            normed = normalize_path(line)
            if normed:
                current.add(normed)
    if current:
        changesets.append(frozenset(current))
    return changesets


# ---------------------------------------------------------------------------
# Signal 2: session events
# ---------------------------------------------------------------------------

def _extract_paths_from_events_file(events_path: Path) -> set[str]:
    """Extract all write_file/edit_file target paths from one events.jsonl."""
    paths: set[str] = set()
    with open(events_path) as f:
        for line in f:
            try:
                ev = json.loads(line)
            except json.JSONDecodeError:
                continue
            if ev.get("type") != "tool_call":
                continue
            if ev.get("name") not in WRITE_TOOLS:
                continue
            inp = ev.get("input", {})
            raw = inp.get("path", "")
            if raw:
                normed = normalize_path(raw)
                if normed:
                    paths.add(normed)
    return paths


def collect_session_changesets() -> list[frozenset[str]]:
    """Return one frozenset of written files per session."""
    changesets = []

    # Old flat events.jsonl at root of sessions dir
    flat = SESSIONS_DIR / "events.jsonl"
    if flat.exists():
        paths = _extract_paths_from_events_file(flat)
        if paths:
            changesets.append(frozenset(paths))

    # Per-session directories
    for entry in sorted(SESSIONS_DIR.iterdir()):
        if not entry.is_dir():
            continue
        ef = entry / "events.jsonl"
        if not ef.exists():
            continue
        paths = _extract_paths_from_events_file(ef)
        if len(paths) >= 2:  # only interesting if ≥2 files touched
            changesets.append(frozenset(paths))

    return changesets


# ---------------------------------------------------------------------------
# Co-occurrence matrix
# ---------------------------------------------------------------------------

def build_cooccurrence(changesets: list[frozenset[str]]) -> dict[tuple[str,str], int]:
    cooc: dict[tuple[str,str], int] = defaultdict(int)
    for cs in changesets:
        files = sorted(cs)
        for a, b in combinations(files, 2):
            cooc[(a, b)] += 1
    return cooc


def file_frequencies(changesets: list[frozenset[str]]) -> dict[str, int]:
    freq: dict[str, int] = defaultdict(int)
    for cs in changesets:
        for f in cs:
            freq[f] += 1
    return freq


def jaccard(cooc: int, freq_a: int, freq_b: int) -> float:
    """Jaccard = cooc / (freq_a + freq_b - cooc)"""
    denom = freq_a + freq_b - cooc
    return cooc / denom if denom > 0 else 0.0


def lift(cooc: int, freq_a: int, freq_b: int, n: int) -> float:
    """Lift = P(A∩B) / (P(A)*P(B))  where P = freq/n."""
    if freq_a == 0 or freq_b == 0 or n == 0:
        return 0.0
    return (cooc / n) / ((freq_a / n) * (freq_b / n))


# ---------------------------------------------------------------------------
# Report
# ---------------------------------------------------------------------------

def report(signal_name: str, changesets: list[frozenset[str]], top_n: int = 30):
    n = len(changesets)
    print(f"\n{'='*70}")
    print(f"  Signal: {signal_name}  ({n} changesets)")
    print(f"{'='*70}")

    if n == 0:
        print("  (no data)")
        return

    cooc = build_cooccurrence(changesets)
    freq = file_frequencies(changesets)

    if not cooc:
        print("  (no co-change pairs found)")
        return

    # Build ranked pairs
    pairs = []
    for (a, b), cnt in cooc.items():
        j = jaccard(cnt, freq[a], freq[b])
        l = lift(cnt, freq[a], freq[b], n)
        depth = shared_prefix_depth(a, b)
        pairs.append((a, b, cnt, freq[a], freq[b], j, l, depth))

    # Sort by Jaccard descending
    pairs.sort(key=lambda x: -x[5])

    # --- File frequency table ---
    print(f"\n  Most frequently changed files (top 20):")
    top_files = sorted(freq.items(), key=lambda x: -x[1])[:20]
    for path, cnt in top_files:
        bar = "█" * min(cnt, 40)
        print(f"    {cnt:4d} {bar}  {path}")

    # --- Cross-directory pairs ---
    print(f"\n  Top cross-directory pairs by Jaccard (shared prefix depth shown):")
    print(f"  {'Jaccard':>7}  {'lift':>6}  {'cooc':>4}  {'fA':>4}  {'fB':>4}  depth  pair")
    print(f"  {'-'*7}  {'-'*6}  {'-'*4}  {'-'*4}  {'-'*4}  -----  ----")

    cross_shown = 0
    for a, b, cnt, fa, fb, j, l, depth in pairs:
        if directory_of(a) == directory_of(b):
            continue  # same directory — expected coupling
        if cnt < 2:
            continue  # noise
        print(f"  {j:7.3f}  {l:6.1f}  {cnt:4d}  {fa:4d}  {fb:4d}  d={depth}    {a}  ↔  {b}")
        cross_shown += 1
        if cross_shown >= top_n:
            break

    # --- Same-directory pairs for contrast ---
    print(f"\n  Top SAME-directory pairs by Jaccard (sanity check):")
    print(f"  {'Jaccard':>7}  {'cooc':>4}  pair")
    same_shown = 0
    for a, b, cnt, fa, fb, j, l, depth in pairs:
        if directory_of(a) != directory_of(b):
            continue
        if cnt < 2:
            continue
        print(f"  {j:7.3f}  {cnt:4d}  {a}  ↔  {b}")
        same_shown += 1
        if same_shown >= 15:
            break

    # --- Dislocation summary: cluster cross-dir pairs by top_dir ---
    print(f"\n  Cross-layer coupling summary (cross-top-dir pairs, cooc≥3):")
    layer_counts: dict[str, int] = defaultdict(int)
    for a, b, cnt, fa, fb, j, l, depth in pairs:
        if top_dir(a) == top_dir(b):
            continue
        if cnt < 3:
            continue
        key = f"{top_dir(a)}  ↔  {top_dir(b)}"
        layer_counts[key] += 1
    for key, c in sorted(layer_counts.items(), key=lambda x: -x[1]):
        print(f"    {c:4d} pairs   {key}")


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def load_current_files() -> set[str]:
    """Files currently tracked by git (excluding built assets)."""
    result = subprocess.run(
        ["git", "ls-files"],
        cwd=PROJECT_ROOT, capture_output=True, text=True
    )
    current = set()
    for line in result.stdout.splitlines():
        normed = normalize_path(line.strip())
        if normed:
            current.add(normed)
    return current


def filter_to_current(changesets: list[frozenset[str]], current: set[str]) -> list[frozenset[str]]:
    """Drop files not in the current tree; drop changesets that shrink to <2 files."""
    out = []
    for cs in changesets:
        filtered = frozenset(f for f in cs if f in current)
        if len(filtered) >= 2:
            out.append(filtered)
    return out


def main():
    print("Omega Co-Change Analysis")
    print("========================\n")
    print(f"Project root: {PROJECT_ROOT}")

    current = load_current_files()
    print(f"Currently tracked files (excl. built assets): {len(current)}")

    # Git
    git_cs_raw = collect_git_changesets()
    git_cs = filter_to_current(git_cs_raw, current)
    print(f"Git commits with ≥1 src file: {len(git_cs_raw)} → {len(git_cs)} after current-file filter")

    # Sessions
    sess_cs_raw = collect_session_changesets()
    sess_cs = filter_to_current(sess_cs_raw, current)
    print(f"Sessions with ≥2 written files: {len(sess_cs_raw)} → {len(sess_cs)} after current-file filter")

    # --- Combined signal (current files only) ---
    combined = git_cs + sess_cs
    report("Combined — current files only (git + sessions)", combined, top_n=40)


if __name__ == "__main__":
    main()
