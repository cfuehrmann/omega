/**
 * Tests for adaptive thinking — `thinking: { type: "adaptive" }`.
 *
 * Covers:
 *  - ThinkingSignal events are yielded during streaming
 *  - llm_response event has thinking field populated
 *  - thinking field survives events.jsonl round-trip (serialize → deserialize)
 *  - Multiple thinking blocks are concatenated with divider
 *  - processStreamEvents handles thinking_delta
 */

import { describe, it, expect, afterEach } from "bun:test";
import type Anthropic from "@anthropic-ai/sdk";
import { readFileSync } from "fs";

import { Agent, type OmegaEvent, type StreamSignal, type StreamProvider, processStreamEvents } from "./agent.js";
import { makeTestAgent } from "./test-utils.js";

const disposeAll: (() => void)[] = [];
afterEach(() => { disposeAll.splice(0).forEach(d => d()); });

// ---------------------------------------------------------------------------
// Mock helpers
// ---------------------------------------------------------------------------

function makeMockStream(events: any[], message: any) {
  return {
    async *[Symbol.asyncIterator]() {
      for (const e of events) yield e;
    },
    finalMessage: async () => message,
  };
}

/** Stream events for a response with a single thinking block then text. */
function thinkingThenTextStreamEvents(thinking: string, text: string): any[] {
  return [
    { type: "content_block_start", index: 0, content_block: { type: "thinking", thinking: "" } },
    { type: "content_block_delta", index: 0, delta: { type: "thinking_delta", thinking: thinking.slice(0, 10) } },
    { type: "content_block_delta", index: 0, delta: { type: "thinking_delta", thinking: thinking.slice(10) } },
    { type: "content_block_stop", index: 0 },
    { type: "content_block_start", index: 1, content_block: { type: "text", text: "" } },
    { type: "content_block_delta", index: 1, delta: { type: "text_delta", text } },
    { type: "content_block_stop", index: 1 },
    { type: "message_delta", delta: { stop_reason: "end_turn" }, usage: { output_tokens: 30 } },
    { type: "message_stop" },
  ];
}

/** Final message for a response with thinking + text. */
function thinkingThenTextMessage(thinking: string, text: string): any {
  return {
    id: "msg_thinking_test",
    type: "message",
    role: "assistant",
    model: "claude-sonnet-4-6",
    content: [
      { type: "thinking", thinking, signature: "test-sig" },
      { type: "text", text },
    ],
    stop_reason: "end_turn",
    stop_sequence: null,
    usage: { input_tokens: 100, output_tokens: 30 },
  };
}

/** Stream events for a response with two thinking blocks then text. */
function twoThinkingBlocksStreamEvents(
  thinking1: string,
  thinking2: string,
  text: string
): any[] {
  return [
    { type: "content_block_start", index: 0, content_block: { type: "thinking", thinking: "" } },
    { type: "content_block_delta", index: 0, delta: { type: "thinking_delta", thinking: thinking1 } },
    { type: "content_block_stop", index: 0 },
    { type: "content_block_start", index: 1, content_block: { type: "thinking", thinking: "" } },
    { type: "content_block_delta", index: 1, delta: { type: "thinking_delta", thinking: thinking2 } },
    { type: "content_block_stop", index: 1 },
    { type: "content_block_start", index: 2, content_block: { type: "text", text: "" } },
    { type: "content_block_delta", index: 2, delta: { type: "text_delta", text } },
    { type: "content_block_stop", index: 2 },
    { type: "message_delta", delta: { stop_reason: "end_turn" }, usage: { output_tokens: 40 } },
    { type: "message_stop" },
  ];
}

/** Collect all emitted signals/events from a sendMessage call. */
async function collectAll(
  agent: Agent,
  message: string
): Promise<(OmegaEvent | StreamSignal)[]> {
  const results: (OmegaEvent | StreamSignal)[] = [];
  for await (const ev of agent.sendMessage(message, async () => true)) {
    results.push(ev);
  }
  return results;
}

// ---------------------------------------------------------------------------
// processStreamEvents — unit tests (no agent needed)
// ---------------------------------------------------------------------------

describe("processStreamEvents — thinking_delta", () => {
  it("emits ThinkingSignal for thinking_delta events", () => {
    const rawEvents = [
      { type: "content_block_start", index: 0, content_block: { type: "thinking", thinking: "" } },
      { type: "content_block_delta", index: 0, delta: { type: "thinking_delta", thinking: "Let me think..." } },
      { type: "content_block_delta", index: 0, delta: { type: "thinking_delta", thinking: " More thinking." } },
      { type: "content_block_stop", index: 0 },
    ];
    const signals = processStreamEvents(rawEvents);
    const thinkingSignals = signals.filter(s => s.type === "thinking");
    expect(thinkingSignals).toHaveLength(2);
    expect(thinkingSignals[0]).toEqual({ type: "thinking", text: "Let me think..." });
    expect(thinkingSignals[1]).toEqual({ type: "thinking", text: " More thinking." });
  });

  it("emits both thinking and text signals in order", () => {
    const rawEvents = [
      { type: "content_block_start", index: 0, content_block: { type: "thinking", thinking: "" } },
      { type: "content_block_delta", index: 0, delta: { type: "thinking_delta", thinking: "thinking" } },
      { type: "content_block_stop", index: 0 },
      { type: "content_block_start", index: 1, content_block: { type: "text", text: "" } },
      { type: "content_block_delta", index: 1, delta: { type: "text_delta", text: "response" } },
      { type: "content_block_stop", index: 1 },
    ];
    const signals = processStreamEvents(rawEvents);
    expect(signals).toHaveLength(2);
    expect(signals[0]).toEqual({ type: "thinking", text: "thinking" });
    expect(signals[1]).toEqual({ type: "text", text: "response" });
  });

  it("emits no thinking signals for text-only response", () => {
    const rawEvents = [
      { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } },
      { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "hello" } },
      { type: "content_block_stop", index: 0 },
    ];
    const signals = processStreamEvents(rawEvents);
    expect(signals.filter(s => s.type === "thinking")).toHaveLength(0);
    expect(signals.filter(s => s.type === "text")).toHaveLength(1);
  });
});

// ---------------------------------------------------------------------------
// Agent integration — thinking yielded and persisted
// ---------------------------------------------------------------------------

describe("Agent — adaptive thinking", () => {
  it("yields ThinkingSignals during streaming", async () => {
    const thinking = "Let me reason about this carefully.";
    const text = "Here is my answer.";

    const provider: StreamProvider = async () =>
      makeMockStream(
        thinkingThenTextStreamEvents(thinking, text),
        thinkingThenTextMessage(thinking, text)
      );

    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);

    const events = await collectAll(agent, "test question");
    const thinkingSignals = events.filter(e => e.type === "thinking");
    expect(thinkingSignals.length).toBeGreaterThan(0);
    // All thinking signals concatenated should equal the full thinking string
    const assembled = thinkingSignals
      .map(e => (e as { type: "thinking"; text: string }).text)
      .join("");
    expect(assembled).toBe(thinking);
  });

  it("includes thinking field on llm_response event", async () => {
    const thinking = "I need to think carefully before answering.";
    const text = "The answer is 42.";

    const provider: StreamProvider = async () =>
      makeMockStream(
        thinkingThenTextStreamEvents(thinking, text),
        thinkingThenTextMessage(thinking, text)
      );

    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);

    const events = await collectAll(agent, "what is the answer?");
    const llmResponse = events.find(e => e.type === "llm_response") as
      | (OmegaEvent & { type: "llm_response" })
      | undefined;

    expect(llmResponse).toBeDefined();
    expect(llmResponse!.thinking).toBe(thinking);
    expect(llmResponse!.text).toBe(text);
  });

  it("thinking field absent when no thinking blocks present", async () => {
    const text = "Plain response without thinking.";

    const provider: StreamProvider = async () =>
      makeMockStream(
        [
          { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } },
          { type: "content_block_delta", index: 0, delta: { type: "text_delta", text } },
          { type: "content_block_stop", index: 0 },
          { type: "message_delta", delta: { stop_reason: "end_turn" }, usage: { output_tokens: 10 } },
          { type: "message_stop" },
        ],
        {
          id: "msg_no_thinking",
          type: "message",
          role: "assistant",
          model: "claude-sonnet-4-6",
          content: [{ type: "text", text }],
          stop_reason: "end_turn",
          stop_sequence: null,
          usage: { input_tokens: 10, output_tokens: 10 },
        }
      );

    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);

    const events = await collectAll(agent, "hello");
    const llmResponse = events.find(e => e.type === "llm_response") as
      | (OmegaEvent & { type: "llm_response" })
      | undefined;

    expect(llmResponse).toBeDefined();
    expect(llmResponse!.thinking).toBeUndefined();
  });

  it("concatenates multiple thinking blocks with divider", async () => {
    const thinking1 = "First reasoning block.";
    const thinking2 = "Second reasoning block.";
    const text = "Final answer.";

    const provider: StreamProvider = async () =>
      makeMockStream(
        twoThinkingBlocksStreamEvents(thinking1, thinking2, text),
        {
          id: "msg_two_thinking",
          type: "message",
          role: "assistant",
          model: "claude-sonnet-4-6",
          content: [
            { type: "thinking", thinking: thinking1, signature: "sig1" },
            { type: "thinking", thinking: thinking2, signature: "sig2" },
            { type: "text", text },
          ],
          stop_reason: "end_turn",
          stop_sequence: null,
          usage: { input_tokens: 50, output_tokens: 40 },
        }
      );

    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);

    const events = await collectAll(agent, "think twice");
    const llmResponse = events.find(e => e.type === "llm_response") as
      | (OmegaEvent & { type: "llm_response" })
      | undefined;

    expect(llmResponse).toBeDefined();
    expect(llmResponse!.thinking).toBe(`${thinking1}\n\n---\n\n${thinking2}`);
  });

  it("thinking field survives events.jsonl round-trip", async () => {
    const thinking = "Round-trip thinking content.";
    const text = "Round-trip response.";

    const provider: StreamProvider = async () =>
      makeMockStream(
        thinkingThenTextStreamEvents(thinking, text),
        thinkingThenTextMessage(thinking, text)
      );

    const { agent, eventsFile, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);

    await collectAll(agent, "round-trip test");

    // Read events.jsonl and find the llm_response line
    const lines = readFileSync(eventsFile, "utf-8")
      .split("\n")
      .filter(l => l.trim());
    const llmResponseLine = lines.find(l => {
      try { return JSON.parse(l).type === "llm_response"; } catch { return false; }
    });

    expect(llmResponseLine).toBeDefined();
    const parsed = JSON.parse(llmResponseLine!);
    expect(parsed.thinking).toBe(thinking);
    expect(parsed.text).toBe(text);
  });
});
