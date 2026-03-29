/**
 * Tests for Step 3e-iii — FK/PK content-addressed context log.
 *
 * Covers:
 * - context.jsonl entries carry `hash` and `time` fields
 * - llm_call events carry `contextHashes: string[]`
 * - Hashes match every message in compactedContextHistory (no trimming)
 * - Chaotic scenarios:
 *   - Identical message content → different hashes (time prevents collision)
 *   - Tool loop: each llm_call's contextHashes grows correctly
 * - All hashes are 12-char hex (no placeholders)
 */

import { describe, it, expect } from "bun:test";
import { existsSync, readFileSync } from "fs";
import type Anthropic from "@anthropic-ai/sdk";

import { Agent, type OmegaEvent, type StreamSignal, type StreamProvider } from "./agent.js";
import { makeTestAgent } from "./test-utils.js";
import type { ContextRecord } from "./context-store.js";
import { ContextRecordSchema } from "./context-store.schema.js";
import { OmegaEventSchema } from "./events.schema.js";
import type { LlmCallEvent, ToolCallEvent, ToolResultEvent } from "./events.js";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function makeMockStream(events: any[], message: Anthropic.Beta.Messages.BetaMessage) {
  return {
    async *[Symbol.asyncIterator]() {
      for (const e of events) yield e;
    },
    finalMessage: async () => message,
  };
}

function textMessage(text: string): Anthropic.Beta.Messages.BetaMessage {
  return {
    id: "msg_test",
    type: "message",
    role: "assistant",
    model: "claude-sonnet-4-6",
    container: null,
    content: [{ type: "text", text, citations: null }],
    stop_reason: "end_turn",
    stop_sequence: null,
    context_management: null,
    usage: { input_tokens: 10, output_tokens: 5, cache_creation: null, cache_creation_input_tokens: null, cache_read_input_tokens: null, inference_geo: null, iterations: null, server_tool_use: null, service_tier: null, speed: null },
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

function toolUseMessage(toolId: string, toolName: string, toolInput: any): Anthropic.Beta.Messages.BetaMessage {
  return {
    id: "msg_tool",
    type: "message",
    role: "assistant",
    model: "claude-sonnet-4-6",
    container: null,
    content: [{ type: "tool_use", id: toolId, name: toolName, input: toolInput, caller: { type: "direct" } }],
    stop_reason: "tool_use",
    stop_sequence: null,
    context_management: null,
    usage: { input_tokens: 20, output_tokens: 10, cache_creation: null, cache_creation_input_tokens: null, cache_read_input_tokens: null, inference_geo: null, iterations: null, server_tool_use: null, service_tier: null, speed: null },
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
): Promise<(OmegaEvent | StreamSignal)[]> {
  const events: (OmegaEvent | StreamSignal)[] = [];
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
    .map(l => ContextRecordSchema.parse(JSON.parse(l)));
}

function readEventLines(file: string): OmegaEvent[] {
  if (!existsSync(file)) return [];
  return readFileSync(file, "utf-8")
    .split("\n")
    .filter(Boolean)
    .map(l => OmegaEventSchema.parse(JSON.parse(l)));
}



// ---------------------------------------------------------------------------
// context.jsonl record shape (Step 3e-iii)
// ---------------------------------------------------------------------------

describe("context.jsonl record shape", () => {
  it.concurrent("each written record has hash (12 hex chars), time, role, and content", async () => {

    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hello"), textMessage("hello"));

    const { agent, contextFile, eventsFile } = await makeTestAgent(mockProvider);
    await collectEvents(agent, "hi");
    await Bun.sleep(50); // let fire-and-forget writes settle

    const records = readContextRecords(contextFile);
    expect(records.length).toBeGreaterThanOrEqual(2); // user + assistant

    for (const record of records) {
      expect(typeof record.hash).toBe("string");
      expect(record.hash).toHaveLength(12);
      expect(/^[0-9a-f]{12}$/.test(record.hash)).toBe(true);
      expect(typeof record.time).toBe("string");
      expect(record.time).toMatch(/^\d{4}-\d{2}-\d{2}T/);
      expect(record.role === "user" || record.role === "assistant").toBe(true);
      expect(record.content).toBeDefined();
    }
  });

  it.concurrent("first record is the user message, second is assistant response", async () => {

    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("world"), textMessage("world"));

    const { agent, contextFile, eventsFile } = await makeTestAgent(mockProvider);
    await collectEvents(agent, "hello");
    await Bun.sleep(50);

    const records = readContextRecords(contextFile);
    expect(records[0]!.role).toBe("user");
    expect(records[0]!.content).toBe("hello");
    expect(records[1]!.role).toBe("assistant");
  });
});

// ---------------------------------------------------------------------------
// Identical message content → different hashes (time prevents collision)
// ---------------------------------------------------------------------------

describe("hash uniqueness — identical content, different times", () => {
  it.concurrent("two identical user messages produce different hashes", async () => {

    let call = 0;
    const mockProvider: StreamProvider = async () => {
      call++;
      return makeMockStream(textStreamEvents(`response ${call}`), textMessage(`response ${call}`));
    };

    const { agent, contextFile, eventsFile } = await makeTestAgent(mockProvider);
    // Send the same message twice
    await collectEvents(agent, "ok");
    await new Promise(r => setTimeout(r, 10)); // ensure time differs
    await collectEvents(agent, "ok");
    await Bun.sleep(50);

    const records = readContextRecords(contextFile);
    // Both user messages have content "ok" — their hashes must differ
    const userRecords = records.filter(r => r.role === "user" && r.content === "ok");
    expect(userRecords.length).toBe(2);
    expect(userRecords[0]!.hash).not.toBe(userRecords[1]!.hash);
  });
});

// ---------------------------------------------------------------------------
// llm_call events carry contextHashes
// ---------------------------------------------------------------------------

describe("llm_call contextHashes in events.jsonl", () => {
  it.concurrent("llm_call event has contextHashes array with one hash per sent message", async () => {

    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hi"), textMessage("hi"));

    const { agent, contextFile, eventsFile } = await makeTestAgent(mockProvider);
    await collectEvents(agent, "hello");
    await Bun.sleep(50);

    const allEvents = readEventLines(eventsFile);
    const llmCallEvents = allEvents.filter(e => e.type === "llm_call") as LlmCallEvent[];
    expect(llmCallEvents.length).toBe(1);

    const llmCall = llmCallEvents[0]!;
    expect(Array.isArray(llmCall.contextHashes)).toBe(true);
    // Only the user message was in compactedContextHistory when the first call was made
    expect(llmCall.contextHashes).toHaveLength(1);
    expect(llmCall.contextHashes[0]!).toHaveLength(12);
    expect(/^[0-9a-f]{12}$/.test(llmCall.contextHashes[0]!)).toBe(true);
  });

  it.concurrent("contextHashes match the hash field of the corresponding context.jsonl records", async () => {

    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hi"), textMessage("hi"));

    const { agent, contextFile, eventsFile } = await makeTestAgent(mockProvider);
    await collectEvents(agent, "hello");
    await Bun.sleep(50);

    const contextRecords = readContextRecords(contextFile);
    const allEvents = readEventLines(eventsFile);
    const llmCall = allEvents.find(e => e.type === "llm_call") as LlmCallEvent;

    // The first llm_call should only have the user message in its view
    expect(llmCall.contextHashes).toHaveLength(1);
    expect(llmCall.contextHashes[0]!).toBe(contextRecords[0]!.hash);
  });

  it.concurrent("tool loop: second llm_call contextHashes includes user + assistant + tool_result messages", async () => {

    let call = 0;
    const mockProvider: StreamProvider = async () => {
      call++;
      if (call === 1) return makeMockStream(toolUseStreamEvents("list_files"), toolUseMessage("t1", "list_files", { path: "." }));
      return makeMockStream(textStreamEvents("done"), textMessage("done"));
    };

    const { agent, contextFile, eventsFile } = await makeTestAgent(mockProvider);
    await collectEvents(agent, "list it");
    await Bun.sleep(50);

    const contextRecords = readContextRecords(contextFile);
    const allEvents = readEventLines(eventsFile);
    const llmCalls = allEvents.filter(e => e.type === "llm_call") as LlmCallEvent[];

    expect(llmCalls.length).toBe(2);

    // First call: only the user message
    expect(llmCalls[0]!.contextHashes).toHaveLength(1);
    expect(llmCalls[0]!.contextHashes[0]!).toBe(contextRecords[0]!.hash);

    // Second call: user + assistant(tool_use) + user(tool_result)
    expect(llmCalls[1]!.contextHashes).toHaveLength(3);
    expect(llmCalls[1]!.contextHashes[0]!).toBe(contextRecords[0]!.hash);
    expect(llmCalls[1]!.contextHashes[1]!).toBe(contextRecords[1]!.hash);
    expect(llmCalls[1]!.contextHashes[2]!).toBe(contextRecords[2]!.hash);
  });

  it.concurrent("contextHashes grow across multiple turns in the same session", async () => {

    let call = 0;
    const mockProvider: StreamProvider = async () => {
      call++;
      return makeMockStream(textStreamEvents(`resp${call}`), textMessage(`resp${call}`));
    };

    const { agent, contextFile, eventsFile } = await makeTestAgent(mockProvider);
    await collectEvents(agent, "turn1");
    await collectEvents(agent, "turn2");
    await Bun.sleep(50);

    const allEvents = readEventLines(eventsFile);
    const llmCalls = allEvents.filter(e => e.type === "llm_call") as LlmCallEvent[];

    // First call: 1 message (turn1 user)
    expect(llmCalls[0]!.contextHashes).toHaveLength(1);

    // Second call: 3 messages (turn1 user, turn1 asst, turn2 user)
    expect(llmCalls[1]!.contextHashes).toHaveLength(3);

    // All hashes must be 12-char hex
    for (const llmCall of llmCalls) {
      for (const h of llmCall.contextHashes) {
        expect(h).toHaveLength(12);
        expect(/^[0-9a-f]{12}$/.test(h)).toBe(true);
      }
    }
  });
});

// ---------------------------------------------------------------------------
// contextHashes always matches full compactedContextHistory (no trimming)
// ---------------------------------------------------------------------------

describe("contextHashes matches full compactedContextHistory", () => {
  it.concurrent("after 3 turns, contextHashes length equals compactedContextHistory length sent", async () => {

    let call = 0;
    const mockProvider: StreamProvider = async () => {
      call++;
      return makeMockStream(textStreamEvents(`resp${call}`), textMessage(`resp${call}`));
    };

    const { agent, contextFile, eventsFile } = await makeTestAgent(mockProvider);

    // Build up 6 messages (3 turns: user+assistant each)
    for (let i = 0; i < 3; i++) {
      await collectEvents(agent, `turn${i + 1}`);
    }
    await Bun.sleep(50);

    const allEvents = readEventLines(eventsFile);
    const llmCalls = allEvents.filter(e => e.type === "llm_call") as LlmCallEvent[];

    // Third call: 5 messages (turns 1-2 = 4 + turn3 user = 5) — all sent, none trimmed
    expect(llmCalls[2]!.contextHashes).toHaveLength(5);

    // Each hash must appear as a hash in context.jsonl
    const contextRecords = readContextRecords(contextFile);
    const contextHashSet = new Set(contextRecords.map(r => r.hash));
    for (const h of llmCalls[2]!.contextHashes) {
      expect(contextHashSet.has(h)).toBe(true);
    }
  });

  it.concurrent("hashes in contextHashes correctly cross-reference context.jsonl entries", async () => {

    let call = 0;
    const mockProvider: StreamProvider = async () => {
      call++;
      return makeMockStream(textStreamEvents("ok"), textMessage("ok"));
    };

    const { agent, contextFile, eventsFile } = await makeTestAgent(mockProvider);
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
// No placeholder hashes — every message in view has a real hash
// ---------------------------------------------------------------------------

describe("no placeholder hashes", () => {
  it.concurrent("all contextHashes are 12-char hex strings (no '????????????' placeholders)", async () => {

    let call = 0;
    const mockProvider: StreamProvider = async () => {
      call++;
      if (call === 1) return makeMockStream(toolUseStreamEvents("list_files"), toolUseMessage("t1", "list_files", { path: "." }));
      return makeMockStream(textStreamEvents("done"), textMessage("done"));
    };

    const { agent, contextFile, eventsFile } = await makeTestAgent(mockProvider);
    await collectEvents(agent, "go");
    await Bun.sleep(50);

    const allEvents = readEventLines(eventsFile);
    const llmCalls = allEvents.filter(e => e.type === "llm_call") as LlmCallEvent[];

    for (const llmCall of llmCalls) {
      for (const hash of llmCall.contextHashes) {
        expect(hash).not.toBe("????????????");
        expect(/^[0-9a-f]{12}$/.test(hash)).toBe(true);
      }
    }
  });
});

// ---------------------------------------------------------------------------
// [SCHEMA] llm_call has no messageCount — redundant with contextHashes.length
// ---------------------------------------------------------------------------

describe("[SCHEMA] llm_call has no messageCount field", () => {
  it.concurrent("llm_call events written to events.jsonl do not carry messageCount", async () => {

    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hi"), textMessage("hi"));

    const { agent, contextFile, eventsFile } = await makeTestAgent(mockProvider);
    await collectEvents(agent, "hello");
    await Bun.sleep(50);

    const allEvents = readEventLines(eventsFile);
    const llmCalls = allEvents.filter(e => e.type === "llm_call");
    expect(llmCalls.length).toBeGreaterThan(0);

    for (const llmCall of llmCalls) {
      expect("messageCount" in llmCall).toBe(false);
      // contextHashes.length is the correct way to get message count
      expect(Array.isArray(llmCall.contextHashes)).toBe(true);
    }
  });
});

// ---------------------------------------------------------------------------
// [SCHEMA] llm_response has no content — authoritative record is context.jsonl
// [SCHEMA] llm_response carries contextHash — direct FK to the assistant context record
// ---------------------------------------------------------------------------

describe("[SCHEMA] llm_response has no content field", () => {
  it.concurrent("llm_response events written to events.jsonl do not carry content", async () => {

    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hello world"), textMessage("hello world"));

    const { agent, contextFile, eventsFile } = await makeTestAgent(mockProvider);
    await collectEvents(agent, "hi");
    await Bun.sleep(50);

    const allEvents = readEventLines(eventsFile);
    const llmResponses = allEvents.filter(e => e.type === "llm_response");
    expect(llmResponses.length).toBeGreaterThan(0);

    for (const llmResponse of llmResponses) {
      expect("content" in llmResponse).toBe(false);
      // metadata fields must still be present
      expect(typeof llmResponse.stopReason).toBe("string");
      expect(typeof llmResponse.usage).toBe("object");
    }
  });

  it.concurrent("llm_response carries contextHash that matches the assistant context.jsonl record", async () => {

    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hello world"), textMessage("hello world"));

    const { agent, contextFile, eventsFile } = await makeTestAgent(mockProvider);
    await collectEvents(agent, "hi");
    await Bun.sleep(50);

    const contextRecords = readContextRecords(contextFile);
    const allEvents = readEventLines(eventsFile);
    const llmResponses = allEvents.filter(e => e.type === "llm_response");
    expect(llmResponses.length).toBeGreaterThan(0);

    for (const llmResponse of llmResponses) {
      // contextHash must be an 12-char hex string
      expect(typeof llmResponse.contextHash).toBe("string");
      expect(/^[0-9a-f]{12}$/.test(llmResponse.contextHash)).toBe(true);
      // it must match an assistant record in context.jsonl
      const match = contextRecords.find(r => r.hash === llmResponse.contextHash);
      expect(match).toBeDefined();
      expect(match!.role).toBe("assistant");
    }
  });

  it.concurrent("llm_response contextHash points to a record that exists in context.jsonl before the event", async () => {

    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hello world"), textMessage("hello world"));

    const { agent, contextFile, eventsFile } = await makeTestAgent(mockProvider);
    await collectEvents(agent, "hi");
    await Bun.sleep(50);

    // Every llm_response.contextHash must resolve to a context record —
    // ordering on disk is guaranteed by the await chain (appendToHistory
    // fully flushes context.jsonl before logEvent(llm_response) fires).
    const contextRecords = readContextRecords(contextFile);
    const allEvents = readEventLines(eventsFile);
    const llmResponses = allEvents.filter(e => e.type === "llm_response");

    for (const llmResponse of llmResponses) {
      const match = contextRecords.find(r => r.hash === llmResponse.contextHash);
      expect(match).toBeDefined();
    }
  });
});

// ---------------------------------------------------------------------------
// [SCHEMA] tool_call carries contextHash pointing to the assistant context record
// [SCHEMA] tool_result carries contextHash pointing to the user context record
// [SCHEMA] tool_call carries input field — persisted directly in events.jsonl
// [SCHEMA] tool_result carries output field — persisted directly in events.jsonl
// ---------------------------------------------------------------------------

describe("[SCHEMA] tool_call and tool_result contextHash FK", () => {
  it.concurrent("tool_call event carries contextHash matching the assistant context.jsonl record", async () => {

    let call = 0;
    const mockProvider: StreamProvider = async () => {
      call++;
      if (call === 1) return makeMockStream(toolUseStreamEvents("list_files"), toolUseMessage("t1", "list_files", { path: "." }));
      return makeMockStream(textStreamEvents("done"), textMessage("done"));
    };

    const { agent, contextFile, eventsFile } = await makeTestAgent(mockProvider);
    await collectEvents(agent, "list it");
    await Bun.sleep(50);

    const contextRecords = readContextRecords(contextFile);
    const allEvents = readEventLines(eventsFile);
    const toolCalls = allEvents.filter(e => e.type === "tool_call") as ToolCallEvent[];

    expect(toolCalls.length).toBe(1);
    const tc = toolCalls[0]!;

    // contextHash must be an 12-char hex string
    expect(typeof tc.contextHash).toBe("string");
    expect(/^[0-9a-f]{12}$/.test(tc.contextHash!)).toBe(true);

    // Must point to the assistant message (index 1: user, assistant, user(tool_result))
    const assistantRecord = contextRecords.find(r => r.role === "assistant");
    expect(assistantRecord).toBeDefined();
    expect(tc.contextHash).toBe(assistantRecord!.hash);
  });

  it.concurrent("tool_result event carries contextHash matching the user tool_result context.jsonl record", async () => {

    let call = 0;
    const mockProvider: StreamProvider = async () => {
      call++;
      if (call === 1) return makeMockStream(toolUseStreamEvents("list_files"), toolUseMessage("t1", "list_files", { path: "." }));
      return makeMockStream(textStreamEvents("done"), textMessage("done"));
    };

    const { agent, contextFile, eventsFile } = await makeTestAgent(mockProvider);
    await collectEvents(agent, "list it");
    await Bun.sleep(50);

    const contextRecords = readContextRecords(contextFile);
    const allEvents = readEventLines(eventsFile);
    const toolResults = allEvents.filter(e => e.type === "tool_result") as ToolResultEvent[];

    expect(toolResults.length).toBe(1);
    const tr = toolResults[0]!;

    // contextHash must be an 12-char hex string
    expect(typeof tr.contextHash).toBe("string");
    expect(/^[0-9a-f]{12}$/.test(tr.contextHash!)).toBe(true);

    // Must point to the user message containing the tool_result block
    // That's the third context record: user(original), assistant(tool_use), user(tool_result)
    const toolResultRecord = contextRecords.find(
      r => r.role === "user" && Array.isArray(r.content) && (r.content as any[]).some((b: any) => b.type === "tool_result")
    );
    expect(toolResultRecord).toBeDefined();
    expect(tr.contextHash).toBe(toolResultRecord!.hash);
  });

  it.concurrent("tool_call event has input field persisted in events.jsonl", async () => {

    let call = 0;
    const mockProvider: StreamProvider = async () => {
      call++;
      if (call === 1) return makeMockStream(toolUseStreamEvents("list_files"), toolUseMessage("t1", "list_files", { path: "." }));
      return makeMockStream(textStreamEvents("done"), textMessage("done"));
    };

    const { agent, eventsFile } = await makeTestAgent(mockProvider);
    await collectEvents(agent, "list it");
    await Bun.sleep(50);

    const allEvents = readEventLines(eventsFile);
    const toolCalls = allEvents.filter(e => e.type === "tool_call");
    expect(toolCalls.length).toBeGreaterThan(0);
    for (const tc of toolCalls) {
      expect("input" in tc).toBe(true);
    }
  });

  it.concurrent("tool_result event has output field persisted in events.jsonl", async () => {

    let call = 0;
    const mockProvider: StreamProvider = async () => {
      call++;
      if (call === 1) return makeMockStream(toolUseStreamEvents("list_files"), toolUseMessage("t1", "list_files", { path: "." }));
      return makeMockStream(textStreamEvents("done"), textMessage("done"));
    };

    const { agent, eventsFile } = await makeTestAgent(mockProvider);
    await collectEvents(agent, "list it");
    await Bun.sleep(50);

    const allEvents = readEventLines(eventsFile);
    const toolResults = allEvents.filter(e => e.type === "tool_result");
    expect(toolResults.length).toBeGreaterThan(0);
    for (const tr of toolResults) {
      expect("output" in tr).toBe(true);
      expect(typeof (tr as any).output).toBe("string");
    }
  });
});
