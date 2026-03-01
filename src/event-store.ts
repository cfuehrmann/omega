/**
 * Event persistence — append-only JSONL writer for the session events file.
 *
 * Each session writes to its own timestamped directory (see session-dir.ts),
 * so no file rotation is needed — every session starts with a fresh file.
 *
 * Naming authority: persisted names are the single source of truth. Do not
 * rename event types in events.jsonl to match stream-facing names — it is
 * always the other way around.
 */

import { appendFile, mkdir } from "fs/promises";
import { dirname } from "path";
import { assertNotProductionPath } from "./test-guard.js";
import type { OmegaEvent } from "./events.js";

export const DEFAULT_EVENTS_FILE = "sessions/events.jsonl";

// ---------------------------------------------------------------------------
// Persistence helpers
// ---------------------------------------------------------------------------

/**
 * Fields that are UI-only and must NOT be written to events.jsonl.
 * These fields exist on OmegaEvent variants for stream consumers but are
 * intentionally absent from the persisted record (content lives in context.jsonl).
 */
const UI_ONLY_FIELDS: Record<string, string[]> = {
  llm_call: ["request"],
  llm_response: ["content", "raw"],
  tool_call: ["input", "formatted"],
  tool_result: ["result", "formatted"],
};

/**
 * Strip UI-only fields before writing to disk.
 * Returns a plain object safe to JSON.stringify into events.jsonl.
 */
function toPersistedEvent(event: OmegaEvent): object {
  const uiOnly = UI_ONLY_FIELDS[(event as any).type] ?? [];
  if (uiOnly.length === 0) return event;
  const copy: any = { ...event };
  for (const field of uiOnly) {
    delete copy[field];
  }
  return copy;
}

/**
 * Append a single OmegaEvent to the events JSONL file.
 * Creates the file (and parent directories) if they don't exist.
 * UI-only fields are stripped before writing.
 * Pass `null` to disable the write (test isolation).
 */
export async function appendEvent(
  event: OmegaEvent,
  filePath: string | null = DEFAULT_EVENTS_FILE
): Promise<void> {
  if (filePath === null) return;
  assertNotProductionPath(filePath, "appendEvent");
  await mkdir(dirname(filePath), { recursive: true });
  await appendFile(filePath, JSON.stringify(toPersistedEvent(event)) + "\n", "utf-8");
}


