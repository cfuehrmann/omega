#!/usr/bin/env bun
/**
 * run_command analysis — what commands are actually run, how often, how long.
 *
 * Strategy:
 *   1. Join tool_call (has command text) + tool_result (has durationMs) by id
 *   2. Normalize each command to a "pattern" for grouping
 *   3. Report frequency + timing per pattern, sorted by total time
 *
 * Usage:  bun scripts/run-command-stats.ts [sessions-dir]
 */

import { readdir, readFile } from "fs/promises";
import { join } from "path";
import { homedir } from "os";

const sessionsDir =
  process.argv[2] ??
  join(homedir(), "omega", "dev", ".omega", "sessions");

// ---------------------------------------------------------------------------
// Load and join
// ---------------------------------------------------------------------------

interface RunEntry {
  command: string;
  timeout?: number;
  durationMs: number;
  isError: boolean;
}

const dirs = (await readdir(sessionsDir, { withFileTypes: true }))
  .filter((d) => d.isDirectory())
  .map((d) => join(sessionsDir, d.name));

const entries: RunEntry[] = [];

for (const dir of dirs) {
  let raw: string;
  try { raw = await readFile(join(dir, "events.jsonl"), "utf-8"); }
  catch { continue; }

  // Two-pass: collect calls (id → command), then match with results
  const calls = new Map<string, { command: string; timeout?: number }>();

  for (const line of raw.split("\n").filter(Boolean)) {
    let ev: any;
    try { ev = JSON.parse(line); } catch { continue; }

    if (ev.type === "tool_call" && ev.name === "run_command" && ev.input?.command) {
      calls.set(ev.id, { command: ev.input.command, timeout: ev.input.timeout });
    }
  }

  for (const line of raw.split("\n").filter(Boolean)) {
    let ev: any;
    try { ev = JSON.parse(line); } catch { continue; }

    if (ev.type === "tool_result" && ev.name === "run_command" &&
        typeof ev.durationMs === "number") {
      const call = calls.get(ev.id);
      if (call) {
        entries.push({
          command: call.command,
          timeout: call.timeout,
          durationMs: ev.durationMs,
          isError: ev.isError ?? false,
        });
      }
    }
  }
}

// ---------------------------------------------------------------------------
// Normalisation
// ---------------------------------------------------------------------------

function normalise(raw: string): string {
  // Strip leading "cd <path> && " prefix (common wrapper)
  let cmd = raw.replace(/^cd\s+\S+\s*&&\s*/, "").trim();
  // Collapse multiple spaces
  cmd = cmd.replace(/\s+/g, " ");
  // Strip trailing redirects and 2>&1
  cmd = cmd.replace(/\s*2>&1\s*$/, "").replace(/\s*2>\/dev\/null\s*$/, "").trim();

  // ── Special multi-word commands first ──────────────────────────────────

  // just <subcommand>  (keep full "just X" or "just X Y" for e.g. "just e2e ...")
  const justM = cmd.match(/^just\s+(\S+)/);
  if (justM) {
    const sub = justM[1]!;
    // For "just e2e" keep it generic (args vary); others keep as-is
    if (sub === "e2e") return "just e2e [args]";
    return `just ${sub}`;
  }

  // git <subcommand>
  const gitM = cmd.match(/^git\s+(\S+)/);
  if (gitM) return `git ${gitM[1]}`;

  // bun test [file/args]
  if (/^bun\s+test/.test(cmd)) {
    if (cmd === "bun test") return "bun test";
    return "bun test [args]";
  }

  // bun scripts/<name>
  const bunScriptM = cmd.match(/^bun\s+(scripts\/\S+)/);
  if (bunScriptM) return `bun ${bunScriptM[1]}`;

  // bun run / bun <other>
  const bunM = cmd.match(/^bun\s+(\S+)/);
  if (bunM) return `bun ${bunM[1]}`;

  // npm / npx / pnpm / yarn
  const npmM = cmd.match(/^(npm|npx|pnpm|yarn)\s+(\S+)/);
  if (npmM) return `${npmM[1]} ${npmM[2]}`;

  // cat / head / tail / wc  (file inspection)
  const catM = cmd.match(/^(cat|head|tail|wc|file|stat)\s/);
  if (catM) return catM[1]!;

  // ls / echo / pwd / which / type
  const simpleM = cmd.match(/^(ls|echo|pwd|which|type|mkdir|rm|mv|cp|touch|diff)\b/);
  if (simpleM) return simpleM[1]!;

  // python / python3
  if (/^python3?\s/.test(cmd)) return "python3";

  // node
  if (/^node\s/.test(cmd)) return "node [script]";

  // cargo
  const cargoM = cmd.match(/^cargo\s+(\S+)/);
  if (cargoM) return `cargo ${cargoM[1]}`;

  // make
  const makeM = cmd.match(/^make\s+(\S+)/);
  if (makeM) return `make ${makeM[1]}`;
  if (/^make\b/.test(cmd)) return "make";

  // Commands with pipes — use first command only
  if (cmd.includes("|")) {
    const first = cmd.split("|")[0]!.trim();
    return normalise(first) + " | …";
  }

  // Long unrecognised commands — truncate to 50 chars
  if (cmd.length > 50) return cmd.slice(0, 47) + "…";
  return cmd;
}

// ---------------------------------------------------------------------------
// Group by pattern
// ---------------------------------------------------------------------------

interface PatternStats {
  pattern: string;
  count: number;
  errors: number;
  totalMs: number;
  durations: number[];
  examples: string[]; // up to 3 distinct raw commands
}

const byPattern = new Map<string, PatternStats>();

for (const e of entries) {
  const pat = normalise(e.command);
  if (!byPattern.has(pat)) {
    byPattern.set(pat, { pattern: pat, count: 0, errors: 0,
                          totalMs: 0, durations: [], examples: [] });
  }
  const s = byPattern.get(pat)!;
  s.count++;
  if (e.isError) s.errors++;
  s.totalMs += e.durationMs;
  s.durations.push(e.durationMs);
  if (s.examples.length < 3 && !s.examples.includes(e.command)) {
    s.examples.push(e.command);
  }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function pct(arr: number[], p: number): number {
  if (arr.length === 0) return 0;
  const sorted = [...arr].sort((a, b) => a - b);
  return sorted[Math.max(0, Math.ceil((p / 100) * sorted.length) - 1)]!;
}

function fmt(ms: number): string {
  if (ms < 1000) return ms.toFixed(0) + "ms";
  if (ms < 60_000) return (ms / 1000).toFixed(1) + "s";
  return (ms / 60_000).toFixed(1) + "min";
}

function pad(s: string | number, n: number, right = false): string {
  const str = String(s);
  return right ? str.padEnd(n) : str.padStart(n);
}

// ---------------------------------------------------------------------------
// Report
// ---------------------------------------------------------------------------

const totalCmds = entries.length;
const totalMs = entries.reduce((a, e) => a + e.durationMs, 0);

const W = 100;
console.log("\n" + "═".repeat(W));
console.log("  run_command analysis");
console.log("═".repeat(W) + "\n");
console.log(`Commands with known text : ${entries.length}  (of ~${entries.length + (1546 - 1387)} total run_command calls)`);
console.log(`Total execution time     : ${fmt(totalMs)}`);
console.log(`Unique patterns          : ${byPattern.size}`);

// ── By total time ──────────────────────────────────────────────────────────
console.log("\n" + "─".repeat(W));
console.log("  Sorted by total time\n");

const byTime = [...byPattern.values()].sort((a, b) => b.totalMs - a.totalMs);

const HDR  = ["Pattern", "N", "Err", "Total", "Avg", "p50", "p90", "p99", "Max"];
const COL  = [32, 5, 5, 9, 8, 8, 8, 8, 8];
console.log(HDR.map((h, i) => pad(h, COL[i]!, i === 0)).join(""));
console.log("─".repeat(COL.reduce((a, b) => a + b, 0)));

for (const s of byTime) {
  const share = ((s.totalMs / totalMs) * 100).toFixed(1);
  const bar = "█".repeat(Math.round(Number(share) / 3));
  console.log(
    pad(s.pattern, COL[0]!, true) +
    pad(s.count, COL[1]!) +
    pad(s.errors || "-", COL[2]!) +
    pad(fmt(s.totalMs), COL[3]!) +
    pad(fmt(s.totalMs / s.count), COL[4]!) +
    pad(fmt(pct(s.durations, 50)), COL[5]!) +
    pad(fmt(pct(s.durations, 90)), COL[6]!) +
    pad(fmt(pct(s.durations, 99)), COL[7]!) +
    pad(fmt(Math.max(...s.durations)), COL[8]!) +
    `  ${share}% ${bar}`
  );
}

// ── By frequency ─────────────────────────────────────────────────────────
console.log("\n" + "─".repeat(W));
console.log("  Sorted by frequency\n");

const byFreq = [...byPattern.values()].sort((a, b) => b.count - a.count);

console.log(HDR.map((h, i) => pad(h, COL[i]!, i === 0)).join(""));
console.log("─".repeat(COL.reduce((a, b) => a + b, 0)));

for (const s of byFreq.slice(0, 30)) {
  const pctFreq = ((s.count / totalCmds) * 100).toFixed(1);
  console.log(
    pad(s.pattern, COL[0]!, true) +
    pad(s.count, COL[1]!) +
    pad(s.errors || "-", COL[2]!) +
    pad(fmt(s.totalMs), COL[3]!) +
    pad(fmt(s.totalMs / s.count), COL[4]!) +
    pad(fmt(pct(s.durations, 50)), COL[5]!) +
    pad(fmt(pct(s.durations, 90)), COL[6]!) +
    pad(fmt(pct(s.durations, 99)), COL[7]!) +
    pad(fmt(Math.max(...s.durations)), COL[8]!) +
    `  ${pctFreq}%`
  );
}

// ── Long-running patterns ─────────────────────────────────────────────────
console.log("\n" + "─".repeat(W));
console.log("  Patterns with median > 1s\n");

const slowPatterns = [...byPattern.values()]
  .filter((s) => pct(s.durations, 50) >= 1000)
  .sort((a, b) => pct(b.durations, 50) - pct(a.durations, 50));

for (const s of slowPatterns) {
  console.log(`  ${pad(s.pattern, 34, true)} n=${pad(s.count, 4)}  ` +
    `p50=${fmt(pct(s.durations, 50))}  p90=${fmt(pct(s.durations, 90))}  max=${fmt(Math.max(...s.durations))}`);
}

// ── Quick patterns (p50 < 100ms, called ≥5 times) ─────────────────────────
console.log("\n" + "─".repeat(W));
console.log("  Fast patterns (median < 100ms, ≥5 calls)\n");

const fastPatterns = [...byPattern.values()]
  .filter((s) => pct(s.durations, 50) < 100 && s.count >= 5)
  .sort((a, b) => b.count - a.count);

for (const s of fastPatterns) {
  console.log(`  ${pad(s.pattern, 34, true)} n=${pad(s.count, 4)}  ` +
    `p50=${fmt(pct(s.durations, 50))}  p90=${fmt(pct(s.durations, 90))}`);
}

// ── Examples for top patterns ─────────────────────────────────────────────
console.log("\n" + "─".repeat(W));
console.log("  Example commands for top-20 patterns by frequency\n");

for (const s of byFreq.slice(0, 20)) {
  console.log(`  ${s.pattern}`);
  for (const ex of s.examples) {
    const display = ex.length > 90 ? ex.slice(0, 87) + "…" : ex;
    console.log(`    → ${display}`);
  }
}

console.log("\n" + "═".repeat(W) + "\n");
