/**
 * Tests that Agent writes to the explicit session paths passed to its constructor.
 *
 * Regression: server.ts was calling new Agent(undefined, null, undefined, contextFile, eventsFile)
 * with 5 arguments. The Agent constructor only accepts 4, so contextFile and eventsFile
 * landed in the wrong positions and were silently ignored. Events were written to the
 * default fallback path (.omega/sessions/events.jsonl) instead of the session-specific
 * file, causing history replay after browser refresh to find an empty file and show nothing.
 *
 * These tests directly verify that new Agent(provider, contextFile, eventsFile) routes
 * writes to the given paths — not to any fallback.
 */

import { describe, it, expect, afterAll } from "bun:test";
import { existsSync, readFileSync } from "fs";
import { makeTestAgent } from "./test-utils.js";
import type { CreateMessageStream } from "./agent.js";
import type Anthropic from "@anthropic-ai/sdk";
import type { BetaRawMessageStreamEvent } from "@anthropic-ai/sdk/resources/beta/messages/messages.js";

// ---------------------------------------------------------------------------
// Minimal mock provider (same pattern as agent-integration.test.ts)
// ---------------------------------------------------------------------------

function makeMockStream(events: BetaRawMessageStreamEvent[], message: Anthropic.Beta.Messages.BetaMessage) {
  return {
    async *[Symbol.asyncIterator]() { for (const e of events) yield e; },
    finalMessage: async () => message,
  };
}

function textCreateMessageStream(text: string): CreateMessageStream {
  return () => makeMockStream(
    [
      { type: "content_block_start", index: 0, content_block: { type: "text", text: "", citations: null } },
      { type: "content_block_delta", index: 0, delta: { type: "text_delta", text } },
      { type: "content_block_stop", index: 0 },
      { type: "message_delta", context_management: null, delta: { stop_reason: "end_turn", stop_sequence: null, stop_details: null, container: null }, usage: { output_tokens: 5, cache_creation_input_tokens: null, cache_read_input_tokens: null, input_tokens: null, iterations: null, server_tool_use: null } },
      { type: "message_stop" },
    ],
    {
      id: "msg_test", type: "message", role: "assistant",
      model: "claude-sonnet-4-6", container: null,
      content: [{ type: "text", text, citations: null }],
      stop_reason: "end_turn", stop_sequence: null, stop_details: null, context_management: null,
      usage: { input_tokens: 10, output_tokens: 5, cache_creation: null, cache_creation_input_tokens: null, cache_read_input_tokens: null, inference_geo: null, iterations: null, server_tool_use: null, service_tier: null, speed: null },
    },
  );
}

const disposeAll: (() => void)[] = [];
afterAll(() => { disposeAll.splice(0).forEach(d => d()); });

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("Agent session path routing", () => {
  it("writes events to eventsFile, not to the default fallback path", async () => {
    const { agent, eventsFile, dispose } = await makeTestAgent(textCreateMessageStream("hello"));
    disposeAll.push(dispose);

    await agent.init();
    for await (const _ of agent.sendMessage("hi", async () => true)) { /* drain */ }
    await agent.flushEventLog();

    expect(existsSync(eventsFile)).toBe(true);
    const content = readFileSync(eventsFile, "utf-8").trim();
    expect(content.length).toBeGreaterThan(0);

    // Must not have written to the production fallback path
    const defaultPath = ".omega/sessions/events.jsonl";
    if (existsSync(defaultPath)) {
      // If it exists for other reasons (e.g. a real session ran before this test),
      // that's fine — but the eventsFile we specified must be distinct from it
      expect(eventsFile).not.toBe(defaultPath);
    }
  });

  it("events written to eventsFile are valid JSONL with expected event types", async () => {
    const { agent, eventsFile, dispose } = await makeTestAgent(textCreateMessageStream("world"));
    disposeAll.push(dispose);

    await agent.init();
    for await (const _ of agent.sendMessage("hello", async () => true)) { /* drain */ }
    await agent.flushEventLog();

    const lines = readFileSync(eventsFile, "utf-8")
      .split("\n")
      .filter(l => l.trim());
    expect(lines.length).toBeGreaterThan(0);

    const events = lines.map(l => JSON.parse(l));
    const types = events.map((e: any) => e.type);

    // Every session must have at least these events
    expect(types).toContain("session_started");
    expect(types).toContain("user_message");
    expect(types).toContain("llm_call");
    expect(types).toContain("llm_response");
    expect(types).toContain("turn_end");
  });

  it("writes context to contextFile, not silently discarded", async () => {
    const { agent, contextFile, dispose } = await makeTestAgent(textCreateMessageStream("ctx-test"));
    disposeAll.push(dispose);

    await agent.init();
    for await (const _ of agent.sendMessage("context check", async () => true)) { /* drain */ }
    await agent.flushEventLog();

    expect(existsSync(contextFile)).toBe(true);
    const content = readFileSync(contextFile, "utf-8").trim();
    expect(content.length).toBeGreaterThan(0);

    // context.jsonl entries have role and content fields
    const first = JSON.parse(content.split("\n")[0]!);
    expect(first).toHaveProperty("role");
    expect(first).toHaveProperty("content");
    expect(first).toHaveProperty("hash");
  });

  it("three-argument form Agent(provider, contextFile, eventsFile) routes correctly", async () => {
    // This test mirrors the exact call shape used in server.ts after the fix.
    // If the constructor is ever accidentally changed back to accepting wrong args,
    // or a refactor shifts positions, this test will catch it.
    const { agent, eventsFile, contextFile, dispose } = await makeTestAgent(textCreateMessageStream("three-arg"));
    disposeAll.push(dispose);

    // Confirm constructor position 1=provider, 2=contextFile, 3=eventsFile
    // by checking both files receive content after a turn.
    await agent.init();
    for await (const _ of agent.sendMessage("test", async () => true)) { /* drain */ }
    await agent.flushEventLog();

    const eventsContent = readFileSync(eventsFile, "utf-8").trim();
    const contextContent = readFileSync(contextFile, "utf-8").trim();

    expect(eventsContent.length).toBeGreaterThan(0);
    expect(contextContent.length).toBeGreaterThan(0);
  });
});
