/**
 * Tests for server-side compaction integration.
 *
 * Covers:
 *   - compacted event is emitted when the response contains a compaction block
 *   - compacted event carries the full usage object verbatim (including iterations)
 *   - compacted event is persisted to events.jsonl
 *   - compactedContextHistory is pruned: only the compacting assistant message
 *     and subsequent messages remain after compaction
 *   - compactedContextHashes is pruned in parallel with history
 *   - normal turns (no compaction block) do NOT emit compacted event
 *   - the turn continues normally after compaction (tool_use → tool_result cycle works)
 *   - compacted event appears before llm_response in the event stream
 *   - compacted event carries a time field
 */

import { describe, it, expect, afterAll } from "bun:test";
import type { CreateMessageStream, OmegaEvent, StreamSignal } from "./agent.js";
import { makeTestAgent } from "./test-utils.js";
import { OmegaEventSchema } from "./events.schema.js";
import { readFileSync, existsSync } from "fs";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function makeMockStream(streamEvents: any[], finalMsg: any) {
  return {
    async *[Symbol.asyncIterator]() {
      for (const e of streamEvents) yield e;
    },
    finalMessage: async () => finalMsg,
  };
}

function textStreamEvents(text: string): any[] {
  return [
    { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } },
    { type: "content_block_delta", index: 0, delta: { type: "text_delta", text } },
    { type: "content_block_stop", index: 0 },
    { type: "message_delta", delta: { stop_reason: "end_turn" }, usage: { output_tokens: 5 } },
    { type: "message_stop" },
  ];
}

function textMessage(text: string, extra: Record<string, unknown> = {}): any {
  return {
    id: "msg_test",
    type: "message",
    role: "assistant",
    model: "claude-sonnet-4-6",
    content: [{ type: "text", text }],
    stop_reason: "end_turn",
    stop_sequence: null,
    usage: { input_tokens: 10, output_tokens: 5 },
    ...extra,
  };
}

/** A finalMessage with a compaction block at index 0, then the actual text response. */
function compactionMessage(summaryText: string, replyText: string, usageOverride?: Record<string, unknown>): any {
  return {
    id: "msg_compacted",
    type: "message",
    role: "assistant",
    model: "claude-sonnet-4-6",
    content: [
      { type: "compaction", content: summaryText },
      { type: "text", text: replyText },
    ],
    stop_reason: "end_turn",
    stop_sequence: null,
    usage: {
      input_tokens: 500,
      output_tokens: 50,
      cache_creation_input_tokens: null,
      cache_read_input_tokens: null,
      service_tier: "standard",
      ...usageOverride,
    },
  };
}

/** Stream events for a compacting response: compaction block (one delta) + text. */
function compactionStreamEvents(summaryText: string, replyText: string): any[] {
  return [
    // Compaction block — arrives as a single delta (no incremental streaming)
    { type: "content_block_start", index: 0, content_block: { type: "compaction", content: "" } },
    { type: "content_block_delta", index: 0, delta: { type: "compaction_delta", content: summaryText } },
    { type: "content_block_stop", index: 0 },
    // Actual text response
    { type: "content_block_start", index: 1, content_block: { type: "text", text: "" } },
    { type: "content_block_delta", index: 1, delta: { type: "text_delta", text: replyText } },
    { type: "content_block_stop", index: 1 },
    { type: "message_delta", delta: { stop_reason: "end_turn" }, usage: { output_tokens: 50 } },
    { type: "message_stop" },
  ];
}

async function collectEvents(agent: any, message: string): Promise<(OmegaEvent | StreamSignal)[]> {
  const events: (OmegaEvent | StreamSignal)[] = [];
  for await (const e of agent.sendMessage(message, async () => true)) {
    events.push(e);
  }
  return events;
}

function readEventsFile(path: string): OmegaEvent[] {
  if (!existsSync(path)) return [];
  return readFileSync(path, "utf-8")
    .split("\n")
    .filter(Boolean)
    .map(line => OmegaEventSchema.parse(JSON.parse(line)));
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("server-side compaction — compacted event emitted", () => {
  it("emits a compacted event when response contains a compaction block", async () => {
    const provider: CreateMessageStream = () =>
      makeMockStream(
        compactionStreamEvents("Summary of prior work.", "Got it!"),
        compactionMessage("Summary of prior work.", "Got it!"),
      );
    const { agent, dispose } = await makeTestAgent(provider);
    afterAll(dispose);

    const events = await collectEvents(agent, "hello");
    const types = events.map(e => e.type);
    expect(types).toContain("compacted");
  });

  it("does NOT emit compacted event on a normal (non-compacting) response", async () => {
    const provider: CreateMessageStream = () =>
      makeMockStream(textStreamEvents("hello back"), textMessage("hello back"));
    const { agent, dispose } = await makeTestAgent(provider);
    afterAll(dispose);

    const events = await collectEvents(agent, "hello");
    expect(events.find(e => e.type === "compacted")).toBeUndefined();
  });
});

describe("server-side compaction — compacted event fields", () => {
  it("compacted event carries a time field", async () => {
    const provider: CreateMessageStream = () =>
      makeMockStream(
        compactionStreamEvents("summary", "reply"),
        compactionMessage("summary", "reply"),
      );
    const { agent, dispose } = await makeTestAgent(provider);
    afterAll(dispose);

    const events = await collectEvents(agent, "hello");
    const ev = events.find(e => e.type === "compacted") as any;
    expect(ev).toBeDefined();
    expect(typeof ev.time).toBe("string");
    expect(ev.time).toMatch(/^\d{4}-\d{2}-\d{2}T/);
  });

  it("compacted event preserves the full usage object verbatim", async () => {
    const usage = {
      input_tokens: 500,
      output_tokens: 50,
      cache_creation_input_tokens: null,
      cache_read_input_tokens: null,
      service_tier: "standard",
      iterations: [
        { type: "compaction", input_tokens: 80000, output_tokens: 300 },
        { type: "message", input_tokens: 500, output_tokens: 50 },
      ],
    };
    const provider: CreateMessageStream = () =>
      makeMockStream(
        compactionStreamEvents("summary", "reply"),
        compactionMessage("summary", "reply", { iterations: usage.iterations }),
      );
    const { agent, dispose } = await makeTestAgent(provider);
    afterAll(dispose);

    const events = await collectEvents(agent, "hello");
    const ev = events.find(e => e.type === "compacted") as any;
    expect(ev.usage).toMatchObject({ input_tokens: 500, output_tokens: 50 });
    expect(ev.usage.iterations).toEqual(usage.iterations);
  });

  it("compacted event usage preserved even without iterations (no compaction in this call)", async () => {
    const provider: CreateMessageStream = () =>
      makeMockStream(
        compactionStreamEvents("summary", "reply"),
        compactionMessage("summary", "reply"),
      );
    const { agent, dispose } = await makeTestAgent(provider);
    afterAll(dispose);

    const events = await collectEvents(agent, "hello");
    const ev = events.find(e => e.type === "compacted") as any;
    expect(ev.usage).toMatchObject({ input_tokens: 500, output_tokens: 50 });
  });
});

describe("server-side compaction — event ordering", () => {
  it("compacted appears before llm_response in the stream", async () => {
    const provider: CreateMessageStream = () =>
      makeMockStream(
        compactionStreamEvents("summary", "reply"),
        compactionMessage("summary", "reply"),
      );
    const { agent, dispose } = await makeTestAgent(provider);
    afterAll(dispose);

    const events = await collectEvents(agent, "hello");
    const compactedIdx = events.findIndex(e => e.type === "compacted");
    const llmRespIdx = events.findIndex(e => e.type === "llm_response");
    expect(compactedIdx).toBeGreaterThanOrEqual(0);
    expect(llmRespIdx).toBeGreaterThan(compactedIdx);
  });
});

describe("server-side compaction — history pruning", () => {
  it("compactedContextHistory is cleared before the compacting assistant message is appended", async () => {
    // Seed two prior turns, then trigger compaction on the third message.
    let call = 0;
    const provider: CreateMessageStream = () => {
      call++;
      if (call <= 2) {
        return makeMockStream(textStreamEvents(`reply ${call}`), textMessage(`reply ${call}`));
      }
      return makeMockStream(
        compactionStreamEvents("summary", "post-compact reply"),
        compactionMessage("summary", "post-compact reply"),
      );
    };
    const { agent, dispose } = await makeTestAgent(provider);
    afterAll(dispose);

    await collectEvents(agent, "turn 1");
    await collectEvents(agent, "turn 2");
    // History before compaction: [user1, assistant1, user2, assistant2, user3]
    // (user3 is appended at start of sendMessage before the LLM call)
    await collectEvents(agent, "turn 3 — compaction fires");

    // After compaction: history should be [assistant_with_compaction_block, user3_tool_result?]
    // In the simple text case: [assistant_with_compaction_block]
    // Then user3 message was appended before compaction fired, but was cleared.
    // Only the compacting assistant message remains (appended after the clear).
    const history = agent.getCompactedContextHistory();
    expect(history.length).toBe(1);
    expect(history[0]!.role).toBe("assistant");
    // The compaction block should be the first content block
    const content = history[0]!.content as any[];
    expect(content[0]!.type).toBe("compaction");
  });

  it("compactedContextHashes length matches compactedContextHistory length after compaction", async () => {
    let call = 0;
    const provider: CreateMessageStream = () => {
      call++;
      if (call <= 2) {
        return makeMockStream(textStreamEvents(`reply ${call}`), textMessage(`reply ${call}`));
      }
      return makeMockStream(
        compactionStreamEvents("summary", "reply"),
        compactionMessage("summary", "reply"),
      );
    };
    const { agent, dispose } = await makeTestAgent(provider);
    afterAll(dispose);

    await collectEvents(agent, "turn 1");
    await collectEvents(agent, "turn 2");
    await collectEvents(agent, "turn 3 — compaction fires");

    const history = agent.getCompactedContextHistory();
    const hashes = agent.getCompactedContextHashes();
    expect(hashes.length).toBe(history.length);
  });

  it("agent can continue sending messages after compaction", async () => {
    let call = 0;
    const provider: CreateMessageStream = () => {
      call++;
      if (call === 1) {
        return makeMockStream(
          compactionStreamEvents("summary", "compacted reply"),
          compactionMessage("summary", "compacted reply"),
        );
      }
      return makeMockStream(textStreamEvents("post-compact reply"), textMessage("post-compact reply"));
    };
    const { agent, dispose } = await makeTestAgent(provider);
    afterAll(dispose);

    await collectEvents(agent, "first turn — compaction fires");
    const events = await collectEvents(agent, "second turn — normal");

    const types = events.map(e => e.type);
    expect(types).toContain("llm_response");
    expect(types).not.toContain("llm_error");
    expect(types).not.toContain("agent_error");
  });

  it("history grows correctly after compaction + one more turn", async () => {
    let call = 0;
    const provider: CreateMessageStream = () => {
      call++;
      if (call === 1) {
        return makeMockStream(
          compactionStreamEvents("summary", "compacted reply"),
          compactionMessage("summary", "compacted reply"),
        );
      }
      return makeMockStream(textStreamEvents("reply"), textMessage("reply"));
    };
    const { agent, dispose } = await makeTestAgent(provider);
    afterAll(dispose);

    await collectEvents(agent, "first turn — compaction fires");
    const lenAfterCompact = agent.getCompactedContextHistory().length;
    // After compaction: [assistant_with_compaction_block] = 1 message

    await collectEvents(agent, "second turn");
    const lenAfterSecond = agent.getCompactedContextHistory().length;
    // After second turn: [assistant_with_compaction_block, user2, assistant2] = 3
    expect(lenAfterSecond).toBe(lenAfterCompact + 2);
  });
});

describe("server-side compaction — persistence", () => {
  it("compacted event is written to events.jsonl", async () => {
    const provider: CreateMessageStream = () =>
      makeMockStream(
        compactionStreamEvents("summary", "reply"),
        compactionMessage("summary", "reply"),
      );
    const { agent, eventsFile, dispose } = await makeTestAgent(provider);
    afterAll(dispose);

    await collectEvents(agent, "hello");

    const persisted = readEventsFile(eventsFile);
    const ev = persisted.find(e => e.type === "compacted");
    expect(ev).toBeDefined();
  });

  it("persisted compacted event round-trips usage correctly", async () => {
    const iterations = [
      { type: "compaction", input_tokens: 80000, output_tokens: 300 },
      { type: "message", input_tokens: 500, output_tokens: 50 },
    ];
    const provider: CreateMessageStream = () =>
      makeMockStream(
        compactionStreamEvents("summary", "reply"),
        compactionMessage("summary", "reply", { iterations }),
      );
    const { agent, eventsFile, dispose } = await makeTestAgent(provider);
    afterAll(dispose);

    await collectEvents(agent, "hello");

    const persisted = readEventsFile(eventsFile);
    const ev = persisted.find(e => e.type === "compacted") as any;
    expect(ev).toBeDefined();
    expect(ev.usage.iterations).toEqual(iterations);
  });
});

describe("server-side compaction — token counting is message-iteration only", () => {
  const ITERATIONS = [
    { type: "compaction", input_tokens: 80000, output_tokens: 300 },
    { type: "message",    input_tokens: 500,   output_tokens: 50  },
  ];

  it("turn_end metrics reflect message iteration only (compaction excluded)", async () => {
    const provider: CreateMessageStream = () =>
      makeMockStream(
        compactionStreamEvents("summary", "reply"),
        compactionMessage("summary", "reply", { iterations: ITERATIONS }),
      );
    const { agent, dispose } = await makeTestAgent(provider);
    afterAll(dispose);

    const events = await collectEvents(agent, "hello");
    const turnEndEv = events.find(e => e.type === "turn_end") as any;
    expect(turnEndEv).toBeDefined();
    // Message-iteration only: 500 / 50 (not 80500 / 350)
    expect(turnEndEv.metrics.inputTokens).toBe(500);
    expect(turnEndEv.metrics.outputTokens).toBe(50);
  });

  it("llm_response usage reflects message iteration only", async () => {
    const provider: CreateMessageStream = () =>
      makeMockStream(
        compactionStreamEvents("summary", "reply"),
        compactionMessage("summary", "reply", { iterations: ITERATIONS }),
      );
    const { agent, dispose } = await makeTestAgent(provider);
    afterAll(dispose);

    const events = await collectEvents(agent, "hello");
    const llmRespEv = events.find(e => e.type === "llm_response") as any;
    expect(llmRespEv).toBeDefined();
    expect(llmRespEv.usage.input_tokens).toBe(500);
    expect(llmRespEv.usage.output_tokens).toBe(50);
  });

  it("session accumulators reflect message iteration only", async () => {
    const provider: CreateMessageStream = () =>
      makeMockStream(
        compactionStreamEvents("summary", "reply"),
        compactionMessage("summary", "reply", { iterations: ITERATIONS }),
      );
    const { agent, dispose } = await makeTestAgent(provider);
    afterAll(dispose);

    await collectEvents(agent, "hello");
    expect(agent.sessionInputTokens).toBe(500);
    expect(agent.sessionOutputTokens).toBe(50);
  });

  it("compacted event still carries full usage with iterations for client-side extraction", async () => {
    const provider: CreateMessageStream = () =>
      makeMockStream(
        compactionStreamEvents("summary", "reply"),
        compactionMessage("summary", "reply", { iterations: ITERATIONS }),
      );
    const { agent, dispose } = await makeTestAgent(provider);
    afterAll(dispose);

    const events = await collectEvents(agent, "hello");
    const compactedEv = events.find(e => e.type === "compacted") as any;
    expect(compactedEv).toBeDefined();
    expect(compactedEv.usage.iterations).toEqual(ITERATIONS);
  });

  it("token counting is unaffected for normal turns without iterations", async () => {
    const provider: CreateMessageStream = () =>
      makeMockStream(textStreamEvents("hello back"), textMessage("hello back"));
    const { agent, dispose } = await makeTestAgent(provider);
    afterAll(dispose);

    const events = await collectEvents(agent, "hello");
    const llmRespEv = events.find(e => e.type === "llm_response") as any;
    expect(llmRespEv.usage.input_tokens).toBe(10);
    expect(llmRespEv.usage.output_tokens).toBe(5);
    expect(agent.sessionInputTokens).toBe(10);
    expect(agent.sessionOutputTokens).toBe(5);
  });
});
