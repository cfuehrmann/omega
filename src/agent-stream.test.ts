import { describe, test, expect } from "bun:test";
import type { AgentEvent } from "./agent.js";

/**
 * This test reproduces the "stuck updating plan" bug.
 *
 * The bug: when the model streams text then generates a tool_use block
 * (e.g. write_file with a large file), the agent only yields `text` events
 * for text_delta. The tool_use input_json_delta events are ignored. So after
 * the last text token, the UI gets NO events for potentially 10-20 seconds
 * while the model generates the tool input JSON.
 *
 * The user sees: streaming text with cursor, then nothing. "Stuck."
 *
 * The fix: the agent should emit a feedback event (e.g. status "generating
 * tool input...") when it detects a tool_use block starting in the stream.
 */

// Simulate the stream events that the Anthropic SDK produces
// when the model outputs text followed by a write_file tool call.
function* fakeStreamEvents(): Generator<any> {
  // Text block
  yield { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } };
  yield { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "I'll update " } };
  yield { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "the plan now." } };
  yield { type: "content_block_stop", index: 0 };

  // Tool use block — this is where the UI goes silent in the buggy version
  yield { type: "content_block_start", index: 1, content_block: { type: "tool_use", id: "tool_1", name: "write_file" } };
  // Many input_json_delta events (simulating large file content)
  yield { type: "content_block_delta", index: 1, delta: { type: "input_json_delta", partial_json: '{"path": "plan/' } };
  yield { type: "content_block_delta", index: 1, delta: { type: "input_json_delta", partial_json: 'overview.md", ' } };
  yield { type: "content_block_delta", index: 1, delta: { type: "input_json_delta", partial_json: '"content": "# Omega...' } };
  yield { type: "content_block_delta", index: 1, delta: { type: "input_json_delta", partial_json: '"}' } };
  yield { type: "content_block_stop", index: 1 };

  yield { type: "message_delta", delta: { stop_reason: "tool_use" }, usage: { output_tokens: 500 } };
  yield { type: "message_stop" };
}

/**
 * This function extracts agent events from stream events using the same
 * logic as the streaming loop in agent.ts sendMessage(). 
 * 
 * It is exported from agent.ts as processStreamEvents() so we can test it.
 * If that export doesn't exist yet, this test will fail to compile — 
 * that's intentional (red phase).
 */
import { processStreamEvents } from "./agent.js";

describe("stream event handling (stuck-on-plan-update bug)", () => {
  test("emits a status event when tool_use block starts in stream", () => {
    const events = processStreamEvents([...fakeStreamEvents()]);

    // Text events should be present
    const textEvents = events.filter((e) => e.type === "text");
    expect(textEvents).toHaveLength(2);
    expect(textEvents[0]).toEqual({ type: "text", text: "I'll update " });
    expect(textEvents[1]).toEqual({ type: "text", text: "the plan now." });

    // A status event MUST appear when the tool_use block starts.
    // Without this, the UI is "stuck" for the entire duration of tool input generation.
    const statusEvents = events.filter((e) => e.type === "status");
    expect(statusEvents.length).toBeGreaterThanOrEqual(1);

    const toolStatus = statusEvents.find(
      (e) => e.type === "status" && e.message.includes("write_file")
    );
    expect(toolStatus).toBeDefined();
  });

  test("status event comes after all text events", () => {
    const events = processStreamEvents([...fakeStreamEvents()]);

    const lastTextIndex = events.findLastIndex((e) => e.type === "text");
    const statusIndex = events.findIndex((e) => e.type === "status");

    expect(statusIndex).toBeGreaterThan(lastTextIndex);
  });

  test("no status event when response is text-only (no tool_use)", () => {
    function* textOnlyStream(): Generator<any> {
      yield { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } };
      yield { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "Hello!" } };
      yield { type: "content_block_stop", index: 0 };
      yield { type: "message_delta", delta: { stop_reason: "end_turn" }, usage: { output_tokens: 5 } };
      yield { type: "message_stop" };
    }

    const events = processStreamEvents([...textOnlyStream()]);
    const statusEvents = events.filter((e) => e.type === "status");
    expect(statusEvents).toHaveLength(0);
  });
});
