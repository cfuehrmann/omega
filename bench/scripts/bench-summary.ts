#!/usr/bin/env bun
/**
 * bench-summary.ts
 *
 * Reads benchmark-results/results.jsonl and benchmark-results/oracle-tasks.json
 * and prints a human-readable summary of Omega's progress on Terminal-Bench 2.0.
 *
 * Usage:
 *   bun scripts/bench-summary.ts              # summary for all models
 *   bun scripts/bench-summary.ts sonnet       # filter by model name substring
 */

import { existsSync, readFileSync } from "fs";
import { join, resolve } from "path";

// ── types ────────────────────────────────────────────────────────────────────

interface TaskMeta {
  short_name: string;
  category: string;
  difficulty: "easy" | "medium" | "hard";
  agent_timeout_sec: number;
  oracle_passes: boolean;
  oracle_fail_reason?: string;
}

interface TrialRecord {
  trial_id: string;
  job_id: string | null;
  task_name: string;
  ingested_at: string;
  started_at: string | null;
  finished_at: string | null;
  runtime_sec: number | null;
  agent: string;
  model: string;
  reward: number | null;
  n_input_tokens: number | null;
  n_output_tokens: number | null;
  n_cache_tokens: number | null;
  exception: string | null;
}

// ── load data ────────────────────────────────────────────────────────────────

const ROOT = resolve(import.meta.dir, "..");
const ORACLE_FILE = join(ROOT, "results", "oracle-tasks.json");
const RESULTS_FILE = join(ROOT, "results", "results.jsonl");

if (!existsSync(ORACLE_FILE)) {
  console.error("Error: benchmark-results/oracle-tasks.json not found");
  process.exit(1);
}

const { tasks: allTasks } = JSON.parse(readFileSync(ORACLE_FILE, "utf-8")) as {
  tasks: TaskMeta[];
};
const oraclePassing = allTasks.filter((t) => t.oracle_passes);
const oraclePassingNames = new Set(oraclePassing.map((t) => `terminal-bench/${t.short_name}`));

const records: TrialRecord[] = [];
if (existsSync(RESULTS_FILE)) {
  for (const line of readFileSync(RESULTS_FILE, "utf-8").split("\n")) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    try {
      records.push(JSON.parse(trimmed) as TrialRecord);
    } catch {
      // skip malformed lines
    }
  }
}

// ── apply model filter ────────────────────────────────────────────────────────

const modelFilter = process.argv[2];
const filtered = modelFilter
  ? records.filter((r) => r.model.includes(modelFilter))
  : records;

// ── aggregate per task ────────────────────────────────────────────────────────

interface TaskStats {
  task_name: string;
  short_name: string;
  meta: TaskMeta | undefined;
  attempts: number;
  passes: number;
  total_runtime_sec: number;
  total_input_tokens: number;
  total_output_tokens: number;
  total_cache_tokens: number;
  exceptions: string[];
}

const byTask = new Map<string, TaskStats>();

for (const r of filtered) {
  if (!byTask.has(r.task_name)) {
    const short = r.task_name.replace("terminal-bench/", "");
    byTask.set(r.task_name, {
      task_name: r.task_name,
      short_name: short,
      meta: allTasks.find((t) => t.short_name === short),
      attempts: 0,
      passes: 0,
      total_runtime_sec: 0,
      total_input_tokens: 0,
      total_output_tokens: 0,
      total_cache_tokens: 0,
      exceptions: [],
    });
  }
  const s = byTask.get(r.task_name)!;
  s.attempts++;
  if (r.reward === 1.0) s.passes++;
  if (r.runtime_sec) s.total_runtime_sec += r.runtime_sec;
  if (r.n_input_tokens) s.total_input_tokens += r.n_input_tokens;
  if (r.n_output_tokens) s.total_output_tokens += r.n_output_tokens;
  if (r.n_cache_tokens) s.total_cache_tokens += r.n_cache_tokens;
  if (r.exception) s.exceptions.push(r.exception);
}

// ── helpers ───────────────────────────────────────────────────────────────────

function fmtTime(sec: number | null): string {
  if (!sec) return "—";
  const m = Math.floor(sec / 60);
  const s = sec % 60;
  return m > 0 ? `${m}m ${s}s` : `${s}s`;
}

function fmtTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${Math.round(n / 1_000)}k`;
  return String(n);
}

function pct(a: number, b: number): string {
  if (b === 0) return "—";
  return `${Math.round((a / b) * 100)}%`;
}

function bar(passes: number, attempts: number, width = 10): string {
  if (attempts === 0) return " ".repeat(width);
  const filled = Math.round((passes / attempts) * width);
  return "█".repeat(filled) + "░".repeat(width - filled);
}

// ── estimate cost ─────────────────────────────────────────────────────────────
// Sonnet 4.6 pricing: $3/$15 per MTok input/output, $0.30 cache read

function estimateCost(input: number, output: number, cache: number): number {
  return (input * 3 + output * 15 + cache * 0.30) / 1_000_000;
}

// ── print ─────────────────────────────────────────────────────────────────────

const W = 80;
const line = "─".repeat(W);
const dline = "═".repeat(W);

const attempted = [...byTask.values()];
const oracleAttempted = attempted.filter((s) => oraclePassingNames.has(s.task_name));
// totalTrialPasses: counts every reward=1 trial (can exceed #tasks for multi-trial tasks)
const totalTrialPasses = oracleAttempted.reduce((a, s) => a + s.passes, 0);
// totalPassingTasks: number of distinct tasks with ≥1 pass — the correct leaderboard metric
const totalPassingTasks = oracleAttempted.reduce((a, s) => a + (s.passes > 0 ? 1 : 0), 0);
const totalAttempts = oracleAttempted.reduce((a, s) => a + s.attempts, 0);
const totalTasks = oraclePassing.length;

// Total tokens across all records (oracle-scope only)
const allOracleRecords = filtered.filter((r) => oraclePassingNames.has(r.task_name));
const totalInputTok  = allOracleRecords.reduce((a, r) => a + (r.n_input_tokens ?? 0), 0);
const totalOutputTok = allOracleRecords.reduce((a, r) => a + (r.n_output_tokens ?? 0), 0);
const totalCacheTok  = allOracleRecords.reduce((a, r) => a + (r.n_cache_tokens ?? 0), 0);
const totalCost      = estimateCost(totalInputTok, totalOutputTok, totalCacheTok);

const modelLabel = modelFilter ? `  model filter: ${modelFilter}` : "  (all models)";

console.log(dline);
console.log("  Omega — Terminal-Bench 2.0 Results");
console.log(modelLabel);
console.log(`  Generated: ${new Date().toISOString()}`);
console.log(dline);

// ── overall ───────────────────────────────────────────────────────────────────
console.log("\n  OVERALL (oracle-passing tasks only)\n");
console.log(`  Tasks in scope:      ${totalTasks}  (oracle-passing)`);
console.log(`  Tasks attempted:     ${oracleAttempted.length} / ${totalTasks}  (${pct(oracleAttempted.length, totalTasks)})`);
console.log(`  Total trials run:    ${totalAttempts}`);
if (totalAttempts > 0) {
  console.log(`  Pass rate (tried):   ${totalTrialPasses} / ${totalAttempts}  (${pct(totalTrialPasses, totalAttempts)})`);
  console.log(`  Tasks passed (≥1):   ${totalPassingTasks} / ${totalTasks}  (${pct(totalPassingTasks, totalTasks)})  ← leaderboard metric`);
  if (totalTrialPasses !== totalPassingTasks) {
    console.log(`  (trial passes=${totalTrialPasses} vs unique passing tasks=${totalPassingTasks}; ${totalTrialPasses - totalPassingTasks} extra trial passes from multi-trial tasks)`);
  }
  console.log(`  Estimated API cost:  $${totalCost.toFixed(3)}`);
}

// ── by category ───────────────────────────────────────────────────────────────
if (oracleAttempted.length > 0) {
  console.log(`\n  BY CATEGORY\n`);
  const catMap = new Map<string, { attempts: number; passes: number; tasks: number }>();
  for (const t of oraclePassing) {
    const c = catMap.get(t.category) ?? { attempts: 0, passes: 0, tasks: 0 };
    c.tasks++;
    catMap.set(t.category, c);
  }
  for (const s of oracleAttempted) {
    const cat = s.meta?.category ?? "unknown";
    const c = catMap.get(cat) ?? { attempts: 0, passes: 0, tasks: 0 };
    c.attempts += s.attempts;
    c.passes += s.passes;
    catMap.set(cat, c);
  }
  const cats = [...catMap.entries()]
    .filter(([, v]) => v.attempts > 0)
    .sort(([a], [b]) => a.localeCompare(b));

  const cw = Math.max(...cats.map(([c]) => c.length), 22);
  console.log(`  ${"Category".padEnd(cw)}  Scope  Tried  Pass  Rate   ${" ".repeat(10)}`);
  console.log(`  ${line.slice(0, cw + 40)}`);
  for (const [cat, v] of cats) {
    const b = bar(v.passes, v.attempts);
    console.log(
      `  ${cat.padEnd(cw)}  ${String(v.tasks).padStart(5)}  ${String(v.attempts).padStart(5)}  ${String(v.passes).padStart(4)}  ${pct(v.passes, v.attempts).padStart(4)}   ${b}`
    );
  }
}

// ── per-task table ────────────────────────────────────────────────────────────
if (oracleAttempted.length > 0) {
  console.log(`\n  PER-TASK RESULTS\n`);
  const tw = 34;
  console.log(
    `  ${"Task".padEnd(tw)} ${"Cat".padEnd(22)} ${"Diff".padEnd(6)} ${"Att".padStart(3)} ${"Pass".padStart(4)} ${"Rate".padStart(5)}  ${"AvgTime".padStart(8)}  ${"AvgTok".padStart(7)}`
  );
  console.log(`  ${line}`);

  const sorted = [...oracleAttempted].sort((a, b) => {
    const ca = a.meta?.category ?? "z";
    const cb = b.meta?.category ?? "z";
    if (ca !== cb) return ca.localeCompare(cb);
    const da = a.meta?.difficulty ?? "";
    const db = b.meta?.difficulty ?? "";
    const order = { easy: 0, medium: 1, hard: 2 };
    return (order[da as keyof typeof order] ?? 9) - (order[db as keyof typeof order] ?? 9);
  });

  for (const s of sorted) {
    const avgTime = s.attempts > 0 ? fmtTime(Math.round(s.total_runtime_sec / s.attempts)) : "—";
    const avgTok = s.attempts > 0
      ? fmtTokens(Math.round((s.total_input_tokens + s.total_output_tokens + s.total_cache_tokens) / s.attempts))
      : "—";
    const excNote = s.exceptions.length > 0 ? `  [${[...new Set(s.exceptions)].join(",")}]` : "";
    const passEmoji = s.attempts > 0 ? (s.passes === s.attempts ? "✓" : s.passes === 0 ? "✗" : "~") : " ";
    console.log(
      `  ${passEmoji} ${s.short_name.padEnd(tw - 2)} ${(s.meta?.category ?? "?").padEnd(22)} ${(s.meta?.difficulty ?? "?").padEnd(6)} ${String(s.attempts).padStart(3)} ${String(s.passes).padStart(4)} ${pct(s.passes, s.attempts).padStart(5)}  ${avgTime.padStart(8)}  ${avgTok.padStart(7)}${excNote}`
    );
  }
}

// ── not yet attempted ─────────────────────────────────────────────────────────
const notAttempted = oraclePassing.filter((t) => !byTask.has(`terminal-bench/${t.short_name}`));
if (notAttempted.length > 0) {
  console.log(`\n  NOT YET ATTEMPTED  (${notAttempted.length} tasks)\n`);
  for (const diff of ["easy", "medium", "hard"] as const) {
    const group = notAttempted.filter((t) => t.difficulty === diff);
    if (group.length === 0) continue;
    const names = group.map((t) => t.short_name).join(", ");
    const wrapped = wrapWords(`  ${diff.toUpperCase()}: ${names}`, W - 2, `           `);
    console.log(wrapped);
  }
}

console.log("\n" + dline + "\n");

// ── word-wrap helper ──────────────────────────────────────────────────────────
function wrapWords(text: string, maxWidth: number, continuation: string): string {
  const words = text.split(" ");
  const lines: string[] = [];
  let current = "";
  for (const word of words) {
    if (current.length + word.length + 1 > maxWidth && current.length > 0) {
      lines.push(current);
      current = continuation + word;
    } else {
      current = current.length === 0 ? word : current + " " + word;
    }
  }
  if (current) lines.push(current);
  return lines.join("\n");
}
