#!/usr/bin/env bun
/**
 * Tool usage statistics across all Omega sessions.
 *
 * Usage:  bun scripts/tool-stats.ts [sessions-dir]
 * Default sessions dir: ~/.omega/sessions  (relative to cwd's home)
 */

import { readdir, readFile } from "fs/promises";
import { join } from "path";
import { homedir } from "os";

const sessionsDir =
  process.argv[2] ??
  join(homedir(), "omega", "dev", ".omega", "sessions");

// ---------------------------------------------------------------------------
// Data collection
// ---------------------------------------------------------------------------

interface ToolStats {
  calls: number;
  errors: number;
  totalDurationMs: number;
  durations: number[]; // for median/p95
}

const byTool = new Map<string, ToolStats>();
const sessionCount = { total: 0, withTools: 0 };
let totalToolCalls = 0;

// Pairs: track tool_call → tool_result by id to get duration from tool_call side too
// (tool_result has its own durationMs for execution time, which is what we want)

// Per-session call counts (for "calls per active session" stat)
const callsPerSession: number[] = [];

// Error details: tool → list of output snippets
// We'll collect from context.jsonl where available, but events.jsonl is enough
// for error rates since it has isError.

// Parallel usage: count how many times >1 tool was called in same contextHash
const parallelBatches: number[] = []; // batch sizes
let lastContextHash = "";
let batchSize = 0;

function ensureTool(name: string): ToolStats {
  if (!byTool.has(name)) {
    byTool.set(name, { calls: 0, errors: 0, totalDurationMs: 0, durations: [] });
  }
  return byTool.get(name)!;
}

const dirs = (await readdir(sessionsDir, { withFileTypes: true }))
  .filter((d) => d.isDirectory())
  .map((d) => join(sessionsDir, d.name));

sessionCount.total = dirs.length;

for (const dir of dirs) {
  const eventsPath = join(dir, "events.jsonl");
  let raw: string;
  try {
    raw = await readFile(eventsPath, "utf-8");
  } catch {
    continue;
  }

  const lines = raw.split("\n").filter(Boolean);
  let sessionToolCalls = 0;

  // Collect tool_call events grouped by contextHash to detect parallel batches
  const callsByHash = new Map<string, string[]>(); // hash → tool names

  for (const line of lines) {
    let ev: any;
    try { ev = JSON.parse(line); } catch { continue; }

    if (ev.type === "tool_call") {
      sessionToolCalls++;
      totalToolCalls++;
      const hash = ev.contextHash ?? "__none__";
      if (!callsByHash.has(hash)) callsByHash.set(hash, []);
      callsByHash.get(hash)!.push(ev.name);
    }

    if (ev.type === "tool_result") {
      const stats = ensureTool(ev.name);
      stats.calls++;
      if (ev.isError) stats.errors++;
      if (typeof ev.durationMs === "number") {
        stats.totalDurationMs += ev.durationMs;
        stats.durations.push(ev.durationMs);
      }
    }
  }

  // Record parallel batch sizes
  for (const [, names] of callsByHash) {
    parallelBatches.push(names.length);
  }

  if (sessionToolCalls > 0) {
    sessionCount.withTools++;
    callsPerSession.push(sessionToolCalls);
  }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function percentile(sorted: number[], p: number): number {
  if (sorted.length === 0) return 0;
  const idx = Math.ceil((p / 100) * sorted.length) - 1;
  return sorted[Math.max(0, idx)]!;
}

function pad(s: string | number, n: number, right = false): string {
  const str = String(s);
  return right ? str.padEnd(n) : str.padStart(n);
}

// ---------------------------------------------------------------------------
// Report
// ---------------------------------------------------------------------------

console.log("\n═══════════════════════════════════════════════════════════");
console.log("  Omega Tool Usage Statistics");
console.log("═══════════════════════════════════════════════════════════\n");

console.log(`Sessions analysed : ${sessionCount.total}`);
console.log(`Sessions with tool use : ${sessionCount.withTools} (${pct(sessionCount.withTools, sessionCount.total)}%)`);
console.log(`Total tool calls  : ${totalToolCalls}`);
if (callsPerSession.length > 0) {
  const sorted = [...callsPerSession].sort((a, b) => a - b);
  const avg = callsPerSession.reduce((a, b) => a + b, 0) / callsPerSession.length;
  console.log(`Calls/active session: avg ${avg.toFixed(1)}, median ${percentile(sorted, 50)}, p95 ${percentile(sorted, 95)}, max ${sorted[sorted.length - 1]}`);
}

// Parallel usage
const parallelSorted = [...parallelBatches].sort((a, b) => a - b);
const parallelCount = parallelBatches.filter((n) => n > 1).length;
const totalBatches = parallelBatches.length;
console.log(`\nParallel batches  : ${parallelCount} of ${totalBatches} (${pct(parallelCount, totalBatches)}% use ≥2 tools)`);
if (parallelBatches.length > 0) {
  const maxBatch = Math.max(...parallelBatches);
  const avgBatch = parallelBatches.reduce((a, b) => a + b, 0) / parallelBatches.length;
  console.log(`Batch size        : avg ${avgBatch.toFixed(2)}, max ${maxBatch}`);
}

// Per-tool table
console.log("\n───────────────────────────────────────────────────────────");
console.log("  Per-tool breakdown");
console.log("───────────────────────────────────────────────────────────\n");

const tools = [...byTool.entries()].sort((a, b) => b[1].calls - a[1].calls);

// Header
const COL = [28, 7, 7, 8, 8, 8, 8];
const headers = ["Tool", "Calls", "Errors", "Err%", "AvgMs", "MedMs", "p95Ms"];
console.log(
  pad(headers[0]!, COL[0]!, true) +
  headers.slice(1).map((h, i) => pad(h, COL[i + 1]!)).join(""),
);
console.log("─".repeat(COL.reduce((a, b) => a + b, 0)));

for (const [name, s] of tools) {
  const sorted = [...s.durations].sort((a, b) => a - b);
  const avg = s.calls > 0 ? s.totalDurationMs / s.durations.length : 0;
  const med = percentile(sorted, 50);
  const p95 = percentile(sorted, 95);
  const errPct = pct(s.errors, s.calls);

  const errFlag = s.errors > 0 ? ` ← ${s.errors} errors` : "";
  console.log(
    pad(name, COL[0]!, true) +
    pad(s.calls, COL[1]!) +
    pad(s.errors, COL[2]!) +
    pad(errPct + "%", COL[3]!) +
    pad(avg.toFixed(1), COL[4]!) +
    pad(med.toFixed(1), COL[5]!) +
    pad(p95.toFixed(1), COL[6]!) +
    errFlag,
  );
}

// Share of total
console.log("\n───────────────────────────────────────────────────────────");
console.log("  Share of total calls\n");
for (const [name, s] of tools) {
  const share = ((s.calls / totalToolCalls) * 100).toFixed(1);
  const bar = "█".repeat(Math.round(Number(share) / 2));
  console.log(`  ${pad(name, 26, true)} ${pad(share + "%", 6)} ${bar}`);
}

// Error analysis
const errorTools = tools.filter(([, s]) => s.errors > 0);
if (errorTools.length > 0) {
  console.log("\n───────────────────────────────────────────────────────────");
  console.log("  Error rates (tools with any errors)\n");
  for (const [name, s] of errorTools.sort((a, b) => b[1].errors / b[1].calls - a[1].errors / a[1].calls)) {
    const rate = ((s.errors / s.calls) * 100).toFixed(1);
    console.log(`  ${pad(name, 26, true)} ${pad(s.errors, 5)} errors / ${s.calls} calls = ${rate}%`);
  }
}

// Never-used tools
const knownTools = [
  "read_file", "write_file", "edit_file", "run_command", "list_files",
  "web_search", "fetch_url", "grep_files", "find_files",
  "run_background", "wait_process", "kill_process",
];
const unusedTools = knownTools.filter((t) => !byTool.has(t));
if (unusedTools.length > 0) {
  console.log("\n───────────────────────────────────────────────────────────");
  console.log("  Never-used tools: " + unusedTools.join(", "));
}

console.log("\n═══════════════════════════════════════════════════════════\n");

function pct(n: number, total: number): string {
  if (total === 0) return "0";
  return ((n / total) * 100).toFixed(1);
}
