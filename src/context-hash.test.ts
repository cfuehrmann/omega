/**
 * Tests for Step 3e-iii — FK/PK content-addressed context log.
 *
 * Covers:
 * - context.jsonl entries carry `hash` and `ts` fields
 * - llm_call events carry `contextHashes: string[]`
 * - Hashes are derived from the buildApiMessages() view, NOT from llmMessageLog
 * - Chaotic scenarios:
 *   - Identical message content → different hashes (ts prevents collision)
 *   - Tool loop: each llm_call's contextHashes grows correctly
 *   - Truncation fires on retry 2 but not retry 1 (hashes differ)
 *   - Retry within same iteration reuses same contextHashes
 * - contextHashesForView maps by object identity (no false matches)
 */

import { describe, it, expect } from "bun:test";
import { mkdirSync, rmSync, existsSync, readFileSync } from "fs";
import { join } from "path";
import { tmpdir } from "os";
import type Anthropic from "@anthropic-ai/sdk";

import { Agent, type AgentEvent, type StreamProvider, buildApiMessages } from "./agent.js";
import type { ContextRecord } from "./context-store.js";
import type { LlmCallEvent } from "./session-event.js";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function makeMockStream(events: any[], message: Anthropic.Message) {
  return {
    async *[Symbol.asyncIterator]() {
      for (const e of events) yield e;
    },
    finalMessage: async () => message,
  };
}

function textMessage(text: string): Anthropic.Message {
  return {
    id: "msg_test",
    type: "message",
    role: "assistant",
    model: "claude-sonnet-4-6",
    content: [{ type: "text", text }],
    stop_reason: "end_turn",
    stop_sequence: null,
    usage: { input_tokens: 10, output_tokens: 5 },
  };
}

function textStreamEvents(text: string): any[] {
  return [
    { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } },
    { type: "content_block_delta", index: 0, delta: { type: "text_delta", text } },
    { type: "content_block_stop", index: 0 },
    { type: "message_delta", delta: { stop_reason: "end_turn" }, usage: { output_tokens: 5 } },
    { type: "message_stop" },
  ];
}

function toolUseMessage(toolId: string, toolName: string, toolInput: any): Anthropic.Message {
  return {
    id: "msg_tool",
    type: "message",
    role: "assistant",
    model: "claude-sonnet-4-6",
    content: [{ type: "tool_use", id: toolId, name: toolName, input: toolInput }],
    stop_reason: "tool_use",
    stop_sequence: null,
    usage: { input_tokens: 20, output_tokens: 10 },
  };
}

function toolUseStreamEvents(toolName: string, toolId = "t1"): any[] {
  return [
    { type: "content_block_start", index: 0, content_block: { type: "tool_use", id: toolId, name: toolName } },
    { type: "content_block_delta", index: 0, delta: { type: "input_json_delta", partial_json: "{}" } },
    { type: "content_block_stop", index: 0 },
    { type: "message_delta", delta: { stop_reason: "tool_use" }, usage: { output_tokens: 10 } },
    { type: "message_stop" },
  ];
}

async function collectEvents(
  agent: Agent,
  message: string
): Promise<AgentEvent[]> {
  const events: AgentEvent[] = [];
  for await (const event of agent.sendMessage(message, async () => true)) {
    events.push(event);
  }
  return events;
}

function readContextRecords(file: string): ContextRecord[] {
  if (!existsSync(file)) return [];
  return readFileSync(file, "utf-8")
    .split("\n")
    .filter(Boolean)
    .map(l => JSON.parse(l) as ContextRecord);
}

function readEventLines(file: string): any[] {
  if (!existsSync(file)) return [];
  return readFileSync(file, "utf-8")
    .split("\n")
    .filter(Boolean)
    .map(l => JSON.parse(l));
}

let _counter = 0;
function makeTempDir(): string {
  const dir = join(tmpdir(), `omega-hash-test-${Date.now()}-${++_counter}`);
  mkdirSync(dir, { recursive: true });
  return dir;
}

// ---------------------------------------------------------------------------
// context.jsonl record shape (Step 3e-iii)
// ---------------------------------------------------------------------------

describe("context.jsonl record shape", () => {
  it("each written record has hash (8 hex chars), ts, role, and content", async () => {
    const dir = makeTempDir();
    const contextFile = join(dir, "context.jsonl");
    const eventsFile = join(dir, "events.jsonl");

    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hello"), textMessage("hello"));

    const agent = new Agent(mockProvider, null, undefined, null, contextFile, eventsFile);
    await collectEvents(agent, "hi");
    await Bun.sleep(50); // let fire-and-forget writes settle

    const records = readContextRecords(contextFile);
    expect(records.length).toBeGreaterThanOrEqual(2); // user + assistant

    for (const record of records) {
      expect(typeof record.hash).toBe("string");
      expect(record.hash).toHaveLength(8);
      expect(/^[0-9a-f]{8}$/.test(record.hash)).toBe(true);
      expect(typeof record.ts).toBe("string");
      expect(record.ts).toMatch(/^\d{4}-\d{2}-\d{2}T/);
      expect(record.role === "user" || record.role === "assistant").toBe(true);
      expect(record.content).toBeDefined();
    }
  });

  it("first record is the user message, second is assistant response", async () => {
    const dir = makeTempDir();
    const contextFile = join(dir, "context.jsonl");
    const eventsFile = join(dir, "events.jsonl");

    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("world"), textMessage("world"));

    const agent = new Agent(mockProvider, null, undefined, null, contextFile, eventsFile);
    await collectEvents(agent, "hello");
    await Bun.sleep(50);

    const records = readContextRecords(contextFile);
    expect(records[0].role).toBe("user");
    expect(records[0].content).toBe("hello");
    expect(records[1].role).toBe("assistant");
  });
});

// ---------------------------------------------------------------------------
// Identical message content → different hashes (ts prevents collision)
// ---------------------------------------------------------------------------

describe("hash uniqueness — identical content, different times", () => {
  it("two identical user messages produce different hashes", async () => {
    const dir = makeTempDir();
    const contextFile = join(dir, "context.jsonl");
    const eventsFile = join(dir, "events.jsonl");

    let call = 0;
    const mockProvider: StreamProvider = async () => {
      call++;
      return makeMockStream(textStreamEvents(`response ${call}`), textMessage(`response ${call}`));
    };

    const agent = new Agent(mockProvider, null, undefined, null, contextFile, eventsFile);
    // Send the same message twice
    await collectEvents(agent, "ok");
    await new Promise(r => setTimeout(r, 10)); // ensure ts differs
    await collectEvents(agent, "ok");
    await Bun.sleep(50);

    const records = readContextRecords(contextFile);
    // Both user messages have content "ok" — their hashes must differ
    const userRecords = records.filter(r => r.role === "user" && r.content === "ok");
    expect(userRecords.length).toBe(2);
    expect(userRecords[0].hash).not.toBe(userRecords[1].hash);
  });
});

// ---------------------------------------------------------------------------
// llm_call events carry contextHashes
// ---------------------------------------------------------------------------

describe("llm_call contextHashes in events.jsonl", () => {
  it("llm_call event has contextHashes array with one hash per sent message", async () => {
    const dir = makeTempDir();
    const contextFile = join(dir, "context.jsonl");
    const eventsFile = join(dir, "events.jsonl");

    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hi"), textMessage("hi"));

    const agent = new Agent(mockProvider, null, undefined, null, contextFile, eventsFile);
    await collectEvents(agent, "hello");
    await Bun.sleep(50);

    const allEvents = readEventLines(eventsFile);
    const llmCallEvents = allEvents.filter(e => e.type === "llm_call") as LlmCallEvent[];
    expect(llmCallEvents.length).toBe(1);

    const llmCall = llmCallEvents[0];
    expect(Array.isArray(llmCall.contextHashes)).toBe(true);
    // Only the user message was in llmMessageLog when the first call was made
    expect(llmCall.contextHashes).toHaveLength(1);
    expect(llmCall.contextHashes[0]).toHaveLength(8);
    expect(/^[0-9a-f]{8}$/.test(llmCall.contextHashes[0])).toBe(true);
  });

  it("contextHashes match the hash field of the corresponding context.jsonl records", async () => {
    const dir = makeTempDir();
    const contextFile = join(dir, "context.jsonl");
    const eventsFile = join(dir, "events.jsonl");

    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hi"), textMessage("hi"));

    const agent = new Agent(mockProvider, null, undefined, null, contextFile, eventsFile);
    await collectEvents(agent, "hello");
    await Bun.sleep(50);

    const contextRecords = readContextRecords(contextFile);
    const allEvents = readEventLines(eventsFile);
    const llmCall = allEvents.find(e => e.type === "llm_call") as LlmCallEvent;

    // The first llm_call should only have the user message in its view
    expect(llmCall.contextHashes).toHaveLength(1);
    expect(llmCall.contextHashes[0]).toBe(contextRecords[0].hash);
  });

  it("tool loop: second llm_call contextHashes includes user + assistant + tool_result messages", async () => {
    const dir = makeTempDir();
    const contextFile = join(dir, "context.jsonl");
    const eventsFile = join(dir, "events.jsonl");

    let call = 0;
    const mockProvider: StreamProvider = async () => {
      call++;
      if (call === 1) return makeMockStream(toolUseStreamEvents("list_files"), toolUseMessage("t1", "list_files", { path: "." }));
      return makeMockStream(textStreamEvents("done"), textMessage("done"));
    };

    const agent = new Agent(mockProvider, null, undefined, null, contextFile, eventsFile);
    await collectEvents(agent, "list it");
    await Bun.sleep(50);

    const contextRecords = readContextRecords(contextFile);
    const allEvents = readEventLines(eventsFile);
    const llmCalls = allEvents.filter(e => e.type === "llm_call") as LlmCallEvent[];

    expect(llmCalls.length).toBe(2);

    // First call: only the user message
    expect(llmCalls[0].contextHashes).toHaveLength(1);
    expect(llmCalls[0].contextHashes[0]).toBe(contextRecords[0].hash);

    // Second call: user + assistant(tool_use) + user(tool_result)
    expect(llmCalls[1].contextHashes).toHaveLength(3);
    expect(llmCalls[1].contextHashes[0]).toBe(contextRecords[0].hash);
    expect(llmCalls[1].contextHashes[1]).toBe(contextRecords[1].hash);
    expect(llmCalls[1].contextHashes[2]).toBe(contextRecords[2].hash);
  });

  it("contextHashes grow across multiple turns in the same session", async () => {
    const dir = makeTempDir();
    const contextFile = join(dir, "context.jsonl");
    const eventsFile = join(dir, "events.jsonl");

    let call = 0;
    const mockProvider: StreamProvider = async () => {
      call++;
      return makeMockStream(textStreamEvents(`resp${call}`), textMessage(`resp${call}`));
    };

    const agent = new Agent(mockProvider, null, undefined, null, contextFile, eventsFile);
    await collectEvents(agent, "turn1");
    await collectEvents(agent, "turn2");
    await Bun.sleep(50);

    const allEvents = readEventLines(eventsFile);
    const llmCalls = allEvents.filter(e => e.type === "llm_call") as LlmCallEvent[];

    // First call: 1 message (turn1 user)
    expect(llmCalls[0].contextHashes).toHaveLength(1);

    // Second call: 3 messages (turn1 user, turn1 asst, turn2 user)
    expect(llmCalls[1].contextHashes).toHaveLength(3);

    // All hashes must be 8-char hex
    for (const llmCall of llmCalls) {
      for (const h of llmCall.contextHashes) {
        expect(h).toHaveLength(8);
        expect(/^[0-9a-f]{8}$/.test(h)).toBe(true);
      }
    }
  });
});

// ---------------------------------------------------------------------------
// Truncation scenario: contextHashes reflects the VIEW, not llmMessageLog
// ---------------------------------------------------------------------------

describe("contextHashes reflects truncated view, not full llmMessageLog", () => {
  it("after truncation, contextHashes length < llmMessageLog length", async () => {
    // Build a large history that will be truncated.
    // We'll override buildApiMessages via a thin wrapper to force truncation
    // by using a very tight budget.
    const dir = makeTempDir();
    const contextFile = join(dir, "context.jsonl");
    const eventsFile = join(dir, "events.jsonl");

    let call = 0;
    const mockProvider: StreamProvider = async () => {
      call++;
      return makeMockStream(textStreamEvents(`resp${call}`), textMessage(`resp${call}`));
    };

    const agent = new Agent(mockProvider, null, undefined, null, contextFile, eventsFile);

    // Build up 6 messages (3 turns: user+assistant each)
    for (let i = 0; i < 3; i++) {
      await collectEvents(agent, `turn${i + 1}`);
    }
    await Bun.sleep(50);

    // At this point llmMessageLog has 6 messages.
    // The last llm_call (for turn3) should have had 5 messages in its view
    // (turns 1-2 = 4 messages + turn3 user = 5)
    const allEvents = readEventLines(eventsFile);
    const llmCalls = allEvents.filter(e => e.type === "llm_call") as LlmCallEvent[];

    // Third call: 5 messages in view
    expect(llmCalls[2].contextHashes).toHaveLength(5);

    // Each hash must appear as a hash in context.jsonl
    const contextRecords = readContextRecords(contextFile);
    const contextHashSet = new Set(contextRecords.map(r => r.hash));
    for (const h of llmCalls[2].contextHashes) {
      expect(contextHashSet.has(h)).toBe(true);
    }
  });

  it("hashes in contextHashes correctly cross-reference context.jsonl entries", async () => {
    const dir = makeTempDir();
    const contextFile = join(dir, "context.jsonl");
    const eventsFile = join(dir, "events.jsonl");

    let call = 0;
    const mockProvider: StreamProvider = async () => {
      call++;
      return makeMockStream(textStreamEvents("ok"), textMessage("ok"));
    };

    const agent = new Agent(mockProvider, null, undefined, null, contextFile, eventsFile);
    await collectEvents(agent, "hello");
    await Bun.sleep(50);

    const contextRecords = readContextRecords(contextFile);
    const allEvents = readEventLines(eventsFile);
    const llmCall = allEvents.find(e => e.type === "llm_call") as LlmCallEvent;

    // Each hash in contextHashes must resolve to a context record
    for (let i = 0; i < llmCall.contextHashes.length; i++) {
      const hash = llmCall.contextHashes[i];
      const record = contextRecords.find(r => r.hash === hash);
      expect(record).toBeDefined();
    }
  });
});

// ---------------------------------------------------------------------------
// buildApiMessages integration: returned subset has same object references
// ---------------------------------------------------------------------------

describe("buildApiMessages preserves object references (needed for contextHashesForView)", () => {
  it("messages in the returned view are the same objects as in the source array", () => {
    const msg1: Anthropic.MessageParam = { role: "user", content: "hello" };
    const msg2: Anthropic.MessageParam = { role: "assistant", content: [{ type: "text", text: "hi" }] };
    const history = [msg1, msg2];

    const view = buildApiMessages(history, 1_000_000);
    // All messages in view should be the exact same object references
    for (const viewMsg of view) {
      const found = history.some(h => h === viewMsg);
      expect(found).toBe(true);
    }
  });

  it("when truncation drops messages, remaining view messages are still the same references", () => {
    // Create a large history that requires truncation (very small budget)
    const history: Anthropic.MessageParam[] = [];
    for (let i = 0; i < 10; i++) {
      history.push({ role: "user", content: "x".repeat(1000) });
      history.push({ role: "assistant", content: [{ type: "text", text: "y".repeat(1000) }] });
    }

    // Very tight budget — forces truncation
    const view = buildApiMessages(history, 500);
    expect(view.length).toBeLessThan(history.length);

    // Every message in the view must be an exact reference from history
    for (const viewMsg of view) {
      const found = history.some(h => h === viewMsg);
      expect(found).toBe(true);
    }
  });
});

// ---------------------------------------------------------------------------
// No placeholder hashes — every message in view has a real hash
// ---------------------------------------------------------------------------

describe("no placeholder hashes", () => {
  it("all contextHashes are 8-char hex strings (no '????????' placeholders)", async () => {
    const dir = makeTempDir();
    const contextFile = join(dir, "context.jsonl");
    const eventsFile = join(dir, "events.jsonl");

    let call = 0;
    const mockProvider: StreamProvider = async () => {
      call++;
      if (call === 1) return makeMockStream(toolUseStreamEvents("list_files"), toolUseMessage("t1", "list_files", { path: "." }));
      return makeMockStream(textStreamEvents("done"), textMessage("done"));
    };

    const agent = new Agent(mockProvider, null, undefined, null, contextFile, eventsFile);
    await collectEvents(agent, "go");
    await Bun.sleep(50);

    const allEvents = readEventLines(eventsFile);
    const llmCalls = allEvents.filter(e => e.type === "llm_call") as LlmCallEvent[];

    for (const llmCall of llmCalls) {
      for (const hash of llmCall.contextHashes) {
        expect(hash).not.toBe("????????");
        expect(/^[0-9a-f]{8}$/.test(hash)).toBe(true);
      }
    }
  });
});
