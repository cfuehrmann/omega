/**
 * Tests for makeTestAgent factory (src/test-utils.ts).
 *
 * Verifies that:
 * - makeTestAgent() returns an Agent backed by a real session dir under .omega/test-sessions/
 * - makeTestAgent() with a mock provider runs the agentic loop and writes real files
 * - The factory is safe to call with no arguments
 * - Each call produces a distinct session dir (parallelism-safe)
 * - dispose() is a no-op (test sessions are preserved for inspection)
 */

import { describe, it, expect, afterEach } from "bun:test";
import { makeTestAgent } from "./test-utils.js";
import { Agent } from "./agent.js";
import { existsSync } from "fs";
import { TEST_SESSIONS_ROOT } from "./session-dir.js";
import type Anthropic from "@anthropic-ai/sdk";
import type { BetaRawMessageStreamEvent } from "@anthropic-ai/sdk/resources/beta/messages/messages.js";

// Minimal mock stream that returns one text block and stops.
function makeMinimalProvider(text = "hello"): Parameters<typeof makeTestAgent>[0] {
  const message: Anthropic.Beta.Messages.BetaMessage = {
    id: "msg_test",
    type: "message",
    role: "assistant",
    container: null,
    content: [{ type: "text", text, citations: null }],
    model: "claude-sonnet-4-6",
    stop_reason: "end_turn",
    stop_sequence: null,
    stop_details: null,
    context_management: null,
    usage: { input_tokens: 10, output_tokens: 5, cache_creation: null, cache_creation_input_tokens: null, cache_read_input_tokens: null, inference_geo: null, iterations: null, server_tool_use: null, service_tier: null, speed: null },
  };
  return (_params) => ({
    async *[Symbol.asyncIterator](): AsyncGenerator<BetaRawMessageStreamEvent> {
      yield { type: "content_block_start", index: 0, content_block: { type: "text", text: "", citations: null } };
      yield { type: "content_block_delta", index: 0, delta: { type: "text_delta", text } };
      yield { type: "content_block_stop", index: 0 };
      yield { type: "message_delta", context_management: null, delta: { stop_reason: "end_turn", stop_sequence: null, stop_details: null, container: null }, usage: { output_tokens: 5, cache_creation_input_tokens: null, cache_read_input_tokens: null, input_tokens: null, iterations: null, server_tool_use: null } };
    },
    finalMessage: async () => message,
  });
}

const disposeAll: (() => void)[] = [];
afterEach(() => { disposeAll.splice(0).forEach(d => d()); });

describe("makeTestAgent", () => {
  it("returns an Agent instance", async () => {
    const { agent, dispose } = await makeTestAgent();
    disposeAll.push(dispose);
    expect(agent).toBeInstanceOf(Agent);
  });

  it("returns an Agent with a sessionId", async () => {
    const { agent, dispose } = await makeTestAgent();
    disposeAll.push(dispose);
    expect(typeof agent.sessionId).toBe("string");
    expect(agent.sessionId.length).toBeGreaterThan(0);
  });

  it("exposes sessionDir, contextFile, and eventsFile paths under .omega/test-sessions/", async () => {
    const { sessionDir, contextFile, eventsFile, dispose } = await makeTestAgent();
    disposeAll.push(dispose);
    expect(sessionDir).toContain(TEST_SESSIONS_ROOT);
    expect(contextFile).toContain("context.jsonl");
    expect(eventsFile).toContain("events.jsonl");
  });

  it("creates the session directory on disk", async () => {
    const { sessionDir, dispose } = await makeTestAgent();
    disposeAll.push(dispose);
    expect(existsSync(sessionDir)).toBe(true);
  });

  it("accepts a mock stream provider and completes a turn without errors", async () => {
    const { agent, dispose } = await makeTestAgent(makeMinimalProvider("world"));
    disposeAll.push(dispose);
    const events: string[] = [];
    for await (const event of agent.sendMessage("hi", async () => true)) {
      events.push(event.type);
    }
    expect(events).toContain("text");
    expect(events).toContain("turn_end");
  });

  it("writes real context.jsonl and events.jsonl during a turn", async () => {
    const { agent, contextFile, eventsFile, dispose } = await makeTestAgent(makeMinimalProvider("safe"));
    disposeAll.push(dispose);
    for await (const _ of agent.sendMessage("test", async () => true)) { /* drain */ }
    await agent.flushEventLog();
    expect(existsSync(contextFile)).toBe(true);
    expect(existsSync(eventsFile)).toBe(true);
  });

  it("two agents from makeTestAgent have distinct sessionIds", async () => {
    const a = await makeTestAgent();
    const b = await makeTestAgent();
    disposeAll.push(a.dispose, b.dispose);
    expect(a.agent.sessionId).not.toBe(b.agent.sessionId);
  });

  it("two agents from makeTestAgent have distinct sessionDirs", async () => {
    const a = await makeTestAgent();
    const b = await makeTestAgent();
    disposeAll.push(a.dispose, b.dispose);
    expect(a.sessionDir).not.toBe(b.sessionDir);
  });

  it("dispose() is a no-op — session dir is preserved for inspection", async () => {
    const { sessionDir, dispose } = await makeTestAgent();
    expect(existsSync(sessionDir)).toBe(true);
    dispose();
    // dir should still exist — test sessions are intentionally preserved
    expect(existsSync(sessionDir)).toBe(true);
  });
});
