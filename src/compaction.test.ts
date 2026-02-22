/**
 * Tests for context compaction (zone 2 + zone 1).
 */

import { describe, it, expect } from "bun:test";
import type { StreamProvider } from "./agent.js";
import { compactTurn, compactWorldState } from "./compaction.js";
import type { MessageParam } from "@anthropic-ai/sdk/resources/messages";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function makeMockProvider(responseText: string): StreamProvider {
  return async () => ({
    async *[Symbol.asyncIterator]() {
      // minimal stream: one text delta, then done
      yield { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } };
      yield { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: responseText } };
      yield { type: "content_block_stop", index: 0 };
      yield { type: "message_delta", delta: { stop_reason: "end_turn" }, usage: { output_tokens: 5 } };
    },
    async finalMessage() {
      return {
        id: "msg_test",
        type: "message",
        role: "assistant",
        content: [{ type: "text", text: responseText }],
        model: "claude-sonnet-4-6",
        stop_reason: "end_turn",
        stop_sequence: null,
        usage: { input_tokens: 10, output_tokens: 5 },
      } as any;
    },
  });
}

// ---------------------------------------------------------------------------
// compactTurn
// ---------------------------------------------------------------------------

describe("compactTurn", () => {
  it("returns a 2-message synthetic exchange", async () => {
    const provider = makeMockProvider("User asked to read a file. Assistant read foo.ts and found 42 lines.");
    const turn: MessageParam[] = [
      { role: "user", content: "read foo.ts" },
      { role: "assistant", content: [{ type: "tool_use", id: "t1", name: "read_file", input: { path: "foo.ts" } }] as any },
      { role: "user", content: [{ type: "tool_result", tool_use_id: "t1", content: "42 lines" }] as any },
      { role: "assistant", content: [{ type: "text", text: "I read it." }] as any },
    ];
    const result = await compactTurn(turn, null, provider);
    expect(result).toHaveLength(2);
    expect(result[0].role).toBe("user");
    expect(result[1].role).toBe("assistant");
    expect(typeof result[0].content).toBe("string");
    expect(typeof result[1].content).toBe("string");
  });

  it("includes previous summary when provided", async () => {
    let capturedPrompt = "";
    const provider: StreamProvider = async (params) => {
      capturedPrompt = params.messages[0].content as string;
      return makeMockProvider("Updated summary.")(params);
    };
    const turn: MessageParam[] = [
      { role: "user", content: "do something" },
      { role: "assistant", content: [{ type: "text", text: "done" }] as any },
    ];
    await compactTurn(turn, "Previous summary: we read foo.ts.", provider);
    expect(capturedPrompt).toContain("Previous summary: we read foo.ts.");
  });

  it("result is shorter than input (in characters)", async () => {
    const longContent = "x".repeat(2000);
    const provider = makeMockProvider("Short summary.");
    const turn: MessageParam[] = [
      { role: "user", content: longContent },
      { role: "assistant", content: [{ type: "text", text: longContent }] as any },
    ];
    const result = await compactTurn(turn, null, provider);
    const resultSize = JSON.stringify(result).length;
    const inputSize = JSON.stringify(turn).length;
    expect(resultSize).toBeLessThan(inputSize);
  });
});

// ---------------------------------------------------------------------------
// compactWorldState
// ---------------------------------------------------------------------------

describe("compactWorldState", () => {
  it("returns a string (the new world state)", async () => {
    const provider = makeMockProvider("State: foo.ts has 42 lines. We refactored bar.ts.");
    const sessionHistory: MessageParam[] = [
      { role: "user", content: "read foo.ts" },
      { role: "assistant", content: [{ type: "text", text: "done" }] as any },
    ];
    const result = await compactWorldState("Old world state.", sessionHistory, provider);
    expect(typeof result).toBe("string");
    expect(result.length).toBeGreaterThan(0);
  });

  it("works with empty prior world state", async () => {
    const provider = makeMockProvider("Initial state: nothing done yet.");
    const sessionHistory: MessageParam[] = [
      { role: "user", content: "hello" },
      { role: "assistant", content: [{ type: "text", text: "hi" }] as any },
    ];
    const result = await compactWorldState(null, sessionHistory, provider);
    expect(typeof result).toBe("string");
    expect(result.length).toBeGreaterThan(0);
  });
});
