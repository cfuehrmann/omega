#!/usr/bin/env bun
/**
 * bench-ingest.ts
 *
 * Scans jobs/ for Harbor trial result.json files and appends any new ones to
 * benchmark-results/results.jsonl.  Idempotent: safe to run after every
 * `harbor run`.
 *
 * Usage:
 *   bun scripts/bench-ingest.ts              # scan all jobs/
 *   bun scripts/bench-ingest.ts jobs/2026-05-01__10-00-00   # one job dir only
 */

import { existsSync, readFileSync, appendFileSync, readdirSync, statSync } from "fs";
import { join, resolve } from "path";

// ── paths ────────────────────────────────────────────────────────────────────

const ROOT = resolve(import.meta.dir, "..");
const JOBS_DIR = join(ROOT, "jobs");
const RESULTS_FILE = join(ROOT, "benchmark-results", "results.jsonl");
const SKIP_FILE = join(ROOT, "benchmark-results", ".skip-trials");

// ── load seen / skip sets ────────────────────────────────────────────────────

const seen = new Set<string>();

// Trials already in results.jsonl
if (existsSync(RESULTS_FILE)) {
  for (const line of readFileSync(RESULTS_FILE, "utf-8").split("\n")) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    try {
      const r = JSON.parse(trimmed) as { trial_id: string };
      if (r.trial_id) seen.add(r.trial_id);
    } catch {
      // malformed line — ignore
    }
  }
}

// Trials permanently excluded (teething-problem infrastructure failures)
if (existsSync(SKIP_FILE)) {
  for (const line of readFileSync(SKIP_FILE, "utf-8").split("\n")) {
    const id = line.trim();
    if (id && !id.startsWith("#")) seen.add(id);
  }
}

// ── resolve job directories to scan ─────────────────────────────────────────

const jobDirs: string[] = [];
const arg = process.argv[2];

if (arg) {
  // Explicit job directory supplied on command line
  const p = resolve(arg);
  if (!existsSync(p)) {
    console.error(`Error: path not found: ${p}`);
    process.exit(1);
  }
  jobDirs.push(p);
} else {
  // Scan all entries under jobs/
  if (!existsSync(JOBS_DIR)) {
    console.error(`Error: jobs/ directory not found at ${JOBS_DIR}`);
    process.exit(1);
  }
  for (const entry of readdirSync(JOBS_DIR)) {
    const p = join(JOBS_DIR, entry);
    if (statSync(p).isDirectory()) jobDirs.push(p);
  }
}

// ── scan and ingest ──────────────────────────────────────────────────────────

let newCount = 0;
let skippedCount = 0;

for (const jobDir of jobDirs) {
  for (const trialEntry of readdirSync(jobDir)) {
    const trialPath = join(jobDir, trialEntry);
    if (!statSync(trialPath).isDirectory()) continue;

    const resultPath = join(trialPath, "result.json");
    if (!existsSync(resultPath)) continue;

    let raw: Record<string, unknown>;
    try {
      raw = JSON.parse(readFileSync(resultPath, "utf-8")) as Record<string, unknown>;
    } catch {
      console.warn(`  warn: could not parse ${resultPath}`);
      continue;
    }

    // Trial results have trial_name; job-level summaries have stats.  Skip the latter.
    if (!raw.trial_name) continue;

    const trialId = raw.id as string;
    if (!trialId) continue;

    if (seen.has(trialId)) {
      skippedCount++;
      continue;
    }

    const agentInfo = raw.agent_info as Record<string, unknown> | undefined;
    const agentResult = raw.agent_result as Record<string, unknown> | undefined;
    const verifierResult = raw.verifier_result as Record<string, unknown> | undefined;
    const rewards = verifierResult?.rewards as Record<string, unknown> | undefined;
    const exceptionInfo = raw.exception_info as Record<string, unknown> | undefined;
    const modelInfo = agentInfo?.model_info as Record<string, unknown> | undefined;
    const config = raw.config as Record<string, unknown> | undefined;

    const startedAt = raw.started_at as string;
    const finishedAt = raw.finished_at as string;
    const runtimeSec = startedAt && finishedAt
      ? Math.round((new Date(finishedAt).getTime() - new Date(startedAt).getTime()) / 1000)
      : null;

    const record = {
      trial_id:        trialId,
      job_id:          config?.job_id ?? null,
      task_name:       raw.task_name as string,
      ingested_at:     new Date().toISOString(),
      started_at:      startedAt ?? null,
      finished_at:     finishedAt ?? null,
      runtime_sec:     runtimeSec,
      agent:           agentInfo?.name ?? "unknown",
      model:           modelInfo?.name ?? "unknown",
      reward:          rewards?.reward ?? null,
      n_input_tokens:  agentResult?.n_input_tokens ?? null,
      n_output_tokens: agentResult?.n_output_tokens ?? null,
      n_cache_tokens:  agentResult?.n_cache_tokens ?? null,
      exception:       exceptionInfo?.exception_type ?? null,
    };

    appendFileSync(RESULTS_FILE, JSON.stringify(record) + "\n");
    seen.add(trialId);
    newCount++;

    const rewardStr = typeof record.reward === "number" ? record.reward.toFixed(1) : "n/a";
    const excStr = record.exception ? ` [${record.exception}]` : "";
    console.log(`  + ${record.task_name}  reward=${rewardStr}${excStr}`);
  }
}

console.log(
  `\nIngested: ${newCount} new trial(s)  |  skipped: ${skippedCount} already seen  |  total in store: ${seen.size}`
);
