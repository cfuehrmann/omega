/**
 * Diagnostic snapshot writer.
 *
 * When a hard API error occurs (non-retryable, session-breaking), call
 * `writeDiagnostic()` to capture the full context at the moment of failure:
 * error body, request payload, conversation history, and a plain-English
 * summary. The file is written to `diagnosis/` at the repo root and persists across
 * sessions so the next Omega instance can read it with hard data rather than
 * reconstructing from memory.
 *
 * At session start, `checkDiagnostics()` returns any existing diagnosis files
 * so the UI can warn the operator immediately.
 */

import { writeFile, mkdir, readdir } from "fs/promises";
import { join } from "path";

export interface DiagnosticData {
  /** One-line human-readable summary of what went wrong. */
  summary: string;
  /** The error message / body from the API (verbatim). */
  errorMessage: string;
  /** HTTP status code if available. */
  httpStatus?: number;
  /** Provider name ("anthropic" | "openai"). */
  provider: string;
  /** Model that was active when the error occurred. */
  model: string;
  /** API call number within the turn (1-based). */
  callNumber?: number;
  /** The exact `messages` array that was sent to the API. */
  requestMessages: unknown;
  /** System prompt blocks (without cache_control for readability). */
  systemBlocks?: unknown;
  /** Conversation history at the moment of failure (this.history). */
  history: unknown;
  /** Any additional structured context. */
  extra?: Record<string, unknown>;
}

const DIAGNOSIS_DIR = "diagnosis";

/**
 * Write a diagnostic snapshot file. Silently swallows any I/O errors —
 * the caller should never crash because of the diagnostic writer.
 */
export async function writeDiagnostic(data: DiagnosticData): Promise<string | null> {
  try {
    await mkdir(DIAGNOSIS_DIR, { recursive: true });

    const ts = new Date().toISOString().replace(/[:.]/g, "-").replace("Z", "Z");
    const filename = `${ts}.json`;
    const path = join(DIAGNOSIS_DIR, filename);

    const snapshot = {
      _omega_diagnostic: true,
      timestamp: new Date().toISOString(),
      summary: data.summary,
      provider: data.provider,
      model: data.model,
      httpStatus: data.httpStatus ?? null,
      callNumber: data.callNumber ?? null,
      errorMessage: data.errorMessage,
      // The exact messages array sent to the API — the most important artifact
      requestMessages: data.requestMessages,
      systemBlocks: data.systemBlocks ?? null,
      // Full in-memory history at moment of failure
      history: data.history,
      extra: data.extra ?? null,
      _instructions: [
        "Read this file at the start of a debugging session.",
        "requestMessages is what was literally sent to the API.",
        "history is the agent's in-memory conversation history.",
        "Compare them: are there orphaned tool_result blocks?",
        "Are tool_use IDs in assistant messages matched by tool_result IDs?",
        "Delete this file once the bug is diagnosed and fixed.",
      ],
    };

    await writeFile(path, JSON.stringify(snapshot, null, 2), "utf-8");
    return path;
  } catch {
    return null;
  }
}

/**
 * Return paths of any existing diagnosis files, sorted oldest-first.
 * Returns an empty array if the directory doesn't exist or is empty.
 */
export async function checkDiagnostics(): Promise<string[]> {
  try {
    const entries = await readdir(DIAGNOSIS_DIR);
    return entries
      .filter(e => e.endsWith(".json"))
      .sort()
      .map(e => join(DIAGNOSIS_DIR, e));
  } catch {
    return [];
  }
}
