/**
 * Session directory management (SESSION-1, SESSION-2).
 *
 * Each Omega session gets its own timestamped folder under `.omega/sessions/`
 * in the current working directory (SESSION-2: sessions live alongside the
 * project being worked on, not alongside the Omega source):
 *
 *   .omega/sessions/2025-07-04T14-32-05/
 *     context.jsonl
 *     events.jsonl
 *
 * The folder name is an ISO 8601 datetime truncated to seconds, with colons
 * replaced by hyphens so the name is valid on all filesystems.
 *
 * Benefits:
 *   - Sessions are co-located with the project — can be committed to the
 *     project's own VCS if the operator chooses (no automatic .gitignore).
 *   - No rotation / `.prev` file management needed — every session starts clean.
 *   - Old sessions are preserved indefinitely (until explicitly pruned).
 *   - Folders can be renamed to human-readable names (SESSION-5).
 *   - `.omega/` namespace leaves room for future artefacts (config, world-state).
 */

import { mkdir, readdir } from "fs/promises";
import { join } from "path";

/** Root directory for all session folders. Relative to cwd (SESSION-2). */
export const SESSIONS_ROOT = ".omega/sessions";

/**
 * Generate a session folder name from the current timestamp.
 * Format: `YYYY-MM-DDTHH-MM-SS` (colons replaced with hyphens).
 */
export function makeSessionDirName(now: Date = new Date()): string {
  // toISOString() → "2025-07-04T14:32:05.123Z"
  // Take the first 19 chars ("2025-07-04T14:32:05"), replace colons
  return now.toISOString().slice(0, 19).replace(/:/g, "-");
}

export interface SessionPaths {
  /** Absolute-or-relative path to the session directory (no trailing slash). */
  dir: string;
  /** Path to context.jsonl inside the session dir. */
  contextFile: string;
  /** Path to events.jsonl inside the session dir. */
  eventsFile: string;
}

/**
 * Create the session directory for the current session and return the paths
 * to use for context and event persistence.
 *
 * Creates `.omega/sessions/<timestamp>/` if it doesn't exist.
 * Returns the paths; the caller passes them to Agent.
 */
export async function makeSessionDir(now: Date = new Date()): Promise<SessionPaths> {
  const dirName = makeSessionDirName(now);
  const dir = join(SESSIONS_ROOT, dirName);
  await mkdir(dir, { recursive: true });
  return {
    dir,
    contextFile: join(dir, "context.jsonl"),
    eventsFile: join(dir, "events.jsonl"),
  };
}

/**
 * Find the most recent *previous* session directory — i.e. the newest folder
 * in `.omega/sessions/` that is not `currentDir`.
 *
 * Returns the events.jsonl path inside that folder, or `null` if no previous
 * session exists.
 *
 * Used by the terminal UI at startup to warn about prior session crashes.
 */
export async function findPreviousEventsFile(
  currentDir: string
): Promise<string | null> {
  let entries: string[];
  try {
    entries = await readdir(SESSIONS_ROOT);
  } catch {
    return null; // .omega/sessions/ doesn't exist yet
  }

  // Filter to directories that look like session dirs (timestamp pattern)
  // and exclude the current session's dir name.
  const currentDirName = currentDir.split("/").pop() ?? currentDir;
  const sessionDirs = entries
    .filter((e) => e !== currentDirName && /^\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}$/.test(e))
    .sort(); // lexicographic = chronological for ISO-format names

  if (sessionDirs.length === 0) return null;

  const mostRecent = sessionDirs[sessionDirs.length - 1];
  return join(SESSIONS_ROOT, mostRecent, "events.jsonl");
}
