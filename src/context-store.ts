/**
 * Append-only context store.
 *
 * Writes each MessageParam to a JSONL file as it is pushed to the agent's
 * history. Each record is augmented with:
 *   - `time` — ISO timestamp of when the message was appended
 *   - `hash` — SHA-256(JSON of stored record) truncated to 8 hex chars,
 *              serving as a content-addressed primary key.
 *
 * The hash is computed from the full stored record (including `time`) so
 * that identical message content sent at different times gets different
 * hashes. This is the "view hash" used by `llm_call` events to cross-
 * reference which messages were actually sent to the LLM.
 *
 * Each session writes to its own timestamped directory (see session-dir.ts),
 * so no file rotation is needed — every session starts with a fresh file.
 */

import { appendFile, mkdir } from "fs/promises";
import { dirname } from "path";
import { assertNotProductionPath } from "./test-guard.js";
import type Anthropic from "@anthropic-ai/sdk";

/** Default path for the context JSONL file. Relative to cwd (SESSION-2). */
const DEFAULT_CONTEXT_FILE = ".omega/sessions/context.jsonl";

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
  time: string;
  /** Original MessageParam fields. */
  role: "user" | "assistant";
  content: Anthropic.Beta.Messages.BetaMessageParam["content"];
}

/**
 * Compute the hash and build the ContextRecord for a message, but do NOT
 * write it to disk. This lets the caller get the hash synchronously (after
 * awaiting the hash computation) while deferring or skipping the I/O.
 *
 * Hash input: JSON of `{ time, role, content }`. Including `time` prevents
 * collisions between identical messages sent at different times.
 */
export async function buildContextRecord(
  msg: Anthropic.Beta.Messages.BetaMessageParam
): Promise<ContextRecord> {
  const time = new Date().toISOString();
  const recordWithoutHash = { time, role: msg.role, content: msg.content };
  const hash = await sha256hex8(JSON.stringify(recordWithoutHash));
  return { hash, ...recordWithoutHash };
}

/**
 * Append a single MessageParam to the context JSONL file.
 * Creates the file (and parent directories) if they don't exist.
 * Pass `null` to disable the write (used when running in test mode).
 *
 * Since Step 3e-iii the written record is augmented with `time` and `hash`
 * fields. The hash is computed from the full JSON of the record (including
 * `time`) so identical content sent at different times gets different hashes.
 *
 * Returns the 8-char hex hash of the stored record so the caller can
 * reference it in `llm_call` events without re-reading the file.
 */
export async function appendContextMessage(
  msg: Anthropic.Beta.Messages.BetaMessageParam,
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


