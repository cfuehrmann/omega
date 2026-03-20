/**
 * Tests for session resilience: open turns on shutdown, partial-turn replay.
 *
 * Covers three related bugs that caused the "stuck streaming" UI state:
 *  1. Server saves partial turn on shutdown → replay locks UI in streaming=true
 *  2. Individual streaming text events bloat the session file uselessly
 *  3. Store replay has no defence against an already-open turn
 */

import { describe, it, expect } from "bun:test";
import { closeOpenTurn, shouldLogEvent } from "./server.js";
import { dispatch, state } from "./client/store.js";

// ---------------------------------------------------------------------------
// closeOpenTurn
// ---------------------------------------------------------------------------

describe("closeOpenTurn", () => {
  it("returns the log unchanged when it is empty", () => {
    expect(closeOpenTurn([])).toEqual([]);
  });

  it("returns the log unchanged when the last turn is already closed by turn_end", () => {
    const log = [
      { type: "user_message", content: "hi" },
      { type: "turn_end", metrics: {} },
    ];
    expect(closeOpenTurn(log)).toEqual(log);
  });

  it("returns the log unchanged when the last turn is already closed by turn_interrupted", () => {
    const log = [
      { type: "user_message", content: "hi" },
      { type: "turn_interrupted" },
    ];
    expect(closeOpenTurn(log)).toEqual(log);
  });

  it("appends turn_interrupted when the last user_message has no closing event", () => {
    const log = [
      { type: "user_message", content: "hi" },
      { type: "llm_call", provider: "anthropic", url: "https://api.anthropic.com/v1/messages", request: {} },
      { type: "text", text: "Partial" },
    ];
    const result = closeOpenTurn(log);
    expect(result).toHaveLength(log.length + 1);
    expect(result[result.length - 1]).toEqual({ type: "turn_interrupted" });
  });

  it("handles multiple turns: only patches the open last turn", () => {
    const log = [
      { type: "user_message", content: "first" },
      { type: "turn_end", metrics: {} },
      { type: "user_message", content: "second" },
      { type: "llm_call", provider: "anthropic", url: "https://api.anthropic.com/v1/messages", request: {} },
    ];
    const result = closeOpenTurn(log);
    expect(result).toHaveLength(log.length + 1);
    expect(result[result.length - 1]).toEqual({ type: "turn_interrupted" });
  });

  it("does not mutate the original array", () => {
    const log = [{ type: "user_message", content: "hi" }];
    const original = [...log];
    closeOpenTurn(log);
    expect(log).toEqual(original);
  });
});

// ---------------------------------------------------------------------------
// shouldLogEvent — streaming text events must be excluded
// ---------------------------------------------------------------------------

describe("shouldLogEvent", () => {
  it("allows user_message events", () => {
    expect(shouldLogEvent({ type: "user_message", content: "hi" })).toBe(true);
  });

  it("allows turn_end events", () => {
    expect(shouldLogEvent({ type: "turn_end" })).toBe(true);
  });

  it("allows tool_call events", () => {
    expect(shouldLogEvent({ type: "tool_call", id: "x", name: "read_file", input: {} })).toBe(true);
  });

  it("allows tool_result events", () => {
    expect(shouldLogEvent({ type: "tool_result", id: "x", name: "read_file", output: "", isError: false, durationMs: 0, contextHash: "ab12cd34" })).toBe(true);
  });

  it("allows model_changed events", () => {
    expect(shouldLogEvent({ type: "model_changed", model: "claude-sonnet-4-6" })).toBe(true);
  });

  it("allows llm_response events", () => {
    expect(shouldLogEvent({ type: "llm_response" })).toBe(true);
  });

  it("EXCLUDES streaming text events", () => {
    expect(shouldLogEvent({ type: "text", text: "Hello" })).toBe(false);
  });

  it("EXCLUDES connected events (already excluded)", () => {
    expect(shouldLogEvent({ type: "connected" })).toBe(false);
  });


});

// ---------------------------------------------------------------------------
// Store: history replay closes open turns
// ---------------------------------------------------------------------------

describe("store history replay — open turn recovery", () => {
  it("clears streaming flag when replaying a complete turn", () => {
    // Reset store by dispatching a history with one complete turn
    dispatch({
      type: "history",
      events: [
        { type: "user_message", content: "hello" },
        { type: "turn_end", metrics: { inputTokens: 10, outputTokens: 5 } },
      ],
    });
    expect(state.streaming).toBe(false);
  });

  it("clears streaming flag when replaying a turn closed by turn_interrupted", () => {
    dispatch({
      type: "history",
      events: [
        { type: "user_message", content: "hello" },
        { type: "turn_interrupted" },
      ],
    });
    expect(state.streaming).toBe(false);
  });

  it("clears streaming flag when replaying an open turn (no turn_end)", () => {
    // This is the core regression: an open turn in history must not leave streaming=true
    dispatch({
      type: "history",
      events: [
        { type: "user_message", content: "hello" },
        { type: "model_changed", model: "claude-sonnet-4-6" } as any,
        // NO turn_end — simulates a crash mid-turn
      ],
    });
    expect(state.streaming).toBe(false);
  });

  it("marks the recovered open turn as interrupted in the events list", () => {
    dispatch({
      type: "history",
      events: [
        { type: "user_message", content: "hello" },
        { type: "model_changed", model: "claude-sonnet-4-6" } as any,
      ],
    });
    const lastTurn = state.turns[state.turns.length - 1];
    expect(lastTurn).toBeDefined();
    const lastEvent = lastTurn.events[lastTurn.events.length - 1];
    expect(lastEvent.type).toBe("turn_interrupted");
  });

  it("replays a complete ping/pong session with all event types", () => {
    // Exact events from a real session (events.jsonl), filtered through shouldLogEvent
    // (text and connected are excluded; the rest survive)
    dispatch({
      type: "history",
      events: [
        { type: "session_start", authMode: "claude-max", model: "claude-sonnet-4-6", provider: "anthropic", systemPrompt: "..." } as any,
        { type: "user_message", content: "ping" },
        { type: "llm_call", provider: "anthropic", url: "https://api.anthropic.com/v1/messages", model: "claude-sonnet-4-6", contextHashes: ["5fce3362"], cacheBreakpointIndex: 0 } as any,
        { type: "llm_response", stopReason: "end_turn", usage: { input_tokens: 3, output_tokens: 5, cache_creation_input_tokens: 320, cache_read_input_tokens: 3318, service_tier: "standard" }, text: "pong" } as any,
        { type: "turn_end", metrics: { inputTokens: 3, outputTokens: 5, cacheCreationTokens: 320, cacheReadTokens: 3318 } } as any,
        { type: "session_end", outcome: "clean" } as any,
      ],
    });

    expect(state.streaming).toBe(false);
    expect(state.connected).toBe(true);
    expect(state.turns.length).toBe(1);

    const turn = state.turns[0];
    expect(turn.done).toBe(true);
    expect(turn.done).toBe(true);

    // The turn should contain an llm_response event with the text
    const llmResponse = turn.events.find((e: any) => e.type === "llm_response") as any;
    expect(llmResponse).toBeDefined();
    expect(llmResponse.text).toBe("pong");
  });
});
