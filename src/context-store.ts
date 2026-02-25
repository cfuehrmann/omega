/**
 * Append-only context store (Step 3a).
 *
 * Writes each MessageParam to a JSONL file as it is pushed to the agent's
 * history. Pure side-effect — no changes to agentic loop logic.
 */

import { appendFile, writeFile, mkdir } from "fs/promises";
import { dirname } from "path";
import type Anthropic from "@anthropic-ai/sdk";

/** Default path for the context file, relative to cwd. */
export const DEFAULT_CONTEXT_FILE = "sessions/context.jsonl";

/**
 * Append a single MessageParam to the context JSONL file.
 * Creates the file (and parent directories) if they don't exist.
 */
export async function appendContextMessage(
  msg: Anthropic.MessageParam,
  filePath: string = DEFAULT_CONTEXT_FILE
): Promise<void> {
  await mkdir(dirname(filePath), { recursive: true });
  await appendFile(filePath, JSON.stringify(msg) + "\n", "utf-8");
}

/**
 * Truncate the context file to empty.
 * Used before rewriting it (e.g. after /compact collapses history).
 * No-op if the file does not exist.
 */
export async function clearContextStore(
  filePath: string = DEFAULT_CONTEXT_FILE
): Promise<void> {
  try {
    await writeFile(filePath, "", "utf-8");
  } catch (err: any) {
    if (err.code === "ENOENT") return; // file doesn't exist — that's fine
    throw err;
  }
}
