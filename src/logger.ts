/**
 * Structured JSON-lines logger.
 *
 * Writes one JSON object per line to a log file. The log file is created
 * in ~/.local/share/omega/logs/ with a datestamped filename.
 *
 * Usage:
 *   import { logger } from "./logger.js";
 *   logger.info("some event", { key: "value" });
 *   logger.apiCall({ model, inputTokens, outputTokens, costUsd, ... });
 *
 * Log files are readable by the agent for self-analysis.
 */

import { appendFile, mkdir } from "fs/promises";
import { join, dirname } from "path";
import { homedir } from "os";

export type LogLevel = "debug" | "info" | "warn" | "error";

export interface LogEntry {
  ts: string;          // ISO 8601
  level: LogLevel;
  event: string;       // event type / name
  [key: string]: any;  // arbitrary structured fields
}

export interface ApiCallLog {
  model: string;
  inputTokens: number;
  outputTokens: number;
  costUsd: number;
  ttftMs: number | null;
  totalMs: number;
  toolCalls: string[];   // tool names used in this turn
  stopReason: string;
}

class Logger {
  private logPath: string;
  private initialized = false;
  private sessionId: string;

  constructor() {
    this.sessionId = `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
    const date = new Date().toISOString().slice(0, 10); // YYYY-MM-DD
    const logDir = join(homedir(), ".local", "share", "omega", "logs");
    this.logPath = join(logDir, `${date}.jsonl`);
  }

  private async ensureInit(): Promise<void> {
    if (this.initialized) return;
    const dir = dirname(this.logPath);
    await mkdir(dir, { recursive: true });
    this.initialized = true;
  }

  private async write(entry: LogEntry): Promise<void> {
    try {
      await this.ensureInit();
      await appendFile(this.logPath, JSON.stringify(entry) + "\n", "utf-8");
    } catch {
      // Never throw from logger — log failures are silently swallowed
      // to avoid crashing the agent over a log write issue
    }
  }

  private log(level: LogLevel, event: string, fields: Record<string, any> = {}): void {
    const entry: LogEntry = {
      ts: new Date().toISOString(),
      level,
      event,
      session: this.sessionId,
      ...fields,
    };
    // Fire and forget — don't await, don't block the caller
    this.write(entry).catch(() => {});
  }

  debug(event: string, fields?: Record<string, any>): void {
    this.log("debug", event, fields);
  }

  info(event: string, fields?: Record<string, any>): void {
    this.log("info", event, fields);
  }

  warn(event: string, fields?: Record<string, any>): void {
    this.log("warn", event, fields);
  }

  error(event: string, fields?: Record<string, any>): void {
    this.log("error", event, fields);
  }

  /** Log an API call turn with timing and token metrics. */
  apiCall(data: ApiCallLog): void {
    this.log("info", "api_call", data);
  }

  /** Log a tool execution. */
  toolExec(data: {
    name: string;
    autoApproved: boolean;
    approved: boolean;
    isError: boolean;
    durationMs: number;
  }): void {
    this.log("info", "tool_exec", data);
  }

  /** Log agent startup. */
  startup(data: { authMode: string; model: string }): void {
    this.log("info", "startup", data);
  }

  /** Log a self-modification attempt. */
  selfModify(data: {
    description: string;
    filesChanged: string[];
    testsPassed: boolean;
    committed: boolean;
    commitHash?: string;
    revertReason?: string;
  }): void {
    this.log("info", "self_modify", data);
  }

  getLogPath(): string {
    return this.logPath;
  }

  getSessionId(): string {
    return this.sessionId;
  }
}

// Singleton logger instance shared across the process
export const logger = new Logger();
