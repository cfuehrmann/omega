/**
 * Session persistence for the web server.
 *
 * Serialises the in-memory event log to `sessions/current.jsonl` (one JSON
 * object per line). On server startup the file is read back so that history
 * replay works across restarts and crashes.
 *
 * Design choices:
 * - JSONL format: append-friendly, readable with standard tools, easy to parse
 * - One file (`current.jsonl`): keeps "which session" trivially solved
 * - Rewrite the whole file on save: event log is small (≤ a few thousand
 *   events per session); rewrite is safe and avoids partial-write corruption
 * - No LLM calls: pure serialisation, near-zero cost and latency
 * - Async I/O throughout to avoid blocking the event loop
 */

import { join } from "path";
import { mkdir, writeFile, readFile } from "fs/promises";
import { existsSync } from "fs";

const SESSIONS_DIR = join(process.cwd(), "sessions");
const SESSION_FILE = join(SESSIONS_DIR, "current.jsonl");

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/**
 * Load the persisted event log from disk.
 * Returns an empty array if no file exists yet.
 */
export async function loadSession(): Promise<object[]> {
  if (!existsSync(SESSION_FILE)) return [];
  try {
    const text = await readFile(SESSION_FILE, "utf8");
    const events: object[] = [];
    for (const line of text.split("\n")) {
      const trimmed = line.trim();
      if (!trimmed) continue;
      try {
        events.push(JSON.parse(trimmed));
      } catch {
        // Skip malformed lines (e.g. from a truncated crash write)
      }
    }
    return events;
  } catch {
    return [];
  }
}

/**
 * Persist the entire event log to disk (full rewrite).
 * Creates the `sessions/` directory if needed.
 */
export async function saveSession(events: object[]): Promise<void> {
  await mkdir(SESSIONS_DIR, { recursive: true });
  const lines = events.map(e => JSON.stringify(e)).join("\n");
  // Write to a temp file then rename for atomicity
  const tmp = SESSION_FILE + ".tmp";
  await writeFile(tmp, lines, "utf8");
  await Bun.file(tmp).text(); // flush
  // Bun.write rename workaround — use fs rename via shell-free Node compat
  const { rename } = await import("fs/promises");
  await rename(tmp, SESSION_FILE);
}

/**
 * Delete the session file (full reset).
 */
export async function clearSession(): Promise<void> {
  if (!existsSync(SESSION_FILE)) return;
  const { unlink } = await import("fs/promises");
  await unlink(SESSION_FILE);
}
