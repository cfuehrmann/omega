/**
 * System prompt — Part 3: Optional append content.
 *
 * If the project contains a `.omega/system-prompt-append.md` file, its
 * contents are appended verbatim to the system prompt at session start.
 * This is the generic opt-in mechanism for injecting persistent
 * project-specific content (e.g. architecture notes, key rules, or any
 * other operator-maintained text) into every API call.
 *
 * The file is project-owned and source-controlled. It is never written
 * automatically — only by the operator or by an explicit compaction command.
 * If the file is absent, nothing is appended and the system prompt is
 * unchanged — making this safe to use when Omega operates on foreign repos.
 *
 * Path: <cwd>/.omega/system-prompt-append.md
 */

import { readFile, writeFile, mkdir } from "fs/promises";
import { join, dirname } from "path";

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

/**
 * Return the system-prompt-append path for the given project directory.
 * The file lives at .omega/system-prompt-append.md inside the project,
 * so it travels with the repo and is under source control.
 */
export function systemPromptAppendPath(cwd: string = process.cwd()): string {
  return join(cwd, ".omega", "system-prompt-append.md");
}

// ---------------------------------------------------------------------------
// File I/O
// ---------------------------------------------------------------------------

/**
 * Read the append content from disk.
 * Returns null if the file does not exist.
 */
export async function readSystemPromptAppend(
  path: string = systemPromptAppendPath()
): Promise<string | null> {
  try {
    return await readFile(path, "utf-8");
  } catch (err: any) {
    if (err.code === "ENOENT") return null;
    throw err;
  }
}

/**
 * Write the append content to disk.
 * Creates parent directories as needed.
 */
export async function writeSystemPromptAppend(
  content: string,
  path: string = systemPromptAppendPath()
): Promise<void> {
  await mkdir(dirname(path), { recursive: true });
  await writeFile(path, content, "utf-8");
}


