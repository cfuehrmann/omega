/**
 * Tests for session resilience: open turns on shutdown, partial-turn replay.
 *
 * Covers three related bugs that caused the "stuck streaming" UI state:
 *  1. Server saves partial turn on shutdown → replay locks UI in streaming=true
 *  2. Individual streaming text events bloat the session file uselessly
 *  3. Store replay has no defence against an already-open turn
 */

import { describe, it, expect } from "bun:test";
import { closeOpenTurn, shouldLogEvent } from "./server-helpers.js";
import { dispatch, state, computeRenderGroups } from "./client/state.js";
import { now } from "../iso-timestamp.js";

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
      { type: "llm_call", url: "https://api.anthropic.com/v1/messages", request: {} },
      { type: "text", text: "Partial" },
    ];
    const result = closeOpenTurn(log);
    expect(result).toHaveLength(log.length + 1);
    expect(result[result.length - 1]).toMatchObject({ type: "turn_interrupted" });
    expect(typeof (result[result.length - 1] as any).time).toBe("string");
  });

  it("handles multiple turns: only patches the open last turn", () => {
    const log = [
      { type: "user_message", content: "first" },
      { type: "turn_end", metrics: {} },
      { type: "user_message", content: "second" },
      { type: "llm_call", url: "https://api.anthropic.com/v1/messages", request: {} },
    ];
    const result = closeOpenTurn(log);
    expect(result).toHaveLength(log.length + 1);
    expect(result[result.length - 1]).toMatchObject({ type: "turn_interrupted" });
    expect(typeof (result[result.length - 1] as any).time).toBe("string");
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
    expect(shouldLogEvent({ type: "tool_result", id: "x", name: "read_file", output: "", isError: false, durationMs: 0 })).toBe(true);
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

  it("EXCLUDES ready events (already excluded)", () => {
    expect(shouldLogEvent({ type: "ready" })).toBe(false);
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
        { type: "user_message", time: now(), content: "hello" },
        { type: "turn_end", time: now(), metrics: { inputTokens: 10, outputTokens: 5 } },
      ],
    });
    expect(state.streaming).toBe(false);
  });

  it("clears streaming flag when replaying a turn closed by turn_interrupted", () => {
    dispatch({
      type: "history",
      events: [
        { type: "user_message", time: now(), content: "hello" },
        { type: "turn_interrupted", time: now() },
      ],
    });
    expect(state.streaming).toBe(false);
  });

  it("clears streaming flag when replaying an open turn (no turn_end)", () => {
    // This is the core regression: an open turn in history must not leave streaming=true
    dispatch({
      type: "history",
      events: [
        { type: "user_message", time: now(), content: "hello" },
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
        { type: "user_message", time: now(), content: "hello" },
        { type: "model_changed", model: "claude-sonnet-4-6" } as any,
      ],
    });
    // The last event in the flat list should be a synthetic turn_interrupted
    const lastEvent = state.events[state.events.length - 1]!;
    expect(lastEvent).toBeDefined();
    expect(lastEvent.type).toBe("turn_interrupted");
  });

  // ---------------------------------------------------------------------------
  // Browser-refresh resilience: history with streaming=true (server has an
  // in-flight turn that survived a WS reconnect)
  // ---------------------------------------------------------------------------

  it("does NOT inject synthetic turn_interrupted when history.streaming=true", () => {
    // Simulates a browser refresh mid-turn: the server sends partial history
    // with streaming=true because the agent turn is still running.
    dispatch({
      type: "history",
      events: [
        { type: "user_message", time: now(), content: "hello" },
        { type: "llm_call", url: "https://api.anthropic.com/v1/messages", model: "claude-sonnet-4-6", contextHashes: ["abc"], cacheBreakpointIndex: 0 } as any,
        // NO turn_end — turn is still in progress
      ],
      streaming: true,
    } as any);
    // No synthetic turn_interrupted should have been appended
    const lastEvent = state.events[state.events.length - 1]!;
    expect(lastEvent.type).not.toBe("turn_interrupted");
    expect(lastEvent.type).toBe("llm_call");
  });

  it("still injects synthetic turn_interrupted when history.streaming is absent (crash recovery)", () => {
    // Absent streaming flag = server crashed mid-turn, not a live browser refresh
    dispatch({
      type: "history",
      events: [
        { type: "user_message", time: now(), content: "hello" },
        { type: "llm_call", url: "https://api.anthropic.com/v1/messages", model: "claude-sonnet-4-6", contextHashes: ["abc"], cacheBreakpointIndex: 0 } as any,
      ],
      // no streaming field
    } as any);
    const lastEvent = state.events[state.events.length - 1]!;
    expect(lastEvent.type).toBe("turn_interrupted");
  });

  it("ready with streaming=true sets state.streaming to true", () => {
    // history always resets streaming=false; ready with streaming=true then
    // corrects it to reflect the server's live in-flight turn.
    dispatch({
      type: "history",
      events: [{ type: "user_message", time: now(), content: "hello" }],
      streaming: true,
    } as any);
    dispatch({ type: "ready", streaming: true } as any);
    expect(state.streaming).toBe(true);
  });

  it("ready without streaming field keeps streaming=false", () => {
    dispatch({
      type: "history",
      events: [
        { type: "user_message", time: now(), content: "hello" },
        { type: "turn_end", time: now(), metrics: { inputTokens: 1, outputTokens: 1 } },
      ],
    } as any);
    dispatch({ type: "ready" });
    expect(state.streaming).toBe(false);
  });

  it("live turn_end after streaming=true reconnect clears streaming", () => {
    // Simulate: reconnect mid-turn, get history+ready with streaming=true,
    // then the live turn_end arrives via broadcast()
    dispatch({
      type: "history",
      events: [{ type: "user_message", time: now(), content: "hello" }],
      streaming: true,
    } as any);
    dispatch({ type: "ready", streaming: true } as any);
    expect(state.streaming).toBe(true);

    // Live turn_end arrives via broadcast
    dispatch({ type: "turn_end", time: now(), metrics: { inputTokens: 5, outputTokens: 10 } } as any);
    expect(state.streaming).toBe(false);
  });

  it("replays a complete ping/pong session with all event types", () => {
    // Exact events from a real session (events.jsonl), filtered through shouldLogEvent
    // (text and connected are excluded; the rest survive)
    dispatch({
      type: "history",
      events: [
        { type: "session_started", path: ".omega/sessions/test", model: "claude-sonnet-4-6", effort: "medium", systemPrompt: "..." } as any,
        { type: "user_message", content: "ping" },
        { type: "llm_call", url: "https://api.anthropic.com/v1/messages", model: "claude-sonnet-4-6", contextHashes: ["5fce3362aabb"], cacheBreakpointIndex: 0 } as any,
        { type: "llm_response", stopReason: "end_turn", usage: { input_tokens: 3, output_tokens: 5, cache_creation_input_tokens: 320, cache_read_input_tokens: 3318, service_tier: "standard" }, text: "pong" } as any,
        { type: "turn_end", metrics: { inputTokens: 3, outputTokens: 5, cacheCreationTokens: 320, cacheReadTokens: 3318 } } as any,
        { type: "server_stopped", outcome: "clean" } as any,
      ],
    });

    expect(state.streaming).toBe(false);
    expect(state.connected).toBe(true);

    // Derive turn groups from the flat event list
    const groups = computeRenderGroups(state.events);
    const turns = groups.filter(g => g.kind === "turn");
    expect(turns.length).toBe(1);

    const turn = turns[0] as Extract<typeof turns[0], { kind: "turn" }>;
    expect(turn.done).toBe(true);

    // The turn should contain an llm_response event with the text
    const llmResponse = turn.events.find((e: any) => e.type === "llm_response") as any;
    expect(llmResponse).toBeDefined();
    expect(llmResponse.text).toBe("pong");

    // session_started should appear as a free group before the turn
    expect(groups[0]!.kind).toBe("free");
    expect(groups[0]!.events[0]!.type).toBe("session_started");

    // server_stopped should appear as a free group after the turn
    const lastGroup = groups[groups.length - 1]!;
    expect(lastGroup.kind).toBe("free");
    expect(lastGroup.events[0]!.type).toBe("server_stopped");
  });
});

// ---------------------------------------------------------------------------
// Store: retrying state
// ---------------------------------------------------------------------------

describe("store retrying state", () => {
  // Reset to a known baseline before each test
  function startTurn() {
    dispatch({ type: "history", events: [] });          // clear store
    dispatch({ type: "ready" });
    dispatch({ type: "user_message", content: "hi" } as any);
  }

  it("starts as false", () => {
    dispatch({ type: "history", events: [] });
    expect(state.retrying).toBe(false);
  });

  it("becomes true when llm_retry is received", () => {
    startTurn();
    dispatch({
      type: "llm_retry",
      attempt: 1,
      waitMs: 1000,
      error: "overloaded",
    } as any);
    expect(state.retrying).toBe(true);
  });

  it("clears to false when llm_response arrives after a retry", () => {
    startTurn();
    dispatch({ type: "llm_retry", attempt: 1, waitMs: 100, error: "overloaded" } as any);
    expect(state.retrying).toBe(true);
    dispatch({
      type: "llm_response",
      stopReason: "end_turn",
      usage: { input_tokens: 5, output_tokens: 2 },
      contextHash: "ab12cd34ef56",
    } as any);
    expect(state.retrying).toBe(false);
  });

  it("clears to false when turn_end arrives", () => {
    startTurn();
    dispatch({ type: "llm_retry", attempt: 1, waitMs: 100, error: "overloaded" } as any);
    expect(state.retrying).toBe(true);
    dispatch({ type: "turn_end", metrics: { inputTokens: 5, outputTokens: 2 } } as any);
    expect(state.retrying).toBe(false);
  });

  it("clears to false when turn_interrupted arrives", () => {
    startTurn();
    dispatch({ type: "llm_retry", attempt: 1, waitMs: 100, error: "overloaded" } as any);
    expect(state.retrying).toBe(true);
    dispatch({ type: "turn_interrupted", reason: "error" } as any);
    expect(state.retrying).toBe(false);
  });

  it("clears to false on reset_done", () => {
    startTurn();
    dispatch({ type: "llm_retry", attempt: 1, waitMs: 100, error: "overloaded" } as any);
    expect(state.retrying).toBe(true);
    dispatch({ type: "reset_done" } as any);
    expect(state.retrying).toBe(false);
  });

  it("remains false during streaming without a retry", () => {
    startTurn();
    dispatch({ type: "text", text: "hello" } as any);
    expect(state.retrying).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// Store: resumption streaming state
// ---------------------------------------------------------------------------

describe("store resumption streaming state", () => {
  function resetStore() {
    dispatch({ type: "history", events: [] });
    dispatch({ type: "ready" });
  }

  it("sets streaming=true when resuming_session is dispatched", () => {
    resetStore();
    expect(state.streaming).toBe(false);
    dispatch({
      type: "resuming_session",
      time: now(),
      resumedFrom: "2025-01-01T00-00-00-000-aaaaaaaa",
      basis: "some basis",
    } as any);
    expect(state.streaming).toBe(true);
  });

  it("clears streaming on ready after resumption completes", () => {
    resetStore();
    dispatch({
      type: "resuming_session",
      time: now(),
      resumedFrom: "2025-01-01T00-00-00-000-aaaaaaaa",
      basis: "some basis",
    } as any);
    expect(state.streaming).toBe(true);
    // Simulate the events that follow resumption
    dispatch({
      type: "llm_call",
      time: now(),
      url: "https://api.anthropic.com/v1/messages",
      model: "claude-sonnet-4-6",
      contextHashes: ["abc123"],
      cacheBreakpointIndex: null,
    } as any);
    dispatch({
      type: "llm_response",
      time: now(),
      stopReason: "end_turn",
      usage: { input_tokens: 10, output_tokens: 5 },
      contextHash: "def456",
    } as any);
    dispatch({
      type: "session_resumed",
      time: now(),
      resumedFrom: "2025-01-01T00-00-00-000-aaaaaaaa",
      summary: "Prior summary.",
    } as any);
    // streaming remains true until ready
    expect(state.streaming).toBe(true);
    dispatch({ type: "ready" });
    expect(state.streaming).toBe(false);
  });

  it("resuming_session event appears in the events array", () => {
    resetStore();
    const before = state.events.length;
    dispatch({
      type: "resuming_session",
      time: now(),
      resumedFrom: "old-dir",
      basis: "basis text",
    } as any);
    expect(state.events.length).toBe(before + 1);
    expect(state.events[state.events.length - 1]!.type).toBe("resuming_session");
  });
});

// ---------------------------------------------------------------------------
// Resumption llm_call must not override liveModel
// ---------------------------------------------------------------------------

describe("resumption llm_call does not override liveModel", () => {
  it("liveModel stays at user-chosen model when resumption llm_call uses a different model", () => {
    // Simulate the resume flow: history sets model to opus via model_changed,
    // then a streamed resumption llm_call arrives using sonnet.
    dispatch({
      type: "history",
      events: [
        { type: "session_started", path: ".omega/sessions/test", model: "claude-sonnet-4-6", effort: "low", systemPrompt: "..." } as any,
        { type: "model_changed", time: now(), model: "claude-opus-4-6" } as any,
        { type: "effort_changed", time: now(), effort: "low" } as any,
      ],
    });

    // After history replay, model should be opus
    expect(state.liveModel).toBe("claude-opus-4-6");
    expect(state.liveEffort).toBe("low");

    // Now a resumption llm_call arrives using sonnet (the resumption model)
    dispatch({
      type: "llm_call",
      url: "https://api.anthropic.com/v1/messages",
      model: "claude-sonnet-4-6",
      contextHashes: ["abc123"],
      cacheBreakpointIndex: 0,
    } as any);

    // liveModel must still be opus — the user's chosen model
    expect(state.liveModel).toBe("claude-opus-4-6");
  });

  it("history replay ignores llm_call model for liveModel derivation", () => {
    // A resumed session where model_changed set opus, but an llm_call used sonnet
    dispatch({
      type: "history",
      events: [
        { type: "session_started", path: ".omega/sessions/test", model: "claude-sonnet-4-6", effort: "medium", systemPrompt: "..." } as any,
        { type: "model_changed", time: now(), model: "claude-opus-4-6" } as any,
        { type: "resuming_session", time: now(), resumedFrom: "prev-session", basis: "..." } as any,
        { type: "llm_call", url: "https://api.anthropic.com/v1/messages", model: "claude-sonnet-4-6", contextHashes: ["abc"], cacheBreakpointIndex: 0 } as any,
        { type: "llm_response", stopReason: "end_turn", usage: { input_tokens: 10, output_tokens: 20 }, text: "summary" } as any,
        { type: "session_resumed", time: now(), resumedFrom: "prev-session" } as any,
      ],
    });

    // liveModel should be opus (from model_changed), not sonnet (from llm_call)
    expect(state.liveModel).toBe("claude-opus-4-6");
  });
});
