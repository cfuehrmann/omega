/**
 * Tests for src/event-store.ts.
 *
 * Covers:
 * - Round-trip serialisation of every OmegaEvent variant
 * - appendEvent file I/O
 * - null path is a no-op (test isolation)
 */

import { describe, it, expect, beforeEach, afterEach } from "bun:test";
import { existsSync, readFileSync } from "fs";
import { mkdtemp, rm } from "fs/promises";
import { tmpdir } from "os";
import { join } from "path";
import { appendEvent } from "./event-store.js";
import { OmegaEventSchema } from "./events.schema.js";
import { iso } from "./iso-timestamp.js";
import { asHash } from "./context-hash.js";
import type {
  OmegaEvent,
  UserMessageEvent,
  LlmResponseEvent,
  ToolCallEvent,
  ToolResultEvent,
  TurnEndEvent,
  LlmErrorEvent,
  AgentErrorEvent,
  TurnInterruptedEvent,
  CompactedEvent,
  SessionStartEvent,
  ServerStartedEvent,
  ServerStoppedEvent,
  LlmCallEvent,
  TransportErrorEvent,
} from "./events.js";
// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function readEvents(file: string): OmegaEvent[] {
  if (!existsSync(file)) return [];
  return readFileSync(file, "utf-8")
    .split("\n")
    .filter(Boolean)
    .map((line) => OmegaEventSchema.parse(JSON.parse(line)));
}

// ---------------------------------------------------------------------------
// Round-trip serialisation — one test per variant
// ---------------------------------------------------------------------------

describe("OmegaEvent round-trip serialisation", () => {
  let tempDir: string;
  let testFile: string;

  beforeEach(async () => {
    tempDir = await mkdtemp(join(tmpdir(), "omega-event-store-test-"));
    testFile = join(tempDir, "events.jsonl");
  });
  afterEach(async () => {
    await rm(tempDir, { recursive: true, force: true });
  });

  it("session_start", async () => {
    const e: SessionStartEvent = { type: "session_start", time: iso("2025-01-01T00:00:00.000Z"), sessionId: "abc123", model: "claude-sonnet-4-6", authMode: "api-key", systemPrompt: "You are a helpful assistant." };
    await appendEvent(e, testFile);
    const [read] = readEvents(testFile);
    expect(read).toEqual(e);
  });

  it("server_started", async () => {
    const e: ServerStartedEvent = { type: "server_started", time: iso("2025-01-01T00:00:00.000Z") };
    await appendEvent(e, testFile);
    const [read] = readEvents(testFile);
    expect(read).toEqual(e);
  });

  it("server_stopped (clean)", async () => {
    const e: ServerStoppedEvent = { type: "server_stopped", time: iso("2025-01-01T00:00:00.000Z"), outcome: "clean" };
    await appendEvent(e, testFile);
    const [read] = readEvents(testFile);
    expect(read).toEqual(e);
  });

  it("server_stopped (error with reason)", async () => {
    const e: ServerStoppedEvent = { type: "server_stopped", time: iso("2025-01-01T00:00:00.000Z"), outcome: "error", reason: "unhandled exception" };
    await appendEvent(e, testFile);
    const [read] = readEvents(testFile);
    expect(read).toEqual(e);
  });

  it("user_message", async () => {
    const e: UserMessageEvent = { type: "user_message", time: iso("2025-01-01T00:00:00.000Z"), content: "hello world" };
    await appendEvent(e, testFile);
    const [read] = readEvents(testFile);
    expect(read).toEqual(e);
  });

  it("llm_call", async () => {
    const e: LlmCallEvent = { type: "llm_call", time: iso("2025-01-01T00:00:00.000Z"), url: "https://api.anthropic.com/v1/messages", model: "claude-sonnet-4-6", contextHashes: [asHash("abc123456789"), asHash("def678901234"), asHash("112233445566")], cacheBreakpointIndex: 2, requestBytes: 1024 };
    await appendEvent(e, testFile);
    const [read] = readEvents(testFile);
    expect(read).toEqual(e);
  });

  it("llm_response", async () => {
    const e: LlmResponseEvent = {
      type: "llm_response",
      time: iso("2025-01-01T00:00:00.000Z"),
      stopReason: "end_turn",
      usage: { input_tokens: 100, output_tokens: 20, cache_creation_input_tokens: 0, cache_read_input_tokens: 50 },
      contextHash: asHash("ab12cd34ef56"),
    };
    await appendEvent(e, testFile);
    const [read] = readEvents(testFile);
    expect(read).toEqual(e);
  });

  it("tool_call", async () => {
    const e: ToolCallEvent = { type: "tool_call", time: iso("2025-01-01T00:00:00.000Z"), id: "tool_abc", name: "read_file", input: { path: "src/foo.ts" }, contextHash: asHash("ab12cd34ef56") };
    await appendEvent(e, testFile);
    const [read] = readEvents(testFile);
    expect(read).toEqual(e);
  });

  it("tool_result", async () => {
    const e: ToolResultEvent = { type: "tool_result", time: iso("2025-01-01T00:00:00.000Z"), id: "tool_abc", name: "read_file", isError: false, durationMs: 12, output: "file contents here" };
    await appendEvent(e, testFile);
    const [read] = readEvents(testFile);
    expect(read).toEqual(e);
  });

  it("turn_end", async () => {
    const e: TurnEndEvent = {
      type: "turn_end",
      time: iso("2025-01-01T00:00:00.000Z"),
      metrics: { inputTokens: 200, outputTokens: 50, cacheCreationTokens: 0, cacheReadTokens: 100 },
    };
    await appendEvent(e, testFile);
    const [read] = readEvents(testFile);
    expect(read).toEqual(e);
  });

  it("llm_error", async () => {
    const e: LlmErrorEvent = { type: "llm_error", time: iso("2025-01-01T00:00:00.000Z"), url: "https://api.anthropic.com/v1/messages", error: "rate limited", httpStatus: 429 };
    await appendEvent(e, testFile);
    const [read] = readEvents(testFile);
    expect(read).toEqual(e);
  });

  it("agent_error", async () => {
    const e: AgentErrorEvent = { type: "agent_error", time: iso("2025-01-01T00:00:00.000Z"), error: "something went wrong" };
    await appendEvent(e, testFile);
    const [read] = readEvents(testFile);
    expect(read).toEqual(e);
  });

  it("turn_interrupted", async () => {
    const e: TurnInterruptedEvent = { type: "turn_interrupted", time: iso("2025-01-01T00:00:00.000Z") };
    await appendEvent(e, testFile);
    const [read] = readEvents(testFile);
    expect(read).toEqual(e);
  });

  it("turn_interrupted with reason=aborted", async () => {
    const e: TurnInterruptedEvent = { type: "turn_interrupted", time: iso("2025-01-01T00:00:00.000Z"), reason: "aborted" };
    await appendEvent(e, testFile);
    const [read] = readEvents(testFile);
    expect(read).toEqual(e);
  });

  it("turn_interrupted with reason=error", async () => {
    const e: TurnInterruptedEvent = { type: "turn_interrupted", time: iso("2025-01-01T00:00:00.000Z"), reason: "error" };
    await appendEvent(e, testFile);
    const [read] = readEvents(testFile);
    expect(read).toEqual(e);
  });

  it("compacted (no iterations)", async () => {
    const e: CompactedEvent = {
      type: "compacted",
      time: iso("2025-01-01T00:00:00.000Z"),
      usage: { input_tokens: 500, output_tokens: 200 },
    };
    await appendEvent(e, testFile);
    const [read] = readEvents(testFile);
    expect(read).toEqual(e);
  });

  it("compacted (with iterations array)", async () => {
    const e: CompactedEvent = {
      type: "compacted",
      time: iso("2025-01-01T00:00:00.000Z"),
      usage: {
        input_tokens: 500,
        output_tokens: 200,
        iterations: [
          { type: "compaction", input_tokens: 80000, output_tokens: 300 },
          { type: "message", input_tokens: 500, output_tokens: 200 },
        ],
      },
    };
    await appendEvent(e, testFile);
    const [read] = readEvents(testFile);
    expect(read).toEqual(e);
  });

  it("transport_error (no context)", async () => {
    const e: TransportErrorEvent = { type: "transport_error", time: iso("2025-01-01T00:00:00.000Z"), error: "Invalid JSON from client" };
    await appendEvent(e, testFile);
    const [read] = readEvents(testFile);
    expect(read).toEqual(e);
  });

  it("transport_error (with context)", async () => {
    const e: TransportErrorEvent = { type: "transport_error", time: iso("2025-01-01T00:00:00.000Z"), error: "Turn already in progress", context: "handleMessage" };
    await appendEvent(e, testFile);
    const [read] = readEvents(testFile);
    expect(read).toEqual(e);
  });
});

// ---------------------------------------------------------------------------
// appendEvent I/O
// ---------------------------------------------------------------------------

describe("appendEvent", () => {
  let tempDir: string;
  let testFile: string;

  beforeEach(async () => {
    tempDir = await mkdtemp(join(tmpdir(), "omega-event-store-test-"));
    testFile = join(tempDir, "events.jsonl");
  });
  afterEach(async () => {
    await rm(tempDir, { recursive: true, force: true });
  });

  it("creates the file if it does not exist", async () => {
    expect(existsSync(testFile)).toBe(false);
    await appendEvent({ type: "turn_interrupted", time: iso("2025-01-01T00:00:00.000Z") }, testFile);
    expect(existsSync(testFile)).toBe(true);
  });

  it("appends multiple events as separate JSONL lines", async () => {
    const e1: OmegaEvent = { type: "user_message", time: iso("2025-01-01T00:00:00.000Z"), content: "first" };
    const e2: OmegaEvent = { type: "user_message", time: iso("2025-01-01T00:00:01.000Z"), content: "second" };
    await appendEvent(e1, testFile);
    await appendEvent(e2, testFile);
    const events = readEvents(testFile);
    expect(events).toHaveLength(2);
    expect((events[0] as UserMessageEvent).content).toBe("first");
    expect((events[1] as UserMessageEvent).content).toBe("second");
  });

  it("null path is a no-op — no file created", async () => {
    await appendEvent({ type: "turn_interrupted", time: iso("2025-01-01T00:00:00.000Z") }, null);
    expect(existsSync(testFile)).toBe(false);
  });
});


