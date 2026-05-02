/**
 * Pure helpers extracted from the now-deleted `src/web/server.ts`.
 *
 * The production server is the Rust `omega-server` binary; these helpers
 * are still used by:
 *
 *  - `e2e/fixtures/test-server.ts` — the in-test mock fixture used by the
 *    `chromium` Playwright project (which never starts a real Agent).
 *  - `src/web/session-resilience.test.ts` — exercises `closeOpenTurn` /
 *    `shouldLogEvent` directly.
 *  - `src/web/file-completion.test.ts` — exercises `listFilesForCompletion`.
 *
 * Keeping them here means we don't need to ship the historical TypeScript
 * server just to satisfy a handful of unit tests.
 */

import { join } from "path";
import { readdir } from "fs/promises";
import { now } from "../iso-timestamp.js";

// ---------------------------------------------------------------------------
// History replay — pure helpers
// ---------------------------------------------------------------------------

/**
 * Events that should not be replayed to a reconnecting browser.
 *   ready     — server-sent after history batch; meaningless to replay
 *   text      — streaming text fragments; assembled response is in context.jsonl
 */
const REPLAY_EXCLUDE = new Set(["ready", "text"]);

/**
 * Returns true if the event should be included in history replay.
 * Mirrors the set of events Agent persists to events.jsonl — streaming
 * text fragments and transient transport signals are excluded.
 */
export function shouldLogEvent(event: object): boolean {
  return "type" in event && !REPLAY_EXCLUDE.has((event as { type: string }).type);
}

/**
 * Ensures the event log has no open (un-closed) turn at the tail.
 *
 * A turn is "open" when a `user_message` appears after the last
 * `turn_end` / `turn_interrupted` marker — the server crashed mid-turn.
 * Replaying such a log leaves `streaming = true` in the client with no
 * recovery path. We append a synthetic `turn_interrupted` to close it.
 *
 * Returns a new array (does not mutate the input).
 */
export function closeOpenTurn(log: object[]): object[] {
  for (let i = log.length - 1; i >= 0; i--) {
    const entry = log[i]!;
    if (!("type" in entry)) continue;
    const t = (entry as { type: string }).type;
    if (t === "turn_end" || t === "turn_interrupted") return log;
    if (t === "user_message") {
      return [...log, { type: "turn_interrupted", time: now() }];
    }
  }
  return log;
}

// ---------------------------------------------------------------------------
// File completion — used by the chromium-project test-server fixture
// ---------------------------------------------------------------------------

/**
 * List filesystem entries whose names match a typed path prefix, for the
 * @-completion dropdown. `prefix` is the text after `@` in the user's input,
 * e.g. "src/web/cl" or "/home/carsten/.".
 *
 * Returns up to 50 paths (relative or absolute), with directories suffixed
 * with "/" and sorted dirs-first then alphabetically.
 */
export async function listFilesForCompletion(prefix: string, cwd = process.cwd()): Promise<string[]> {
  const lastSlash = prefix.lastIndexOf("/");
  const dir    = lastSlash >= 0 ? prefix.slice(0, lastSlash + 1) : "";
  const filter = lastSlash >= 0 ? prefix.slice(lastSlash + 1)    : prefix;
  const isAbs  = prefix.startsWith("/");

  const targetDir = isAbs
    ? (dir || "/")
    : join(cwd, dir || ".");

  let entries: { name: string; isDir: boolean }[];
  try {
    const dirents = await readdir(targetDir, { withFileTypes: true });
    entries = dirents
      .filter(d => !filter || d.name.startsWith(filter))
      .map(d => ({ name: d.name, isDir: d.isDirectory() }));
  } catch {
    return [];
  }

  entries.sort((a, b) => {
    const ad = a.isDir ? 0 : 1;
    const bd = b.isDir ? 0 : 1;
    if (ad !== bd) return ad - bd;
    return a.name.localeCompare(b.name);
  });

  return entries.slice(0, 50).map(e => dir + e.name + (e.isDir ? "/" : ""));
}
