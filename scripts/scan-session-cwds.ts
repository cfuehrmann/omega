#!/usr/bin/env bun
/**
 * Scans every events.jsonl in a sessions root and extracts the CWD
 * baked into the system prompt of the session_started event.
 *
 * Usage:
 *   bun scripts/scan-session-cwds.ts [sessions-root]
 *
 * Default sessions-root: .omega/sessions (relative to cwd)
 */

import { readdir, readFile } from "fs/promises";
import { join } from "path";

const sessionsRoot = process.argv[2] ?? ".omega/sessions";

const CWD_RE = /Your working directory is ([^\n.]+)/;

async function extractCwd(eventsFile: string): Promise<string | null> {
  const text = await readFile(eventsFile, "utf-8");
  for (const line of text.split("\n")) {
    if (!line.trim()) continue;
    try {
      const ev = JSON.parse(line);
      if (ev.type === "session_started" || ev.type === "session_start") {
        const prompt: string = ev.systemPrompt ?? "";
        const m = CWD_RE.exec(prompt);
        return m ? m[1]!.trim() : "(no cwd found in prompt)";
      }
    } catch {
      // malformed line — skip
    }
  }
  return null; // no session_started event found
}

async function main() {
  let entries: string[];
  try {
    entries = await readdir(sessionsRoot);
  } catch {
    console.error(`Cannot read sessions root: ${sessionsRoot}`);
    process.exit(1);
  }

  const results: { dir: string; cwd: string }[] = [];

  for (const entry of entries.sort()) {
    const eventsFile = join(sessionsRoot, entry, "events.jsonl");
    try {
      const cwd = await extractCwd(eventsFile);
      if (cwd !== null) {
        results.push({ dir: entry, cwd });
      }
    } catch {
      // file missing or unreadable — skip
    }
  }

  // Print all results
  for (const { dir, cwd } of results) {
    const flag = cwd.includes("main") ? "  ← MAIN" : "";
    console.log(`${dir}  ${cwd}${flag}`);
  }

  // Summary
  const mainSessions = results.filter(r => r.cwd.includes("main"));
  console.log(`\n${results.length} sessions scanned, ${mainSessions.length} pointing to 'main'.`);
}

main();
