/**
 * Append-only context store.
 *
 * Writes each MessageParam to a JSONL file as it is pushed to the agent's
 * history. Each record is augmented with:
 *   - `time` — ISO timestamp of when the message was appended
 *   - `hash` — 6 random bytes encoded as 12 lowercase hex characters,
 *              serving as a unique primary key for the record.
 *
 * This is the "view hash" used by `llm_call` events to cross-reference
 * which messages were actually sent to the LLM.
 *
 * Each session writes to its own timestamped directory (see session-dir.ts),
 * so no file rotation is needed — every session starts with a fresh file.
 */

import { appendFile, mkdir } from "fs/promises";
import { dirname } from "path";
import { assertNotProductionPath } from "./test-guard.js";
import type Anthropic from "@anthropic-ai/sdk";
import { type ISOTimestamp, now } from "./iso-timestamp.js";
import { type ContextHash, randomHash } from "./context-hash.js";

/** Default path for the context JSONL file. Relative to cwd (SESSION-2). */
const DEFAULT_CONTEXT_FILE = ".omega/sessions/context.jsonl";

/**
 * The on-disk shape of a context record (what actually gets written to
 * context.jsonl). Extends MessageParam with persistence metadata.
 */
export interface ContextRecord {
  /** Unique primary key: 12 lowercase hex characters (6 random bytes). */
  hash: ContextHash;
  /** ISO timestamp when this message was appended. */
  time: ISOTimestamp;
  /** Original MessageParam fields. */
  role: "user" | "assistant";
  content: Anthropic.Beta.Messages.BetaMessageParam["content"];
}

/**
 * Build a ContextRecord for a message without writing it to disk.
 * Synchronous — the hash is generated from random bytes, not computed
 * from content, so no async work is needed.
 */
export function buildContextRecord(
  msg: Anthropic.Beta.Messages.BetaMessageParam
): ContextRecord {
  return { hash: randomHash(), time: now(), role: msg.role, content: msg.content };
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
 * Returns the 12-char hex hash of the stored record so the caller can
 * reference it in `llm_call` events without re-reading the file.
 */
export async function appendContextMessage(
  msg: Anthropic.Beta.Messages.BetaMessageParam,
  filePath: string | null = DEFAULT_CONTEXT_FILE
): Promise<ContextHash> {
  const record = buildContextRecord(msg);

  if (filePath !== null) {
    assertNotProductionPath(filePath, "appendContextMessage");
    await mkdir(dirname(filePath), { recursive: true });
    await appendFile(filePath, JSON.stringify(record) + "\n", "utf-8");
  }

  return record.hash;
}


