/**
 * Structured JSON-lines logger backed by pino.
 *
 * Every log entry has a `kind` field: either `"message"` (a sender→receiver
 * communication event) or `"infra"` (internal lifecycle bookkeeping).
 *
 * See docs/log-taxonomy.md for the full authoritative specification.
 *
 * Writes one JSON object per line to omega.log at the repo root.
 * On startup: renames omega.log → omega.prev.log (if it exists), then creates
 * a fresh omega.log for the current session.  This keeps exactly two log
 * files: the current session and the previous one.
 *
 * The log level is controlled by the OMEGA_LOG_LEVEL environment variable
 * (default: "debug").  All levels are written; pino filters internally.
 *
 * Log writes are synchronous — each entry is written to disk immediately.
 * Our log volume (human-paced typing + LLM responses) is far too low for
 * async buffering to matter, and sync writes are simpler and more reliable.
 *
 * Usage:
 *   import { logger, makeLogEntry } from "./logger.js";
 *   logger.info(makeLogEntry("infra", { event: "turn_end", ... }));
 *   logger.debug(makeLogEntry("message", { sender: "agent", receiver: "llm", message: "call", ... }));
 */

import pino from "pino";
import { renameSync, existsSync } from "fs";

// ---------------------------------------------------------------------------
// Log taxonomy types  (docs/log-taxonomy.md)
// ---------------------------------------------------------------------------

/**
 * A message entry represents a real sender→receiver communication event.
 * All seven message types in the taxonomy use this shape.
 */
export interface MessageEntry {
  kind: "message";
  sender: "agent" | "user" | "llm";
  receiver: "agent" | "user" | "llm";
  message: "call" | "response" | "tool_call" | "tool_result" | "compact_turn" | "compact_session";
  [key: string]: unknown;
}

/**
 * An infra entry captures internal lifecycle and aggregate bookkeeping.
 * It has no sender/receiver — it is not a communication between parties.
 */
export interface InfraEntry {
  kind: "infra";
  event: string;
  [key: string]: unknown;
}

/** Discriminated union of all valid log entry shapes. */
export type LogEntry = MessageEntry | InfraEntry;

// ---------------------------------------------------------------------------
// makeLogEntry — shape factory
// ---------------------------------------------------------------------------

/**
 * Build a taxonomy-compliant message log entry.
 * All other fields from `fields` are spread alongside the required ones.
 *
 * @example
 *   makeLogEntry("message", { sender: "agent", receiver: "llm", message: "call", model })
 */
export function makeLogEntry(
  kind: "message",
  fields: Omit<MessageEntry, "kind">
): MessageEntry;

/**
 * Build a taxonomy-compliant infra log entry.
 *
 * @example
 *   makeLogEntry("infra", { event: "turn_end", inputTokens, outputTokens, costUsd })
 */
export function makeLogEntry(
  kind: "infra",
  fields: Omit<InfraEntry, "kind">
): InfraEntry;

export function makeLogEntry(
  kind: "message" | "infra",
  fields: Record<string, unknown>
): LogEntry {
  return { kind, ...fields } as LogEntry;
}

// ---------------------------------------------------------------------------
// Log file selection & rotation
// ---------------------------------------------------------------------------

// When OMEGA_LOG_FILE is set (e.g. by the test-setup preload), use that path
// directly and skip rotation — tests must not touch omega.log.
const PROD_LOG_FILE = "omega.log";
const PREV_LOG_FILE = "omega.prev.log";

const LOG_FILE: string = process.env.OMEGA_LOG_FILE ?? PROD_LOG_FILE;
const IS_PRODUCTION_LOG = LOG_FILE === PROD_LOG_FILE;

/**
 * Rotate logs: rename omega.log → omega.prev.log (overwriting).
 * Only called in production (when OMEGA_LOG_FILE is not set).
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

if (IS_PRODUCTION_LOG) {
  rotateLogs();
}

// ---------------------------------------------------------------------------
// Pino destination — synchronous, no buffering needed at our log volume
// ---------------------------------------------------------------------------

const dest = pino.destination({
  dest: LOG_FILE,
  sync: true,   // synchronous writes — no buffering, no flush ceremony
  flags: "a",   // we already rotated above; append from here
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
// Thin wrapper — two call-site APIs:
//
//   1. Typed: logger.debug(makeLogEntry("message", { ... }))
//   2. Legacy: logger.debug("event_name", { fields }) — kept for error/warn sites
//              that don't yet have taxonomy-compliant shapes
//
// Both forms produce valid JSON in omega.log.
// ---------------------------------------------------------------------------

type LogArg = LogEntry | string;

function buildPinoArg(eventOrEntry: LogArg, fields?: Record<string, unknown>): Record<string, unknown> {
  if (typeof eventOrEntry === "string") {
    // Legacy form: ("event_name", { fields })
    // Preserves backwards compat for warn/error sites not yet on taxonomy.
    return { event: eventOrEntry, ...fields };
  }
  // Typed form: the LogEntry is already a plain object — pass directly.
  return eventOrEntry as Record<string, unknown>;
}

/**
 * Structured logger.
 *
 * Preferred call-site API (taxonomy-compliant):
 *   logger.debug(makeLogEntry("message", { sender, receiver, message, ...fields }))
 *   logger.info(makeLogEntry("infra", { event: "turn_end", ...fields }))
 *
 * Legacy call-site API (warn/error sites, backwards-compat):
 *   logger.warn("event_name", { key: value })
 *   logger.error("event_name", { key: value })
 */
export const logger = {
  debug(eventOrEntry: LogArg, fields?: Record<string, unknown>): void {
    _pino.debug(buildPinoArg(eventOrEntry, fields));
  },
  info(eventOrEntry: LogArg, fields?: Record<string, unknown>): void {
    _pino.info(buildPinoArg(eventOrEntry, fields));
  },
  warn(eventOrEntry: LogArg, fields?: Record<string, unknown>): void {
    _pino.warn(buildPinoArg(eventOrEntry, fields));
  },
  error(eventOrEntry: LogArg, fields?: Record<string, unknown>): void {
    _pino.error(buildPinoArg(eventOrEntry, fields));
  },
};

// ---------------------------------------------------------------------------
// flushLog — retained as a no-op shim; call sites will be cleaned up later
// ---------------------------------------------------------------------------

/**
 * No-op. Previously flushed an async buffer; now that writes are synchronous
 * there is nothing to flush. Kept so existing call sites continue to compile.
 */
export function flushLog(): void {
  // no-op — writes are synchronous
}

export function getLogFile(): string {
  return LOG_FILE;
}

// ---------------------------------------------------------------------------
// Typed convenience wrapper for session startup
// ---------------------------------------------------------------------------

/**
 * Log agent startup — infra entry.
 */
export function startup(data: { authMode: string; model: string }): void {
  logger.info(makeLogEntry("infra", { event: "startup", ...data }));
}
