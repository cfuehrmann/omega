#!/usr/bin/env python3
"""
Analyze event logs for all failing (and some passing) trials.
Extracts turn counts, tool usage, stop reasons, final assistant text,
thinking snippets, and flags interesting patterns.
"""

import json, sys, os, re
from collections import defaultdict, Counter
from pathlib import Path

BENCH_DIR = Path(__file__).parent.parent
JOBS_DIR = BENCH_DIR / "jobs"
RESULTS_FILE = BENCH_DIR / "results" / "results.jsonl"

def parse_events(path):
    try:
        with open(path) as f:
            lines = f.read().strip().split("\n")
        return [json.loads(l) for l in lines if l.strip()]
    except Exception as e:
        return []

def analyze_events(events):
    """Extract structured info from an event log."""
    llm_responses = [e for e in events if e["type"] == "llm_response"]
    tool_calls = [e for e in events if e["type"] == "tool_call"]
    tool_results = [e for e in events if e["type"] == "tool_result"]
    turn_ends = [e for e in events if e["type"] == "turn_end"]
    
    stop_reasons = [e.get("stopReason","?") for e in llm_responses]
    tool_names = [e.get("name","?") for e in tool_calls]
    tool_name_counts = Counter(tool_names)
    
    # Tool error rate
    tool_errors = sum(1 for e in tool_results if e.get("isError", False))
    
    # Token usage per turn
    output_tokens_per_resp = [
        e.get("usage", {}).get("output_tokens", 0) for e in llm_responses
    ]
    max_output_tokens = max(output_tokens_per_resp) if output_tokens_per_resp else 0
    
    # Total metrics from turn_end
    total_metrics = {"inputTokens": 0, "outputTokens": 0, "cacheReadTokens": 0}
    for te in turn_ends:
        m = te.get("metrics", {})
        for k in total_metrics:
            total_metrics[k] += m.get(k, 0)
    
    # Thinking snippets (first 200 chars each)
    thinking_snippets = []
    for e in llm_responses:
        t = e.get("thinking", "")
        if t:
            thinking_snippets.append(t[:300])
    
    # Final assistant text (last llm_response text)
    final_text = ""
    for e in reversed(llm_responses):
        t = e.get("text", "")
        if t:
            final_text = t
            break
    
    # Tool error details
    error_details = []
    for e in tool_results:
        if e.get("isError"):
            out = e.get("output", "")[:200]
            error_details.append(f"{e.get('name','?')}: {out}")
    
    # Tool result output summaries (last result for each tool call)
    last_tool_outputs = []
    for e in tool_results[-3:]:
        out = e.get("output", "")[:200]
        last_tool_outputs.append(f"  [{e.get('name','?')}] {out}")
    
    # Check for repetition: same tool called with same input multiple times
    tool_signatures = []
    for e in tool_calls:
        sig = (e.get("name","?"), json.dumps(e.get("input",{}), sort_keys=True))
        tool_signatures.append(sig)
    sig_counts = Counter(tool_signatures)
    repeated_tools = {f"{k[0]}({k[1][:60]})": v for k, v in sig_counts.items() if v > 1}
    
    return {
        "n_llm_responses": len(llm_responses),
        "n_tool_calls": len(tool_calls),
        "n_tool_errors": tool_errors,
        "stop_reasons": stop_reasons,
        "tool_name_counts": dict(tool_name_counts),
        "max_output_tokens": max_output_tokens,
        "total_output_tokens": sum(output_tokens_per_resp),
        "total_metrics": total_metrics,
        "thinking_snippets": thinking_snippets,
        "final_text": final_text[-600:],
        "error_details": error_details,
        "last_tool_outputs": last_tool_outputs,
        "repeated_tools": repeated_tools,
        "tool_signatures": tool_signatures,
    }

# Build task -> list of (job, logpath) mappings
task_logs = defaultdict(list)
for job_dir in sorted(JOBS_DIR.iterdir()):
    if not job_dir.is_dir():
        continue
    for trial_dir in job_dir.iterdir():
        if not trial_dir.is_dir():
            continue
        log_path = trial_dir / "agent" / "events.jsonl"
        if not log_path.exists():
            continue
        task_short = re.sub(r"__[^_]+$", "", trial_dir.name)
        task_name = f"terminal-bench/{task_short}"
        task_logs[task_name].append((job_dir.name, str(log_path)))

# Load results
results = []
with open(RESULTS_FILE) as f:
    for line in f:
        if line.strip():
            results.append(json.loads(line))

# Separate passes and fails
fails = [r for r in results if r.get("reward", 1) == 0]
passes = [r for r in results if r.get("reward", 1) == 1]

print(f"Total results: {len(results)}, passes: {len(passes)}, fails: {len(fails)}")
print()

# For each failing task, pick the LAST failing trial log
def get_latest_log(task_name, results_list):
    """Get the last trial log for a task matching results list."""
    logs = task_logs.get(task_name, [])
    if not logs:
        return None, None
    # Return the last one (most recent job)
    return logs[-1]

print("=" * 80)
print("FAILING TRIAL DETAILED ANALYSIS")
print("=" * 80)

# Cluster results
by_shape = defaultdict(list)

for r in sorted(fails, key=lambda x: x["task_name"]):
    task = r["task_name"]
    short = task.replace("terminal-bench/", "")
    exc = r.get("exception") or "none"
    rt = r.get("runtime_sec", 0)
    
    log_entry = get_latest_log(task, fails)
    if not log_entry:
        print(f"\n[{short}] NO LOG FOUND")
        continue
    
    job, log_path = log_entry
    events = parse_events(log_path)
    info = analyze_events(events)
    
    print(f"\n{'='*60}")
    print(f"TASK: {short}")
    print(f"  job={job}  rt={rt:.0f}s  exc={exc}")
    print(f"  LLM responses: {info['n_llm_responses']}  tool calls: {info['n_tool_calls']}  tool errors: {info['n_tool_errors']}")
    print(f"  Stop reasons: {Counter(info['stop_reasons']).most_common()}")
    print(f"  Tools used: {info['tool_name_counts']}")
    print(f"  Max output tokens in one response: {info['max_output_tokens']}")
    if info['repeated_tools']:
        print(f"  *** REPEATED TOOL CALLS: {info['repeated_tools']}")
    if info['error_details']:
        print(f"  Tool errors:")
        for e in info['error_details'][:3]:
            print(f"    {e}")
    print(f"  Last tool outputs:")
    for o in info['last_tool_outputs']:
        print(f"    {o}")
    print(f"  Final assistant text (last 400 chars):")
    print(f"    {info['final_text'][-400:]}")
    
    # Classify into shapes
    if exc == "AgentTimeoutError":
        by_shape["TIMEOUT_HARD"].append(short)
    elif info['n_llm_responses'] >= 45:
        by_shape["TURN_EXHAUST"].append(short)
    elif any(r == "max_tokens" for r in info['stop_reasons']):
        by_shape["MAX_TOKENS"].append(short)
    elif exc == "NonZeroAgentExitCodeError":
        by_shape["INFRA_EXIT"].append(short)
    elif rt < 200:
        by_shape["FAST_FAIL"].append(short)
    else:
        by_shape["WRONG_ANSWER"].append(short)

print("\n\n" + "="*80)
print("SHAPE CLUSTERS (based on actual log analysis)")
print("="*80)
for shape, tasks in by_shape.items():
    print(f"\n{shape} ({len(tasks)}):")
    for t in tasks:
        print(f"  - {t}")

print("\n\n" + "="*80)
print("SELECTED PASSING TRIALS FOR CONTRAST")
print("="*80)

# Sample a few passing trials in weak categories
contrast_tasks = [
    "terminal-bench/schemelike-metacircular-eval",  # passed despite AgentTimeoutError
    "terminal-bench/model-extraction-relu-logits",  # passed despite AgentTimeoutError
    "terminal-bench/feal-differential-cryptanalysis",  # math pass
    "terminal-bench/crack-7z-hash",  # flaky
    "terminal-bench/write-compressor",  # partial pass
]

for task in contrast_tasks:
    logs = task_logs.get(task, [])
    if not logs:
        continue
    # Find a passing trial
    passing_results = [r for r in passes if r["task_name"] == task]
    if not passing_results:
        continue
    
    short = task.replace("terminal-bench/", "")
    # Use the last log that matches a passing trial
    job, log_path = logs[-1]
    events = parse_events(log_path)
    info = analyze_events(events)
    
    print(f"\n{'='*60}")
    print(f"PASS: {short}")
    print(f"  job={job}  rt={passing_results[-1].get('runtime_sec',0):.0f}s")
    print(f"  LLM responses: {info['n_llm_responses']}  tool calls: {info['n_tool_calls']}")
    print(f"  Stop reasons: {Counter(info['stop_reasons']).most_common()}")
    print(f"  Tools used: {info['tool_name_counts']}")
    print(f"  Final assistant text:")
    print(f"    {info['final_text'][-400:]}")
