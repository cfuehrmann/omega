/**
 * Tests for makeTestAgent factory (src/test-utils.ts).
 *
 * Verifies that:
 * - makeTestAgent() returns an Agent with no production file writes
 * - makeTestAgent() with a mock provider runs the agentic loop without writing files
 * - The factory is safe to call with no arguments
 */

import { describe, it, expect } from "bun:test";
import { makeTestAgent } from "./test-utils.js";
import { Agent } from "./agent.js";
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

describe("makeTestAgent", () => {
  it("returns an Agent instance", () => {
    const agent = makeTestAgent();
    expect(agent).toBeInstanceOf(Agent);
  });

  it("returns an Agent with a sessionId", () => {
    const agent = makeTestAgent();
    expect(typeof agent.sessionId).toBe("string");
    expect(agent.sessionId.length).toBeGreaterThan(0);
  });

  it("accepts a mock stream provider and completes a turn without errors", async () => {
    const agent = makeTestAgent(makeMinimalProvider("world"));
    const events: string[] = [];
    for await (const event of agent.sendMessage("hi")) {
      events.push(event.type);
    }
    expect(events).toContain("text");
    expect(events).toContain("turn_end");
  });

  it("does not write to sessions/ during a turn", async () => {
    // Layer b (assertNotProductionPath) would throw synchronously if any write
    // to sessions/ were attempted. The turn completing without error is
    // sufficient proof.
    const agent = makeTestAgent(makeMinimalProvider("safe"));
    const events = [];
    for await (const event of agent.sendMessage("test")) {
      events.push(event.type);
    }
    expect(events).toContain("turn_end");
  });

  it("two agents from makeTestAgent have distinct sessionIds", () => {
    const a = makeTestAgent();
    const b = makeTestAgent();
    expect(a.sessionId).not.toBe(b.sessionId);
  });
});
