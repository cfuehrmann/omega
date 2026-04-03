#!/usr/bin/env bun
/**
 * Tool performance analysis across all Omega sessions.
 *
 * Focuses on:
 *   1. Per-tool duration distribution (min / percentiles / max)
 *   2. Duration histograms (log-scale buckets)
 *   3. Wall-clock contribution — correctly handles parallel batches
 *      (batch wall-clock = max of concurrent tool durations, not sum)
 *   4. Single-tool-batch vs multi-tool-batch comparison
 *
 * Usage:  bun scripts/tool-perf.ts [sessions-dir]
 */

import { readdir, readFile } from "fs/promises";
import { join } from "path";
import { homedir } from "os";

const sessionsDir =
  process.argv[2] ??
  join(homedir(), "omega", "dev", ".omega", "sessions");

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

interface ToolResult {
  name: string;
  durationMs: number;
  isError: boolean;
  contextHash: string;
  ts: string;
}

// ---------------------------------------------------------------------------
// Load data
// ---------------------------------------------------------------------------

const allResults: ToolResult[] = [];

const dirs = (await readdir(sessionsDir, { withFileTypes: true }))
  .filter((d) => d.isDirectory())
  .map((d) => join(sessionsDir, d.name));

for (const dir of dirs) {
  let raw: string;
  try { raw = await readFile(join(dir, "events.jsonl"), "utf-8"); }
  catch { continue; }

  for (const line of raw.split("\n").filter(Boolean)) {
    let ev: any;
    try { ev = JSON.parse(line); } catch { continue; }
    if (ev.type === "tool_result" && typeof ev.durationMs === "number" && ev.name) {
      allResults.push({
        name: ev.name,
        durationMs: ev.durationMs,
        isError: ev.isError ?? false,
        contextHash: ev.contextHash ?? "",
        ts: ev.ts ?? "",
      });
    }
  }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function pct(arr: number[], p: number): number {
  if (arr.length === 0) return 0;
  const sorted = [...arr].sort((a, b) => a - b);
  const idx = Math.ceil((p / 100) * sorted.length) - 1;
  return sorted[Math.max(0, idx)]!;
}

function mean(arr: number[]): number {
  return arr.length === 0 ? 0 : arr.reduce((a, b) => a + b, 0) / arr.length;
}

function fmt(ms: number): string {
  if (ms < 1) return ms.toFixed(2) + "ms";
  if (ms < 1000) return ms.toFixed(1) + "ms";
  return (ms / 1000).toFixed(2) + "s";
}

function pad(s: string | number, n: number, right = false): string {
  const str = String(s);
  return right ? str.padEnd(n) : str.padStart(n);
}

// Log-scale buckets: <1ms, 1-10ms, 10-100ms, 100ms-1s, 1-10s, >10s
const BUCKETS = [1, 10, 100, 1_000, 10_000, Infinity];
const BUCKET_LABELS = ["<1ms", "1-10ms", "10-100ms", "100ms-1s", "1-10s", ">10s"];

function histogram(durations: number[]): number[] {
  const counts = new Array(BUCKET_LABELS.length).fill(0);
  for (const d of durations) {
    for (let i = 0; i < BUCKETS.length; i++) {
      if (d < BUCKETS[i]!) { counts[i]++; break; }
    }
  }
  return counts;
}

function histBar(count: number, total: number, width = 20): string {
  const pctVal = total === 0 ? 0 : count / total;
  const filled = Math.round(pctVal * width);
  return "▓".repeat(filled) + "░".repeat(width - filled) +
    " " + (pctVal * 100).toFixed(0).padStart(3) + "%";
}

// ---------------------------------------------------------------------------
// Group by tool
// ---------------------------------------------------------------------------

const byTool = new Map<string, number[]>();
for (const r of allResults) {
  if (!r.isError) { // exclude error results from timing (they may be fast for wrong reasons)
    if (!byTool.has(r.name)) byTool.set(r.name, []);
    byTool.get(r.name)!.push(r.durationMs);
  }
}

// Sort tools by total time descending
const tools = [...byTool.entries()]
  .map(([name, durs]) => ({ name, durs, total: durs.reduce((a, b) => a + b, 0) }))
  .sort((a, b) => b.total - a.total);

// ---------------------------------------------------------------------------
// Parallel batch analysis
// ---------------------------------------------------------------------------

// Group results by contextHash to find parallel batches
const byHash = new Map<string, ToolResult[]>();
for (const r of allResults) {
  if (!r.contextHash) continue;
  if (!byHash.has(r.contextHash)) byHash.set(r.contextHash, []);
  byHash.get(r.contextHash)!.push(r);
}

const singleBatches = [...byHash.values()].filter((g) => g.length === 1);
const multiBatches  = [...byHash.values()].filter((g) => g.length > 1);

// For multi-batches: wall-clock = max(durationMs), sum = sum(durationMs)
let parallelSumMs = 0;
let parallelWallMs = 0;
for (const batch of multiBatches) {
  const durs = batch.map((r) => r.durationMs);
  parallelSumMs  += durs.reduce((a, b) => a + b, 0);
  parallelWallMs += Math.max(...durs);
}
const parallelSavedMs = parallelSumMs - parallelWallMs;

// Single-batch totals
const singleWallMs = singleBatches
  .flatMap((g) => g)
  .reduce((a, r) => a + r.durationMs, 0);

const totalWallMs = singleWallMs + parallelWallMs;
const totalSumMs  = allResults.reduce((a, r) => a + r.durationMs, 0);

// Per-tool wall-clock contribution using single-call batches only (clean)
const bySingleTool = new Map<string, number[]>();
for (const batch of singleBatches) {
  const r = batch[0]!;
  if (!r.isError) {
    if (!bySingleTool.has(r.name)) bySingleTool.set(r.name, []);
    bySingleTool.get(r.name)!.push(r.durationMs);
  }
}

// ---------------------------------------------------------------------------
// Report
// ---------------------------------------------------------------------------

const W = 80;
console.log("\n" + "═".repeat(W));
console.log("  Omega Tool Performance Analysis");
console.log("═".repeat(W) + "\n");

console.log(`Total tool results analysed : ${allResults.length}`);
console.log(`  — successful              : ${allResults.filter((r) => !r.isError).length}`);
console.log(`  — errors (excluded)       : ${allResults.filter((r) => r.isError).length}`);
console.log(`\nBatches (by contextHash)    : ${byHash.size}`);
console.log(`  — single-tool batches     : ${singleBatches.length} (${pct100(singleBatches.length, byHash.size)}%)`);
console.log(`  — multi-tool batches      : ${multiBatches.length} (${pct100(multiBatches.length, byHash.size)}%)`);

console.log(`\nTotal raw execution time    : ${fmt(totalSumMs)}`);
console.log(`Estimated wall-clock time   : ${fmt(totalWallMs)}`);
console.log(`Time saved by parallelism   : ${fmt(parallelSavedMs)} (${pct100(parallelSavedMs, totalSumMs)}% of raw)`);

// ── Per-tool summary ──────────────────────────────────────────────────────
console.log("\n" + "─".repeat(W));
console.log("  Per-tool duration statistics  (successful calls only)\n");

const HDR = ["Tool", "N", "Mean", "p50", "p90", "p95", "p99", "Max", "Total"];
const COL  = [22, 5, 8, 8, 8, 8, 8, 8, 10];
console.log(
  pad(HDR[0]!, COL[0]!, true) +
  HDR.slice(1).map((h, i) => pad(h, COL[i + 1]!)).join(""),
);
console.log("─".repeat(COL.reduce((a, b) => a + b, 0)));

for (const { name, durs, total } of tools) {
  const sorted = [...durs].sort((a, b) => a - b);
  console.log(
    pad(name, COL[0]!, true) +
    pad(durs.length, COL[1]!) +
    pad(fmt(mean(durs)), COL[2]!) +
    pad(fmt(pct(sorted, 50)), COL[3]!) +
    pad(fmt(pct(sorted, 90)), COL[4]!) +
    pad(fmt(pct(sorted, 95)), COL[5]!) +
    pad(fmt(pct(sorted, 99)), COL[6]!) +
    pad(fmt(Math.max(...durs)), COL[7]!) +
    pad(fmt(total), COL[8]!),
  );
}

// ── Wall-clock contribution by tool ───────────────────────────────────────
console.log("\n" + "─".repeat(W));
console.log("  Wall-clock contribution  (single-call batches only — unambiguous)\n");

const singleTotalWall = [...bySingleTool.values()]
  .flatMap((d) => d)
  .reduce((a, b) => a + b, 0);

const singleToolsSorted = [...bySingleTool.entries()]
  .map(([name, durs]) => ({ name, total: durs.reduce((a, b) => a + b, 0), n: durs.length }))
  .sort((a, b) => b.total - a.total);

for (const { name, total, n } of singleToolsSorted) {
  const sharePct = singleTotalWall === 0 ? 0 : (total / singleTotalWall) * 100;
  const bar = "█".repeat(Math.round(sharePct / 3));
  console.log(
    `  ${pad(name, 20, true)} ${pad(fmt(total), 10)} / ${n} calls  ${pad(sharePct.toFixed(1) + "%", 6)}  ${bar}`,
  );
}

// ── Histograms ─────────────────────────────────────────────────────────────
console.log("\n" + "─".repeat(W));
console.log("  Duration distribution histograms  (log-scale buckets)\n");

for (const { name, durs } of tools) {
  const counts = histogram(durs);
  const n = durs.length;
  console.log(`  ${name}  (n=${n}, median=${fmt(pct(durs, 50))})`);
  for (let i = 0; i < BUCKET_LABELS.length; i++) {
    if (counts[i]! > 0 || i < 3) {
      console.log(`    ${pad(BUCKET_LABELS[i]!, 10, true)} ${histBar(counts[i]!, n)}`);
    }
  }
  console.log();
}

// ── run_command deep-dive ──────────────────────────────────────────────────
const rcDurs = byTool.get("run_command") ?? [];
if (rcDurs.length > 0) {
  console.log("─".repeat(W));
  console.log("  run_command deep-dive  (highest variance tool)\n");

  const fast   = rcDurs.filter((d) => d < 1_000);
  const medium = rcDurs.filter((d) => d >= 1_000 && d < 10_000);
  const slow   = rcDurs.filter((d) => d >= 10_000 && d < 60_000);
  const verySlow = rcDurs.filter((d) => d >= 60_000);

  console.log(`  Fast    (<1s)   : ${fast.length.toString().padStart(4)} calls  avg ${fmt(mean(fast))}`);
  console.log(`  Medium  (1-10s) : ${medium.length.toString().padStart(4)} calls  avg ${fmt(mean(medium))}`);
  console.log(`  Slow    (10-60s): ${slow.length.toString().padStart(4)} calls  avg ${fmt(mean(slow))}`);
  console.log(`  VerySlow(>60s)  : ${verySlow.length.toString().padStart(4)} calls  avg ${fmt(mean(verySlow))}`);
  console.log();

  // Top 10 slowest commands — need to correlate with tool_call events
  // We only have durationMs from tool_result; show the distribution of slow calls
  const sorted = [...rcDurs].sort((a, b) => b - a);
  console.log("  Slowest 10 durations:");
  for (const d of sorted.slice(0, 10)) {
    const bar = "█".repeat(Math.min(30, Math.round(d / 3000)));
    console.log(`    ${fmt(d).padStart(8)}  ${bar}`);
  }
  console.log();
}

// ── Parallel batch timing ──────────────────────────────────────────────────
if (multiBatches.length > 0) {
  console.log("─".repeat(W));
  console.log("  Parallel batch analysis\n");

  // Distribution of batch sizes
  const sizeCounts = new Map<number, number>();
  for (const batch of multiBatches) {
    const n = batch.length;
    sizeCounts.set(n, (sizeCounts.get(n) ?? 0) + 1);
  }
  console.log("  Batch size distribution:");
  for (const [size, count] of [...sizeCounts.entries()].sort((a, b) => a[0] - b[0])) {
    console.log(`    ${size} tools: ${count} batches`);
  }

  // Parallelism efficiency: wall-clock / sum for each batch
  const efficiencies: number[] = [];
  for (const batch of multiBatches) {
    const durs = batch.map((r) => r.durationMs);
    const sum = durs.reduce((a, b) => a + b, 0);
    const wall = Math.max(...durs);
    if (sum > 0) efficiencies.push(wall / sum); // 0=perfect, 1=sequential
  }
  const avgEff = mean(efficiencies);
  console.log(`\n  Avg parallelism efficiency: ${(avgEff * 100).toFixed(1)}%`);
  console.log(`  (wall-clock / sum-of-parts; 100% = fully sequential, ~50% = two equal-length tools)`);
  console.log(`  Time saved by running tools in parallel: ${fmt(parallelSavedMs)}`);
}

console.log("\n" + "═".repeat(W) + "\n");

function pct100(n: number, total: number): string {
  return total === 0 ? "0" : ((n / total) * 100).toFixed(1);
}
