#!/usr/bin/env python3
"""
Sequential TB2 sweep runner — Phase 2.2.2(c) infrastructure.

Runs one Harbor trial per task, sequentially, with Docker cleanup between
trials.  Avoids parallel resource contention that caused install timeouts
in earlier sweep attempts.

Pre-requisites
--------------
1. Build the portable host binary before invoking this script::

       cd <repo-root>
       ./bench/build_release_binary.sh

   This builds inside ubuntu:20.04 (glibc 2.31) so the binary runs on
   every TB2 task image, including those with older base images
   (glibc < 2.38).  Output lands in ``target-builder/release/omega``.

2. Set ANTHROPIC_API_KEY in the environment.

3. Run from the bench/ directory so Harbor writes jobs/ output there::

       cd <repo-root>/bench
       python run_sequential_sweep.py [options]

Flags
-----
--resume          Skip tasks whose job dir already contains a result.json
                  with n_trials >= 1 (not just exceptions).
--tasks-from FILE Read newline-separated task names from FILE instead of
                  the default discovery list.
--dry-run         Print the task list and planned commands; do nothing.
--max-tasks N     Run only the first N tasks (smoke / spot-check).
"""
from __future__ import annotations

import argparse
import datetime
import json
import os
import signal
import subprocess
import sys
import time
from pathlib import Path
from typing import NamedTuple

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

TASKS_CACHE_ROOT = Path.home() / ".cache" / "harbor" / "tasks"

# Tasks to exclude from the default discovery list.
EXCLUDE_TASKS = {"mteb-retrieve", "hf-model-inference", "terminal-bench"}

AGENT_IMPORT_PATH = "omega_agent:OmegaRustAgent"
BENCHMARK_DATASET = "terminal-bench@2.0"
MODEL = "anthropic/claude-sonnet-4-6"
PRESET = "repl-centric"
JOB_PREFIX = "v0115-seq"
JOB_SUFFIX = "sonnet-medium-repl-centric"
N_TRIALS = 1

SUMMARY_FILE = Path(__file__).parent / "jobs" / f"{JOB_PREFIX}-{JOB_SUFFIX}-summary.json"

# ---------------------------------------------------------------------------
# Task discovery
# ---------------------------------------------------------------------------


def discover_tasks() -> list[str]:
    """
    Return an alphabetically-sorted list of all TB2 task names found in the
    Harbor task cache, excluding EXCLUDE_TASKS.

    The cache layout is:
        ~/.cache/harbor/tasks/<hash-dir>/<task-name>/task.toml
    We read the task name from the subdirectory name, not from task.toml,
    because the dir name IS the task name in Harbor's cache structure.
    """
    task_names: set[str] = set()
    if not TASKS_CACHE_ROOT.is_dir():
        return []
    for hash_dir in TASKS_CACHE_ROOT.iterdir():
        if not hash_dir.is_dir():
            continue
        for task_dir in hash_dir.iterdir():
            if task_dir.is_dir():
                task_names.add(task_dir.name)
    return sorted(task_names - EXCLUDE_TASKS)


# ---------------------------------------------------------------------------
# Resume helpers
# ---------------------------------------------------------------------------


def job_dir_for_task(task: str) -> Path:
    """Return the expected job output directory for a given task."""
    job_name = f"{JOB_PREFIX}-{task}-{JOB_SUFFIX}"
    return Path(__file__).parent / "jobs" / job_name


def _read_job_result(job_dir: Path) -> dict | None:
    """
    Read the top-level job result.json and return the first eval's stats dict,
    or None if the file is absent / malformed.

    Harbor v0.9.0 top-level result.json structure::

        {
          "stats": {
            "evals": {
              "<agent>__<model>__<job>": {
                "n_trials": int,
                "n_errors": int,
                "metrics": [{"mean": float}],
                "exception_stats": {"ExcClass": ["trial_id", ...]}
              }
            }
          }
        }
    """
    rfile = job_dir / "result.json"
    if not rfile.exists():
        return None
    try:
        data = json.loads(rfile.read_text())
    except (json.JSONDecodeError, OSError):
        return None
    evals = (data.get("stats") or {}).get("evals") or {}
    if not evals:
        return None
    # Return the first eval entry (there is always exactly one per Harbor job).
    return next(iter(evals.values()))


def result_is_complete(task: str) -> bool:
    """
    Return True if the task already has a valid result.json with at least
    one non-exception trial.
    """
    job_dir = job_dir_for_task(task)
    eval_stats = _read_job_result(job_dir)
    if eval_stats is None:
        return False
    n_trials = eval_stats.get("n_trials", 0)
    n_errors = eval_stats.get("n_errors", 0)
    return n_trials >= 1 and n_errors < n_trials


# ---------------------------------------------------------------------------
# Docker pruning
# ---------------------------------------------------------------------------


def docker_prune() -> None:
    """
    Remove stopped containers and dangling images.
    Never uses -a (would remove useful cached images) or --volumes.
    Failures are logged but do not abort the sweep.
    """
    for cmd in (
        ["docker", "container", "prune", "-f"],
        ["docker", "image", "prune", "-f"],
    ):
        try:
            subprocess.run(cmd, check=True, capture_output=True, timeout=60)
        except (subprocess.CalledProcessError, subprocess.TimeoutExpired) as exc:
            print(f"  [warn] docker prune step failed: {exc}", file=sys.stderr)


# ---------------------------------------------------------------------------
# Per-task run
# ---------------------------------------------------------------------------


class TaskResult(NamedTuple):
    task: str
    status: str       # "pass" | "fail" | "error" | "skipped" | "dry-run"
    elapsed_sec: float
    note: str


def run_task(task: str, dry_run: bool) -> TaskResult:
    """Run a single Harbor trial for *task* and return the outcome."""
    job_name = f"{JOB_PREFIX}-{task}-{JOB_SUFFIX}"
    cmd = [
        "harbor", "run",
        "-d", BENCHMARK_DATASET,
        "--agent-import-path", AGENT_IMPORT_PATH,
        "-m", MODEL,
        f"--ae", f"ANTHROPIC_API_KEY={os.environ.get('ANTHROPIC_API_KEY', '')}",
        "--agent-kwarg", f"preset={PRESET}",
        "-n", str(N_TRIALS),
        "-t", f"terminal-bench/{task}",
        "--job-name", job_name,
    ]

    if dry_run:
        print(f"  [dry-run] {' '.join(cmd)}")
        return TaskResult(task=task, status="dry-run", elapsed_sec=0.0, note="")

    t0 = time.monotonic()
    try:
        proc = subprocess.run(
            cmd,
            timeout=None,   # Harbor enforces per-task timeouts internally
            check=False,    # we inspect returncode ourselves
        )
        elapsed = time.monotonic() - t0
        rc = proc.returncode
    except Exception as exc:
        elapsed = time.monotonic() - t0
        return TaskResult(task=task, status="error", elapsed_sec=elapsed, note=str(exc))

    # Inspect result.json written by Harbor to determine pass/fail.
    eval_stats = _read_job_result(job_dir_for_task(task))
    mean: float | None = None
    exception_class: str | None = None

    if eval_stats is not None:
        exc_stats = eval_stats.get("exception_stats") or {}
        if exc_stats:
            exception_class = next(iter(exc_stats.keys()))
        metrics = eval_stats.get("metrics") or []
        if metrics and "mean" in metrics[0]:
            mean = metrics[0]["mean"]

    if exception_class:
        note = exception_class
        status = "error"
    elif mean is None:
        note = f"rc={rc} no result.json"
        status = "fail"
    elif mean == 1.0:
        note = f"mean={mean:.1f}"
        status = "pass"
    else:
        note = f"mean={mean:.2f}"
        status = "fail"

    return TaskResult(task=task, status=status, elapsed_sec=elapsed, note=note)


# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------


def collect_summary(results: list[TaskResult]) -> dict:
    """Aggregate per-task results into a summary dict."""
    passed = [r for r in results if r.status == "pass"]
    failed = [r for r in results if r.status == "fail"]
    errors = [r for r in results if r.status == "error"]

    exception_breakdown: dict[str, int] = {}
    for r in errors:
        key = r.note or "unknown"
        exception_breakdown[key] = exception_breakdown.get(key, 0) + 1

    return {
        "date": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "job_prefix": JOB_PREFIX,
        "model": MODEL,
        "preset": PRESET,
        "total_attempted": len(results),
        "passed": len(passed),
        "failed": len(failed),
        "errors": len(errors),
        "exception_breakdown": exception_breakdown,
        "total_wall_sec": sum(r.elapsed_sec for r in results),
        "tasks": [
            {
                "name": r.task,
                "status": r.status,
                "elapsed_sec": round(r.elapsed_sec, 1),
                "note": r.note,
            }
            for r in results
        ],
    }


def write_summary(summary: dict) -> None:
    SUMMARY_FILE.parent.mkdir(parents=True, exist_ok=True)
    SUMMARY_FILE.write_text(json.dumps(summary, indent=2))
    print(f"\nSummary written to {SUMMARY_FILE}")


def print_summary(summary: dict) -> None:
    total = summary["total_attempted"]
    passed = summary["passed"]
    failed = summary["failed"]
    errors = summary["errors"]
    wall = summary["total_wall_sec"]
    print(f"\n{'='*70}")
    print(f"Sweep complete — {total} tasks | {passed} passed | {failed} failed | {errors} errors")
    print(f"Total wall time: {wall/3600:.2f} h ({wall:.0f} s)")
    if summary["exception_breakdown"]:
        print("Exception breakdown:")
        for exc, count in summary["exception_breakdown"].items():
            print(f"  {count:3d}x  {exc}")
    print(f"{'='*70}")


# ---------------------------------------------------------------------------
# Ctrl-C handler
# ---------------------------------------------------------------------------

_interrupted = False


def _sigint_handler(signum, frame):
    global _interrupted
    if not _interrupted:
        print("\n[Ctrl-C received — finishing current task, then stopping]", file=sys.stderr)
        _interrupted = True
    else:
        # Second Ctrl-C: give up immediately.
        sys.exit(130)


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Sequential TB2 sweep with Docker cleanup between trials.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    parser.add_argument(
        "--resume",
        action="store_true",
        help="Skip tasks that already have a valid result.json (n_trials >= 1).",
    )
    parser.add_argument(
        "--tasks-from",
        metavar="FILE",
        help="Read newline-separated task names from FILE instead of auto-discovery.",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Print planned commands without running anything.",
    )
    parser.add_argument(
        "--max-tasks",
        type=int,
        metavar="N",
        help="Run at most N tasks (smoke / spot-check mode).",
    )
    args = parser.parse_args()

    # Build task list.
    if args.tasks_from:
        src = Path(args.tasks_from)
        if src.name == "-" or str(src) == "/dev/stdin":
            task_list = [
                line.strip()
                for line in sys.stdin
                if line.strip() and not line.startswith("#")
            ]
        else:
            task_list = [
                line.strip()
                for line in src.read_text().splitlines()
                if line.strip() and not line.startswith("#")
            ]
    else:
        task_list = discover_tasks()

    if not task_list:
        print("No tasks found — check ~/.cache/harbor/tasks/ or --tasks-from.", file=sys.stderr)
        return 1

    if args.max_tasks:
        task_list = task_list[: args.max_tasks]

    print(f"Tasks to run: {len(task_list)}")

    if args.dry_run:
        print("\n--- Planned tasks (dry-run) ---")
        for task in task_list:
            print(f"  {task}")
            run_task(task, dry_run=True)
        return 0

    # Install Ctrl-C handler.
    signal.signal(signal.SIGINT, _sigint_handler)

    results: list[TaskResult] = []
    sweep_start = time.monotonic()

    for i, task in enumerate(task_list, 1):
        if _interrupted:
            print("[Sweep interrupted — summarising completed tasks]")
            break

        print(f"\n[{i}/{len(task_list)}] {task}", flush=True)

        # --resume: skip if already done.
        if args.resume and result_is_complete(task):
            print(f"  → skipped (already complete)")
            results.append(TaskResult(task=task, status="skipped", elapsed_sec=0.0, note="resumed"))
            continue

        # Run the trial.
        result = run_task(task, dry_run=False)
        results.append(result)

        elapsed_str = f"{result.elapsed_sec:.0f}s"
        print(f"  → {result.status.upper():<8} {elapsed_str:<8} {result.note}")

        # Docker cleanup between trials (dangling only; never -a, never --volumes).
        if not _interrupted:
            docker_prune()

    total_wall = time.monotonic() - sweep_start
    summary = collect_summary(results)
    summary["total_wall_sec"] = round(total_wall, 1)

    print_summary(summary)
    write_summary(summary)

    # Print per-task table.
    print(f"\n{'Task':<45} {'Status':<10} {'Elapsed':>9}  Note")
    print("-" * 75)
    for r in results:
        print(f"{r.task:<45} {r.status:<10} {r.elapsed_sec:>8.0f}s  {r.note}")

    return 130 if _interrupted else (0 if all(r.status in ("pass", "skipped") for r in results) else 1)


if __name__ == "__main__":
    sys.exit(main())
