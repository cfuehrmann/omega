/**
 * Tests for src/session-event.ts (Step 3c).
 *
 * Covers:
 * - Round-trip serialisation of every SessionEvent variant
 * - appendSessionEvent / clearSessionEvents file I/O
 * - null path is a no-op (test isolation)
 * - Agent with mock provider does NOT write to disk
 */

import { describe, it, expect, beforeEach, afterEach } from "bun:test";
import { existsSync, readFileSync, mkdirSync, rmSync } from "fs";
import { prevPath } from "./context-store.js";
import {
  appendSessionEvent,
  clearSessionEvents,
  type SessionEvent,
  type UserMessageEvent,
  type LlmResponseEvent,
  type ToolCallEvent,
  type ToolResultEvent,
  type TurnEndEvent,
  type LlmErrorEvent,
  type AgentErrorEvent,
  type TurnInterruptedEvent,
  type SessionCompactedEvent,
  type SessionStartEvent,
  type LlmCallEvent,
} from "./session-event.js";
import { Agent } from "./agent.js";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const TEST_FILE = "sessions-test/session-event-test.jsonl";

function readEvents(file: string): SessionEvent[] {
  if (!existsSync(file)) return [];
  return readFileSync(file, "utf-8")
    .split("\n")
    .filter(Boolean)
    .map((line) => JSON.parse(line) as SessionEvent);
}

// ---------------------------------------------------------------------------
// Round-trip serialisation — one test per variant
// ---------------------------------------------------------------------------

describe("SessionEvent round-trip serialisation", () => {
  beforeEach(() => {
    mkdirSync("sessions-test", { recursive: true });
    if (existsSync(TEST_FILE)) rmSync(TEST_FILE);
  });
  afterEach(() => {
    if (existsSync(TEST_FILE)) rmSync(TEST_FILE);
  });

  it("session_start", async () => {
    const e: SessionStartEvent = { type: "session_start", ts: "2025-01-01T00:00:00.000Z", sessionId: "abc123", model: "claude-sonnet-4-6", provider: "anthropic", authMode: "api-key" };
    await appendSessionEvent(e, TEST_FILE);
    const [read] = readEvents(TEST_FILE);
    expect(read).toEqual(e);
  });

  it("user_message", async () => {
    const e: UserMessageEvent = { type: "user_message", ts: "2025-01-01T00:00:00.000Z", content: "hello world" };
    await appendSessionEvent(e, TEST_FILE);
    const [read] = readEvents(TEST_FILE);
    expect(read).toEqual(e);
  });

  it("llm_call", async () => {
    const e: LlmCallEvent = { type: "llm_call", ts: "2025-01-01T00:00:00.000Z", provider: "anthropic", url: "https://api.anthropic.com/v1/messages", model: "claude-sonnet-4-6", contextHashes: ["abc12345", "def67890", "11223344"] };
    await appendSessionEvent(e, TEST_FILE);
    const [read] = readEvents(TEST_FILE);
    expect(read).toEqual(e);
  });

  it("llm_response", async () => {
    const e: LlmResponseEvent = {
      type: "llm_response",
      ts: "2025-01-01T00:00:00.000Z",
      provider: "anthropic",
      url: "https://api.anthropic.com/v1/messages",
      stopReason: "end_turn",
      model: "claude-sonnet-4-6",
      usage: { input_tokens: 100, output_tokens: 20, cache_creation_input_tokens: 0, cache_read_input_tokens: 50 },
      contextHash: "ab12cd34",
    };
    await appendSessionEvent(e, TEST_FILE);
    const [read] = readEvents(TEST_FILE);
    expect(read).toEqual(e);
  });

  it("tool_call", async () => {
    const e: ToolCallEvent = { type: "tool_call", ts: "2025-01-01T00:00:00.000Z", id: "tool_abc", name: "read_file", contextHash: "ab12cd34" };
    await appendSessionEvent(e, TEST_FILE);
    const [read] = readEvents(TEST_FILE);
    expect(read).toEqual(e);
  });

  it("tool_result", async () => {
    const e: ToolResultEvent = { type: "tool_result", ts: "2025-01-01T00:00:00.000Z", id: "tool_abc", name: "read_file", isError: false, durationMs: 12, contextHash: "ef56ab78" };
    await appendSessionEvent(e, TEST_FILE);
    const [read] = readEvents(TEST_FILE);
    expect(read).toEqual(e);
  });

  it("turn_end", async () => {
    const e: TurnEndEvent = {
      type: "turn_end",
      ts: "2025-01-01T00:00:00.000Z",
      provider: "anthropic",
      model: "claude-sonnet-4-6",
      metrics: { inputTokens: 200, outputTokens: 50, costUsd: 0.001, savedUsd: 0.0005, ttftMs: 300, totalMs: 1200, cacheCreationTokens: 0, cacheReadTokens: 100 },
      toolCalls: ["read_file", "write_file"],
    };
    await appendSessionEvent(e, TEST_FILE);
    const [read] = readEvents(TEST_FILE);
    expect(read).toEqual(e);
  });

  it("llm_error", async () => {
    const e: LlmErrorEvent = { type: "llm_error", ts: "2025-01-01T00:00:00.000Z", provider: "anthropic", url: "https://api.anthropic.com/v1/messages", error: "rate limited", httpStatus: 429 };
    await appendSessionEvent(e, TEST_FILE);
    const [read] = readEvents(TEST_FILE);
    expect(read).toEqual(e);
  });

  it("agent_error", async () => {
    const e: AgentErrorEvent = { type: "agent_error", ts: "2025-01-01T00:00:00.000Z", error: "something went wrong" };
    await appendSessionEvent(e, TEST_FILE);
    const [read] = readEvents(TEST_FILE);
    expect(read).toEqual(e);
  });

  it("turn_interrupted", async () => {
    const e: TurnInterruptedEvent = { type: "turn_interrupted", ts: "2025-01-01T00:00:00.000Z" };
    await appendSessionEvent(e, TEST_FILE);
    const [read] = readEvents(TEST_FILE);
    expect(read).toEqual(e);
  });

  it("session_compacted", async () => {
    const e: SessionCompactedEvent = { type: "session_compacted", ts: "2025-01-01T00:00:00.000Z", originalCount: 40, newCount: 8 };
    await appendSessionEvent(e, TEST_FILE);
    const [read] = readEvents(TEST_FILE);
    expect(read).toEqual(e);
  });
});

// ---------------------------------------------------------------------------
// appendSessionEvent / clearSessionEvents I/O
// ---------------------------------------------------------------------------

describe("appendSessionEvent", () => {
  beforeEach(() => {
    mkdirSync("sessions-test", { recursive: true });
    if (existsSync(TEST_FILE)) rmSync(TEST_FILE);
  });
  afterEach(() => {
    if (existsSync(TEST_FILE)) rmSync(TEST_FILE);
  });

  it("creates the file if it does not exist", async () => {
    expect(existsSync(TEST_FILE)).toBe(false);
    await appendSessionEvent({ type: "turn_interrupted", ts: "2025-01-01T00:00:00.000Z" }, TEST_FILE);
    expect(existsSync(TEST_FILE)).toBe(true);
  });

  it("appends multiple events as separate JSONL lines", async () => {
    const e1: SessionEvent = { type: "user_message", ts: "2025-01-01T00:00:00.000Z", content: "first" };
    const e2: SessionEvent = { type: "user_message", ts: "2025-01-01T00:00:01.000Z", content: "second" };
    await appendSessionEvent(e1, TEST_FILE);
    await appendSessionEvent(e2, TEST_FILE);
    const events = readEvents(TEST_FILE);
    expect(events).toHaveLength(2);
    expect((events[0] as UserMessageEvent).content).toBe("first");
    expect((events[1] as UserMessageEvent).content).toBe("second");
  });

  it("null path is a no-op — no file created", async () => {
    await appendSessionEvent({ type: "turn_interrupted", ts: "2025-01-01T00:00:00.000Z" }, null);
    expect(existsSync(TEST_FILE)).toBe(false);
  });
});

describe("clearSessionEvents", () => {
  const PREV_FILE = prevPath(TEST_FILE);

  beforeEach(() => {
    mkdirSync("sessions-test", { recursive: true });
    if (existsSync(TEST_FILE)) rmSync(TEST_FILE);
    if (existsSync(PREV_FILE)) rmSync(PREV_FILE);
  });
  afterEach(() => {
    if (existsSync(TEST_FILE)) rmSync(TEST_FILE);
    if (existsSync(PREV_FILE)) rmSync(PREV_FILE);
  });

  it("rotates: current file ends up empty, previous content preserved as .prev", async () => {
    await appendSessionEvent({ type: "turn_interrupted", ts: "2025-01-01T00:00:00.000Z" }, TEST_FILE);
    await clearSessionEvents(TEST_FILE);
    expect(readFileSync(TEST_FILE, "utf-8")).toBe("");
    expect(existsSync(PREV_FILE)).toBe(true);
    const prev = readFileSync(PREV_FILE, "utf-8");
    expect(JSON.parse(prev.trim()).type).toBe("turn_interrupted");
  });

  it("creates fresh empty file when nothing existed before", async () => {
    await clearSessionEvents(TEST_FILE);
    expect(existsSync(TEST_FILE)).toBe(true);
    expect(readFileSync(TEST_FILE, "utf-8")).toBe("");
    expect(existsSync(PREV_FILE)).toBe(false);
  });

  it("null path is a no-op — no file created", async () => {
    await clearSessionEvents(null);
    expect(existsSync(TEST_FILE)).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// Test isolation — mock-provider Agent must NOT write events to disk
// ---------------------------------------------------------------------------

describe("Agent test isolation — events file", () => {
  it("mock-provider agent does not write to sessions/events.jsonl", async () => {
    const PROD_FILE = "sessions/events.jsonl";
    // Remove if it exists so we can detect a fresh write
    if (existsSync(PROD_FILE)) rmSync(PROD_FILE);

    const mockProvider = async (_params: any) => ({
      async *[Symbol.asyncIterator]() {
        // no events
      },
      async finalMessage() {
        return {
          id: "test",
          type: "message",
          role: "assistant",
          content: [{ type: "text", text: "ok" }],
          model: "claude-sonnet-4-6",
          stop_reason: "end_turn",
          stop_sequence: null,
          usage: { input_tokens: 10, output_tokens: 5 },
        } as any;
      },
    });

    const agent = new Agent(mockProvider);
    // Call sendMessage — this would trigger user_message logEvent
    const gen = agent.sendMessage("hello", async () => true);
    // Drain without actually running (mock returns end_turn immediately)
    const events = [];
    for await (const e of gen) {
      events.push(e);
    }

    // Production events file must NOT have been created
    expect(existsSync(PROD_FILE)).toBe(false);
  });
});
