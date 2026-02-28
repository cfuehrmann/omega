/**
 * Append-only context store (Step 3a / 3e-iii).
 *
 * Writes each MessageParam to a JSONL file as it is pushed to the agent's
 * history. Since Step 3e-iii, each record is augmented with:
 *   - `ts`   — ISO timestamp of when the message was appended
 *   - `hash` — SHA-256(JSON of stored record) truncated to 8 hex chars,
 *              serving as a content-addressed primary key.
 *
 * The hash is computed from the full stored record (including `ts`) so
 * that identical message content sent at different times gets different
 * hashes. This is the "view hash" used by `llm_call` events to cross-
 * reference which messages were actually sent to the LLM.
 */

import { appendFile, writeFile, mkdir, rename, unlink } from "fs/promises";
import { dirname } from "path";
import { assertNotProductionPath } from "./test-guard.js";
import type Anthropic from "@anthropic-ai/sdk";

// ---------------------------------------------------------------------------
// Hash utilities (Step 3e-iii)
// ---------------------------------------------------------------------------

/**
 * Compute a SHA-256 hash of the given string and return the first 8 hex
 * characters. Used to generate stable, compact primary keys for context
 * records.
 *
 * Uses the Web Crypto API (available in Bun and modern Node).
 */
async function sha256hex8(input: string): Promise<string> {
  const data = new TextEncoder().encode(input);
  const buf = await crypto.subtle.digest("SHA-256", data);
  const hex = Array.from(new Uint8Array(buf))
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
  return hex.slice(0, 8);
}

/**
 * The on-disk shape of a context record (what actually gets written to
 * context.jsonl). Extends MessageParam with persistence metadata.
 */
export interface ContextRecord {
  /** Content-addressed primary key: SHA-256(JSON of this record) truncated to 8 hex chars. */
  hash: string;
  /** ISO timestamp when this message was appended. */
  ts: string;
  /** Original MessageParam fields. */
  role: "user" | "assistant";
  content: Anthropic.MessageParam["content"];
}

/** Default path for the context file, relative to cwd. */
const DEFAULT_CONTEXT_FILE = "sessions/context.jsonl";

/**
 * Derive the "previous" rotation path for a given file path.
 *
 * The `.prev` marker is inserted before the last extension so that the
 * rotated file retains a recognised extension (e.g. for syntax highlighting
 * in editors):
 *
 *   context.jsonl  → context.prev.jsonl
 *   events.jsonl   → events.prev.jsonl
 *   noext          → noext.prev
 */
export function prevPath(filePath: string): string {
  const lastDot = filePath.lastIndexOf(".");
  if (lastDot === -1) return filePath + ".prev";
  return filePath.slice(0, lastDot) + ".prev" + filePath.slice(lastDot);
}

/**
 * Rotate `filePath` → `prevPath(filePath)` (overwriting any existing prev),
 * then create a fresh empty file at `filePath`.
 *
 * If `filePath` does not exist, just ensures the directory exists and
 * creates a fresh empty file (no rename needed).
 *
 * Used at session start so the previous session's data is preserved for
 * diagnostics while the current session starts clean.
 */
export async function rotateFile(filePath: string): Promise<void> {
  const prev = prevPath(filePath);
  await mkdir(dirname(filePath), { recursive: true });
  try {
    await rename(filePath, prev);
  } catch (err: any) {
    if (err.code !== "ENOENT") throw err;
    // file didn't exist — no rename needed, just fall through to create it
  }
  await writeFile(filePath, "", "utf-8");
}

/**
 * Compute the hash and build the ContextRecord for a message, but do NOT
 * write it to disk. This lets the caller get the hash synchronously (after
 * awaiting the hash computation) while deferring or skipping the I/O.
 *
 * Hash input: JSON of `{ ts, role, content }`. Including `ts` prevents
 * collisions between identical messages sent at different times.
 */
export async function buildContextRecord(
  msg: Anthropic.MessageParam
): Promise<ContextRecord> {
  const ts = new Date().toISOString();
  const recordWithoutHash = { ts, role: msg.role, content: msg.content };
  const hash = await sha256hex8(JSON.stringify(recordWithoutHash));
  return { hash, ...recordWithoutHash };
}

/**
 * Append a single MessageParam to the context JSONL file.
 * Creates the file (and parent directories) if they don't exist.
 * Pass `null` to disable the write (used when running in test mode).
 *
 * Since Step 3e-iii the written record is augmented with `ts` and `hash`
 * fields. The hash is computed from the full JSON of the record (including
 * `ts`) so identical content sent at different times gets different hashes.
 *
 * Returns the 8-char hex hash of the stored record so the caller can
 * reference it in `llm_call` events without re-reading the file.
 */
export async function appendContextMessage(
  msg: Anthropic.MessageParam,
  filePath: string | null = DEFAULT_CONTEXT_FILE
): Promise<string> {
  const record = await buildContextRecord(msg);

  if (filePath !== null) {
    assertNotProductionPath(filePath, "appendContextMessage");
    await mkdir(dirname(filePath), { recursive: true });
    await appendFile(filePath, JSON.stringify(record) + "\n", "utf-8");
  }

  return record.hash;
}

/**
 * Rotate context.jsonl → context.prev.jsonl, then start fresh.
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
  assertNotProductionPath(filePath, "clearContextStore");
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
