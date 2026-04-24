#!/usr/bin/env bun
/**
 * load-trial.ts
 *
 * Copies a Harbor benchmark trial's Omega session files (events.jsonl +
 * context.jsonl) into a `.omega/sessions/` directory so that the Omega web
 * UI can load and replay the full session.
 *
 * Usage:
 *   bun scripts/load-trial.ts <trial-dir> [--sessions-root <dir>]
 *
 * Examples:
 *   bun scripts/load-trial.ts jobs/2026-04-24__02-22-20/crack-7z-hash__XXXXX
 *   bun scripts/load-trial.ts jobs/2026-04-24__02-22-20/crack-7z-hash__XXXXX \
 *       --sessions-root /some/other/project/.omega/sessions
 *
 * The trial directory must contain an `agent/` subdirectory with at least
 * `events.jsonl`.  `context.jsonl` is optional (older trials pre-Phase-1
 * won't have it) — the script copies whichever files are present and warns
 * about anything missing.
 *
 * The created session directory name is derived from the trial directory
 * name (job timestamp + trial id) so re-running the script is idempotent:
 * the same trial always maps to the same session dir name.
 *
 * After running, start the Omega web UI from the directory that contains
 * `.omega/` and open the session in the browser.
 */

import { existsSync, mkdirSync, copyFileSync, writeFileSync } from "fs";
import { join, basename, resolve } from "path";

// ---------------------------------------------------------------------------
// CLI argument parsing
// ---------------------------------------------------------------------------

const args = process.argv.slice(2);

if (args.length === 0 || args[0] === "--help" || args[0] === "-h") {
  console.log(`
Usage: bun scripts/load-trial.ts <trial-dir> [--sessions-root <dir>]

  <trial-dir>         Path to a Harbor trial directory, e.g.
                      jobs/2026-04-24__02-22-20/crack-7z-hash__XXXXX

  --sessions-root     Where to create the Omega session directory.
                      Default: .omega/sessions  (relative to cwd)

The script copies events.jsonl and context.jsonl from <trial-dir>/agent/
into a new session directory, then prints the directory name.
Start the Omega web UI from the same root to load the session.
`.trim());
  process.exit(0);
}

let trialDir = args[0]!;
let sessionsRoot = ".omega/sessions";

for (let i = 1; i < args.length; i++) {
  if (args[i] === "--sessions-root" && args[i + 1]) {
    sessionsRoot = args[++i]!;
  }
}

// ---------------------------------------------------------------------------
// Resolve paths
// ---------------------------------------------------------------------------

trialDir = resolve(trialDir);

if (!existsSync(trialDir)) {
  console.error(`Error: trial directory not found: ${trialDir}`);
  process.exit(1);
}

const agentDir = join(trialDir, "agent");
if (!existsSync(agentDir)) {
  console.error(`Error: no agent/ subdirectory in ${trialDir}`);
  process.exit(1);
}

// ---------------------------------------------------------------------------
// Derive a deterministic session directory name from the trial path.
//
// Harbor trial directories look like:
//   jobs/2026-04-24__02-22-20/<task-name>__<trialId>
//
// We want a session dir name that:
//   1. Matches SESSION_DIR_RE: /^\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}(-\d{3})?(-[0-9a-f]{8})?$/
//   2. Encodes enough info to identify the trial at a glance
//   3. Is stable across re-runs (same trial → same dir name)
//
// Strategy:
//   - Extract the job timestamp from the parent directory name
//     (format: YYYY-MM-DD__HH-MM-SS)
//   - Take the 7-char trial ID suffix from the trial dir name (alphanumeric)
//   - Map it to the session format: YYYY-MM-DDTHH-MM-SS-<8hexchars>
//     The 7-char trial ID is padded to 8 hex chars (appending '0' if needed)
//     after lower-casing. Non-hex characters are replaced with '0'.
// ---------------------------------------------------------------------------

function trialDirToSessionName(trialPath: string): string {
  const trialName = basename(trialPath);       // e.g. "crack-7z-hash__rFFM2tA"
  const jobDirName = basename(resolve(join(trialPath, "..")));  // e.g. "2026-04-24__02-22-20"

  // Parse job timestamp: YYYY-MM-DD__HH-MM-SS
  const tsMatch = jobDirName.match(/^(\d{4}-\d{2}-\d{2})__(\d{2}-\d{2}-\d{2})$/);
  if (!tsMatch) {
    // Fallback: use a timestamp-like placeholder
    console.warn(`Warning: cannot parse job timestamp from "${jobDirName}", using epoch`);
    return "2000-01-01T00-00-00-000-00000000";
  }
  const datePart = tsMatch[1]!;       // "2026-04-24"
  const timePart = tsMatch[2]!;       // "02-22-20"

  // Extract the trial ID suffix (after the last __)
  const idMatch = trialName.match(/__([A-Za-z0-9]+)$/);
  const rawId = idMatch ? idMatch[1]! : "0000000";

  // Normalise to 8 lowercase hex chars: keep hex chars, replace others with '0', pad/truncate
  const hexId = (rawId.toLowerCase().replace(/[^0-9a-f]/g, "0") + "00000000").slice(0, 8);

  // Format: YYYY-MM-DDTHH-MM-SS-000-<hex8>
  // (Use -000 for the millisecond slot — trials don't have sub-second precision)
  return `${datePart}T${timePart}-000-${hexId}`;
}

const sessionDirName = trialDirToSessionName(trialDir);
const sessionDir = resolve(join(sessionsRoot, sessionDirName));

// ---------------------------------------------------------------------------
// Create session directory and copy files
// ---------------------------------------------------------------------------

mkdirSync(sessionDir, { recursive: true });

const filesToCopy: Array<{ name: string; required: boolean }> = [
  { name: "events.jsonl", required: true },
  { name: "context.jsonl", required: false },
];

let anyError = false;

for (const { name, required } of filesToCopy) {
  const src = join(agentDir, name);
  const dst = join(sessionDir, name);

  if (!existsSync(src)) {
    if (required) {
      console.error(`Error: required file missing: ${src}`);
      anyError = true;
    } else {
      console.warn(`Warning: optional file not found (pre-Phase-1 trial?): ${src}`);
      // Create an empty placeholder so the session dir is well-formed
      writeFileSync(dst, "", { flag: "a" });
    }
    continue;
  }

  copyFileSync(src, dst);
  const size = Bun.file(src).size;
  console.log(`  Copied ${name}  (${(size / 1024).toFixed(1)} KB)`);
}

if (anyError) {
  process.exit(1);
}

// ---------------------------------------------------------------------------
// Write session.jsonc metadata
// ---------------------------------------------------------------------------

// Derive a human-readable name: task name (strip the job timestamp prefix)
const trialName = basename(trialDir);
// Try to extract task name (before __trialId)
const taskMatch = trialName.match(/^(.+?)__[A-Za-z0-9]+$/);
const taskName = taskMatch ? taskMatch[1]! : trialName;

// Also include the job date
const jobDirName = basename(resolve(join(trialDir, "..")));

const sessionMeta = {
  name: taskName,
  description: `Benchmark trial: ${trialName}  (job: ${jobDirName})`,
};

const metaPath = join(sessionDir, "session.jsonc");
writeFileSync(metaPath, JSON.stringify(sessionMeta, null, 2) + "\n", "utf-8");

// ---------------------------------------------------------------------------
// Done
// ---------------------------------------------------------------------------

const relSession = join(sessionsRoot, sessionDirName);

console.log(`
Session created: ${relSession}

To view in the Omega web UI:
  bun run src/web/server.ts   # or: bun run web
Then open http://localhost:3000 and select "${taskName}" from the session list.
`);
