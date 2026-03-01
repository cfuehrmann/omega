/**
 * Tests for automatic context compaction (performAutoCompact) and the
 * max_tokens mid-tool-call bug fix (BUG-1).
 *
 * Auto-compact:
 *   - Fires when lastPromptTokens > AUTO_COMPACT_THRESHOLD (100k tokens)
 *   - Emits compact_auto_start → compact_auto_done on success
 *   - Emits compact_auto_start → compact_auto_error on LLM failure,
 *     then continues the turn normally (rolling truncation fallback)
 *   - Does NOT fire when context is below threshold
 *   - Correctly updates compactedContextHistory and compactedContextHashes in memory
 *
 * BUG-1 (max_tokens mid-tool-call):
 *   - When stop_reason === "max_tokens" and tool_use blocks are present,
 *     synthetic error tool_results are appended to preserve history integrity.
 *   - agent_error is emitted explaining the truncation.
 *   - The next sendMessage call succeeds (context is well-formed).
 *   - History length after truncated turn = prior + 2 (assistant + synthetic user).
 *
 * Each test uses real session files (contextFile + eventsFile) via makeTestAgent().
 */

import { describe, it, expect, afterEach } from "bun:test";
import { AUTO_COMPACT_THRESHOLD, KEEP_RECENT_TURNS } from "./compaction.js";
import type { StreamProvider, Agent } from "./agent.js";
import type { OmegaEvent, StreamSignal } from "./events.js";
import { makeTestAgent } from "./test-utils.js";
import type Anthropic from "@anthropic-ai/sdk";

// ---------------------------------------------------------------------------
// Mock stream helpers
// ---------------------------------------------------------------------------

function makeMockStream(events: any[], message: any) {
  return {
    async *[Symbol.asyncIterator]() {
      for (const e of events) yield e;
    },
    finalMessage: async () => message,
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

function textMessage(text: string): any {
  return {
    id: "msg_test",
    type: "message",
    role: "assistant",
    model: "claude-sonnet-4-6",
    content: [{ type: "text", text }],
    stop_reason: "end_turn",
    stop_sequence: null,
    usage: { input_tokens: 10, output_tokens: 5 },
  };
}

/** A message whose stop_reason === "max_tokens" with one tool_use block. */
function maxTokensToolUseMessage(toolId: string, toolName: string): any {
  return {
    id: "msg_max_tokens",
    type: "message",
    role: "assistant",
    model: "claude-sonnet-4-6",
    content: [{ type: "tool_use", id: toolId, name: toolName, input: { path: "incomplete" } }],
    stop_reason: "max_tokens",
    stop_sequence: null,
    usage: { input_tokens: 50, output_tokens: 8192 },
  };
}

/** Stream events that match a max_tokens stop with a tool_use block. */
function maxTokensToolUseStreamEvents(toolId: string, toolName: string): any[] {
  return [
    { type: "content_block_start", index: 0, content_block: { type: "tool_use", id: toolId, name: toolName } },
    { type: "content_block_delta", index: 0, delta: { type: "input_json_delta", partial_json: '{"path":' } },
    { type: "content_block_stop", index: 0 },
    { type: "message_delta", delta: { stop_reason: "max_tokens" }, usage: { output_tokens: 8192 } },
    { type: "message_stop" },
  ];
}

/** A message whose stop_reason === "max_tokens" with TWO tool_use blocks. */
function maxTokensTwoToolsMessage(id1: string, id2: string): any {
  return {
    id: "msg_max_tokens_2",
    type: "message",
    role: "assistant",
    model: "claude-sonnet-4-6",
    content: [
      { type: "tool_use", id: id1, name: "read_file", input: { path: "a" } },
      { type: "tool_use", id: id2, name: "run_command", input: { command: "ls" } },
    ],
    stop_reason: "max_tokens",
    stop_sequence: null,
    usage: { input_tokens: 50, output_tokens: 8192 },
  };
}

/** Collect all events from agent.sendMessage(). */
async function collectEvents(
  agent: Agent,
  message: string
): Promise<(OmegaEvent | StreamSignal)[]> {
  const events: (OmegaEvent | StreamSignal)[] = [];
  for await (const ev of agent.sendMessage(message, async () => true)) {
    events.push(ev);
  }
  return events;
}

function omegaEvents(events: (OmegaEvent | StreamSignal)[]): OmegaEvent[] {
  return events.filter((e): e is OmegaEvent => e.type !== "text");
}

/** Build a StreamProvider that returns a plain text response unconditionally. */
function makeTextProvider(text = "ok"): StreamProvider {
  return async () => makeMockStream(textStreamEvents(text), textMessage(text));
}

/** Build a StreamProvider that throws for the summary call, then responds normally. */
function makeFailThenTextProvider(errorMsg: string, text = "ok"): StreamProvider {
  let calls = 0;
  return async () => {
    calls++;
    if (calls === 1) throw new Error(errorMsg);
    return makeMockStream(textStreamEvents(text), textMessage(text));
  };
}

/** Build a StreamProvider that returns a fixed summary for the compaction LLM call,
 *  then a normal text reply for the actual turn. */
function makeSummaryThenTextProvider(summary: string, text = "ok"): StreamProvider {
  let calls = 0;
  return async () => {
    calls++;
    if (calls === 1) {
      return makeMockStream(textStreamEvents(summary), textMessage(summary));
    }
    return makeMockStream(textStreamEvents(text), textMessage(text));
  };
}

/** Seed the agent's compactedContextHistory with N synthetic messages alternating user/assistant. */
function seedHistory(agent: Agent, count: number): void {
  const view = agent.getCompactedContextHistory() as Anthropic.MessageParam[];
  const hashes = agent.getCompactedContextHashes() as string[];
  for (let i = 0; i < count; i++) {
    view.push({
      role: i % 2 === 0 ? "user" : "assistant",
      content: `synthetic message ${i}`,
    });
    hashes.push(`seed${i.toString().padStart(4, "0")}`);
  }
}

/**
 * Set the agent's lastPromptTokens above AUTO_COMPACT_THRESHOLD so that
 * performAutoCompact() will fire on the next sendMessage call.
 */
function setAboveThreshold(agent: Agent): void {
  agent.lastPromptTokens = AUTO_COMPACT_THRESHOLD + 1;
  seedHistory(agent, KEEP_RECENT_TURNS * 2 + 3);
}

/**
 * Set the agent's lastPromptTokens to a value at or below AUTO_COMPACT_THRESHOLD
 * so that performAutoCompact() will NOT fire.
 */
function setBelowThreshold(agent: Agent): void {
  agent.lastPromptTokens = AUTO_COMPACT_THRESHOLD - 1;
}

// ---------------------------------------------------------------------------
// Cleanup
// ---------------------------------------------------------------------------

const disposeAll: (() => void)[] = [];
afterEach(() => { disposeAll.splice(0).forEach(d => d()); });

// ---------------------------------------------------------------------------
// AUTO_COMPACT_THRESHOLD constant sanity check
// ---------------------------------------------------------------------------

describe("AUTO_COMPACT_THRESHOLD", () => {
  it("is exported from compaction.ts and is a positive integer", () => {
    expect(typeof AUTO_COMPACT_THRESHOLD).toBe("number");
    expect(AUTO_COMPACT_THRESHOLD).toBeGreaterThan(0);
    expect(Number.isInteger(AUTO_COMPACT_THRESHOLD)).toBe(true);
  });

  it("is greater than KEEP_RECENT_TURNS * 2 (auto-compact fires before tail-only situation)", () => {
    expect(AUTO_COMPACT_THRESHOLD).toBeGreaterThan(KEEP_RECENT_TURNS * 2);
  });
});

// ---------------------------------------------------------------------------
// Auto-compact: fires above threshold
// ---------------------------------------------------------------------------

describe("auto-compact: fires above threshold", () => {
  it("emits compact_auto_start and compact_auto_done when context exceeds threshold", async () => {
    const provider = makeSummaryThenTextProvider("summary of head");
    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);
    setAboveThreshold(agent);

    const events = omegaEvents(await collectEvents(agent, "hello"));

    expect(events.find(e => e.type === "compact_auto_start")).toBeDefined();
    expect(events.find(e => e.type === "compact_auto_done")).toBeDefined();
  });

  it("compact_auto_start carries messagesBefore equal to history length before compaction", async () => {
    const provider = makeSummaryThenTextProvider("summary");
    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);
    setAboveThreshold(agent);
    const seeded = agent.getCompactedContextHistory().length;

    const events = omegaEvents(await collectEvents(agent, "hello"));

    const start = events.find(e => e.type === "compact_auto_start");
    expect(start).toBeDefined();
    if (start?.type === "compact_auto_start") {
      expect(start.messagesBefore).toBe(seeded + 1);
    }
  });

  it("compact_auto_done.messagesBefore matches compact_auto_start.messagesBefore", async () => {
    const provider = makeSummaryThenTextProvider("summary");
    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);
    setAboveThreshold(agent);

    const events = omegaEvents(await collectEvents(agent, "hello"));

    const start = events.find(e => e.type === "compact_auto_start");
    const done = events.find(e => e.type === "compact_auto_done");
    if (start?.type === "compact_auto_start" && done?.type === "compact_auto_done") {
      expect(done.messagesBefore).toBe(start.messagesBefore);
    }
  });

  it("compact_auto_done.messagesAfter is less than messagesBefore", async () => {
    const provider = makeSummaryThenTextProvider("long session summary");
    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);
    setAboveThreshold(agent);

    const events = omegaEvents(await collectEvents(agent, "hello"));

    const done = events.find(e => e.type === "compact_auto_done");
    expect(done).toBeDefined();
    if (done?.type === "compact_auto_done") {
      expect(done.messagesAfter).toBeLessThan(done.messagesBefore);
    }
  });

  it("compactedContextHistory is shorter after auto-compaction", async () => {
    const provider = makeSummaryThenTextProvider("summary");
    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);
    setAboveThreshold(agent);
    const before = agent.getCompactedContextHistory().length;

    await collectEvents(agent, "hello");

    const after = agent.getCompactedContextHistory().length;
    expect(after).toBeLessThan(before + 1);
  });

  it("compact_auto_start appears before compact_auto_done in event stream", async () => {
    const provider = makeSummaryThenTextProvider("summary");
    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);
    setAboveThreshold(agent);

    const events = omegaEvents(await collectEvents(agent, "hello"));

    const startIdx = events.findIndex(e => e.type === "compact_auto_start");
    const doneIdx = events.findIndex(e => e.type === "compact_auto_done");
    expect(startIdx).toBeGreaterThanOrEqual(0);
    expect(doneIdx).toBeGreaterThan(startIdx);
  });

  it("compact_auto events appear before llm_call in the stream", async () => {
    const provider = makeSummaryThenTextProvider("summary");
    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);
    setAboveThreshold(agent);

    const events = omegaEvents(await collectEvents(agent, "hello"));

    const doneIdx = events.findIndex(e => e.type === "compact_auto_done");
    const llmCallIdx = events.findLastIndex(e => e.type === "llm_call");
    expect(doneIdx).toBeGreaterThanOrEqual(0);
    expect(llmCallIdx).toBeGreaterThan(doneIdx);
  });
});

// ---------------------------------------------------------------------------
// Auto-compact: does NOT fire below threshold
// ---------------------------------------------------------------------------

describe("auto-compact: does not fire below threshold", () => {
  it("emits no compact_auto events when lastPromptTokens is below threshold", async () => {
    const provider = makeTextProvider("ok");
    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);
    setBelowThreshold(agent);

    const events = omegaEvents(await collectEvents(agent, "hello"));

    const autoEvents = events.filter(e =>
      e.type === "compact_auto_start" ||
      e.type === "compact_auto_done" ||
      e.type === "compact_auto_error"
    );
    expect(autoEvents).toHaveLength(0);
  });

  it("emits no compact_auto events when lastPromptTokens is exactly at threshold", async () => {
    const provider = makeTextProvider("ok");
    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);
    agent.lastPromptTokens = AUTO_COMPACT_THRESHOLD;

    const events = omegaEvents(await collectEvents(agent, "hello"));

    const autoEvents = events.filter(e =>
      e.type === "compact_auto_start" ||
      e.type === "compact_auto_done" ||
      e.type === "compact_auto_error"
    );
    expect(autoEvents).toHaveLength(0);
  });

  it("emits no compact_auto events on first turn (lastPromptTokens starts at 0)", async () => {
    const provider = makeTextProvider("ok");
    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);

    const events = omegaEvents(await collectEvents(agent, "hello"));

    const autoEvents = events.filter(e =>
      e.type === "compact_auto_start" ||
      e.type === "compact_auto_done" ||
      e.type === "compact_auto_error"
    );
    expect(autoEvents).toHaveLength(0);
  });
});

// ---------------------------------------------------------------------------
// Auto-compact: error path
// ---------------------------------------------------------------------------

describe("auto-compact: error path", () => {
  it("emits compact_auto_error when LLM throws during compaction", async () => {
    const provider = makeFailThenTextProvider("LLM failed for compaction");
    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);
    setAboveThreshold(agent);

    const events = omegaEvents(await collectEvents(agent, "hello"));

    const errEv = events.find(e => e.type === "compact_auto_error");
    expect(errEv).toBeDefined();
    if (errEv?.type === "compact_auto_error") {
      expect(errEv.error).toContain("LLM failed for compaction");
    }
  });

  it("emits compact_auto_start before compact_auto_error", async () => {
    const provider = makeFailThenTextProvider("boom");
    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);
    setAboveThreshold(agent);

    const events = omegaEvents(await collectEvents(agent, "hello"));

    const startIdx = events.findIndex(e => e.type === "compact_auto_start");
    const errIdx = events.findIndex(e => e.type === "compact_auto_error");
    expect(startIdx).toBeGreaterThanOrEqual(0);
    expect(errIdx).toBeGreaterThan(startIdx);
  });

  it("turn still completes after auto-compact error (rolling truncation fallback)", async () => {
    const provider = makeFailThenTextProvider("boom");
    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);
    setAboveThreshold(agent);

    const events = omegaEvents(await collectEvents(agent, "hello"));

    expect(events.find(e => e.type === "turn_end")).toBeDefined();
  });

  it("compactedContextHistory is unchanged after auto-compact error", async () => {
    const provider = makeFailThenTextProvider("boom");
    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);
    setAboveThreshold(agent);
    const viewBefore = agent.getCompactedContextHistory().length;

    await collectEvents(agent, "hello");

    const viewAfter = agent.getCompactedContextHistory().length;
    expect(viewAfter).toBe(viewBefore + 2);
  });

  it("no compact_auto_done event on error path", async () => {
    const provider = makeFailThenTextProvider("boom");
    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);
    setAboveThreshold(agent);

    const events = omegaEvents(await collectEvents(agent, "hello"));

    expect(events.find(e => e.type === "compact_auto_done")).toBeUndefined();
  });
});

// ---------------------------------------------------------------------------
// BUG-1: max_tokens mid-tool-call — synthetic error tool_results
// ---------------------------------------------------------------------------

describe("BUG-1: max_tokens mid-tool-call context poison prevention", () => {
  it("emits tool_result(isError=true) when stop_reason === max_tokens with tool_use", async () => {
    const provider: StreamProvider = async () =>
      makeMockStream(
        maxTokensToolUseStreamEvents("t_max", "write_file"),
        maxTokensToolUseMessage("t_max", "write_file")
      );
    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);

    const events = omegaEvents(await collectEvents(agent, "write a file"));

    const toolResult = events.find(e => e.type === "tool_result");
    expect(toolResult).toBeDefined();
    if (toolResult?.type === "tool_result") {
      expect(toolResult.isError).toBe(true);
      expect(toolResult.name).toBe("write_file");
    }
  });

  it("emits agent_error explaining the truncation", async () => {
    const provider: StreamProvider = async () =>
      makeMockStream(
        maxTokensToolUseStreamEvents("t_max", "write_file"),
        maxTokensToolUseMessage("t_max", "write_file")
      );
    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);

    const events = omegaEvents(await collectEvents(agent, "write a file"));

    const errEv = events.find(e => e.type === "agent_error");
    expect(errEv).toBeDefined();
    if (errEv?.type === "agent_error") {
      expect(errEv.error).toContain("max_tokens");
      expect(errEv.error).toContain("write_file");
    }
  });

  it("compactedContextHistory ends with a user message (tool_result) after max_tokens", async () => {
    const provider: StreamProvider = async () =>
      makeMockStream(
        maxTokensToolUseStreamEvents("t_max", "write_file"),
        maxTokensToolUseMessage("t_max", "write_file")
      );
    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);

    await collectEvents(agent, "write a file");

    const view = agent.getCompactedContextHistory();
    const last = view[view.length - 1];
    expect(last.role).toBe("user");
    const content = last.content;
    expect(Array.isArray(content)).toBe(true);
    if (Array.isArray(content)) {
      const resultBlocks = content.filter((b: any) => b.type === "tool_result");
      expect(resultBlocks.length).toBeGreaterThan(0);
      const block = resultBlocks[0] as any;
      expect(block.tool_use_id).toBe("t_max");
      expect(block.is_error).toBe(true);
    }
  });

  it("context is well-formed after max_tokens: every tool_use has a matching tool_result", async () => {
    const provider: StreamProvider = async () =>
      makeMockStream(
        maxTokensToolUseStreamEvents("t_max", "write_file"),
        maxTokensToolUseMessage("t_max", "write_file")
      );
    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);

    await collectEvents(agent, "write a file");

    const view = agent.getCompactedContextHistory();
    const toolUseIds = new Set<string>();
    const toolResultIds = new Set<string>();
    for (const msg of view) {
      if (!Array.isArray(msg.content)) continue;
      for (const b of msg.content as any[]) {
        if (b.type === "tool_use") toolUseIds.add(b.id);
        if (b.type === "tool_result") toolResultIds.add(b.tool_use_id);
      }
    }
    for (const id of toolUseIds) {
      expect(toolResultIds.has(id)).toBe(true);
    }
  });

  it("history length after max_tokens turn = initial + 3 (user msg + assistant + synthetic result)", async () => {
    const provider: StreamProvider = async () =>
      makeMockStream(
        maxTokensToolUseStreamEvents("t_max", "write_file"),
        maxTokensToolUseMessage("t_max", "write_file")
      );
    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);
    const initialLength = agent.getCompactedContextHistory().length;

    await collectEvents(agent, "write a file");

    expect(agent.getCompactedContextHistory().length).toBe(initialLength + 3);
  });

  it("next sendMessage succeeds after a max_tokens turn (context not bricked)", async () => {
    let callCount = 0;
    const provider: StreamProvider = async () => {
      callCount++;
      if (callCount === 1) {
        return makeMockStream(
          maxTokensToolUseStreamEvents("t_max", "write_file"),
          maxTokensToolUseMessage("t_max", "write_file")
        );
      }
      return makeMockStream(textStreamEvents("all good"), textMessage("all good"));
    };
    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);

    await collectEvents(agent, "write a file");

    const events2 = omegaEvents(await collectEvents(agent, "ping"));
    expect(events2.filter(e => e.type === "agent_error")).toHaveLength(0);
    expect(events2.find(e => e.type === "turn_end")).toBeDefined();
  });

  it("handles TWO tool_use blocks cut off by max_tokens — both get synthetic results", async () => {
    const provider: StreamProvider = async () =>
      makeMockStream(
        [
          { type: "content_block_start", index: 0, content_block: { type: "tool_use", id: "id1", name: "read_file" } },
          { type: "content_block_stop", index: 0 },
          { type: "content_block_start", index: 1, content_block: { type: "tool_use", id: "id2", name: "run_command" } },
          { type: "content_block_stop", index: 1 },
          { type: "message_delta", delta: { stop_reason: "max_tokens" }, usage: { output_tokens: 8192 } },
          { type: "message_stop" },
        ],
        maxTokensTwoToolsMessage("id1", "id2")
      );
    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);

    const events = omegaEvents(await collectEvents(agent, "do two things"));

    const toolResults = events.filter(e => e.type === "tool_result");
    expect(toolResults.length).toBe(2);
    const names = toolResults.map(e => e.type === "tool_result" ? e.name : "");
    expect(names).toContain("read_file");
    expect(names).toContain("run_command");

    const view = agent.getCompactedContextHistory();
    const toolUseIds = new Set<string>();
    const toolResultIds = new Set<string>();
    for (const msg of view) {
      if (!Array.isArray(msg.content)) continue;
      for (const b of msg.content as any[]) {
        if (b.type === "tool_use") toolUseIds.add(b.id);
        if (b.type === "tool_result") toolResultIds.add(b.tool_use_id);
      }
    }
    for (const id of toolUseIds) {
      expect(toolResultIds.has(id)).toBe(true);
    }
  });

  it("agent_error message names the tool that was cut off", async () => {
    const provider: StreamProvider = async () =>
      makeMockStream(
        maxTokensToolUseStreamEvents("t_cut", "fetch_url"),
        maxTokensToolUseMessage("t_cut", "fetch_url")
      );
    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);

    const events = omegaEvents(await collectEvents(agent, "fetch a url"));

    const errEv = events.find(e => e.type === "agent_error");
    expect(errEv?.type === "agent_error" && errEv.error).toContain("fetch_url");
  });

  it("synthetic tool_result content mentions max_tokens and non-execution", async () => {
    const provider: StreamProvider = async () =>
      makeMockStream(
        maxTokensToolUseStreamEvents("t_max", "write_file"),
        maxTokensToolUseMessage("t_max", "write_file")
      );
    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);

    await collectEvents(agent, "write a file");

    const view = agent.getCompactedContextHistory();
    const last = view[view.length - 1];
    if (Array.isArray(last.content)) {
      const block = (last.content as any[]).find(b => b.type === "tool_result");
      expect(block?.content).toContain("max_tokens");
      expect(block?.content).toContain("not executed");
    }
  });
});

// ---------------------------------------------------------------------------
// lastPromptTokens update
// ---------------------------------------------------------------------------

describe("auto-compact: lastPromptTokens is updated after each turn", () => {
  it("lastPromptTokens reflects the LLM usage from the completed turn", async () => {
    const provider = makeTextProvider("ok");
    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);
    expect(agent.lastPromptTokens).toBe(0);

    await collectEvents(agent, "hello");

    expect(agent.lastPromptTokens).toBeGreaterThan(0);
  });

  it("lastPromptTokens above threshold triggers auto-compact on the next turn", async () => {
    let callCount = 0;
    const provider: StreamProvider = async () => {
      callCount++;
      if (callCount === 1) return makeMockStream(textStreamEvents("first"), textMessage("first"));
      if (callCount === 2) return makeMockStream(textStreamEvents("summary"), textMessage("summary"));
      return makeMockStream(textStreamEvents("second"), textMessage("second"));
    };
    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);

    await collectEvents(agent, "first message");
    setAboveThreshold(agent);

    const events2 = omegaEvents(await collectEvents(agent, "second message"));
    expect(events2.find(e => e.type === "compact_auto_start")).toBeDefined();
    expect(events2.find(e => e.type === "compact_auto_done")).toBeDefined();
  });

  it("lastPromptTokens does NOT trigger auto-compact again within the same turn", async () => {
    const provider = makeSummaryThenTextProvider("summary");
    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);
    setAboveThreshold(agent);

    const events = omegaEvents(await collectEvents(agent, "hello"));

    const compactAutoStartCount = events.filter(e => e.type === "compact_auto_start").length;
    expect(compactAutoStartCount).toBe(1);
  });
});

// ---------------------------------------------------------------------------
// compact_auto_done.messagesAfter shape
// ---------------------------------------------------------------------------

describe("auto-compact: messagesAfter shape", () => {
  it("messagesAfter equals 1 (synthetic) + KEEP_RECENT_TURNS * 2 (tail) when history is long enough", async () => {
    const provider = makeSummaryThenTextProvider("summary");
    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);
    setAboveThreshold(agent);

    const events = omegaEvents(await collectEvents(agent, "hello"));

    const done = events.find(e => e.type === "compact_auto_done");
    expect(done).toBeDefined();
    if (done?.type === "compact_auto_done") {
      expect(done.messagesAfter).toBe(1 + KEEP_RECENT_TURNS * 2);
    }
  });

  it("compactedContextHistory length after auto-compact matches compact_auto_done.messagesAfter", async () => {
    const provider = makeSummaryThenTextProvider("summary");
    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);
    setAboveThreshold(agent);

    const events = omegaEvents(await collectEvents(agent, "hello"));

    const done = events.find(e => e.type === "compact_auto_done");
    if (done?.type === "compact_auto_done") {
      expect(agent.getCompactedContextHistory().length).toBe(done.messagesAfter + 1);
    }
  });
});

// ---------------------------------------------------------------------------
// Hash consistency after auto-compact
// ---------------------------------------------------------------------------

describe("auto-compact: hash consistency (compactedContextHashes stays in sync)", () => {
  it("llm_call.contextHashes length equals compactedContextHistory length at call time", async () => {
    const provider = makeSummaryThenTextProvider("summary");
    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);
    setAboveThreshold(agent);

    const events = omegaEvents(await collectEvents(agent, "hello"));

    const doneIdx = events.findIndex(e => e.type === "compact_auto_done");
    const llmCalls = events.filter((e, i) => e.type === "llm_call" && i > doneIdx);
    expect(llmCalls.length).toBeGreaterThan(0);

    const firstLlmCall = llmCalls[0];
    const done = events.find(e => e.type === "compact_auto_done");
    if (firstLlmCall?.type === "llm_call" && done?.type === "compact_auto_done") {
      expect(firstLlmCall.contextHashes.length).toBe(done.messagesAfter);
    }
  });

  it("first contextHash after auto-compact is a real 8-char hex hash (the synthetic summary)", async () => {
    const provider = makeSummaryThenTextProvider("summary");
    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);
    setAboveThreshold(agent);

    const events = omegaEvents(await collectEvents(agent, "hello"));

    const doneIdx = events.findIndex(e => e.type === "compact_auto_done");
    const llmCalls = events.filter((e, i) => e.type === "llm_call" && i > doneIdx);
    const firstLlmCall = llmCalls[0];
    if (firstLlmCall?.type === "llm_call" && firstLlmCall.contextHashes.length > 0) {
      expect(firstLlmCall.contextHashes[0]).toMatch(/^[0-9a-f]{8}$/);
    }
  });

  it("contextHashes count grows by 1 after the assistant reply is appended post-compaction", async () => {
    const provider = makeSummaryThenTextProvider("summary");
    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);
    setAboveThreshold(agent);

    const events = omegaEvents(await collectEvents(agent, "hello"));

    const done = events.find(e => e.type === "compact_auto_done");
    if (done?.type === "compact_auto_done") {
      expect(agent.getCompactedContextHistory().length).toBe(done.messagesAfter + 1);
    }
  });
});

// ---------------------------------------------------------------------------
// Integration: auto-compact + max_tokens (belt-and-suspenders)
// ---------------------------------------------------------------------------

describe("auto-compact + max_tokens (combined)", () => {
  it("auto-compact fires, then turn completes normally even after max_tokens in a subsequent turn", async () => {
    let callCount = 0;
    const provider: StreamProvider = async () => {
      callCount++;
      if (callCount === 1) return makeMockStream(textStreamEvents("summary"), textMessage("summary"));
      if (callCount === 2) {
        return makeMockStream(
          maxTokensToolUseStreamEvents("t_max", "read_file"),
          maxTokensToolUseMessage("t_max", "read_file")
        );
      }
      return makeMockStream(textStreamEvents("all good"), textMessage("all good"));
    };
    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);
    setAboveThreshold(agent);

    const events1 = omegaEvents(await collectEvents(agent, "do something"));
    expect(events1.find(e => e.type === "compact_auto_start")).toBeDefined();
    expect(events1.find(e => e.type === "compact_auto_done")).toBeDefined();
    expect(events1.find(e => e.type === "agent_error")).toBeDefined();

    const events2 = omegaEvents(await collectEvents(agent, "ping"));
    expect(events2.find(e => e.type === "turn_end")).toBeDefined();
    expect(events2.filter(e => e.type === "agent_error")).toHaveLength(0);
  });
});
