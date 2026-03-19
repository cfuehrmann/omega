/**
 * Tests for computeDurations and computeLiveDurations in the web client store.
 *
 * These functions derive timing metrics purely from event timestamps —
 * never from Date.now() — so they are safe for replay after reconnect.
 */

import { describe, it, expect } from "bun:test";
import { computeDurations, computeLiveDurations } from "./store.js";
import type { WsEvent } from "./store.js";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function ms(base: number, offsetMs: number): string {
  return new Date(base + offsetMs).toISOString();
}

const BASE = 1_700_000_000_000; // arbitrary fixed epoch

// ---------------------------------------------------------------------------
// computeDurations
// ---------------------------------------------------------------------------

describe("computeDurations — LLM time", () => {
  it("returns zeros for an empty event list", () => {
    const d = computeDurations([]);
    expect(d.llmMs).toBe(0);
    expect(d.toolMs).toBe(0);
    expect(d.turnMs).toBe(0);
  });

  it("measures a single llm_call → llm_response pair", () => {
    const events: WsEvent[] = [
      { type: "user_message", ts: ms(BASE, 0), content: "hi" },
      { type: "llm_call", ts: ms(BASE, 100), provider: "anthropic", url: "", model: "m", contextHashes: [], cacheBreakpointIndex: null },
      { type: "llm_response", ts: ms(BASE, 1100), stopReason: "end_turn", usage: { input_tokens: 10, output_tokens: 5 }, contextHash: "abc" },
      { type: "turn_end", ts: ms(BASE, 1200), metrics: { inputTokens: 10, outputTokens: 5 } },
    ];
    const d = computeDurations(events);
    expect(d.llmMs).toBe(1000);
  });

  it("sums multiple llm call durations across a tool loop", () => {
    const events: WsEvent[] = [
      { type: "user_message", ts: ms(BASE, 0), content: "hi" },
      // First LLM call: 500ms
      { type: "llm_call", ts: ms(BASE, 0), provider: "anthropic", url: "", model: "m", contextHashes: [], cacheBreakpointIndex: null },
      { type: "llm_response", ts: ms(BASE, 500), stopReason: "tool_use", usage: { input_tokens: 10, output_tokens: 5 }, contextHash: "abc" },
      { type: "tool_call", ts: ms(BASE, 500), id: "t1", name: "read_file", input: {} },
      { type: "tool_result", ts: ms(BASE, 800), id: "t1", name: "read_file", isError: false, durationMs: 300, output: "x" },
      // Second LLM call: 700ms
      { type: "llm_call", ts: ms(BASE, 900), provider: "anthropic", url: "", model: "m", contextHashes: [], cacheBreakpointIndex: null },
      { type: "llm_response", ts: ms(BASE, 1600), stopReason: "end_turn", usage: { input_tokens: 15, output_tokens: 8 }, contextHash: "def" },
      { type: "turn_end", ts: ms(BASE, 1700), metrics: { inputTokens: 25, outputTokens: 13 } },
    ];
    const d = computeDurations(events);
    expect(d.llmMs).toBe(500 + 700); // 1200
  });
});

describe("computeDurations — tool time", () => {
  it("measures a single tool batch (one tool)", () => {
    const events: WsEvent[] = [
      { type: "user_message", ts: ms(BASE, 0), content: "hi" },
      { type: "llm_call", ts: ms(BASE, 0), provider: "anthropic", url: "", model: "m", contextHashes: [], cacheBreakpointIndex: null },
      { type: "llm_response", ts: ms(BASE, 500), stopReason: "tool_use", usage: { input_tokens: 10, output_tokens: 5 }, contextHash: "abc" },
      // tool call at 500, result at 900 → span = 400ms
      { type: "tool_call", ts: ms(BASE, 500), id: "t1", name: "run_command", input: {} },
      { type: "tool_result", ts: ms(BASE, 900), id: "t1", name: "run_command", isError: false, durationMs: 400, output: "ok" },
      { type: "llm_call", ts: ms(BASE, 950), provider: "anthropic", url: "", model: "m", contextHashes: [], cacheBreakpointIndex: null },
      { type: "llm_response", ts: ms(BASE, 1400), stopReason: "end_turn", usage: { input_tokens: 15, output_tokens: 8 }, contextHash: "def" },
      { type: "turn_end", ts: ms(BASE, 1500), metrics: { inputTokens: 25, outputTokens: 13 } },
    ];
    const d = computeDurations(events);
    expect(d.toolMs).toBe(400);
  });

  it("measures a parallel batch: span = max(result.ts) − min(call.ts)", () => {
    const events: WsEvent[] = [
      { type: "user_message", ts: ms(BASE, 0), content: "hi" },
      { type: "llm_call", ts: ms(BASE, 0), provider: "anthropic", url: "", model: "m", contextHashes: [], cacheBreakpointIndex: null },
      { type: "llm_response", ts: ms(BASE, 200), stopReason: "tool_use", usage: { input_tokens: 10, output_tokens: 5 }, contextHash: "abc" },
      // Two parallel tool calls, first call at 200, second call at 210
      // Results come back at 600 and 800 → span = 800 − 200 = 600ms
      { type: "tool_call", ts: ms(BASE, 200), id: "t1", name: "run_command", input: {} },
      { type: "tool_call", ts: ms(BASE, 210), id: "t2", name: "read_file", input: {} },
      { type: "tool_result", ts: ms(BASE, 600), id: "t1", name: "run_command", isError: false, durationMs: 400, output: "a" },
      { type: "tool_result", ts: ms(BASE, 800), id: "t2", name: "read_file", isError: false, durationMs: 590, output: "b" },
      { type: "llm_call", ts: ms(BASE, 850), provider: "anthropic", url: "", model: "m", contextHashes: [], cacheBreakpointIndex: null },
      { type: "llm_response", ts: ms(BASE, 1500), stopReason: "end_turn", usage: { input_tokens: 20, output_tokens: 10 }, contextHash: "def" },
      { type: "turn_end", ts: ms(BASE, 1600), metrics: { inputTokens: 30, outputTokens: 15 } },
    ];
    const d = computeDurations(events);
    expect(d.toolMs).toBe(600); // 800 − 200
  });

  it("sums tool spans across multiple batches", () => {
    const events: WsEvent[] = [
      { type: "user_message", ts: ms(BASE, 0), content: "hi" },
      { type: "llm_call", ts: ms(BASE, 0), provider: "anthropic", url: "", model: "m", contextHashes: [], cacheBreakpointIndex: null },
      { type: "llm_response", ts: ms(BASE, 100), stopReason: "tool_use", usage: { input_tokens: 5, output_tokens: 2 }, contextHash: "a" },
      // Batch 1: 300ms span
      { type: "tool_call", ts: ms(BASE, 100), id: "t1", name: "read_file", input: {} },
      { type: "tool_result", ts: ms(BASE, 400), id: "t1", name: "read_file", isError: false, durationMs: 300, output: "x" },
      { type: "llm_call", ts: ms(BASE, 420), provider: "anthropic", url: "", model: "m", contextHashes: [], cacheBreakpointIndex: null },
      { type: "llm_response", ts: ms(BASE, 700), stopReason: "tool_use", usage: { input_tokens: 8, output_tokens: 3 }, contextHash: "b" },
      // Batch 2: 200ms span
      { type: "tool_call", ts: ms(BASE, 700), id: "t2", name: "write_file", input: {} },
      { type: "tool_result", ts: ms(BASE, 900), id: "t2", name: "write_file", isError: false, durationMs: 200, output: "ok" },
      { type: "llm_call", ts: ms(BASE, 920), provider: "anthropic", url: "", model: "m", contextHashes: [], cacheBreakpointIndex: null },
      { type: "llm_response", ts: ms(BASE, 1200), stopReason: "end_turn", usage: { input_tokens: 12, output_tokens: 6 }, contextHash: "c" },
      { type: "turn_end", ts: ms(BASE, 1300), metrics: { inputTokens: 25, outputTokens: 11 } },
    ];
    const d = computeDurations(events);
    expect(d.toolMs).toBe(300 + 200); // 500
  });

  it("returns 0 toolMs when no tool calls were made", () => {
    const events: WsEvent[] = [
      { type: "user_message", ts: ms(BASE, 0), content: "hi" },
      { type: "llm_call", ts: ms(BASE, 0), provider: "anthropic", url: "", model: "m", contextHashes: [], cacheBreakpointIndex: null },
      { type: "llm_response", ts: ms(BASE, 800), stopReason: "end_turn", usage: { input_tokens: 10, output_tokens: 5 }, contextHash: "abc" },
      { type: "turn_end", ts: ms(BASE, 900), metrics: { inputTokens: 10, outputTokens: 5 } },
    ];
    const d = computeDurations(events);
    expect(d.toolMs).toBe(0);
  });
});

describe("computeDurations — turn time", () => {
  it("measures turn_end.ts − user_message.ts", () => {
    const events: WsEvent[] = [
      { type: "user_message", ts: ms(BASE, 0), content: "hi" },
      { type: "llm_call", ts: ms(BASE, 50), provider: "anthropic", url: "", model: "m", contextHashes: [], cacheBreakpointIndex: null },
      { type: "llm_response", ts: ms(BASE, 900), stopReason: "end_turn", usage: { input_tokens: 10, output_tokens: 5 }, contextHash: "abc" },
      { type: "turn_end", ts: ms(BASE, 1000), metrics: { inputTokens: 10, outputTokens: 5 } },
    ];
    const d = computeDurations(events);
    expect(d.turnMs).toBe(1000);
  });

  it("returns 0 turnMs when turn_end is absent", () => {
    const events: WsEvent[] = [
      { type: "user_message", ts: ms(BASE, 0), content: "hi" },
      { type: "llm_call", ts: ms(BASE, 50), provider: "anthropic", url: "", model: "m", contextHashes: [], cacheBreakpointIndex: null },
      { type: "llm_response", ts: ms(BASE, 900), stopReason: "end_turn", usage: { input_tokens: 10, output_tokens: 5 }, contextHash: "abc" },
      // No turn_end
    ];
    const d = computeDurations(events);
    expect(d.turnMs).toBe(0);
  });
});

// ---------------------------------------------------------------------------
// computeLiveDurations
// ---------------------------------------------------------------------------

describe("computeLiveDurations — LLM time", () => {
  it("accumulates llmMs as each llm_response arrives", () => {
    const events: WsEvent[] = [
      { type: "user_message", ts: ms(BASE, 0), content: "hi" },
      { type: "llm_call", ts: ms(BASE, 0), provider: "anthropic", url: "", model: "m", contextHashes: [], cacheBreakpointIndex: null },
      { type: "llm_response", ts: ms(BASE, 600), stopReason: "tool_use", usage: { input_tokens: 10, output_tokens: 5 }, contextHash: "abc" },
    ];
    const d = computeLiveDurations(events);
    expect(d.llmMs).toBe(600);
    expect(d.turnMs).toBe(0); // always 0 for live
  });
});

describe("computeLiveDurations — tool time", () => {
  it("flushes a batch as soon as all results arrive", () => {
    const events: WsEvent[] = [
      { type: "user_message", ts: ms(BASE, 0), content: "hi" },
      { type: "llm_call", ts: ms(BASE, 0), provider: "anthropic", url: "", model: "m", contextHashes: [], cacheBreakpointIndex: null },
      { type: "llm_response", ts: ms(BASE, 200), stopReason: "tool_use", usage: { input_tokens: 5, output_tokens: 2 }, contextHash: "a" },
      { type: "tool_call", ts: ms(BASE, 200), id: "t1", name: "read_file", input: {} },
      { type: "tool_call", ts: ms(BASE, 205), id: "t2", name: "write_file", input: {} },
      // Only first result in — batch not yet complete
      { type: "tool_result", ts: ms(BASE, 500), id: "t1", name: "read_file", isError: false, durationMs: 300, output: "x" },
    ];
    // After only t1 result, toolMs should still be 0 (batch not complete)
    const d1 = computeLiveDurations(events);
    expect(d1.toolMs).toBe(0);

    // Now t2 result arrives — batch complete: 700 − 200 = 500ms
    const events2: WsEvent[] = [
      ...events,
      { type: "tool_result", ts: ms(BASE, 700), id: "t2", name: "write_file", isError: false, durationMs: 495, output: "ok" },
    ];
    const d2 = computeLiveDurations(events2);
    expect(d2.toolMs).toBe(500); // 700 − 200
  });

  it("does not double-count a batch after it has been flushed", () => {
    const events: WsEvent[] = [
      { type: "user_message", ts: ms(BASE, 0), content: "hi" },
      { type: "llm_call", ts: ms(BASE, 0), provider: "anthropic", url: "", model: "m", contextHashes: [], cacheBreakpointIndex: null },
      { type: "llm_response", ts: ms(BASE, 100), stopReason: "tool_use", usage: { input_tokens: 5, output_tokens: 2 }, contextHash: "a" },
      { type: "tool_call", ts: ms(BASE, 100), id: "t1", name: "read_file", input: {} },
      { type: "tool_result", ts: ms(BASE, 400), id: "t1", name: "read_file", isError: false, durationMs: 300, output: "x" },
      // Second LLM call starts — this resets the batch window
      { type: "llm_call", ts: ms(BASE, 450), provider: "anthropic", url: "", model: "m", contextHashes: [], cacheBreakpointIndex: null },
      { type: "llm_response", ts: ms(BASE, 900), stopReason: "end_turn", usage: { input_tokens: 10, output_tokens: 5 }, contextHash: "b" },
    ];
    const d = computeLiveDurations(events);
    // toolMs = 300 (only the first batch)
    expect(d.toolMs).toBe(300);
    // llmMs = 100 (first call) + 450 (second call) = 550
    expect(d.llmMs).toBe(100 + 450);
  });

  it("always returns turnMs = 0", () => {
    const events: WsEvent[] = [
      { type: "user_message", ts: ms(BASE, 0), content: "hi" },
      { type: "llm_call", ts: ms(BASE, 0), provider: "anthropic", url: "", model: "m", contextHashes: [], cacheBreakpointIndex: null },
      { type: "llm_response", ts: ms(BASE, 500), stopReason: "end_turn", usage: { input_tokens: 10, output_tokens: 5 }, contextHash: "abc" },
    ];
    const d = computeLiveDurations(events);
    expect(d.turnMs).toBe(0);
  });
});
