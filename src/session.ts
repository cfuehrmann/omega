/**
 * Session persistence for Omega.
 *
 * Saves conversation history to disk so the agent can resume after a restart
 * (including the post-self-modification restart). This is the critical missing
 * piece for the self-improvement loop.
 *
 * Storage layout:
 *   ~/.local/share/omega/sessions/<sessionId>.json
 *
 * Each file is a JSON-serialised Session object. The file is overwritten on
 * every save so there is always exactly one file per session ID.
 *
 * Usage:
 *   import { saveSession, loadLatestSession, listSessions } from "./session.js";
 *
 *   // Save after each turn:
 *   await saveSession({ id, savedAt, model, history });
 *
 *   // On startup, check for a prior session:
 *   const prior = await loadLatestSession();
 *   if (prior) { ... offer to resume ... }
 */

import { readFile, writeFile, mkdir, readdir } from "fs/promises";
import { join } from "path";
import { homedir } from "os";
import { existsSync } from "fs";
import type { MessageParam } from "@anthropic-ai/sdk/resources/messages";

// --- Types ---

export interface Session {
  id: string;
  savedAt: string;   // ISO 8601
  model: string;
  history: MessageParam[];
}



// --- Helpers ---

function defaultDir(): string {
  return join(homedir(), ".local", "share", "omega", "sessions");
}

function sessionPath(id: string, dir: string): string {
  return join(dir, `${id}.json`);
}

async function ensureDir(dir: string): Promise<void> {
  await mkdir(dir, { recursive: true });
}

// --- Public API ---

/**
 * Save a session to disk. Overwrites any existing file for the same session ID.
 * @param session - The session to save.
 * @param dir - Optional override for the storage directory (used in tests).
 */
export async function saveSession(
  session: Session,
  dir: string = defaultDir()
): Promise<void> {
  await ensureDir(dir);
  const path = sessionPath(session.id, dir);
  await writeFile(path, JSON.stringify(session, null, 2), "utf-8");
}

/**
 * Load the most recently saved session.
 * Returns null if no sessions exist or the directory doesn't exist.
 * @param dir - Optional override for the storage directory (used in tests).
 */
export async function loadLatestSession(
  dir: string = defaultDir()
): Promise<Session | null> {
  if (!existsSync(dir)) return null;

  let files: string[];
  try {
    files = await readdir(dir);
  } catch {
    return null;
  }

  const jsonFiles = files.filter((f) => f.endsWith(".json"));
  if (jsonFiles.length === 0) return null;

  // Parse all sessions and sort by savedAt descending
  const sessions: Session[] = [];
  for (const file of jsonFiles) {
    try {
      const raw = await readFile(join(dir, file), "utf-8");
      const parsed = JSON.parse(raw) as Session;
      sessions.push(parsed);
    } catch {
      // Skip malformed files
    }
  }

  if (sessions.length === 0) return null;

  sessions.sort((a, b) => {
    return new Date(b.savedAt).getTime() - new Date(a.savedAt).getTime();
  });

  return sessions[0];
}


