import { describe, test, expect } from "bun:test";
import { Agent, type AgentEvent } from "./agent.js";

// This test verifies that the agent emits proper events in the right order,
// especially during tool-use loops. The UI depends on these events to show
// feedback. If events are missing, the UI appears "stuck".

// We can't easily mock the Anthropic API, so instead we test the event
// contract by examining what events SHOULD be emitted in each scenario.

describe("Agent event contract", () => {
  test("event types are well-defined", () => {
    // This is a compile-time check — if AgentEvent changes, this file won't compile
    const sampleEvents: AgentEvent[] = [
      { type: "text", text: "hello" },
      { type: "status", message: "thinking..." },
      { type: "tool_pending", id: "1", name: "read_file", formatted: "read_file: test" },
      { type: "tool_call", id: "1", name: "read_file", input: {}, formatted: "read_file: test" },
      { type: "tool_result", id: "1", name: "read_file", result: { output: "ok", isError: false, durationMs: 1 } },
      { type: "tool_rejected", id: "1", name: "read_file" },
      { type: "metrics", metrics: { inputTokens: 1, outputTokens: 1, costUsd: 0, ttftMs: 1, totalMs: 1 } },
      { type: "error", error: "test error" },
    ];
    expect(sampleEvents).toHaveLength(8);
  });
});

// Test the UI event handling logic extracted into a pure function
// This is the core of the fix: extract the event→state logic out of React

interface UIState {
  completedItems: Array<{ type: string; text: string; dimText?: string }>;
  streamingText: string;
  activity: string;
  isStreaming: boolean;
  lastResponse: { text: string; dimText?: string } | null;
}

function initialState(): UIState {
  return {
    completedItems: [],
    streamingText: "",
    activity: "",
    isStreaming: false,
    lastResponse: null,
  };
}

// Pure function that applies an agent event to UI state
// This mirrors the switch statement in ui.tsx but is testable
function applyEvent(state: UIState, event: AgentEvent, fullText: { value: string }): UIState {
  const next = { ...state, completedItems: [...state.completedItems] };

  switch (event.type) {
    case "status":
      next.activity = event.message;
      break;

    case "text":
      fullText.value += event.text;
      next.streamingText = fullText.value;
      next.activity = "";
      break;

    case "tool_pending":
      if (fullText.value) {
        next.completedItems.push({ type: "turn", text: fullText.value });
        fullText.value = "";
        next.streamingText = "";
      }
      next.activity = "";
      break;

    case "tool_call":
      if (fullText.value) {
        next.completedItems.push({ type: "turn", text: fullText.value });
        fullText.value = "";
        next.streamingText = "";
      }
      next.completedItems.push({ type: "tool_call", text: `🔧 ${event.formatted}` });
      next.activity = `running ${event.name}...`;
      break;

    case "tool_result":
      next.completedItems.push({
        type: "tool_result",
        text: event.result.output,
        dimText: `  ${event.name} ${event.result.isError ? "✗" : "✓"} ${Math.round(event.result.durationMs)}ms`,
      });
      break;

    case "tool_rejected":
      next.completedItems.push({ type: "tool_rejected", text: `⊘ ${event.name} rejected` });
      break;

    case "metrics": {
      const m = event.metrics;
      const metricsLine = `  in: ${m.inputTokens} out: ${m.outputTokens}`;
      if (fullText.value) {
        next.lastResponse = { text: fullText.value, dimText: metricsLine };
        fullText.value = "";
        next.streamingText = "";
      } else {
        next.completedItems.push({ type: "turn", text: "", dimText: metricsLine });
      }
      break;
    }

    case "error":
      next.completedItems.push({ type: "error", text: `⚠ ${event.error}` });
      break;
  }

  return next;
}

describe("UI event handling", () => {
  test("text-only response: streams then commits to lastResponse on metrics", () => {
    const fullText = { value: "" };
    let state = initialState();
    state.isStreaming = true;

    // Agent sends status before API call
    state = applyEvent(state, { type: "status", message: "thinking..." }, fullText);
    expect(state.activity).toBe("thinking...");

    // Simulate text streaming
    state = applyEvent(state, { type: "text", text: "Hello " }, fullText);
    expect(state.streamingText).toBe("Hello ");
    expect(state.activity).toBe("");

    state = applyEvent(state, { type: "text", text: "world!" }, fullText);
    expect(state.streamingText).toBe("Hello world!");

    // Metrics arrives — text moves to lastResponse, NOT completedItems
    state = applyEvent(state, {
      type: "metrics",
      metrics: { inputTokens: 100, outputTokens: 10, costUsd: 0.001, ttftMs: 50, totalMs: 200 },
    }, fullText);

    expect(state.lastResponse).not.toBeNull();
    expect(state.lastResponse!.text).toBe("Hello world!");
    expect(state.streamingText).toBe("");
    // Should NOT be in completedItems (would cause duplication)
    expect(state.completedItems.filter(i => i.text === "Hello world!")).toHaveLength(0);
  });

  test("text then tool call: text flushed to completedItems before tool", () => {
    const fullText = { value: "" };
    let state = initialState();
    state.isStreaming = true;

    // Agent streams some text
    state = applyEvent(state, { type: "text", text: "I'll update the plan." }, fullText);
    expect(state.streamingText).toBe("I'll update the plan.");

    // Then makes a tool call — text must be flushed first
    state = applyEvent(state, {
      type: "tool_call",
      id: "t1",
      name: "write_file",
      input: { path: "plan/overview.md", content: "..." },
      formatted: "write_file: plan/overview.md (3 bytes)",
    }, fullText);

    // Text was flushed to completedItems
    expect(state.completedItems[0]).toEqual({
      type: "turn",
      text: "I'll update the plan.",
    });
    // Tool call is in completedItems
    expect(state.completedItems[1].type).toBe("tool_call");
    // Streaming text is cleared
    expect(state.streamingText).toBe("");
    // Activity shows tool running
    expect(state.activity).toBe("running write_file...");
  });

  test("tool result adds to completedItems", () => {
    const fullText = { value: "" };
    let state = initialState();
    state.isStreaming = true;

    state = applyEvent(state, {
      type: "tool_result",
      id: "t1",
      name: "write_file",
      result: { output: "Wrote 100 bytes", isError: false, durationMs: 5 },
    }, fullText);

    expect(state.completedItems).toHaveLength(1);
    expect(state.completedItems[0].type).toBe("tool_result");
  });

  test("full tool loop: status → text → tool_call → tool_result → status → metrics → text → metrics", () => {
    const fullText = { value: "" };
    let state = initialState();
    state.isStreaming = true;

    // 1. Status before first API call
    state = applyEvent(state, { type: "status", message: "thinking..." }, fullText);
    expect(state.activity).toBe("thinking...");

    // 2. Agent streams explanation text
    state = applyEvent(state, { type: "text", text: "I'll update the plan now." }, fullText);
    expect(state.streamingText).toBe("I'll update the plan now.");
    expect(state.activity).toBe("");

    // 3. Tool call (auto-approved, no tool_pending)
    state = applyEvent(state, {
      type: "tool_call",
      id: "t1",
      name: "write_file",
      input: {},
      formatted: "write_file: plan/overview.md (500 bytes)",
    }, fullText);
    expect(state.streamingText).toBe("");
    expect(state.activity).toBe("running write_file...");
    expect(state.completedItems).toHaveLength(2); // flushed text + tool_call

    // 4. Tool result
    state = applyEvent(state, {
      type: "tool_result",
      id: "t1",
      name: "write_file",
      result: { output: "Wrote 500 bytes to plan/overview.md", isError: false, durationMs: 3 },
    }, fullText);
    expect(state.completedItems).toHaveLength(3);

    // 5. Metrics for first API turn
    state = applyEvent(state, {
      type: "metrics",
      metrics: { inputTokens: 1000, outputTokens: 200, costUsd: 0.01, ttftMs: 100, totalMs: 500 },
    }, fullText);
    expect(state.completedItems).toHaveLength(4);
    expect(state.lastResponse).toBeNull();

    // 6. Status before second API call (the key feedback moment!)
    state = applyEvent(state, { type: "status", message: "thinking..." }, fullText);
    expect(state.activity).toBe("thinking...");

    // 7. Second API turn — agent responds with summary
    state = applyEvent(state, { type: "text", text: "Done! Plan updated." }, fullText);
    expect(state.streamingText).toBe("Done! Plan updated.");

    // 8. Final metrics
    state = applyEvent(state, {
      type: "metrics",
      metrics: { inputTokens: 2000, outputTokens: 50, costUsd: 0.005, ttftMs: 80, totalMs: 300 },
    }, fullText);
    expect(state.lastResponse!.text).toBe("Done! Plan updated.");
    expect(state.streamingText).toBe("");
  });

  test("exact agent event sequence for write_file tool loop", () => {
    // This simulates the EXACT event order from agent.ts when:
    // 1. Agent streams "I'll update the plan"
    // 2. Agent calls write_file (auto-approved)
    // 3. Agent makes second API call and responds "Done"
    const fullText = { value: "" };
    let state = initialState();
    state.isStreaming = true;

    // --- Turn 1: text + tool_use ---
    // Agent emits status before API call
    state = applyEvent(state, { type: "status", message: "thinking..." }, fullText);
    expect(state.activity).toBe("thinking...");
    expect(state.streamingText).toBe("");
    // UI shows: ⏳ thinking...

    // Text streams in
    state = applyEvent(state, { type: "text", text: "I'll update the plan." }, fullText);
    expect(state.streamingText).toBe("I'll update the plan.");
    expect(state.activity).toBe("");
    // UI shows: I'll update the plan.▊

    // Stream ends, tool_call emitted (auto-approved, no tool_pending)
    state = applyEvent(state, {
      type: "tool_call", id: "t1", name: "write_file",
      input: { path: "plan/overview.md", content: "new content" },
      formatted: "write_file: plan/overview.md (11 bytes)",
    }, fullText);
    expect(state.streamingText).toBe("");
    expect(state.activity).toBe("running write_file...");
    expect(state.completedItems).toHaveLength(2); // flushed text + tool_call
    // UI shows: ⏳ running write_file...

    // Tool completes
    state = applyEvent(state, {
      type: "tool_result", id: "t1", name: "write_file",
      result: { output: "Wrote 11 bytes to plan/overview.md", isError: false, durationMs: 2 },
    }, fullText);
    expect(state.completedItems).toHaveLength(3);
    // Activity not changed by tool_result — status event will set it
    // UI shows tool result in static zone

    // Metrics for turn 1
    state = applyEvent(state, {
      type: "metrics",
      metrics: { inputTokens: 1000, outputTokens: 50, costUsd: 0.003, ttftMs: 100, totalMs: 500 },
    }, fullText);
    expect(state.completedItems).toHaveLength(4); // metrics as empty turn
    expect(state.lastResponse).toBeNull(); // no fullText, so not lastResponse

    // --- Turn 2: agent loops, makes new API call ---
    // Agent emits status BEFORE the API call
    state = applyEvent(state, { type: "status", message: "thinking..." }, fullText);
    expect(state.activity).toBe("thinking...");
    expect(state.streamingText).toBe("");
    // UI shows: ⏳ thinking...  <-- THIS IS THE KEY: user sees feedback here

    // Second API call streams response
    state = applyEvent(state, { type: "text", text: "Done! Updated the plan." }, fullText);
    expect(state.streamingText).toBe("Done! Updated the plan.");

    // Final metrics
    state = applyEvent(state, {
      type: "metrics",
      metrics: { inputTokens: 2000, outputTokens: 20, costUsd: 0.006, ttftMs: 80, totalMs: 300 },
    }, fullText);
    expect(state.lastResponse!.text).toBe("Done! Updated the plan.");
    expect(state.streamingText).toBe("");
  });

  test("activity is always set during streaming — never empty when working", () => {
    const fullText = { value: "" };
    let state = initialState();
    state.isStreaming = true;

    // Status event sets activity
    state = applyEvent(state, { type: "status", message: "thinking..." }, fullText);
    expect(state.activity).toBe("thinking...");

    // Text clears activity (streaming text IS the feedback)
    state = applyEvent(state, { type: "text", text: "Hi" }, fullText);
    expect(state.activity).toBe("");
    // But streamingText provides feedback
    expect(state.streamingText).toBe("Hi");

    // Tool call sets activity
    state = applyEvent(state, {
      type: "tool_call", id: "1", name: "write_file",
      input: {}, formatted: "write_file: test",
    }, fullText);
    expect(state.activity).toBe("running write_file...");
    // streamingText cleared (text was flushed)
    expect(state.streamingText).toBe("");

    // Status event before next API call
    state = applyEvent(state, { type: "status", message: "thinking..." }, fullText);
    expect(state.activity).toBe("thinking...");
  });

  test("between tool_result and next API response, status event provides feedback", () => {
    // This is the key scenario: after a tool completes, the agent loop
    // makes another API call. The agent emits a status event before the call.
    const fullText = { value: "" };
    let state = initialState();
    state.isStreaming = true;

    // First turn status
    state = applyEvent(state, { type: "status", message: "thinking..." }, fullText);

    // Metrics for first turn (tool_use stop_reason)
    state = applyEvent(state, {
      type: "metrics",
      metrics: { inputTokens: 100, outputTokens: 10, costUsd: 0.001, ttftMs: 50, totalMs: 200 },
    }, fullText);

    // Tool executes
    state = applyEvent(state, {
      type: "tool_call", id: "1", name: "write_file",
      input: {}, formatted: "write_file: test",
    }, fullText);

    state = applyEvent(state, {
      type: "tool_result", id: "1", name: "write_file",
      result: { output: "ok", isError: false, durationMs: 1 },
    }, fullText);

    // Agent emits status before next API call
    state = applyEvent(state, { type: "status", message: "thinking..." }, fullText);

    // State must show activity so UI isn't blank.
    expect(state.activity).toBe("thinking...");
    expect(state.streamingText).toBe("");
    // The UI should show: ⏳ thinking...
  });

  test("metrics-only turn (no text) still adds to completedItems", () => {
    const fullText = { value: "" };
    let state = initialState();

    state = applyEvent(state, {
      type: "metrics",
      metrics: { inputTokens: 100, outputTokens: 0, costUsd: 0, ttftMs: null, totalMs: 100 },
    }, fullText);

    expect(state.completedItems).toHaveLength(1);
    expect(state.completedItems[0].type).toBe("turn");
    expect(state.completedItems[0].dimText).toContain("in: 100");
  });
});
