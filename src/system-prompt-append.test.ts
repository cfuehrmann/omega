/**
 * Tests for system-prompt-append persistence and injection into the agent.
 *
 * Three layers tested:
 *  1. File I/O — readSystemPromptAppend / writeSystemPromptAppend
 *  2. Agent.buildSystemPrompt() — returns base prompt alone when no content
 *     is loaded; appends content when it is.
 *  3. Agent.loadSystemPromptAppend() + end-to-end — content loaded from a
 *     real file reaches the system field of the outgoing API request.
 */

import { describe, it, expect, beforeEach, afterEach } from "bun:test";
import { readSystemPromptAppend, writeSystemPromptAppend, systemPromptAppendPath } from "./system-prompt-append.js";
import { Agent, type StreamProvider } from "./agent.js";
import { makeTestAgent } from "./test-utils.js";
import { config } from "./config.js";
import { mkdtemp, rm, mkdir, writeFile } from "fs/promises";
import { tmpdir } from "os";
import { join } from "path";
import type Anthropic from "@anthropic-ai/sdk";

// ---------------------------------------------------------------------------
// Shared cleanup
// ---------------------------------------------------------------------------

let tempDir: string;
const disposeAll: (() => void)[] = [];

beforeEach(async () => {
  tempDir = await mkdtemp(join(tmpdir(), "omega-spa-test-"));
});

afterEach(async () => {
  disposeAll.splice(0).forEach(d => d());
  await rm(tempDir, { recursive: true, force: true });
});

// ---------------------------------------------------------------------------
// Mock stream helpers (mirrors agent-integration.test.ts pattern)
// ---------------------------------------------------------------------------

function makeMockStream(events: any[], message: Anthropic.Message) {
  return {
    async *[Symbol.asyncIterator]() { for (const e of events) yield e; },
    finalMessage: async () => message,
  };
}

function textMessage(text: string): Anthropic.Message {
  return {
    id: "msg_test", type: "message", role: "assistant",
    model: "claude-sonnet-4-6",
    content: [{ type: "text", text }],
    stop_reason: "end_turn", stop_sequence: null,
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

async function collectEvents(agent: Agent, msg: string) {
  const events: any[] = [];
  for await (const e of agent.sendMessage(msg, async () => true)) events.push(e);
  return events;
}

// ---------------------------------------------------------------------------
// 1. File I/O
// ---------------------------------------------------------------------------

describe("readSystemPromptAppend", () => {
  it("returns null when file does not exist", async () => {
    const result = await readSystemPromptAppend(join(tempDir, "system-prompt-append.md"));
    expect(result).toBeNull();
  });

  it("returns content when file exists", async () => {
    const path = join(tempDir, "system-prompt-append.md");
    await writeSystemPromptAppend("Hello append content.", path);
    const result = await readSystemPromptAppend(path);
    expect(result).toBe("Hello append content.");
  });
});

describe("writeSystemPromptAppend", () => {
  it("writes content to file", async () => {
    const path = join(tempDir, "system-prompt-append.md");
    await writeSystemPromptAppend("State: all good.", path);
    const result = await readSystemPromptAppend(path);
    expect(result).toBe("State: all good.");
  });

  it("overwrites existing content", async () => {
    const path = join(tempDir, "system-prompt-append.md");
    await writeSystemPromptAppend("Old content.", path);
    await writeSystemPromptAppend("New content.", path);
    const result = await readSystemPromptAppend(path);
    expect(result).toBe("New content.");
  });

  it("creates parent directories if needed", async () => {
    const path = join(tempDir, "nested", "dir", "system-prompt-append.md");
    await writeSystemPromptAppend("Nested content.", path);
    const result = await readSystemPromptAppend(path);
    expect(result).toBe("Nested content.");
  });
});

// ---------------------------------------------------------------------------
// 2. Agent.buildSystemPrompt()
// ---------------------------------------------------------------------------

describe("Agent.buildSystemPrompt()", () => {
  it("returns base system prompt when no append content is loaded", async () => {
    const { agent, dispose } = await makeTestAgent();
    disposeAll.push(dispose);
    const prompt = agent.buildSystemPrompt();
    expect(prompt).toContain(config.systemPrompt);
    expect(prompt).not.toContain("World State");
  });

  it("appends content under a World State section when content is loaded", async () => {
    const { agent, dispose } = await makeTestAgent();
    disposeAll.push(dispose);
    const appendPath = join(tempDir, "system-prompt-append.md");
    await writeSystemPromptAppend("## My project state\nAll good.", appendPath);
    await agent.loadSystemPromptAppend(appendPath);
    const prompt = agent.buildSystemPrompt();
    expect(prompt).toContain(config.systemPrompt);
    expect(prompt).toContain("World State (from previous sessions)");
    expect(prompt).toContain("## My project state\nAll good.");
  });

  it("append content appears after the base prompt", async () => {
    const { agent, dispose } = await makeTestAgent();
    disposeAll.push(dispose);
    const appendPath = join(tempDir, "system-prompt-append.md");
    await writeSystemPromptAppend("APPENDED", appendPath);
    await agent.loadSystemPromptAppend(appendPath);
    const prompt = agent.buildSystemPrompt();
    expect(prompt.indexOf(config.systemPrompt)).toBeLessThan(prompt.indexOf("APPENDED"));
  });

  it("returns base prompt when file does not exist (graceful no-op)", async () => {
    const { agent, dispose } = await makeTestAgent();
    disposeAll.push(dispose);
    // Point at a non-existent file — should not throw, should leave content null
    await agent.loadSystemPromptAppend(join(tempDir, "nonexistent.md"));
    const prompt = agent.buildSystemPrompt();
    expect(prompt).toBe(agent.buildSystemPrompt()); // stable, no append
    expect(prompt).not.toContain("World State");
  });
});

// ---------------------------------------------------------------------------
// 3. End-to-end: content reaches the API request system field
// ---------------------------------------------------------------------------

describe("system-prompt-append end-to-end: content reaches API request", () => {
  it("system field of outgoing request contains base prompt when no file loaded", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hi"), textMessage("hi"));
    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);

    const events = await collectEvents(agent, "hello");
    const llmCall = events.find(e => e.type === "llm_call") as any;
    expect(llmCall).toBeDefined();

    // system is an array of blocks in the Anthropic request
    const systemText = llmCall.request.system
      .map((b: any) => b.text ?? "")
      .join("");
    expect(systemText).toContain(config.systemPrompt);
    expect(systemText).not.toContain("World State");
  });

  it("system field contains appended content when file is loaded", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hi"), textMessage("hi"));
    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);

    const appendPath = join(tempDir, "system-prompt-append.md");
    await writeSystemPromptAppend("SENTINEL_CONTENT_XYZ", appendPath);
    await agent.loadSystemPromptAppend(appendPath);

    const events = await collectEvents(agent, "hello");
    const llmCall = events.find(e => e.type === "llm_call") as any;
    const systemText = llmCall.request.system
      .map((b: any) => b.text ?? "")
      .join("");
    expect(systemText).toContain("SENTINEL_CONTENT_XYZ");
    expect(systemText).toContain("World State (from previous sessions)");
  });

  it("appended content is absent when file does not exist", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hi"), textMessage("hi"));
    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);

    // Explicitly try to load from a non-existent path
    await agent.loadSystemPromptAppend(join(tempDir, "no-such-file.md"));

    const events = await collectEvents(agent, "hello");
    const llmCall = events.find(e => e.type === "llm_call") as any;
    const systemText = llmCall.request.system
      .map((b: any) => b.text ?? "")
      .join("");
    expect(systemText).not.toContain("World State");
  });

  it("appended content is present across multiple turns (stable in system prompt)", async () => {
    let callCount = 0;
    const mockProvider: StreamProvider = async () => {
      callCount++;
      return makeMockStream(textStreamEvents(`reply ${callCount}`), textMessage(`reply ${callCount}`));
    };
    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);

    const appendPath = join(tempDir, "system-prompt-append.md");
    await writeSystemPromptAppend("PERSISTENT_STATE", appendPath);
    await agent.loadSystemPromptAppend(appendPath);

    const events1 = await collectEvents(agent, "turn one");
    const events2 = await collectEvents(agent, "turn two");

    for (const events of [events1, events2]) {
      const llmCall = events.find(e => e.type === "llm_call") as any;
      const systemText = llmCall.request.system
        .map((b: any) => b.text ?? "")
        .join("");
      expect(systemText).toContain("PERSISTENT_STATE");
    }
  });
});
