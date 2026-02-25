/**
 * Append-only context store (Step 3a).
 *
 * Writes each MessageParam to a JSONL file as it is pushed to the agent's
 * history. Pure side-effect — no changes to agentic loop logic.
 */

import { appendFile, writeFile, mkdir, rename, unlink } from "fs/promises";
import { dirname } from "path";
import type Anthropic from "@anthropic-ai/sdk";

/** Default path for the context file, relative to cwd. */
export const DEFAULT_CONTEXT_FILE = "sessions/context.jsonl";

/**
 * Rotate `filePath` → `filePath.prev` (overwriting any existing .prev),
 * then create a fresh empty file at `filePath`.
 *
 * If `filePath` does not exist, just ensures the directory exists and
 * creates a fresh empty file (no rename needed).
 *
 * Used at session start so the previous session's data is preserved for
 * diagnostics while the current session starts clean.
 */
export async function rotateFile(filePath: string): Promise<void> {
  const prevPath = filePath + ".prev";
  await mkdir(dirname(filePath), { recursive: true });
  try {
    await rename(filePath, prevPath);
  } catch (err: any) {
    if (err.code !== "ENOENT") throw err;
    // file didn't exist — no rename needed, just fall through to create it
  }
  await writeFile(filePath, "", "utf-8");
}

/**
 * Append a single MessageParam to the context JSONL file.
 * Creates the file (and parent directories) if they don't exist.
 * Pass `null` to disable the write (used when running in test mode).
 */
export async function appendContextMessage(
  msg: Anthropic.MessageParam,
  filePath: string | null = DEFAULT_CONTEXT_FILE
): Promise<void> {
  if (filePath === null) return; // disabled — no-op
  await mkdir(dirname(filePath), { recursive: true });
  await appendFile(filePath, JSON.stringify(msg) + "\n", "utf-8");
}

/**
 * Rotate context.jsonl → context.jsonl.prev, then start fresh.
 * Called at session start. Preserves the previous session's context for
 * diagnostics. Pass `null` to disable (test isolation).
 *
 * Also accepts an explicit filePath for rewrites after /compact — in that
 * case it truncates in-place without rotating (rotation is startup-only).
 */
export async function clearContextStore(
  filePath: string | null = DEFAULT_CONTEXT_FILE,
  { rotate = true }: { rotate?: boolean } = {}
): Promise<void> {
  if (filePath === null) return; // disabled — no-op
  if (rotate) {
    await rotateFile(filePath);
  } else {
    try {
      await writeFile(filePath, "", "utf-8");
    } catch (err: any) {
      if (err.code === "ENOENT") return;
      throw err;
    }
  }
}
