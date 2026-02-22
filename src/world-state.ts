/**
 * Persistent world state for Omega.
 *
 * The world state is a plain-text document summarising everything that has
 * happened across all prior sessions. It is injected into the system prompt
 * at session start and updated (by compaction) when each session ends.
 *
 * Default path: ~/.local/share/omega/world-state.md
 */

import { readFile, writeFile, mkdir } from "fs/promises";
import { join, dirname } from "path";
import { homedir } from "os";
import { createHash } from "crypto";

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

/**
 * Return a world-state path that is specific to the given working directory.
 * Two different directories → two different files, so project switching is free.
 *
 * Format: ~/.local/share/omega/world-<slug>-<hash6>.md
 *   slug = last path component, sanitised
 *   hash = first 6 hex chars of SHA-256 of the full path (collision avoidance)
 */
export function projectWorldStatePath(cwd: string = process.cwd()): string {
  const slug = cwd.replace(/[^a-zA-Z0-9]/g, "-").replace(/-+/g, "-").replace(/^-|-$/g, "").slice(-30) || "root";
  const hash = createHash("sha256").update(cwd).digest("hex").slice(0, 6);
  const filename = `world-${slug}-${hash}.md`;
  return join(homedir(), ".local", "share", "omega", filename);
}

/**
 * @deprecated Use projectWorldStatePath() instead.
 * Kept for backward compatibility in tests that pass explicit paths.
 */
export function defaultWorldStatePath(): string {
  return projectWorldStatePath();
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/**
 * Read the world state from disk.
 * Returns null if the file does not exist.
 */
export async function readWorldState(
  path: string = defaultWorldStatePath()
): Promise<string | null> {
  try {
    return await readFile(path, "utf-8");
  } catch (err: any) {
    if (err.code === "ENOENT") return null;
    throw err;
  }
}

/**
 * Write the world state to disk. Creates parent directories as needed.
 */
export async function writeWorldState(
  content: string,
  path: string = defaultWorldStatePath()
): Promise<void> {
  await mkdir(dirname(path), { recursive: true });
  await writeFile(path, content, "utf-8");
}
