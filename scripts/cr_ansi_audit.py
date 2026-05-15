#!/usr/bin/env python3
"""Audit \r-progress and ANSI escape usage in run_command / wait_for_output outputs.

For each tool_result that contains a bare \r (not \r\n) or an ANSI escape
sequence, this script:

  1. Records the command (argv[0]) and the full output length.
  2. Categorises the \r pattern into:
       - "cr_only"   : output has \r but NO \r\n at all  (likely semantic)
       - "cr_mixed"  : output has both \r (standalone) and \r\n  (likely progress + CRLF)
       - "cr_progress": output has many \r not followed by \n    (classic progress bar)
  3. Checks if the simulated cleaning changes the output materially or
     drops non-whitespace content.

Goal: surface edge cases where cleaning would be *wrong*.

Usage:
    python3 scripts/cr_ansi_audit.py [--bench] [--top N]
"""
from __future__ import annotations

import argparse
import json
import re
import shlex
from collections import Counter, defaultdict
from pathlib import Path

# ---------------------------------------------------------------------------
# Patterns
# ---------------------------------------------------------------------------

# Matches the vast majority of ANSI/VT100 sequences:
#   CSI sequences:  \x1b[ ... letter
#   OSC sequences:  \x1b] ... \x07 or \x1b\\
#   Single-char:    \x1b followed by a non-[ non-] char
ANSI_CSI_RE = re.compile(r"\x1b\[[0-9;?]*[A-Za-z]")
ANSI_OSC_RE = re.compile(r"\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)")
ANSI_OTHER_RE = re.compile(r"\x1b[^[\]][A-Za-z]")

ALL_ANSI_RE = re.compile(
    r"\x1b\[[0-9;?]*[A-Za-z]"          # CSI
    r"|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)"  # OSC (hyperlinks, window title)
    r"|\x1b[^[\]][A-Za-z]"              # other single-char escapes
)

# Standalone \r: a carriage-return NOT immediately followed by \n.
BARE_CR_RE = re.compile(r"\r(?!\n)")

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def argv0(cmd: str) -> str:
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
                "bun", "bunx", "npx", "go", "make", "apt", "apt-get"):
        if i + 1 < len(toks) and not toks[i + 1].startswith("-"):
            return f"{head} {toks[i+1]}"
    return head


def simulate_clean(s: str) -> str:
    """Apply the planned cleaning to a string, return the result."""
    # Step 1: collapse \r-overwritten frames within each \n-separated line.
    lines = s.split("\n")
    cleaned_lines = []
    for line in lines:
        # Keep everything after the last bare \r.
        parts = line.split("\r")
        cleaned_lines.append(parts[-1])
    result = "\n".join(cleaned_lines)

    # Step 2: strip ANSI escape sequences.
    result = ALL_ANSI_RE.sub("", result)
    return result


def cleaning_drops_content(original: str, cleaned: str) -> tuple[bool, str]:
    """
    Return (problem, reason) where problem=True means cleaning removed something
    that doesn't look like pure noise.

    Heuristics for "not noise":
    - Non-whitespace, non-progress content that vanishes from \r-collapse.
    - ANSI sequences that carry non-colour meaning (OSC hyperlinks, window titles).
    """
    # Check: does cleaned have significantly less non-whitespace content
    # that can't be explained by \r-collapse or ANSI-colour stripping?

    orig_nows = re.sub(r"\s+", "", original)
    clean_nows = re.sub(r"\s+", "", cleaned)

    # Remove the \r-frame characters from original to estimate what survives
    # pure \r-collapse (ANSI still present at this stage).
    cr_collapsed = "\n".join(line.split("\r")[-1] for line in original.split("\n"))
    cc_nows = re.sub(r"\s+", "", cr_collapsed)
    ansi_only_removed = re.sub(r"\s+", "", ALL_ANSI_RE.sub("", cr_collapsed))

    lost_by_cr = len(orig_nows) - len(cc_nows)
    lost_by_ansi = len(cc_nows) - len(ansi_only_removed)

    # Flag if OSC sequences present (hyperlinks / window titles carry semantic content)
    osc_matches = ANSI_OSC_RE.findall(original)
    if osc_matches:
        sample = osc_matches[0][:80]
        return True, f"OSC escape removed (hyperlink/title?): {repr(sample)}"

    # Flag if \r-collapse removed a lot of content that doesn't look like
    # progress bars (i.e., the \r-overwritten content is substantial and varied).
    if lost_by_cr > 200:
        # Sample the content that was overwritten.
        overwritten_parts = []
        for line in original.split("\n"):
            parts = line.split("\r")
            if len(parts) > 1:
                # Everything except the last part was overwritten.
                overwritten_parts.extend(parts[:-1])
        non_progress = [p for p in overwritten_parts
                        if p.strip()
                        and not re.match(r"^\s*(Read \d+[KMGT]? words|[-#=>\s\.]+|\d+%.*|\[\d+/\d+\]|Downloading|Fetching|Installing|Compiling|Building|Receiving|Counting|Resolving|Unpacking|Processing|Preparing|Verifying|Extracting|Pulling|Pushing|Uploading)\s*$",
                                         p.strip(), re.IGNORECASE)]
        if non_progress:
            sample = non_progress[0][:100]
            return True, f"\\r-collapse removed non-progress content: {repr(sample)}"

    return False, ""


# ---------------------------------------------------------------------------
# Audit
# ---------------------------------------------------------------------------

def audit(event_paths: list[Path], scope: str) -> None:
    # Per-tool counters
    total_results = 0
    cr_results = 0      # has bare \r
    ansi_results = 0    # has ANSI escapes (CSI)
    osc_results = 0     # has OSC escapes (hyperlinks etc.)
    problem_results = 0 # cleaning would drop semantic content

    # By command
    cr_by_cmd: Counter = Counter()
    ansi_by_cmd: Counter = Counter()
    problem_by_cmd: Counter = Counter()

    # CR pattern classification
    cr_pattern: Counter = Counter()

    # Bytes saved estimate
    total_bytes_with_cr = 0
    bytes_after_clean_cr = 0
    total_bytes_with_ansi = 0
    bytes_after_clean_ansi = 0

    problems: list[tuple[str, str, str, str]] = []  # (cmd, reason, preview, session)

    for ev_path in event_paths:
        if not ev_path.exists():
            continue
        calls: dict[str, dict] = {}
        try:
            with ev_path.open() as f:
                events = []
                for line in f:
                    try:
                        e = json.loads(line)
                        events.append(e)
                    except json.JSONDecodeError:
                        continue
        except OSError:
            continue

        sid = str(ev_path.parent.name)

        for e in events:
            if e.get("type") == "tool_call":
                eid = e.get("id") or e.get("toolCallId")
                if eid:
                    calls[eid] = e

        for e in events:
            if e.get("type") != "tool_result":
                continue
            tool = e.get("name", "?")
            if tool not in ("run_command", "wait_for_output"):
                continue

            out = e.get("output", "") or ""
            if isinstance(out, list):
                out = json.dumps(out)

            total_results += 1
            eid = e.get("id") or e.get("toolCallId")
            call = calls.get(eid, {}) if eid else {}
            inp = call.get("input") or {}
            cmd = inp.get("command", "") if tool == "run_command" else f"[wait_for_output pid={inp.get('pid','')}]"
            cmd_key = argv0(cmd) if cmd else "(?)"

            has_bare_cr = bool(BARE_CR_RE.search(out))
            has_ansi_csi = bool(ANSI_CSI_RE.search(out))
            has_ansi_osc = bool(ANSI_OSC_RE.search(out))

            if has_bare_cr:
                cr_results += 1
                cr_by_cmd[cmd_key] += 1
                total_bytes_with_cr += len(out)

                # CR pattern classification
                has_crlf = "\r\n" in out
                bare_cr_count = len(BARE_CR_RE.findall(out))
                if not has_crlf:
                    cr_pattern["cr_only (no CRLF)"] += 1
                elif bare_cr_count >= 5:
                    cr_pattern["cr_mixed (progress + CRLF)"] += 1
                else:
                    cr_pattern["cr_sparse (1-4 bare CRs)"] += 1

                after_cr = simulate_clean(out)
                bytes_after_clean_cr += len(after_cr)

                problem, reason = cleaning_drops_content(out, after_cr)
                if problem:
                    problem_results += 1
                    problem_by_cmd[cmd_key] += 1
                    problems.append((cmd_key, reason, out[:120], sid))

            if has_ansi_csi or has_ansi_osc:
                ansi_results += 1
                ansi_by_cmd[cmd_key] += 1
                total_bytes_with_ansi += len(out)
                after_ansi = ALL_ANSI_RE.sub("", out)
                bytes_after_clean_ansi += len(after_ansi)

            if has_ansi_osc:
                osc_results += 1

    print(f"\n{'='*60}")
    print(f"Scope: {scope}")
    print(f"{'='*60}")
    print(f"run_command + wait_for_output results scanned: {total_results}")
    print()

    print(f"--- Bare \\r (progress bar / CR-overwrite) ---")
    print(f"Results with bare \\r:  {cr_results} / {total_results}  ({cr_results/max(total_results,1):.1%})")
    if total_bytes_with_cr:
        saved = total_bytes_with_cr - bytes_after_clean_cr
        print(f"Bytes in those results: {total_bytes_with_cr:,}  →  after CR-clean: {bytes_after_clean_cr:,}  (save {saved:,}, {saved/total_bytes_with_cr:.1%})")
    print()
    print("CR pattern breakdown:")
    for pat, n in cr_pattern.most_common():
        print(f"  {pat}: {n}")
    print()
    print(f"Top commands with bare \\r:")
    for cmd, n in cr_by_cmd.most_common(20):
        print(f"  {n:4d}  {cmd}")
    print()

    print(f"--- ANSI escape sequences ---")
    print(f"Results with ANSI CSI/OSC: {ansi_results} / {total_results}  ({ansi_results/max(total_results,1):.1%})")
    if total_bytes_with_ansi:
        saved = total_bytes_with_ansi - bytes_after_clean_ansi
        print(f"Bytes in those results: {total_bytes_with_ansi:,}  →  after ANSI-strip: {bytes_after_clean_ansi:,}  (save {saved:,}, {saved/total_bytes_with_ansi:.1%})")
    print(f"Results with OSC sequences (hyperlinks/titles): {osc_results}")
    print()
    print(f"Top commands with ANSI escapes:")
    for cmd, n in ansi_by_cmd.most_common(20):
        print(f"  {n:4d}  {cmd}")
    print()

    print(f"--- Potential problems ---")
    print(f"Results where cleaning might drop semantic content: {problem_results}")
    if problems:
        print()
        print("Flagged cases:")
        for cmd, reason, preview, sid in problems[:20]:
            print(f"  cmd={cmd}")
            print(f"  reason={reason}")
            print(f"  preview={repr(preview[:100])}")
            print(f"  session={sid}")
            print()


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--bench", action="store_true", help="Also scan bench/jobs sessions")
    ap.add_argument("--local-only", action="store_true")
    args = ap.parse_args()

    local_dir = Path(".omega/sessions")
    local_paths = sorted(local_dir.glob("*/events.jsonl")) if local_dir.exists() else []

    if local_paths:
        audit(local_paths, f"local interactive sessions ({len(local_paths)} sessions)")

    if args.bench or not args.local_only:
        bench_root = Path("bench/jobs")
        bench_paths = sorted(bench_root.glob("*/*/agent/events.jsonl")) if bench_root.exists() else []
        if bench_paths:
            audit(bench_paths, f"benchmark trials ({len(bench_paths)} sessions)")
        elif args.bench:
            print("No bench sessions found in bench/jobs/")


if __name__ == "__main__":
    main()
