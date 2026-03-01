/**
 * System-prompt append file for Omega.
 *
 * When `.omega/system-prompt-append.md` exists in the project directory,
 * its contents are appended to the system prompt at session start.
 * This is the opt-in mechanism for injecting persistent project-specific
 * context (e.g. a world-state summary) into every API call.
 *
 * The file is project-owned and source-controlled. It is never written
 * automatically — only by the operator or by an explicit compaction command.
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
// Public API
// ---------------------------------------------------------------------------

/**
 * Read the system-prompt-append content from disk.
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
 * Write the system-prompt-append content to disk.
 * Creates parent directories as needed.
 */
export async function writeSystemPromptAppend(
  content: string,
  path: string = systemPromptAppendPath()
): Promise<void> {
  await mkdir(dirname(path), { recursive: true });
  await writeFile(path, content, "utf-8");
}
