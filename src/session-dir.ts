/**
 * Session directory management (SESSION-1, SESSION-2).
 *
 * Each Omega session gets its own timestamped folder under `.omega/sessions/`
 * in the current working directory (SESSION-2: sessions live alongside the
 * project being worked on, not alongside the Omega source):
 *
 *   .omega/sessions/2025-07-04T14-32-05-123-a3f7c1b2/
 *     context.jsonl
 *     events.jsonl
 *
 * The folder name is an ISO 8601 datetime at millisecond precision (colons
 * and the decimal point replaced with hyphens for filesystem safety), followed
 * by a hyphen and an 8-char random hex suffix. Millisecond precision means
 * that `ls`-by-name ordering matches chronological ordering even for sessions
 * started within the same second. The suffix provides the final uniqueness
 * guarantee for the rare case of two sessions starting at the exact same
 * millisecond.
 *
 * Benefits:
 *   - Sessions are co-located with the project — can be committed to the
 *     project's own VCS if the operator chooses (no automatic .gitignore).
 *   - No rotation / `.prev` file management needed — every session starts clean.
 *   - Old sessions are preserved indefinitely (until explicitly pruned).
 *   - Folders can be renamed to human-readable names (SESSION-5).
 *   - `.omega/` namespace leaves room for authored artefacts (e.g. system-prompt-append.md).
 */

import { mkdir, readdir, readFile, writeFile } from "fs/promises";
import { join } from "path";

/** Root directory for all session folders. Relative to cwd (SESSION-2). */
export const SESSIONS_ROOT = ".omega/sessions";

// ---------------------------------------------------------------------------
// Session metadata
// ---------------------------------------------------------------------------

/** Name of the metadata file inside every session folder. */
export const SESSION_METADATA_FILE = "session.jsonc";

/**
 * Metadata for a session. All fields are optional — a session can exist
 * with no metadata (just an empty `{}`).
 *
 * Written as JSONC so humans can add comments when editing manually.
 * Programmatic writes use plain JSON (a valid subset of JSONC).
 */
export interface SessionMetadata {
  /** Short human-readable label. Not unique — multiple sessions may share a name. */
  name?: string;
  /** Free-text description. Searchable. */
  description?: string;
  /**
   * Relative folder name (within SESSIONS_ROOT) of the session this one
   * continues. Relative for portability — moving the project does not break
   * the lineage chain.
   */
  continuationOf?: string;
}

/** Strip single-line and block comments for JSONC parsing. */
function stripJsoncComments(text: string): string {
  return text
    .replace(/\/\/[^\n]*/g, "")
    .replace(/\/\*[\s\S]*?\*\//g, "");
}

/**
 * Read session metadata from `session.jsonc` inside `dir`.
 * Returns `{}` if the file is absent or unparseable.
 */
export async function readSessionMetadata(dir: string): Promise<SessionMetadata> {
  try {
    const raw = await readFile(join(dir, SESSION_METADATA_FILE), "utf-8");
    const parsed = JSON.parse(stripJsoncComments(raw));
    if (parsed !== null && typeof parsed === "object" && !Array.isArray(parsed)) {
      return parsed as SessionMetadata;
    }
  } catch {
    // absent or malformed — treat as empty
  }
  return {};
}

/**
 * Write (overwrite) session metadata to `session.jsonc` inside `dir`.
 * Only writes the keys that are present in `metadata` — undefined values
 * are omitted so the file stays minimal.
 */
export async function writeSessionMetadata(
  dir: string,
  metadata: SessionMetadata,
): Promise<void> {
  const clean: Record<string, string> = {};
  if (metadata.name !== undefined) clean.name = metadata.name;
  if (metadata.description !== undefined) clean.description = metadata.description;
  if (metadata.continuationOf !== undefined) clean.continuationOf = metadata.continuationOf;
  await writeFile(join(dir, SESSION_METADATA_FILE), JSON.stringify(clean, null, 2) + "\n", "utf-8");
}

/**
 * Merge `patch` into the existing metadata for `dir`.
 * Undefined patch values leave the existing field unchanged.
 */
export async function updateSessionMetadata(
  dir: string,
  patch: Partial<SessionMetadata>,
): Promise<void> {
  const existing = await readSessionMetadata(dir);
  await writeSessionMetadata(dir, { ...existing, ...patch });
}

/** Root directory for e2e test session folders. Distinct from production sessions. */
export const TEST_SESSIONS_ROOT = ".omega/test-sessions";

/**
 * Regex matching a session dir name. Exported for use in server.ts session listing.
 * Accepts all three historical formats:
 *   - old:         YYYY-MM-DDTHH-MM-SS                   (second precision, no suffix)
 *   - v2:          YYYY-MM-DDTHH-MM-SS-<hex8>             (second precision + suffix)
 *   - current:     YYYY-MM-DDTHH-MM-SS-mmm-<hex8>         (millisecond precision + suffix)
 */
export const SESSION_DIR_RE =
  /^\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}(-\d{3})?(-[0-9a-f]{8})?$/;

/**
 * Generate a session folder name from the current timestamp (millisecond
 * precision) plus a random 8-char hex suffix for global uniqueness.
 * Format: `YYYY-MM-DDTHH-MM-SS-mmm-<hex8>`
 *
 * Millisecond precision ensures that `ls`-by-name ordering matches
 * chronological ordering even for sessions started within the same second.
 */
export function makeSessionDirName(now: Date = new Date()): string {
  // toISOString() → "2025-07-04T14:32:05.123Z"
  // Take first 23 chars: "2025-07-04T14:32:05.123"
  // Replace colons and the decimal point with hyphens → "2025-07-04T14-32-05-123"
  const ts = now.toISOString().slice(0, 23).replace(/[:.]/g, "-");
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
  const contextFile = join(dir, "context.jsonl");
  const eventsFile = join(dir, "events.jsonl");
  // Create all three files eagerly so the session directory is complete from birth.
  // flag "wx" = create-only (safe if files somehow already exist from a race).
  await writeFile(contextFile, "", { flag: "wx" });
  await writeFile(eventsFile, "", { flag: "wx" });
  await writeFile(join(dir, SESSION_METADATA_FILE), "{}\n", { flag: "wx" });
  return { dir, contextFile, eventsFile };
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

  const mostRecent = sessionDirs[sessionDirs.length - 1]!;
  return join(SESSIONS_ROOT, mostRecent, "events.jsonl");
}
