/**
 * Diagnostic snapshot writer.
 *
 * When a hard API error occurs (non-retryable, session-breaking), call
 * `writeDiagnostic()` to capture the exact context at the moment of failure:
 * error body, request payload, conversation history, and a pointer to the
 * current omega.log file so post-mortem analysis has full event history.
 *
 * The file is written to `diagnosis/` at the repo root and persists across
 * sessions so the next Omega instance can read it with hard data rather than
 * reconstructing from memory.
 *
 * At session start, `checkDiagnostics()` returns any existing diagnosis files
 * so the UI can warn the operator immediately.
 *
 * Call `flushLog()` from logger.ts BEFORE calling writeDiagnostic() so that
 * the log file referenced by the snapshot is already up to date on disk.
 */

import { writeFile, mkdir, readdir } from "fs/promises";
import { join } from "path";

// ---------------------------------------------------------------------------
// DiagnosticData
// ---------------------------------------------------------------------------

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

const DEFAULT_DIAGNOSIS_DIR = "diagnosis";

// ---------------------------------------------------------------------------
// writeDiagnostic — write a snapshot capturing the error context
// ---------------------------------------------------------------------------

/**
 * Write a diagnostic snapshot file.
 *
 * Flush the log BEFORE calling this (call flushLog() from logger.ts) so
 * the omega.log referenced by this snapshot is already complete on disk.
 *
 * @param data      Diagnostic fields (error, request, history…)
 * @param diagDir   Override the output directory (null = disabled, used in tests)
 *
 * Returns the written path, or null if the write failed or was disabled.
 * Errors are swallowed so the caller never crashes because of the diagnostic writer.
 */
export async function writeDiagnostic(
  data: DiagnosticData,
  diagDir: string | null | undefined = DEFAULT_DIAGNOSIS_DIR,
): Promise<string | null> {
  if (diagDir === null) return null;
  try {
    await mkdir(diagDir, { recursive: true });

    const ts = new Date().toISOString().replace(/[:.]/g, "-").replace("Z", "Z");
    const filename = `${ts}.json`;
    const path = join(diagDir, filename);

    const snapshot = {
      _omega_diagnostic: true,
      timestamp: new Date().toISOString(),
      summary: data.summary,
      provider: data.provider,
      model: data.model,
      httpStatus: data.httpStatus ?? null,
      callNumber: data.callNumber ?? null,
      errorMessage: data.errorMessage,

      // Pointer to the current session log — read this for the full event
      // timeline leading up to the error.
      logFile: "omega.log",

      // The exact messages array sent to the API — the most important artifact.
      // This is an ephemeral, per-call value never stored anywhere else.
      requestMessages: data.requestMessages,
      systemBlocks: data.systemBlocks ?? null,

      // Full in-memory history at moment of failure
      history: data.history,
      extra: data.extra ?? null,
      _instructions: [
        "Read this file at the start of a debugging session.",
        "logFile points to omega.log — read it for the full event timeline.",
        "requestMessages is what was literally sent to the API (ephemeral — only here).",
        "history is the agent's in-memory conversation history at time of failure.",
        "Compare requestMessages vs history: are there orphaned tool_result blocks?",
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

// ---------------------------------------------------------------------------
// checkDiagnostics
// ---------------------------------------------------------------------------

/**
 * Return paths of any existing diagnosis files, sorted oldest-first.
 * Returns an empty array if the directory doesn't exist or is empty.
 */
export async function checkDiagnostics(): Promise<string[]> {
  try {
    const entries = await readdir(DEFAULT_DIAGNOSIS_DIR);
    return entries
      .filter(e => e.endsWith(".json"))
      .sort()
      .map(e => join(DEFAULT_DIAGNOSIS_DIR, e));
  } catch {
    return [];
  }
}
