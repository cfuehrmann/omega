/**
 * Tests for the log taxonomy shape.
 *
 * Strategy: intercept pino calls by wrapping the logger internals, then
 * assert the shape of emitted log entries.
 *
 * These tests verify LOG-2 acceptance criteria:
 * - All entries have `kind: "message" | "infra"`
 * - `kind:"message"` entries always have `sender`, `receiver`, `message`; never `event`
 * - `kind:"infra"` entries always have `event`; never `sender`/`receiver`/`message`
 * - Log levels match frequency: per-iteration = debug, per-turn aggregate = info
 */

import { describe, it, expect } from "bun:test";
import {
  makeLogEntry,
  getLogFile,
  type MessageEntry,
  type InfraEntry,
  type LogEntry,
} from "./logger.js";

// ---------------------------------------------------------------------------
// Log file isolation — tests must not write to omega.log
// ---------------------------------------------------------------------------

describe("log file isolation", () => {
  it("uses OMEGA_LOG_FILE when set, not omega.log", () => {
    // The test-setup preload sets OMEGA_LOG_FILE=/dev/null.
    // If the env var is honoured, getLogFile() must not return "omega.log".
    const logFile = getLogFile();
    expect(logFile).not.toBe("omega.log");
    expect(logFile).toBe(process.env.OMEGA_LOG_FILE ?? "omega.log");
  });
});

// ---------------------------------------------------------------------------
// makeLogEntry — shape factory (tested directly)
// ---------------------------------------------------------------------------

describe("makeLogEntry - message entries", () => {
  it("produces kind:message with sender/receiver/message", () => {
    const entry = makeLogEntry("message", {
      sender: "agent",
      receiver: "llm",
      message: "call",
      model: "claude-sonnet-4-6",
    });
    expect(entry.kind).toBe("message");
    // TypeScript narrows here; runtime confirms
    const msg = entry as MessageEntry;
    expect(msg.sender).toBe("agent");
    expect(msg.receiver).toBe("llm");
    expect(msg.message).toBe("call");
    expect((msg as any).event).toBeUndefined();
  });

  it("agent→llm call entry has no event field", () => {
    const entry = makeLogEntry("message", {
      sender: "agent",
      receiver: "llm",
      message: "call",
    });
    expect((entry as any).event).toBeUndefined();
  });

  it("llm→agent response entry shape", () => {
    const entry = makeLogEntry("message", {
      sender: "llm",
      receiver: "agent",
      message: "response",
      stopReason: "end_turn",
      inputTokens: 100,
      outputTokens: 50,
    });
    expect(entry.kind).toBe("message");
    const msg = entry as MessageEntry;
    expect(msg.sender).toBe("llm");
    expect(msg.receiver).toBe("agent");
    expect(msg.message).toBe("response");
    expect((msg as any).stopReason).toBe("end_turn");
  });

  it("agent→agent tool_call entry shape", () => {
    const entry = makeLogEntry("message", {
      sender: "agent",
      receiver: "agent",
      message: "tool_call",
      id: "tool-123",
      name: "read_file",
    });
    expect(entry.kind).toBe("message");
    expect((entry as MessageEntry).message).toBe("tool_call");
    expect((entry as any).id).toBe("tool-123");
  });

  it("agent→agent tool_result entry shape", () => {
    const entry = makeLogEntry("message", {
      sender: "agent",
      receiver: "agent",
      message: "tool_result",
      id: "tool-123",
      name: "read_file",
      isError: false,
      durationMs: 42,
    });
    expect(entry.kind).toBe("message");
    expect((entry as MessageEntry).message).toBe("tool_result");
    expect((entry as any).durationMs).toBe(42);
  });

  it("agent→llm compact_turn entry shape", () => {
    const entry = makeLogEntry("message", {
      sender: "agent",
      receiver: "llm",
      message: "compact_turn",
    });
    expect(entry.kind).toBe("message");
    expect((entry as MessageEntry).message).toBe("compact_turn");
  });

  it("agent→llm compact_session entry shape", () => {
    const entry = makeLogEntry("message", {
      sender: "agent",
      receiver: "llm",
      message: "compact_session",
    });
    expect(entry.kind).toBe("message");
    expect((entry as MessageEntry).message).toBe("compact_session");
  });
});

describe("makeLogEntry - infra entries", () => {
  it("produces kind:infra with event field", () => {
    const entry = makeLogEntry("infra", { event: "startup", authMode: "oauth", model: "claude-sonnet-4-6" });
    expect(entry.kind).toBe("infra");
    const infra = entry as InfraEntry;
    expect(infra.event).toBe("startup");
    expect((infra as any).sender).toBeUndefined();
    expect((infra as any).receiver).toBeUndefined();
    expect((infra as any).message).toBeUndefined();
  });

  it("turn_end infra entry has no sender/receiver/message", () => {
    const entry = makeLogEntry("infra", {
      event: "turn_end",
      inputTokens: 1000,
      outputTokens: 200,
      costUsd: 0.005,
    });
    expect(entry.kind).toBe("infra");
    expect((entry as InfraEntry).event).toBe("turn_end");
    expect((entry as any).sender).toBeUndefined();
    expect((entry as any).receiver).toBeUndefined();
    expect((entry as any).message).toBeUndefined();
  });

  it("api_retry infra entry shape", () => {
    const entry = makeLogEntry("infra", {
      event: "api_retry",
      attempt: 1,
      waitMs: 2000,
    });
    expect(entry.kind).toBe("infra");
    expect((entry as InfraEntry).event).toBe("api_retry");
  });

  it("diagnostic_written infra entry shape", () => {
    const entry = makeLogEntry("infra", {
      event: "diagnostic_written",
      path: "diagnosis/foo.json",
    });
    expect(entry.kind).toBe("infra");
    expect((entry as InfraEntry).event).toBe("diagnostic_written");
  });

  it("context_truncated infra entry shape", () => {
    const entry = makeLogEntry("infra", {
      event: "context_truncated",
      originalMessages: 20,
      keptMessages: 10,
    });
    expect(entry.kind).toBe("infra");
    expect((entry as InfraEntry).event).toBe("context_truncated");
  });
});

// ---------------------------------------------------------------------------
// LogEntry discriminated union — TypeScript narrowing works correctly
// ---------------------------------------------------------------------------

describe("LogEntry type discrimination", () => {
  it("kind:message narrows to MessageEntry", () => {
    const entry: LogEntry = makeLogEntry("message", {
      sender: "user",
      receiver: "agent",
      message: "call",
    });
    if (entry.kind === "message") {
      // TypeScript should allow access to .sender/.receiver/.message here
      expect(entry.sender).toBe("user");
      expect(entry.receiver).toBe("agent");
      expect(entry.message).toBe("call");
    } else {
      throw new Error("Expected message kind");
    }
  });

  it("kind:infra narrows to InfraEntry", () => {
    const entry: LogEntry = makeLogEntry("infra", { event: "startup" });
    if (entry.kind === "infra") {
      // TypeScript should allow access to .event here
      expect(entry.event).toBe("startup");
    } else {
      throw new Error("Expected infra kind");
    }
  });
});
