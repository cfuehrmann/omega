/**
 * Structured JSON-lines logger backed by pino.
 *
 * Writes one JSON object per line to omega.log at the repo root.
 * On startup: renames omega.log → omega.prev.log (if it exists), then creates
 * a fresh omega.log for the current session.  This keeps exactly two log
 * files: the current session and the previous one.
 *
 * The log level is controlled by the OMEGA_LOG_LEVEL environment variable
 * (default: "debug").  All levels are written; pino filters internally.
 *
 * The buffer flushes automatically when it reaches 1 KB or every 5 seconds
 * (whichever comes first), via SonicBoom's built-in periodicFlush.
 * Call flushLog() before writing a diagnostic snapshot or exiting to ensure
 * all buffered log lines reach disk immediately.
 *
 * Usage:
 *   import { logger, flushLog } from "./logger.js";
 *   logger.info("api_call_complete", { model, inputTokens, outputTokens });
 *   // on error:
 *   flushLog();
 *   await writeDiagnostic({ ... });
 */

import pino from "pino";
import { renameSync, existsSync } from "fs";

// ---------------------------------------------------------------------------
// Log file rotation — previous session kept as omega.prev.log
// ---------------------------------------------------------------------------

const LOG_FILE = "omega.log";
const PREV_LOG_FILE = "omega.prev.log";

/**
 * Rotate logs: rename omega.log → omega.prev.log (overwriting).
 * Called once at module load time so each session starts with a fresh log.
 * Errors are silently swallowed — log rotation failure must not crash startup.
 */
function rotateLogs(): void {
  try {
    if (existsSync(LOG_FILE)) {
      renameSync(LOG_FILE, PREV_LOG_FILE);
    }
  } catch {
    // Rotation failure is non-fatal.
  }
}

rotateLogs();

// ---------------------------------------------------------------------------
// Pino destination — async (buffered), periodic auto-flush, sync-flushable
// ---------------------------------------------------------------------------

const dest = pino.destination({
  dest: LOG_FILE,
  sync: false,          // async hot path — no latency on log writes
  minLength: 1024,      // flush buffer when it reaches 1 KB (~5–10 messages)
  periodicFlush: 5000,  // also flush every 5 s regardless of buffer size
  flags: "a",           // we already rotated above; append from here
});

// ---------------------------------------------------------------------------
// Pino logger instance
// ---------------------------------------------------------------------------

const level = (process.env.OMEGA_LOG_LEVEL ?? "debug") as pino.Level;

const _pino = pino(
  {
    level,
    base: null,             // omit pid/hostname — not useful for our use-case
    timestamp: pino.stdTimeFunctions.isoTime,
  },
  dest,
);

// ---------------------------------------------------------------------------
// Thin wrapper — preserves call-site API: logger.info("event_name", { fields })
// ---------------------------------------------------------------------------

/**
 * Structured logger.  Call-site API:
 *   logger.info("event_name", { key: value })
 *   logger.warn("event_name", { key: value })
 *   logger.error("event_name", { key: value })
 *   logger.debug("event_name", { key: value })
 *
 * The event name is stored as the `event` field.  All other fields are merged
 * at the top level of the JSON log entry.
 */
export const logger = {
  debug(event: string, fields?: Record<string, unknown>): void {
    _pino.debug({ event, ...fields });
  },
  info(event: string, fields?: Record<string, unknown>): void {
    _pino.info({ event, ...fields });
  },
  warn(event: string, fields?: Record<string, unknown>): void {
    _pino.warn({ event, ...fields });
  },
  error(event: string, fields?: Record<string, unknown>): void {
    _pino.error({ event, ...fields });
  },
};

// ---------------------------------------------------------------------------
// flushLog — call before writing a diagnostic snapshot or exiting
// ---------------------------------------------------------------------------

/**
 * Synchronously flush all buffered log lines to omega.log.
 * Call this immediately before writing a diagnostic snapshot so that the
 * snapshot's logFile pointer refers to a file that is already up to date.
 * Also called on clean shutdown.
 */
export function flushLog(): void {
  try {
    dest.flushSync();
  } catch {
    // Flush failure is non-fatal.
  }
}

export function getLogFile(): string {
  return LOG_FILE;
}

// ---------------------------------------------------------------------------
// Typed convenience wrappers (preserve call-site compat with old Logger class)
// ---------------------------------------------------------------------------

/** Log agent startup. */
export function startup(data: { authMode: string; model: string }): void {
  logger.info("startup", data);
}

/** Log a tool execution. */
export function toolExec(data: {
  name: string;
  autoApproved: boolean;
  approved: boolean;
  isError: boolean;
  durationMs: number;
}): void {
  logger.info("tool_exec", data);
}

/** Log an API call turn with timing and token metrics. */
export function apiCall(data: {
  model: string;
  inputTokens: number;
  outputTokens: number;
  costUsd: number;
  ttftMs: number | null;
  totalMs: number;
  toolCalls: string[];
  stopReason: string;
}): void {
  logger.info("api_call", data);
}
