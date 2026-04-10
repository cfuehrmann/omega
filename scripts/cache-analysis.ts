#!/usr/bin/env bun
/**
 * Cache cost analysis: 5-minute vs 1-hour prompt caching
 *
 * Reads all llm_response events from all sessions, computes actual costs
 * (using 5m cache as recorded), then simulates what costs would be with
 * 1h cache — accounting for extra cache hits between calls ≤60 min apart.
 */

import { readdirSync, readFileSync } from "fs";
import { join } from "path";

// ── Pricing (per million tokens) for claude-sonnet-4-6 ──────────────────────
// Other models will fall back to sonnet pricing as a rough estimate.
// Source: https://platform.claude.com/docs/en/build-with-claude/prompt-caching.md
const PRICING: Record<string, {
  input: number;
  write5m: number;
  write1h: number;
  read: number;
  output: number;
}> = {
  "claude-sonnet-4-6": { input: 3.00, write5m: 3.75, write1h: 6.00, read: 0.30, output: 15.00 },
  "claude-opus-4-6":   { input: 5.00, write5m: 6.25, write1h: 10.00, read: 0.50, output: 25.00 },
  // Fallback
  default:             { input: 3.00, write5m: 3.75, write1h: 6.00, read: 0.30, output: 15.00 },
};

const DEFAULT_PRICE = { input: 3.00, write5m: 3.75, write1h: 6.00, read: 0.30, output: 15.00 };
function price(model: string) {
  return PRICING[model] ?? DEFAULT_PRICE;
}

interface LlmCall {
  time: Date;
  model: string;
  inputTokens: number;
  outputTokens: number;
  cacheWrite5m: number;
  cacheWrite1h: number;
  cacheRead: number;
  sessionDir: string;
}

// ── Collect all LLM response events ─────────────────────────────────────────
const sessionsRoot = "/home/carsten/omega/dev/.omega/sessions";
const calls: LlmCall[] = [];

for (const dir of readdirSync(sessionsRoot)) {
  if (!dir.includes("T")) continue; // skip events.jsonl at root
  const eventsPath = join(sessionsRoot, dir, "events.jsonl");
  let text: string;
  try { text = readFileSync(eventsPath, "utf-8"); } catch { continue; }

  for (const line of text.split("\n")) {
    if (!line.trim()) continue;
    let evt: Record<string, unknown>;
    try { evt = JSON.parse(line); } catch { continue; }
    if (evt.type !== "llm_response") continue;

    const usage = (evt as Record<string, Record<string, unknown>>).usage as Record<string, number> | undefined;
    if (!usage) continue;

    // Prefer the detailed cache_creation split from responseSummary if available
    const rs = (evt as Record<string, Record<string, unknown>>).responseSummary as Record<string, unknown> | undefined;
    const rsUsage = rs?.usage as Record<string, unknown> | undefined;
    const cacheCreation = rsUsage?.cache_creation as Record<string, number> | undefined;

    const write5m = cacheCreation?.ephemeral_5m_input_tokens ?? usage.cache_creation_input_tokens ?? 0;
    const write1h = cacheCreation?.ephemeral_1h_input_tokens ?? 0;

    const timeStr = (evt.time ?? evt.ts) as string | undefined;
    if (!timeStr) continue;
    const time = new Date(timeStr);
    if (isNaN(time.getTime())) continue;

    calls.push({
      time,
      model: (evt.model as string) ?? "claude-sonnet-4-6",
      inputTokens: usage.input_tokens ?? 0,
      outputTokens: usage.output_tokens ?? 0,
      cacheWrite5m: write5m,
      cacheWrite1h: write1h,
      cacheRead: usage.cache_read_input_tokens ?? 0,
      sessionDir: dir,
    });
  }
}

// Sort by time
calls.sort((a, b) => a.time.getTime() - b.time.getTime());

// ── Compute actual cost (as recorded) ───────────────────────────────────────
function computeActualCost(calls: LlmCall[]) {
  let total = 0;
  const breakdown = { input: 0, write5m: 0, write1h: 0, read: 0, output: 0 };
  for (const c of calls) {
    const p = price(c.model)!;
    breakdown.input  += c.inputTokens   * p.input   / 1e6;
    breakdown.write5m+= c.cacheWrite5m  * p.write5m / 1e6;
    breakdown.write1h+= c.cacheWrite1h  * p.write1h / 1e6;
    breakdown.read   += c.cacheRead     * p.read    / 1e6;
    breakdown.output += c.outputTokens  * p.output  / 1e6;
  }
  total = Object.values(breakdown).reduce((a, b) => a + b, 0);
  return { total, breakdown };
}

// ── Simulate 1h-cache cost ───────────────────────────────────────────────────
// Heuristic: for each call that wrote to 5m cache (write5m > 0),
// check if the previous call was within 60 minutes.
// If so, under 1h cache those tokens would have been cache reads instead.
//
// Caveat: the cache key is the full prefix; we assume consecutive calls
// within a short window share a large common prefix (true for Omega which
// always starts from the same system prompt + tools block).
function computeSimulated1hCost(calls: LlmCall[]) {
  let total = 0;
  const breakdown = { input: 0, write5m: 0, write1h: 0, read: 0, output: 0 };

  // Track which calls "converted" from write to read
  let extraHits = 0;
  let extraHitTokens = 0;
  let savedWriteTokens = 0;

  for (let i = 0; i < calls.length; i++) {
    const c = calls[i]!;
    const p = price(c.model)!;

    // Check if previous call was within 5 min (already a hit under 5m)
    const prevCall = i > 0 ? calls[i - 1]! : null;
    const gapMs = prevCall ? c.time.getTime() - prevCall.time.getTime() : Infinity;
    const gapMin = gapMs / 60000;

    let simWrite5m = c.cacheWrite5m;
    let simWrite1h = c.cacheWrite1h;
    let simRead    = c.cacheRead;

    // If this call had 5m cache writes AND previous call was within [5min, 60min],
    // simulate conversion: those write tokens become read tokens under 1h cache.
    // (If gap ≤5min, 5m cache already covered it — no change needed.)
    if (c.cacheWrite5m > 0 && gapMin > 5 && gapMin <= 60) {
      extraHits++;
      extraHitTokens += c.cacheWrite5m;
      savedWriteTokens += c.cacheWrite5m;
      simRead   += c.cacheWrite5m;
      simWrite5m = 0;
    }

    // All 5m writes become 1h writes (cost is different)
    breakdown.input  += c.inputTokens * p.input   / 1e6;
    breakdown.write1h+= simWrite5m    * p.write1h / 1e6;  // 5m writes → 1h cost
    breakdown.write1h+= simWrite1h    * p.write1h / 1e6;
    breakdown.read   += simRead       * p.read    / 1e6;
    breakdown.output += c.outputTokens* p.output  / 1e6;
  }

  total = Object.values(breakdown).reduce((a, b) => a + b, 0);
  return { total, breakdown, extraHits, extraHitTokens, savedWriteTokens };
}

// ── Aggregate stats ──────────────────────────────────────────────────────────
const actual   = computeActualCost(calls);
const sim1h    = computeSimulated1hCost(calls);

const totalInput     = calls.reduce((s, c) => s + c.inputTokens, 0);
const totalOutput    = calls.reduce((s, c) => s + c.outputTokens, 0);
const totalWrite5m   = calls.reduce((s, c) => s + c.cacheWrite5m, 0);
const totalWrite1h   = calls.reduce((s, c) => s + c.cacheWrite1h, 0);
const totalRead      = calls.reduce((s, c) => s + c.cacheRead, 0);

// Gap distribution for calls that had 5m writes
const gaps: number[] = [];
for (let i = 1; i < calls.length; i++) {
  if (calls[i]!.cacheWrite5m > 0) {
    const g = (calls[i]!.time.getTime() - calls[i-1]!.time.getTime()) / 60000;
    gaps.push(g);
  }
}
const validGaps = gaps.filter(g => !isNaN(g) && isFinite(g));
validGaps.sort((a, b) => a - b);
// replace gaps reference below
const gapsSorted = validGaps;

function pct(arr: number[], p: number): number {
  const idx = Math.floor(arr.length * p / 100);
  return arr[Math.min(idx, arr.length - 1)]!;
}

// ── Output ───────────────────────────────────────────────────────────────────
console.log("═══════════════════════════════════════════════════════════");
console.log("  Omega Session Cache Analysis");
console.log(`  Sessions analysed : ${readdirSync(sessionsRoot).filter(d => d.includes("T")).length}`);
console.log(`  LLM calls         : ${calls.length.toLocaleString()}`);
console.log(`  Date range        : ${calls[0]!.time.toISOString().slice(0,10)} → ${calls[calls.length-1]!.time.toISOString().slice(0,10)}`);
console.log("═══════════════════════════════════════════════════════════");

console.log("\n── Token totals ────────────────────────────────────────────");
console.log(`  Base input tokens    : ${totalInput.toLocaleString()}`);
console.log(`  Output tokens        : ${totalOutput.toLocaleString()}`);
console.log(`  Cache-write (5m)     : ${totalWrite5m.toLocaleString()}`);
console.log(`  Cache-write (1h)     : ${totalWrite1h.toLocaleString()}`);
console.log(`  Cache-read (hits)    : ${totalRead.toLocaleString()}`);
console.log(`  Cache hit rate       : ${(totalRead / (totalRead + totalWrite5m + totalWrite1h + totalInput) * 100).toFixed(1)}%`);

console.log("\n── Gap between consecutive calls (when 5m-write occurred) ──");
if (gapsSorted.length > 0) {
  console.log(`  p10: ${pct(gapsSorted,10)!.toFixed(1)} min`);
  console.log(`  p25: ${pct(gapsSorted,25)!.toFixed(1)} min`);
  console.log(`  p50: ${pct(gapsSorted,50)!.toFixed(1)} min  (median)`);
  console.log(`  p75: ${pct(gapsSorted,75)!.toFixed(1)} min`);
  console.log(`  p90: ${pct(gapsSorted,90)!.toFixed(1)} min`);
  const within5  = gapsSorted.filter(g => g <=  5).length;
  const within60 = gapsSorted.filter(g => g <= 60).length;
  console.log(`  Within  5 min : ${within5}  / ${gapsSorted.length} (${(within5/gapsSorted.length*100).toFixed(1)}%)`);
  console.log(`  Within 60 min : ${within60} / ${gapsSorted.length} (${(within60/gapsSorted.length*100).toFixed(1)}%)`);
}

console.log("\n── Cost: ACTUAL (5m cache, as used) ───────────────────────");
console.log(`  Base input cost   : $${actual.breakdown.input.toFixed(4)}`);
console.log(`  Cache-write 5m    : $${actual.breakdown.write5m.toFixed(4)}`);
console.log(`  Cache-write 1h    : $${actual.breakdown.write1h.toFixed(4)}`);
console.log(`  Cache-read cost   : $${actual.breakdown.read.toFixed(4)}`);
console.log(`  Output cost       : $${actual.breakdown.output.toFixed(4)}`);
console.log(`  ─────────────────────────────────────────────`);
console.log(`  TOTAL             : $${actual.total.toFixed(4)}`);

console.log("\n── Cost: SIMULATED (1h cache throughout) ───────────────────");
console.log(`  (All 5m writes repriced at 2x; calls within (5-60 min]`);
console.log(`   of prior call converted from writes → reads)`);
console.log(`  Extra hits captured : ${sim1h.extraHits} calls, ${sim1h.extraHitTokens.toLocaleString()} tokens`);
console.log(`  Base input cost   : $${sim1h.breakdown.input.toFixed(4)}`);
console.log(`  Cache-write 1h    : $${sim1h.breakdown.write1h.toFixed(4)}`);
console.log(`  Cache-read cost   : $${sim1h.breakdown.read.toFixed(4)}`);
console.log(`  Output cost       : $${sim1h.breakdown.output.toFixed(4)}`);
console.log(`  ─────────────────────────────────────────────`);
console.log(`  TOTAL             : $${sim1h.total.toFixed(4)}`);

const diff = sim1h.total - actual.total;
console.log(`\n── Delta (1h vs 5m) ─────────────────────────────────────────`);
console.log(`  Difference        : ${diff >= 0 ? "+" : ""}$${diff.toFixed(4)}  (${diff >= 0 ? "1h is MORE expensive" : "1h is CHEAPER"})`);
console.log(`  % change          : ${(diff/actual.total*100).toFixed(1)}%`);
console.log("═══════════════════════════════════════════════════════════");
