#!/usr/bin/env bun
/**
 * analyze-sessions.ts — token cost and activity classification across all sessions
 *
 * Handles two event schema versions:
 *   Old (early sessions): ts, session_start, turn_end with metrics.costUsd
 *   New (later sessions): time, session_started, llm_response with usage
 */

import { readdir, readFile } from "fs/promises";
import { join } from "path";

const SESSIONS_DIR = ".omega/sessions";

// Anthropic pricing (USD per million tokens, as of April 2026)
// claude-sonnet-4-6
const PRICE = {
  "claude-sonnet-4-6": {
    input: 3.0,
    output: 15.0,
    cacheWrite: 3.75,
    cacheRead: 0.30,
  },
  "claude-opus-4-6": {
    input: 15.0,
    output: 75.0,
    cacheWrite: 18.75,
    cacheRead: 1.50,
  },
} as Record<string, { input: number; output: number; cacheWrite: number; cacheRead: number }>;

const DEFAULT_PRICE = PRICE["claude-sonnet-4-6"] ?? { input: 3.0, output: 15.0, cacheWrite: 3.75, cacheRead: 0.3 };

// Tool categories for activity classification
const READ_TOOLS = new Set(["read_file", "list_files", "grep_files", "find_files"]);
const WRITE_TOOLS = new Set(["write_file", "edit_file"]);
const RUN_TOOLS = new Set(["run_command", "run_background", "wait_for_output", "write_stdin"]);
const RESEARCH_TOOLS = new Set(["web_search", "fetch_url"]);

interface SessionStats {
  id: string;
  startTime: string;
  model: string;
  durationMs: number;
  turns: number;
  toolCallCounts: Record<string, number>;
  tokens: {
    input: number;
    output: number;
    cacheWrite: number;
    cacheRead: number;
  };
  costUsd: number;
  savedUsd: number; // only populated for old-format sessions
  // Derived
  readToolCalls: number;
  writeToolCalls: number;
  runToolCalls: number;
  researchToolCalls: number;
  totalToolCalls: number;
  activityLabel: string;
  firstUserMessage: string;
}

function classify(stats: SessionStats): string {
  const { readToolCalls, writeToolCalls, runToolCalls, researchToolCalls, totalToolCalls, turns } = stats;
  if (totalToolCalls === 0) {
    return turns <= 2 ? "chat-only" : "multi-turn-chat";
  }
  const readFrac = readToolCalls / totalToolCalls;
  const writeFrac = writeToolCalls / totalToolCalls;
  const hasCommit = (stats.toolCallCounts["run_command"] ?? 0) > 0;

  if (writeFrac > 0.2) return "implementing";
  if (hasCommit && runToolCalls > 0 && writeFrac > 0) return "implementing";
  if (readFrac > 0.7 && totalToolCalls >= 3) return "reviewing";
  if (researchToolCalls > readToolCalls && researchToolCalls > 0) return "research";
  if (runToolCalls > 0 && writeFrac === 0 && readFrac < 0.5) return "testing";
  if (readFrac > 0.4 && writeFrac < 0.1) return "exploring";
  return "mixed";
}

function computeCost(tokens: SessionStats["tokens"], model: string): number {
  const p = PRICE[model] ?? DEFAULT_PRICE;
  return (
    (tokens.input * p.input +
      tokens.output * p.output +
      tokens.cacheWrite * p.cacheWrite +
      tokens.cacheRead * p.cacheRead) /
    1_000_000
  );
}

async function parseSession(sessionDir: string): Promise<SessionStats | null> {
  const eventsPath = join(sessionDir, "events.jsonl");
  let raw: string;
  try {
    raw = await readFile(eventsPath, "utf-8");
  } catch {
    return null;
  }

  const lines = raw.trim().split("\n").filter(Boolean);
  const events = lines.map((l) => {
    try { return JSON.parse(l); } catch { return null; }
  }).filter(Boolean);

  const dirName = sessionDir.split("/").pop()!;

  let startTime = dirName.slice(0, 19).replace(/-/g, (m, i) => i < 10 ? (i === 4 || i === 7 ? "-" : i === 10 ? "T" : "-") : ":");
  // Parse ISO from dir name: 2026-03-08T15-45-06-...
  const dtMatch = dirName.match(/^(\d{4}-\d{2}-\d{2})T(\d{2})-(\d{2})-(\d{2})/);
  if (dtMatch) {
    startTime = `${dtMatch[1]}T${dtMatch[2]}:${dtMatch[3]}:${dtMatch[4]}Z`;
  }

  let model = "claude-sonnet-4-6";
  let turns = 0;
  let firstTime: number | null = null;
  let lastTime: number | null = null;
  const toolCallCounts: Record<string, number> = {};
  const tokens = { input: 0, output: 0, cacheWrite: 0, cacheRead: 0 };
  let costUsdFromLog = 0;
  let savedUsdFromLog = 0;
  let firstUserMessage = "";

  for (const ev of events) {
    const ts = ev.time ?? ev.ts;
    if (ts) {
      const t = new Date(ts).getTime();
      if (!firstTime || t < firstTime) firstTime = t;
      if (!lastTime || t > lastTime) lastTime = t;
    }

    if (ev.type === "session_start" || ev.type === "session_started") {
      if (ev.model) model = ev.model;
    }

    if (ev.type === "user_message") {
      if (!firstUserMessage && ev.content) {
        firstUserMessage = String(ev.content).slice(0, 120).replace(/\n/g, " ");
      }
    }

    // New format: llm_response with usage
    if (ev.type === "llm_response" && ev.usage) {
      turns++;
      tokens.input += ev.usage.input_tokens ?? 0;
      tokens.output += ev.usage.output_tokens ?? 0;
      tokens.cacheWrite += ev.usage.cache_creation_input_tokens ?? 0;
      tokens.cacheRead += ev.usage.cache_read_input_tokens ?? 0;
    }

    // Old format: turn_end with metrics AND toolCalls array
    if (ev.type === "turn_end" && ev.metrics) {
      turns++;
      tokens.input += ev.metrics.inputTokens ?? 0;
      tokens.output += ev.metrics.outputTokens ?? 0;
      tokens.cacheWrite += ev.metrics.cacheCreationTokens ?? 0;
      tokens.cacheRead += ev.metrics.cacheReadTokens ?? 0;
      costUsdFromLog += ev.metrics.costUsd ?? 0;
      savedUsdFromLog += ev.metrics.savedUsd ?? 0;
      // Old format stores tool call names directly in turn_end
      if (Array.isArray(ev.toolCalls)) {
        for (const name of ev.toolCalls) {
          toolCallCounts[name] = (toolCallCounts[name] ?? 0) + 1;
        }
      }
    }

    // New format: separate tool_call events
    if (ev.type === "tool_call") {
      const name = ev.name ?? "";
      toolCallCounts[name] = (toolCallCounts[name] ?? 0) + 1;
    }
    // Old format: tool calls as separate events (some variants)
    if (ev.type === "tool_use") {
      const name = ev.name ?? "";
      toolCallCounts[name] = (toolCallCounts[name] ?? 0) + 1;
    }
  }

  if (turns === 0) return null;

  let readToolCalls = 0, writeToolCalls = 0, runToolCalls = 0, researchToolCalls = 0;
  for (const [name, count] of Object.entries(toolCallCounts)) {
    if (READ_TOOLS.has(name)) readToolCalls += count;
    else if (WRITE_TOOLS.has(name)) writeToolCalls += count;
    else if (RUN_TOOLS.has(name)) runToolCalls += count;
    else if (RESEARCH_TOOLS.has(name)) researchToolCalls += count;
  }
  const totalToolCalls = readToolCalls + writeToolCalls + runToolCalls + researchToolCalls;

  const stats: SessionStats = {
    id: dirName,
    startTime,
    model,
    durationMs: (lastTime ?? 0) - (firstTime ?? 0),
    turns,
    toolCallCounts,
    tokens,
    costUsd: costUsdFromLog > 0 ? costUsdFromLog : computeCost(tokens, model),
    savedUsd: savedUsdFromLog,
    readToolCalls,
    writeToolCalls,
    runToolCalls,
    researchToolCalls,
    totalToolCalls,
    activityLabel: "",
    firstUserMessage,
  };
  stats.activityLabel = classify(stats);
  return stats;
}

function fmt(n: number, dec = 0): string {
  return n.toLocaleString("en-US", { minimumFractionDigits: dec, maximumFractionDigits: dec });
}

function fmtUsd(n: number): string {
  if (n >= 1) return `$${n.toFixed(2)}`;
  return `$${n.toFixed(4)}`;
}

function fmtDuration(ms: number): string {
  if (ms < 60_000) return `${Math.round(ms / 1000)}s`;
  if (ms < 3_600_000) return `${Math.round(ms / 60_000)}m`;
  return `${(ms / 3_600_000).toFixed(1)}h`;
}

async function main() {
  const entries = await readdir(SESSIONS_DIR, { withFileTypes: true });
  const sessionDirs = entries
    .filter((e) => e.isDirectory())
    .map((e) => join(SESSIONS_DIR, e.name))
    .sort();

  console.error(`Parsing ${sessionDirs.length} sessions...`);

  const allStats: SessionStats[] = [];
  for (const dir of sessionDirs) {
    const s = await parseSession(dir);
    if (s) allStats.push(s);
  }

  console.error(`Parsed ${allStats.length} sessions with LLM activity.\n`);

  // ── OVERALL TOTALS ────────────────────────────────────────────────────────
  const totalCost = allStats.reduce((s, x) => s + x.costUsd, 0);
  const totalSaved = allStats.reduce((s, x) => s + x.savedUsd, 0);
  const totalInput = allStats.reduce((s, x) => s + x.tokens.input, 0);
  const totalOutput = allStats.reduce((s, x) => s + x.tokens.output, 0);
  const totalCacheWrite = allStats.reduce((s, x) => s + x.tokens.cacheWrite, 0);
  const totalCacheRead = allStats.reduce((s, x) => s + x.tokens.cacheRead, 0);
  const totalTurns = allStats.reduce((s, x) => s + x.turns, 0);

  console.log("═══════════════════════════════════════════════════════════════");
  console.log("  OMEGA SESSION ANALYSIS");
  console.log("═══════════════════════════════════════════════════════════════");
  console.log(`  Sessions analysed : ${allStats.length}`);
  console.log(`  Total LLM turns   : ${fmt(totalTurns)}`);
  console.log(`  Total cost (est.) : ${fmtUsd(totalCost)}`);
  console.log(`  Input tokens      : ${fmt(totalInput)} (uncached)`);
  console.log(`  Output tokens     : ${fmt(totalOutput)}`);
  console.log(`  Cache-write tokens: ${fmt(totalCacheWrite)}`);
  console.log(`  Cache-read tokens : ${fmt(totalCacheRead)}`);
  const cacheReadFrac = totalCacheRead / (totalCacheRead + totalInput + totalCacheWrite) * 100;
  console.log(`  Cache-read %      : ${cacheReadFrac.toFixed(1)}% of total context tokens`);
  if (totalSaved > 0) {
    console.log(`  Cache savings (logged): ${fmtUsd(totalSaved)} (old-format sessions only)`);
    console.log(`  Effective cost        : ${fmtUsd(totalCost)} (already-paid after discounts)`);
  }
  console.log();

  // ── BY ACTIVITY LABEL ─────────────────────────────────────────────────────
  const byActivity = new Map<string, SessionStats[]>();
  for (const s of allStats) {
    if (!byActivity.has(s.activityLabel)) byActivity.set(s.activityLabel, []);
    byActivity.get(s.activityLabel)!.push(s);
  }

  console.log("─── By Activity Type ──────────────────────────────────────────");
  console.log(
    `${"Activity".padEnd(20)} ${"Sessions".padStart(8)} ${"Turns".padStart(6)} ${"Cost (est)".padStart(12)} ${"% cost".padStart(8)} ${"Avg cost".padStart(10)} ${"Avg turns".padStart(10)}`
  );
  console.log("─".repeat(80));

  const labels = [...byActivity.keys()].sort((a, b) => {
    const ca = byActivity.get(a)!.reduce((s, x) => s + x.costUsd, 0);
    const cb = byActivity.get(b)!.reduce((s, x) => s + x.costUsd, 0);
    return cb - ca;
  });

  for (const label of labels) {
    const group = byActivity.get(label)!;
    const cost = group.reduce((s, x) => s + x.costUsd, 0);
    const turns = group.reduce((s, x) => s + x.turns, 0);
    console.log(
      `${label.padEnd(20)} ${fmt(group.length).padStart(8)} ${fmt(turns).padStart(6)} ${fmtUsd(cost).padStart(12)} ${(cost / totalCost * 100).toFixed(1).padStart(7)}% ${fmtUsd(cost / group.length).padStart(10)} ${(turns / group.length).toFixed(1).padStart(10)}`
    );
  }
  console.log();

  // ── TOKEN COMPOSITION PER ACTIVITY ───────────────────────────────────────
  console.log("─── Token Composition per Activity ───────────────────────────");
  console.log(
    `${"Activity".padEnd(20)} ${"Input".padStart(10)} ${"Output".padStart(10)} ${"CacheWr".padStart(10)} ${"CacheRd".padStart(10)} ${"CacheRd%".padStart(10)}`
  );
  console.log("─".repeat(75));
  for (const label of labels) {
    const group = byActivity.get(label)!;
    const inp = group.reduce((s, x) => s + x.tokens.input, 0);
    const out = group.reduce((s, x) => s + x.tokens.output, 0);
    const cw = group.reduce((s, x) => s + x.tokens.cacheWrite, 0);
    const cr = group.reduce((s, x) => s + x.tokens.cacheRead, 0);
    const crPct = cr / (cr + inp + cw) * 100;
    console.log(
      `${label.padEnd(20)} ${fmt(inp).padStart(10)} ${fmt(out).padStart(10)} ${fmt(cw).padStart(10)} ${fmt(cr).padStart(10)} ${crPct.toFixed(1).padStart(9)}%`
    );
  }
  console.log();

  // ── TOP 20 MOST EXPENSIVE SESSIONS ────────────────────────────────────────
  const top20 = [...allStats].sort((a, b) => b.costUsd - a.costUsd).slice(0, 20);
  console.log("─── Top 20 Most Expensive Sessions ────────────────────────────");
  console.log(
    `${"Session (start)".padEnd(26)} ${"Activity".padEnd(14)} ${"Turns".padStart(6)} ${"Cost".padStart(8)} ${"Duration".padStart(10)} ${"First user message".padEnd(40)}`
  );
  console.log("─".repeat(110));
  for (const s of top20) {
    console.log(
      `${s.startTime.slice(0, 19).replace("T", " ").padEnd(26)} ${s.activityLabel.padEnd(14)} ${String(s.turns).padStart(6)} ${fmtUsd(s.costUsd).padStart(8)} ${fmtDuration(s.durationMs).padStart(10)}  ${s.firstUserMessage.slice(0, 40)}`
    );
  }
  console.log();

  // ── COST OVER TIME (monthly) ──────────────────────────────────────────────
  const byMonth = new Map<string, { cost: number; sessions: number; turns: number }>();
  for (const s of allStats) {
    const month = s.startTime.slice(0, 7);
    if (!byMonth.has(month)) byMonth.set(month, { cost: 0, sessions: 0, turns: 0 });
    const m = byMonth.get(month)!;
    m.cost += s.costUsd;
    m.sessions++;
    m.turns += s.turns;
  }
  console.log("─── Cost Over Time (by month) ─────────────────────────────────");
  console.log(`${"Month".padEnd(10)} ${"Sessions".padStart(9)} ${"Turns".padStart(8)} ${"Cost (est)".padStart(12)}`);
  console.log("─".repeat(44));
  for (const [month, data] of [...byMonth.entries()].sort()) {
    console.log(`${month.padEnd(10)} ${fmt(data.sessions).padStart(9)} ${fmt(data.turns).padStart(8)} ${fmtUsd(data.cost).padStart(12)}`);
  }
  console.log();

  // ── TOOL CALL HEATMAP ─────────────────────────────────────────────────────
  const toolTotals: Record<string, number> = {};
  for (const s of allStats) {
    for (const [tool, count] of Object.entries(s.toolCallCounts)) {
      toolTotals[tool] = (toolTotals[tool] ?? 0) + count;
    }
  }
  const sortedTools = Object.entries(toolTotals).sort((a, b) => b[1] - a[1]);
  console.log("─── All-Time Tool Call Totals ──────────────────────────────────");
  for (const [tool, count] of sortedTools) {
    const maxCount = sortedTools[0]?.[1] ?? 1;
    const bar = "█".repeat(Math.round(count / (maxCount / 40)));
    console.log(`  ${tool.padEnd(20)} ${fmt(count).padStart(6)}  ${bar}`);
  }
  console.log();

  // ── COST SAVING HYPOTHETICAL ──────────────────────────────────────────────
  // If "exploring" and "reviewing" sessions were done with a cheaper model (haiku),
  // or if reading-heavy turns saved cache reads instead of recomputing context…
  const exploringAndReviewing = allStats.filter(s =>
    s.activityLabel === "reviewing" || s.activityLabel === "exploring" || s.activityLabel === "chat-only"
  );
  const explRevCost = exploringAndReviewing.reduce((s, x) => s + x.costUsd, 0);
  const implSessions = allStats.filter(s => s.activityLabel === "implementing" || s.activityLabel === "mixed");
  const implCost = implSessions.reduce((s, x) => s + x.costUsd, 0);

  console.log("─── Context Overhead Analysis ─────────────────────────────────");
  // Per-session: ratio of cache-read to (input + cacheRead)
  // High cache-read ratio → benefit of session continuity
  // Low cache-read ratio → starting fresh wouldn't cost much
  const avgCacheReadFrac = allStats.reduce((s, x) => {
    const total = x.tokens.input + x.tokens.cacheRead + x.tokens.cacheWrite;
    return s + (total > 0 ? x.tokens.cacheRead / total : 0);
  }, 0) / allStats.length;

  const longSessions = allStats.filter(s => s.turns >= 5);
  const avgCacheReadFracLong = longSessions.reduce((s, x) => {
    const total = x.tokens.input + x.tokens.cacheRead + x.tokens.cacheWrite;
    return s + (total > 0 ? x.tokens.cacheRead / total : 0);
  }, 0) / (longSessions.length || 1);

  console.log(`  Sessions with ≥5 turns : ${longSessions.length}`);
  console.log(`  Avg cache-read frac (all)   : ${(avgCacheReadFrac * 100).toFixed(1)}%`);
  console.log(`  Avg cache-read frac (≥5 turns): ${(avgCacheReadFracLong * 100).toFixed(1)}%`);
  console.log();
  console.log(`  "Exploring/reviewing/chat" sessions: ${exploringAndReviewing.length} sessions, ${fmtUsd(explRevCost)} cost`);
  console.log(`  "Implementing/mixed" sessions      : ${implSessions.length} sessions, ${fmtUsd(implCost)} cost`);
  console.log();

  // Distribution of session sizes
  const buckets = [1, 2, 3, 5, 10, 20, 50, Infinity];
  const bucketLabels = ["1 turn", "2 turns", "3-4 turns", "5-9 turns", "10-19 turns", "20-49 turns", "50+ turns"];
  const bucketCounts = new Array(bucketLabels.length).fill(0);
  const bucketCosts = new Array(bucketLabels.length).fill(0);
  for (const s of allStats) {
    let bi = 0;
    for (let i = 0; i < buckets.length - 1; i++) {
      if (s.turns >= (buckets[i] ?? 0) && s.turns < (buckets[i + 1] ?? Infinity)) { bi = i; break; }
    }
    bucketCounts[bi]++;
    bucketCosts[bi] += s.costUsd;
  }
  console.log("─── Sessions by Length ─────────────────────────────────────────");
  console.log(`${"Size".padEnd(12)} ${"Sessions".padStart(9)} ${"Cost (est)".padStart(12)} ${"% cost".padStart(8)}`);
  console.log("─".repeat(46));
  for (let i = 0; i < bucketLabels.length; i++) {
    if ((bucketCounts[i] as number) > 0)
      console.log(`${(bucketLabels[i] as string).padEnd(12)} ${fmt(bucketCounts[i] as number).padStart(9)} ${fmtUsd(bucketCosts[i] as number).padStart(12)} ${((bucketCosts[i] as number) / totalCost * 100).toFixed(1).padStart(7)}%`);
  }
}

main().catch(console.error);
