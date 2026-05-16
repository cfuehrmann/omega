#!/usr/bin/env python3
"""
Analyze Omega and Harbor sessions to find common orientation prefixes.

For each session, extracts:
  - Source (omega / harbor)
  - Task/prompt (first user message)
  - First N tool calls (name + normalized key argument)

Then groups by:
  1. Tool-call sequence pattern  →  frequency table
  2. Task-type cluster           →  most common orientation pattern per type

Usage:
    python3 bench/scripts/analyze_orientation.py [--top N] [--prefix-len N]
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from collections import Counter, defaultdict
from pathlib import Path
from textwrap import shorten

# ---------------------------------------------------------------------------
# Paths
# ---------------------------------------------------------------------------

REPO_ROOT = Path(__file__).parent.parent.parent  # omega/dev
OMEGA_SESSIONS_DIR = REPO_ROOT / ".omega" / "sessions"
HARBOR_JOBS_DIR = REPO_ROOT / "bench" / "jobs"

# ---------------------------------------------------------------------------
# Rust-era discriminator (see orientation.md for background)
# ---------------------------------------------------------------------------
#
# `.omega/sessions/` historically contained TypeScript-era sessions (Mar–early
# May 2026) with mock test fixtures (`abort_sleep_test` → `sleep 10` etc.).
# Those were archived to `.omega/sessions-archive-ts/`. To stay defensive
# against any future TS-era artifacts or mock-test pollution sneaking back in,
# this script filters to Rust-clean sessions on every load.
#
# Discriminator:
#   - sessionId matches Rust ISO format (matches directory name)
#     vs. TypeScript `<unix-ms>-<rand6>` format.
#   - no mock LLM URL (127.0.0.1 / localhost)
#   - msg/tool IDs (if any) match real Anthropic prefixes

_RUST_SESSION_ID_RE = re.compile(
    r"^\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}-\d{3}-[0-9a-f]+$"
)
_REAL_MSG_ID_RE  = re.compile(r"^msg_01[A-Za-z0-9]+$")
_REAL_TOOL_ID_RE = re.compile(r"^(toolu|srvtoolu)_01[A-Za-z0-9]+$")


def _is_rust_clean_session(events: list[dict]) -> bool:
    """Return True iff this is a Rust-era session free of mock-test pollution.

    See orientation.md for the rationale and the historical TS contamination
    that motivated this filter.
    """
    started = next(
        (e for e in events if e.get("type") in ("session_started", "session_start")),
        None,
    )
    if started is None:
        return False
    if not _RUST_SESSION_ID_RE.match(started.get("sessionId", "")):
        return False
    for ev in events:
        t = ev.get("type", "")
        if t == "llm_call":
            url = ev.get("url", "")
            if "127.0.0.1" in url or "localhost" in url:
                return False
        elif t == "llm_response":
            mid = (ev.get("responseSummary", {}) or {}).get("id", "")
            if mid and not _REAL_MSG_ID_RE.match(mid):
                return False
        elif t == "tool_call":
            tid = ev.get("id", "")
            if tid and not _REAL_TOOL_ID_RE.match(tid):
                return False
    return True

# ---------------------------------------------------------------------------
# Tool-call normalisation
# ---------------------------------------------------------------------------

ORIENTATION_TOOL_DEPTH = 8   # how many tool calls constitute the "prefix"


def _strip_cwd(path: str) -> str:
    """Remove common container/host prefixes so paths are comparable."""
    for prefix in ("/app", "/home/carsten/omega/dev", "/home/carsten/omega/main"):
        if path.startswith(prefix):
            path = path[len(prefix):]
    return path or "/"


def _normalise_path(path: str) -> str:
    p = _strip_cwd(path)
    # Collapse deep paths to just first two components
    parts = [x for x in p.split("/") if x]
    if len(parts) > 2:
        return "/" + "/".join(parts[:2]) + "/…"
    return "/" + "/".join(parts) if parts else "/"


def _normalise_command(cmd: str) -> str:
    """Keep the first word (binary) and up to two more tokens."""
    tokens = cmd.split()
    # strip leading env vars / sudo
    while tokens and ("=" in tokens[0] or tokens[0] in ("sudo", "env")):
        tokens.pop(0)
    return " ".join(tokens[:3]) if tokens else cmd


def normalise_tool_call(name: str, input_: dict) -> str:
    """Return a compact, comparable label for a tool call."""
    match name:
        case "read_file":
            p = input_.get("path", "")
            return f"read_file({_normalise_path(p)})"
        case "write_file":
            p = input_.get("path", "")
            return f"write_file({_normalise_path(p)})"
        case "edit_file":
            p = input_.get("path", "")
            return f"edit_file({_normalise_path(p)})"
        case "list_files":
            p = input_.get("path", "")
            rec = ",r" if input_.get("recursive") else ""
            return f"list_files({_normalise_path(p)}{rec})"
        case "find_files":
            pat = input_.get("pattern", "*")
            return f"find_files({pat[:30]})"
        case "grep_files":
            pat = input_.get("pattern", "")
            return f"grep_files({pat[:30]})"
        case "run_command":
            cmd = input_.get("command", "")
            return f"run_command({_normalise_command(cmd)})"
        case "run_background":
            cmd = input_.get("command", "")
            return f"run_background({_normalise_command(cmd)})"
        case "web_search":
            q = input_.get("query", "")
            return f"web_search({q[:30]})"
        case "fetch_url":
            url = input_.get("url", "")
            return f"fetch_url({url[:40]})"
        case _:
            return name


# ---------------------------------------------------------------------------
# Session loading
# ---------------------------------------------------------------------------

def _events_from_file(path: Path) -> list[dict]:
    events = []
    try:
        for line in path.read_text(errors="replace").splitlines():
            line = line.strip()
            if not line:
                continue
            try:
                events.append(json.loads(line))
            except json.JSONDecodeError:
                pass
    except OSError:
        pass
    return events


def _first_user_message(events: list[dict]) -> str | None:
    for ev in events:
        if ev.get("type") in ("user_message",):
            c = ev.get("content", "")
            if isinstance(c, str):
                return c.strip()
            if isinstance(c, list):
                parts = [b.get("text", "") for b in c if isinstance(b, dict)]
                return " ".join(parts).strip()
    return None


def _tool_prefix(events: list[dict], depth: int) -> list[str]:
    calls = []
    for ev in events:
        if ev.get("type") == "tool_call":
            name = ev.get("name", "")
            inp  = ev.get("input", {})
            calls.append(normalise_tool_call(name, inp))
            if len(calls) >= depth:
                break
    return calls


def _total_tool_calls(events: list[dict]) -> int:
    return sum(1 for ev in events if ev.get("type") == "tool_call")


def _reward(events: list[dict]) -> float | None:
    for ev in reversed(events):
        if "reward" in ev:
            return ev["reward"]
    return None


# ---- Omega sessions --------------------------------------------------------

def load_omega_sessions(depth: int) -> list[dict]:
    sessions = []
    if not OMEGA_SESSIONS_DIR.exists():
        return sessions
    for session_dir in sorted(OMEGA_SESSIONS_DIR.iterdir()):
        if not session_dir.is_dir():
            continue
        ef = session_dir / "events.jsonl"
        if not ef.exists():
            continue
        events = _events_from_file(ef)
        # Defensive: skip TS-era / mock-polluted sessions even though they
        # should already be archived. See orientation.md.
        if not _is_rust_clean_session(events):
            continue
        msg = _first_user_message(events)
        if not msg:
            continue
        sessions.append({
            "source":    "omega",
            "id":        session_dir.name,
            "task":      None,
            "prompt":    msg,
            "prefix":    _tool_prefix(events, depth),
            "n_tools":   _total_tool_calls(events),
            "reward":    None,
        })
    return sessions


# ---- Harbor bench sessions -------------------------------------------------

def load_harbor_sessions(depth: int) -> list[dict]:
    sessions = []
    if not HARBOR_JOBS_DIR.exists():
        return sessions

    # Find all events.jsonl under bench/jobs
    for ef in sorted(HARBOR_JOBS_DIR.rglob("events.jsonl")):
        # Derive task name from directory structure:
        # bench/jobs/<run>/<task_slug>/agent/...
        parts = ef.relative_to(HARBOR_JOBS_DIR).parts
        task_slug = parts[1] if len(parts) > 1 else "unknown"
        # strip the random suffix (last __XYZ component)
        task_name = re.sub(r"__[A-Za-z0-9]+$", "", task_slug)

        events = _events_from_file(ef)
        msg = _first_user_message(events)
        if not msg:
            continue

        # Try to read task.toml for metadata
        task_meta: dict = {}
        task_toml = ef.parent
        for _ in range(5):
            tt = task_toml / "task.toml"
            if tt.exists():
                try:
                    import tomllib  # 3.11+
                    task_meta = tomllib.loads(tt.read_text())
                except Exception:
                    pass
                break
            task_toml = task_toml.parent

        category = task_meta.get("metadata", {}).get("category", "unknown")
        difficulty = task_meta.get("metadata", {}).get("difficulty", "unknown")

        sessions.append({
            "source":     "harbor",
            "id":         str(ef.relative_to(HARBOR_JOBS_DIR)),
            "task":       task_name,
            "category":   category,
            "difficulty": difficulty,
            "prompt":     msg,
            "prefix":     _tool_prefix(events, depth),
            "n_tools":    _total_tool_calls(events),
            "reward":     None,
        })
    return sessions


# ---------------------------------------------------------------------------
# Task-type classification
# ---------------------------------------------------------------------------

TASK_TYPE_PATTERNS: list[tuple[str, list[str]]] = [
    ("orientation/readme",    ["readme", "agent.md", "claude.md", "agents.md"]),
    ("orient+code",           ["orient", "look for", "project", "codebase"]),
    ("file-analysis",         ["read", "file", "parse", "extract", "what does", "what is in"]),
    ("code-writing",          ["implement", "write", "create", "add", "build a", "make a"]),
    ("bug-fix",               ["fix", "bug", "error", "broken", "crash", "failing"]),
    ("refactor",              ["refactor", "rename", "move", "restructure", "clean"]),
    ("test-writing",          ["test", "spec", "unit test", "write test"]),
    ("cli/shell",             ["command", "bash", "shell", "script", "run"]),
    ("data-analysis",         ["data", "csv", "json", "analyze", "statistics", "calculate"]),
    ("docker/infra",          ["docker", "container", "deploy", "infra", "kubernetes"]),
    ("language-specific-c",   [" c ", ".c ", " c++", "cmake", "makefile"]),
    ("language-specific-rs",  ["rust", "cargo", ".rs"]),
    ("language-specific-py",  ["python", "pip", ".py"]),
    ("language-specific-ts",  ["typescript", "node", "npm", ".ts", ".js"]),
    ("language-specific-go",  ["golang", " go ", "go build"]),
    ("math/algo",             ["algorithm", "sort", "graph", "dynamic programming", "math"]),
    ("meta/ping",             ["ping", "hello", "test"]),
]


def classify_prompt(prompt: str) -> str:
    low = prompt.lower()
    for label, keywords in TASK_TYPE_PATTERNS:
        if any(kw in low for kw in keywords):
            return label
    return "other"


# ---------------------------------------------------------------------------
# Analysis helpers
# ---------------------------------------------------------------------------

def prefix_signature(prefix: list[str]) -> str:
    return " → ".join(prefix) if prefix else "(no tool calls)"


def common_prefix_len(seqs: list[list[str]]) -> int:
    if not seqs:
        return 0
    ref = seqs[0]
    for i, item in enumerate(ref):
        if any(len(s) <= i or s[i] != item for s in seqs[1:]):
            return i
    return len(ref)


def print_section(title: str) -> None:
    print()
    print("=" * 80)
    print(f"  {title}")
    print("=" * 80)


# ---------------------------------------------------------------------------
# Main report
# ---------------------------------------------------------------------------

def run(top: int, prefix_len: int) -> None:
    print("Loading sessions …", file=sys.stderr)
    omega   = load_omega_sessions(prefix_len)
    harbor  = load_harbor_sessions(prefix_len)
    all_s   = omega + harbor
    print(f"  Omega: {len(omega)}  Harbor: {len(harbor)}  Total: {len(all_s)}", file=sys.stderr)

    # -----------------------------------------------------------------------
    # 1. Overall tool-prefix frequency (both corpora)
    # -----------------------------------------------------------------------
    print_section("1. Most common first-tool-call (all sessions)")
    first_tool: Counter = Counter()
    for s in all_s:
        if s["prefix"]:
            first_tool[s["prefix"][0]] += 1
    for tool, cnt in first_tool.most_common(top):
        pct = 100 * cnt / len(all_s)
        print(f"  {cnt:4d} ({pct:5.1f}%)  {tool}")

    print_section("2. Most common prefix sequences (all sessions, first 3 calls)")
    seq3: Counter = Counter()
    for s in all_s:
        sig = prefix_signature(s["prefix"][:3])
        seq3[sig] += 1
    for sig, cnt in seq3.most_common(top):
        pct = 100 * cnt / len(all_s)
        print(f"  {cnt:4d} ({pct:5.1f}%)  {sig}")

    # -----------------------------------------------------------------------
    # 2. Harbor-only: by task category
    # -----------------------------------------------------------------------
    print_section("3. Harbor: first tool call by task category")
    cat_first: dict[str, Counter] = defaultdict(Counter)
    for s in harbor:
        cat = s.get("category", "unknown")
        if s["prefix"]:
            cat_first[cat][s["prefix"][0]] += 1
    for cat in sorted(cat_first):
        print(f"\n  [{cat}]")
        total = sum(cat_first[cat].values())
        for tool, cnt in cat_first[cat].most_common(5):
            print(f"    {cnt:3d}/{total}  {tool}")

    # -----------------------------------------------------------------------
    # 3. Harbor-only: by prompt-type classification
    # -----------------------------------------------------------------------
    print_section("4. Harbor: orientation prefix by prompt type (first 4 calls)")
    ptype_prefixes: dict[str, list[list[str]]] = defaultdict(list)
    for s in harbor:
        pt = classify_prompt(s["prompt"])
        ptype_prefixes[pt].append(s["prefix"][:4])

    ptype_sig: dict[str, Counter] = defaultdict(Counter)
    for s in harbor:
        pt = classify_prompt(s["prompt"])
        sig = prefix_signature(s["prefix"][:4])
        ptype_sig[pt][sig] += 1

    for pt in sorted(ptype_sig, key=lambda x: -sum(ptype_sig[x].values())):
        total = sum(ptype_sig[pt].values())
        print(f"\n  [{pt}]  n={total}")
        for sig, cnt in ptype_sig[pt].most_common(3):
            print(f"    {cnt:3d}  {sig}")

    # -----------------------------------------------------------------------
    # 4. Omega-only: by prompt type
    # -----------------------------------------------------------------------
    print_section("5. Omega sessions: first 4 tool calls by prompt type")
    omega_ptype_sig: dict[str, Counter] = defaultdict(Counter)
    for s in omega:
        pt = classify_prompt(s["prompt"])
        sig = prefix_signature(s["prefix"][:4])
        omega_ptype_sig[pt][sig] += 1

    for pt in sorted(omega_ptype_sig, key=lambda x: -sum(omega_ptype_sig[x].values())):
        total = sum(omega_ptype_sig[pt].values())
        print(f"\n  [{pt}]  n={total}")
        for sig, cnt in omega_ptype_sig[pt].most_common(3):
            print(f"    {cnt:3d}  {sig}")

    # -----------------------------------------------------------------------
    # 5. No-orientation sessions (sessions with 0 tool calls in prefix)
    # -----------------------------------------------------------------------
    print_section("6. Sessions with NO tool calls (answered without any tools)")
    no_tools_omega  = sum(1 for s in omega   if not s["prefix"])
    no_tools_harbor = sum(1 for s in harbor  if not s["prefix"])
    print(f"  Omega:  {no_tools_omega}/{len(omega)} ({100*no_tools_omega/max(len(omega),1):.1f}%)")
    print(f"  Harbor: {no_tools_harbor}/{len(harbor)} ({100*no_tools_harbor/max(len(harbor),1):.1f}%)")

    # -----------------------------------------------------------------------
    # 6. Files most frequently read in the orientation prefix
    # -----------------------------------------------------------------------
    print_section("7. Files read most often in orientation prefix (read_file / list_files)")
    path_counter: Counter = Counter()
    for s in all_s:
        for call in s["prefix"]:
            m = re.match(r"(read_file|list_files)\((.+?)\)", call)
            if m:
                path_counter[m.group(2)] += 1
    for path, cnt in path_counter.most_common(top):
        pct = 100 * cnt / len(all_s)
        print(f"  {cnt:4d} ({pct:5.1f}%)  {path}")

    # -----------------------------------------------------------------------
    # 7. Harbor: common 2-call orientation sequences with task name sample
    # -----------------------------------------------------------------------
    print_section("8. Harbor: top prefix sequences with example task names")
    sig_tasks: dict[str, list[str]] = defaultdict(list)
    for s in harbor:
        sig = prefix_signature(s["prefix"][:3])
        task = s.get("task", "?")
        if task not in sig_tasks[sig]:
            sig_tasks[sig].append(task)
    sig_counts = Counter({sig: len(tasks) for sig, tasks in sig_tasks.items()})
    for sig, cnt in sig_counts.most_common(top):
        tasks_sample = ", ".join(sig_tasks[sig][:3])
        print(f"  {cnt:3d}  {sig}")
        print(f"       tasks: {tasks_sample}")

    # -----------------------------------------------------------------------
    # 8. Harbor: sessions where agent reads instruction.md immediately
    # -----------------------------------------------------------------------
    print_section("9. Harbor: does agent read instruction.md / task description first?")
    reads_instruction_first = 0
    reads_instruction_within3 = 0
    for s in harbor:
        if not s["prefix"]:
            continue
        for i, call in enumerate(s["prefix"][:3]):
            if "instruction" in call.lower() or "task" in call.lower() or "readme" in call.lower():
                if i == 0:
                    reads_instruction_first += 1
                reads_instruction_within3 += 1
                break
    n = len(harbor)
    print(f"  Reads instruction/task/readme as first call: {reads_instruction_first}/{n} ({100*reads_instruction_first/max(n,1):.1f}%)")
    print(f"  Reads it within first 3 calls:              {reads_instruction_within3}/{n} ({100*reads_instruction_within3/max(n,1):.1f}%)")

    # -----------------------------------------------------------------------
    # 9. Prompt samples per type (to validate classifier)
    # -----------------------------------------------------------------------
    print_section("10. Prompt-type classifier: sample prompts")
    pt_samples: dict[str, list[str]] = defaultdict(list)
    for s in all_s:
        pt = classify_prompt(s["prompt"])
        if len(pt_samples[pt]) < 2:
            pt_samples[pt].append(shorten(s["prompt"].replace("\n", " "), 100))
    for pt in sorted(pt_samples):
        print(f"\n  [{pt}]")
        for p in pt_samples[pt]:
            print(f'    "{p}"')

    print()


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--top", type=int, default=20, help="Top-N rows per table")
    ap.add_argument("--prefix-len", type=int, default=8, help="Tool calls to consider as orientation prefix")
    args = ap.parse_args()
    run(args.top, args.prefix_len)
