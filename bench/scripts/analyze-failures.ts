#!/usr/bin/env bun
/**
 * Analyzes event logs for all failing trials, extracting key patterns.
 * Usage: bun scripts/analyze-failures.ts
 */

import { readdirSync, readFileSync, existsSync } from "fs";
import { join, resolve } from "path";

const BENCH_DIR = resolve(import.meta.dir, "..");

interface EventRecord {
  type: string;
  ts?: string;
  turn?: number;
  content?: unknown;
  tool?: string;
  tool_name?: string;
  stop_reason?: string;
  usage?: { input_tokens?: number; output_tokens?: number };
  error?: string;
  text?: string;
  result?: unknown;
  [key: string]: unknown;
}

interface TrialSummary {
  task: string;
  job: string;
  logPath: string;
  reward: number;
  exception: string | null;
  runtimeSec: number;
  nTurns: number;
  nToolCalls: number;
  toolCounts: Record<string, number>;
  stopReasons: string[];
  finalEvents: string[];
  lastAssistantText: string;
  errorMessages: string[];
  totalOutputTokens: number;
  maxOutputTokensInOneTurn: number;
  interesting: string[];
}

function parseEvents(path: string): EventRecord[] {
  try {
    const lines = readFileSync(path, "utf-8").trim().split("\n");
    return lines.filter(Boolean).map((l) => {
      try { return JSON.parse(l); }
      catch { return { type: "parse_error", raw: l }; }
    });
  } catch { return []; }
}

function analyzeTrial(
  logPath: string,
  task: string,
  job: string,
  reward: number,
  exception: string | null,
  runtimeSec: number
): TrialSummary {
  const events = parseEvents(logPath);
  
  const toolCounts: Record<string, number> = {};
  const stopReasons: string[] = [];
  const errorMessages: string[] = [];
  let nTurns = 0;
  let nToolCalls = 0;
  let lastAssistantText = "";
  let totalOutputTokens = 0;
  let maxOutputTokensInOneTurn = 0;
  const interesting: string[] = [];

  for (const ev of events) {
    if (ev.type === "turn_start") nTurns++;
    if (ev.type === "tool_use") {
      nToolCalls++;
      const name = ev.tool_name as string || ev.tool as string || "unknown";
      toolCounts[name] = (toolCounts[name] || 0) + 1;
    }
    if (ev.type === "turn_end") {
      const reason = ev.stop_reason as string;
      if (reason) stopReasons.push(reason);
      const out = (ev.usage as any)?.output_tokens || 0;
      totalOutputTokens += out;
      if (out > maxOutputTokensInOneTurn) maxOutputTokensInOneTurn = out;
      if (reason === "max_tokens") interesting.push(`max_tokens stop on turn ${nTurns}`);
    }
    if (ev.type === "text" || ev.type === "assistant_text") {
      lastAssistantText = (ev.text as string) || (ev.content as string) || lastAssistantText;
    }
    if (ev.type === "error" || ev.type === "agent_error") {
      const msg = ev.error as string || ev.message as string || JSON.stringify(ev);
      errorMessages.push(msg);
    }
  }

  // Get final 5 event types
  const finalEvents = events.slice(-6).map(e => e.type);

  return {
    task, job, logPath, reward, exception, runtimeSec,
    nTurns, nToolCalls, toolCounts, stopReasons, finalEvents,
    lastAssistantText: lastAssistantText.slice(-800),
    errorMessages,
    totalOutputTokens, maxOutputTokensInOneTurn,
    interesting,
  };
}

// Build task->logPath map from all jobs
const JOBS_DIR = join(BENCH_DIR, "jobs");
const taskLogs: Map<string, { path: string; job: string }[]> = new Map();

for (const job of readdirSync(JOBS_DIR)) {
  const jobDir = join(JOBS_DIR, job);
  let entries: string[];
  try { entries = readdirSync(jobDir); } catch { continue; }
  for (const entry of entries) {
    const logPath = join(jobDir, entry, "agent", "events.jsonl");
    if (!existsSync(logPath)) continue;
    // Extract task name: entry is like "task-name__XXXXX"
    const taskShort = entry.replace(/__[^_]+$/, "");
    const taskName = `terminal-bench/${taskShort}`;
    if (!taskLogs.has(taskName)) taskLogs.set(taskName, []);
    taskLogs.get(taskName)!.push({ path: logPath, job });
  }
}

// Load results
const results: any[] = readFileSync(join(BENCH_DIR, "results", "results.jsonl"), "utf-8")
  .trim().split("\n").filter(Boolean).map(l => JSON.parse(l));

// Analyze each failing trial
const failingResults = results.filter(r => r.reward === 0);
const summaries: TrialSummary[] = [];

for (const r of failingResults) {
  const logs = taskLogs.get(r.task_name) || [];
  // Try to pick the right log (latest matching job if multiple)
  // Use the most recent entry
  if (logs.length === 0) {
    console.warn(`No log found for ${r.task_name}`);
    continue;
  }
  // Use the last entry (typically the most recent run)
  const last = logs[logs.length - 1]!;
  const { path: logPath, job } = last;
  const s = analyzeTrial(logPath, r.task_name, job, r.reward, r.exception, r.runtime_sec);
  summaries.push(s);
}

// Print summary table
console.log("=== FAILING TRIAL ANALYSIS ===\n");
console.log(`${summaries.length} failing trials analyzed\n`);

console.log("TASK".padEnd(45) + "TURNS" + " TOOLS" + " RT(s)" + " STOPREASONS");
console.log("-".repeat(100));

for (const s of summaries.sort((a,b) => a.task.localeCompare(b.task))) {
  const reasons = [...new Set(s.stopReasons)].join(",") || s.exception || "none";
  console.log(
    s.task.replace("terminal-bench/","").padEnd(45) +
    String(s.nTurns).padStart(5) +
    String(s.nToolCalls).padStart(6) +
    String(Math.round(s.runtimeSec)).padStart(6) +
    "  " + reasons
  );
}

console.log("\n=== TOOL USAGE IN FAILURES ===\n");
// Aggregate tool usage across all failures
const allToolCounts: Record<string, number> = {};
let totalTools = 0;
for (const s of summaries) {
  for (const [t, c] of Object.entries(s.toolCounts)) {
    allToolCounts[t] = (allToolCounts[t] || 0) + c;
    totalTools += c;
  }
}
const sorted = Object.entries(allToolCounts).sort((a,b) => b[1]-a[1]);
for (const [t, c] of sorted) {
  console.log(`  ${t.padEnd(30)} ${c} (${(100*c/totalTools).toFixed(1)}%)`);
}

console.log("\n=== INTERESTING FLAGS ===\n");
for (const s of summaries) {
  if (s.interesting.length > 0 || s.maxOutputTokensInOneTurn > 3000) {
    console.log(`  ${s.task.replace("terminal-bench/","")}: ${s.interesting.join("; ")} maxOut=${s.maxOutputTokensInOneTurn}`);
  }
}

console.log("\n=== FINAL EVENTS PATTERNS ===\n");
for (const s of summaries) {
  console.log(`  ${s.task.replace("terminal-bench/","").padEnd(45)} ${s.finalEvents.join(" -> ")}`);
}
