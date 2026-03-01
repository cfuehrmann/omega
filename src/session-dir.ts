/**
 * Session directory management (SESSION-1, SESSION-2).
 *
 * Each Omega session gets its own timestamped folder under `.omega/sessions/`
 * in the current working directory (SESSION-2: sessions live alongside the
 * project being worked on, not alongside the Omega source):
 *
 *   .omega/sessions/2025-07-04T14-32-05-a3f7c1b2/
 *     context.jsonl
 *     events.jsonl
 *
 * The folder name is an ISO 8601 datetime truncated to seconds (colons
 * replaced with hyphens for filesystem safety), followed by a hyphen and an
 * 8-char random hex suffix. The suffix makes every session directory globally
 * unique — no collisions even if two sessions start within the same second.
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

/** Root directory for e2e test session folders. Distinct from production sessions. */
export const TEST_SESSIONS_ROOT = ".omega/test-sessions";

/** Regex matching a session dir name — both old (no suffix) and new (with suffix) formats. */
const SESSION_DIR_RE = /^\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}(-[0-9a-f]{8})?$/;

/**
 * Generate a session folder name from the current timestamp plus a random
 * 8-char hex suffix for global uniqueness.
 * Format: `YYYY-MM-DDTHH-MM-SS-<hex8>`
 */
export function makeSessionDirName(now: Date = new Date()): string {
  // toISOString() → "2025-07-04T14:32:05.123Z"
  // Take the first 19 chars ("2025-07-04T14:32:05"), replace colons
  const ts = now.toISOString().slice(0, 19).replace(/:/g, "-");
  // 4 random bytes → 8 hex chars
  const suffix = Array.from(crypto.getRandomValues(new Uint8Array(4)))
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
  return `${ts}-${suffix}`;
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
 * Creates `<root>/<timestamp>/` if it doesn't exist, where root defaults to
 * `.omega/sessions` (production) but can be overridden — e.g. e2e tests pass
 * `.omega/test-sessions` so their sessions are clearly distinguishable from
 * production sessions and cannot be confused with them.
 *
 * Returns the paths; the caller passes them to Agent.
 */
export async function makeSessionDir(
  now: Date = new Date(),
  root: string = SESSIONS_ROOT,
): Promise<SessionPaths> {
  const dirName = makeSessionDirName(now);
  const dir = join(root, dirName);
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

  // Filter to directories that look like session dirs (timestamp pattern,
  // with or without the random hex suffix) and exclude the current session.
  const currentDirName = currentDir.split("/").pop() ?? currentDir;
  const sessionDirs = entries
    .filter((e) => e !== currentDirName && SESSION_DIR_RE.test(e))
    .sort(); // lexicographic = chronological for ISO-format names

  if (sessionDirs.length === 0) return null;

  const mostRecent = sessionDirs[sessionDirs.length - 1];
  return join(SESSIONS_ROOT, mostRecent, "events.jsonl");
}
