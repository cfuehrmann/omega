/**
 * Tests for world-state compaction (zone 1) and history compaction (step 3b).
 * Turn compaction (zone 2) was removed in manifest Step 2.
 */

import { describe, it, expect } from "bun:test";
import type { StreamProvider } from "./agent.js";
import { compactWorldState, compactHistory, KEEP_RECENT_TURNS } from "./compaction.js";
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

// ---------------------------------------------------------------------------
// compactHistory (Step 3b)
// ---------------------------------------------------------------------------

/** Build a history of N complete message-pairs (user + assistant). */
function makeHistory(pairs: number): MessageParam[] {
  const msgs: MessageParam[] = [];
  for (let i = 0; i < pairs; i++) {
    msgs.push({ role: "user", content: `message ${i}` });
    msgs.push({ role: "assistant", content: [{ type: "text", text: `reply ${i}` }] as any });
  }
  return msgs;
}

describe("compactHistory", () => {
  it("returns history unchanged when already short enough", async () => {
    const history = makeHistory(KEEP_RECENT_TURNS); // exactly at the threshold
    const provider = makeMockProvider("summary"); // should not be called
    const result = await compactHistory(history, provider);
    expect(result.originalCount).toBe(history.length);
    expect(result.newCount).toBe(history.length);
    expect(result.history).toBe(history); // same reference — no copy
  });

  it("compacts when history exceeds KEEP_RECENT_TURNS pairs", async () => {
    const pairs = KEEP_RECENT_TURNS + 3; // 3 extra pairs to compact
    const history = makeHistory(pairs);
    const summaryText = "Agent read several files and wrote some code.";
    const provider = makeMockProvider(summaryText);

    const result = await compactHistory(history, provider);

    // New count: 1 synthetic summary + KEEP_RECENT_TURNS * 2 tail messages
    const expectedNew = 1 + KEEP_RECENT_TURNS * 2;
    expect(result.originalCount).toBe(pairs * 2);
    expect(result.newCount).toBe(expectedNew);
    expect(result.history).toHaveLength(expectedNew);
  });

  it("first message in compacted history is the synthetic summary", async () => {
    const history = makeHistory(KEEP_RECENT_TURNS + 2);
    const summaryText = "Refactored agent.ts.";
    const provider = makeMockProvider(summaryText);

    const { history: compacted } = await compactHistory(history, provider);

    const first = compacted[0];
    expect(first.role).toBe("user");
    expect(typeof first.content).toBe("string");
    expect((first.content as string)).toContain("[Compacted context summary:");
    expect((first.content as string)).toContain(summaryText);
  });

  it("tail messages are preserved verbatim and in order", async () => {
    const history = makeHistory(KEEP_RECENT_TURNS + 2);
    const provider = makeMockProvider("summary");

    const { history: compacted } = await compactHistory(history, provider);

    // The tail should be the last KEEP_RECENT_TURNS * 2 messages of the original
    const expectedTail = history.slice(-(KEEP_RECENT_TURNS * 2));
    const actualTail = compacted.slice(1); // skip synthetic summary
    expect(actualTail).toEqual(expectedTail);
  });

  it("returns syntheticMessage separately (same object as history[0])", async () => {
    const history = makeHistory(KEEP_RECENT_TURNS + 2);
    const provider = makeMockProvider("summary text");

    const { history: compacted, syntheticMessage } = await compactHistory(history, provider);

    expect(syntheticMessage).toBe(compacted[0]); // same object reference
    expect((syntheticMessage.content as string)).toContain("summary text");
  });

  it("returns correct tailStartIndex (offset into original history where tail begins)", async () => {
    const extraPairs = 3;
    const history = makeHistory(KEEP_RECENT_TURNS + extraPairs);
    const provider = makeMockProvider("summary");

    const { tailStartIndex } = await compactHistory(history, provider);

    // head = extraPairs * 2 messages; tail starts right after
    expect(tailStartIndex).toBe(extraPairs * 2);
  });

  it("tail object references are preserved (no copy — same MessageParam objects)", async () => {
    const history = makeHistory(KEEP_RECENT_TURNS + 2);
    const provider = makeMockProvider("summary");

    const { history: compacted, tailStartIndex } = await compactHistory(history, provider);

    const originalTail = history.slice(tailStartIndex);
    const compactedTail = compacted.slice(1);
    for (let i = 0; i < compactedTail.length; i++) {
      expect(compactedTail[i]).toBe(originalTail[i]); // same object reference, not a copy
    }
  });
});
