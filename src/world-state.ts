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

// ---------------------------------------------------------------------------
// Defaults
// ---------------------------------------------------------------------------

export function defaultWorldStatePath(): string {
  return join(homedir(), ".local", "share", "omega", "world-state.md");
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
