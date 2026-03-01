/**
 * Tests for makeTestAgent factory (src/test-utils.ts).
 *
 * Verifies that:
 * - makeTestAgent() returns an Agent backed by a real temp session dir
 * - makeTestAgent() with a mock provider runs the agentic loop and writes real files
 * - The factory is safe to call with no arguments
 * - dispose() removes the temp directory
 */

import { describe, it, expect, afterEach } from "bun:test";
import { makeTestAgent } from "./test-utils.js";
import { Agent } from "./agent.js";
import { existsSync } from "fs";
import type Anthropic from "@anthropic-ai/sdk";

// Minimal mock stream that returns one text block and stops.
function makeMinimalProvider(text = "hello"): Parameters<typeof makeTestAgent>[0] {
  const message: Anthropic.Message = {
    id: "msg_test",
    type: "message",
    role: "assistant",
    content: [{ type: "text", text }],
    model: "claude-sonnet-4-6",
    stop_reason: "end_turn",
    stop_sequence: null,
    usage: { input_tokens: 10, output_tokens: 5, cache_creation_input_tokens: 0, cache_read_input_tokens: 0 },
  };
  return async (_params) => ({
    async *[Symbol.asyncIterator]() {
      yield { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } };
      yield { type: "content_block_delta", index: 0, delta: { type: "text_delta", text } };
      yield { type: "content_block_stop", index: 0 };
      yield { type: "message_delta", delta: { stop_reason: "end_turn" }, usage: { output_tokens: 5 } };
    },
    finalMessage: async () => message,
  });
}

const disposeAll: (() => void)[] = [];
afterEach(() => { disposeAll.splice(0).forEach(d => d()); });

describe("makeTestAgent", () => {
  it("returns an Agent instance", () => {
    const { agent, dispose } = makeTestAgent();
    disposeAll.push(dispose);
    expect(agent).toBeInstanceOf(Agent);
  });

  it("returns an Agent with a sessionId", () => {
    const { agent, dispose } = makeTestAgent();
    disposeAll.push(dispose);
    expect(typeof agent.sessionId).toBe("string");
    expect(agent.sessionId.length).toBeGreaterThan(0);
  });

  it("exposes sessionDir, contextFile, and eventsFile paths", () => {
    const { sessionDir, contextFile, eventsFile, dispose } = makeTestAgent();
    disposeAll.push(dispose);
    expect(sessionDir).toContain("omega-test-");
    expect(contextFile).toContain("context.jsonl");
    expect(eventsFile).toContain("events.jsonl");
  });

  it("accepts a mock stream provider and completes a turn without errors", async () => {
    const { agent, dispose } = makeTestAgent(makeMinimalProvider("world"));
    disposeAll.push(dispose);
    const events: string[] = [];
    for await (const event of agent.sendMessage("hi")) {
      events.push(event.type);
    }
    expect(events).toContain("text");
    expect(events).toContain("turn_end");
  });

  it("writes real context.jsonl and events.jsonl during a turn", async () => {
    const { agent, contextFile, eventsFile, dispose } = makeTestAgent(makeMinimalProvider("safe"));
    disposeAll.push(dispose);
    for await (const _ of agent.sendMessage("test")) { /* drain */ }
    await Bun.sleep(50); // let fire-and-forget writes settle
    expect(existsSync(contextFile)).toBe(true);
    expect(existsSync(eventsFile)).toBe(true);
  });

  it("does not write to .omega/sessions/ (test-guard secondary layer)", async () => {
    // assertNotProductionPath would throw synchronously if any write to .omega/sessions/ occurred.
    const { agent, dispose } = makeTestAgent(makeMinimalProvider("safe"));
    disposeAll.push(dispose);
    const events = [];
    for await (const event of agent.sendMessage("test")) {
      events.push(event.type);
    }
    expect(events).toContain("turn_end");
  });

  it("two agents from makeTestAgent have distinct sessionIds", () => {
    const a = makeTestAgent();
    const b = makeTestAgent();
    disposeAll.push(a.dispose, b.dispose);
    expect(a.agent.sessionId).not.toBe(b.agent.sessionId);
  });

  it("two agents from makeTestAgent have distinct sessionDirs", () => {
    const a = makeTestAgent();
    const b = makeTestAgent();
    disposeAll.push(a.dispose, b.dispose);
    expect(a.sessionDir).not.toBe(b.sessionDir);
  });

  it("dispose() removes the temp directory", () => {
    const { sessionDir, dispose } = makeTestAgent();
    expect(existsSync(sessionDir)).toBe(true);
    dispose();
    expect(existsSync(sessionDir)).toBe(false);
  });
});
