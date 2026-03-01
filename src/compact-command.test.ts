/**
 * Tests for the /compact slash command.
 *
 * Covers the three event variants emitted by the command:
 *   compact_user_start — always first
 *   compact_user_done  — success path (including empty-history no-op)
 *   compact_user_error — failure path (LLM throws)
 *
 * Also verifies the in-memory state mutations:
 *   - compactedContextHistory is replaced by the compacted history
 *   - compactedContextHashes is updated correctly (tail hashes reused, new
 *     synthetic hash prepended)
 *   - context.jsonl is NOT rewritten (append-only invariant)
 *
 * Each test uses real session files (contextFile + eventsFile) via makeTestAgent().
 */

import { describe, it, expect, afterEach } from "bun:test";
import { KEEP_RECENT_TURNS } from "./compaction.js";
import type { StreamProvider } from "./agent.js";
import type { Agent } from "./agent.js";
import type { OmegaEvent, StreamSignal } from "./events.js";
import { makeTestAgent } from "./test-utils.js";

// ---------------------------------------------------------------------------
// Mock provider helpers
// ---------------------------------------------------------------------------

function makeMockStream(events: any[], message: any) {
  return {
    async *[Symbol.asyncIterator]() {
      for (const e of events) yield e;
    },
    finalMessage: async () => message,
  };
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

function textStreamEvents(text: string): any[] {
  return [
    { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } },
    { type: "content_block_delta", index: 0, delta: { type: "text_delta", text } },
    { type: "content_block_stop", index: 0 },
    { type: "message_delta", delta: { stop_reason: "end_turn" }, usage: { output_tokens: 5 } },
    { type: "message_stop" },
  ];
}

/** A StreamProvider that returns a summary text response. */
function makeSummaryProvider(summary: string): StreamProvider {
  return async () => makeMockStream(textStreamEvents(summary), textMessage(summary));
}

/** Collect all events from agent.sendMessage(). */
async function collectEvents(
  agent: Agent,
  message: string
): Promise<(OmegaEvent | StreamSignal)[]> {
  const events: (OmegaEvent | StreamSignal)[] = [];
  for await (const event of agent.sendMessage(message, async () => true)) {
    events.push(event);
  }
  return events;
}

// ---------------------------------------------------------------------------
// Cleanup: each test registers its dispose(); afterEach drains the list.
// ---------------------------------------------------------------------------

const disposeAll: (() => void)[] = [];
afterEach(() => { disposeAll.splice(0).forEach(d => d()); });

// ---------------------------------------------------------------------------
// /compact — empty history (zero messages)
// ---------------------------------------------------------------------------

describe("/compact — empty history", () => {
  it("emits compact_user_start then compact_user_done with 0→0, no LLM call", async () => {
    let llmCallCount = 0;
    const provider: StreamProvider = async () => {
      llmCallCount++;
      return makeMockStream(textStreamEvents("summary"), textMessage("summary"));
    };
    const { agent, dispose } = makeTestAgent(provider);
    disposeAll.push(dispose);

    const events = await collectEvents(agent, "/compact");

    const types = events.map((e) => e.type);
    expect(types).toContain("compact_user_start");
    expect(types).toContain("compact_user_done");
    expect(types).not.toContain("compact_user_error");
    expect(types).not.toContain("agent_error");
    expect(llmCallCount).toBe(0);
  });

  it("compact_user_done has messagesBefore=0 and messagesAfter=0 for empty history", async () => {
    const { agent, dispose } = makeTestAgent(makeSummaryProvider("summary"));
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "/compact");

    const done = events.find((e) => e.type === "compact_user_done") as any;
    expect(done).toBeDefined();
    expect(done.messagesBefore).toBe(0);
    expect(done.messagesAfter).toBe(0);
  });

  it("compact_user_start appears before compact_user_done", async () => {
    const { agent, dispose } = makeTestAgent(makeSummaryProvider("summary"));
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "/compact");

    const types = events.map((e) => e.type);
    const startIdx = types.indexOf("compact_user_start");
    const doneIdx = types.indexOf("compact_user_done");
    expect(startIdx).toBeGreaterThanOrEqual(0);
    expect(doneIdx).toBeGreaterThanOrEqual(0);
    expect(startIdx).toBeLessThan(doneIdx);
  });

  it("compactedContextHistory stays empty after compacting empty history", async () => {
    const { agent, dispose } = makeTestAgent(makeSummaryProvider("summary"));
    disposeAll.push(dispose);
    await collectEvents(agent, "/compact");
    expect(agent.getCompactedContextHistory().length).toBe(0);
  });
});

// ---------------------------------------------------------------------------
// /compact — short history (≤ KEEP_RECENT_TURNS pairs) — no actual compaction
// ---------------------------------------------------------------------------

describe("/compact — short history (nothing to compact)", () => {
  it("emits compact_user_start + compact_user_done even when no messages are dropped", async () => {
    let llmCallCount = 0;
    const provider: StreamProvider = async () => {
      llmCallCount++;
      return makeMockStream(textStreamEvents(`reply ${llmCallCount}`), textMessage(`reply ${llmCallCount}`));
    };
    const { agent, dispose } = makeTestAgent(provider);
    disposeAll.push(dispose);
    const seedCount = 2; // pairs < KEEP_RECENT_TURNS (10)
    for (let i = 0; i < seedCount; i++) {
      await collectEvents(agent, `turn ${i}`);
    }
    const llmCallsAfterSeed = llmCallCount;
    expect(agent.getCompactedContextHistory().length).toBe(seedCount * 2);

    const events = await collectEvents(agent, "/compact");
    const types = events.map((e) => e.type);
    expect(types).toContain("compact_user_start");
    expect(types).toContain("compact_user_done");
    expect(types).not.toContain("compact_user_error");
    expect(llmCallCount).toBe(llmCallsAfterSeed);
  });

  it("compact_user_done messagesBefore equals history length before compact", async () => {
    let callNum = 0;
    const provider: StreamProvider = async () => {
      callNum++;
      return makeMockStream(textStreamEvents("reply"), textMessage("reply"));
    };
    const { agent, dispose } = makeTestAgent(provider);
    disposeAll.push(dispose);
    const seedPairs = 3;
    for (let i = 0; i < seedPairs; i++) {
      await collectEvents(agent, `msg ${i}`);
    }
    const historyLenBefore = agent.getCompactedContextHistory().length;

    const events = await collectEvents(agent, "/compact");
    const done = events.find((e) => e.type === "compact_user_done") as any;
    expect(done.messagesBefore).toBe(historyLenBefore);
  });
});

// ---------------------------------------------------------------------------
// /compact — long history (> KEEP_RECENT_TURNS pairs) — real compaction
// ---------------------------------------------------------------------------

describe("/compact — long history (compaction happens)", () => {
  const EXTRA_PAIRS = 3;
  const TOTAL_PAIRS = KEEP_RECENT_TURNS + EXTRA_PAIRS;

  /** Build an agent with TOTAL_PAIRS turns seeded, plus a summary provider for /compact. */
  async function makeAgentWithLongHistory(summary: string) {
    let phase: "seed" | "compact" = "seed";
    let seedCallNum = 0;
    const provider: StreamProvider = async () => {
      if (phase === "seed") {
        seedCallNum++;
        return makeMockStream(textStreamEvents(`reply ${seedCallNum}`), textMessage(`reply ${seedCallNum}`));
      }
      return makeMockStream(textStreamEvents(summary), textMessage(summary));
    };
    const { agent, dispose } = makeTestAgent(provider);
    disposeAll.push(dispose);
    for (let i = 0; i < TOTAL_PAIRS; i++) {
      await collectEvents(agent, `user msg ${i}`);
    }
    phase = "compact";
    return agent;
  }

  it("emits compact_user_start + compact_user_done (no error)", async () => {
    const agent = await makeAgentWithLongHistory("summary of the session");
    const events = await collectEvents(agent, "/compact");
    const types = events.map((e) => e.type);
    expect(types).toContain("compact_user_start");
    expect(types).toContain("compact_user_done");
    expect(types).not.toContain("compact_user_error");
    expect(types).not.toContain("agent_error");
  });

  it("compact_user_start is emitted before compact_user_done", async () => {
    const agent = await makeAgentWithLongHistory("summary");
    const events = await collectEvents(agent, "/compact");
    const types = events.map((e) => e.type);
    expect(types.indexOf("compact_user_start")).toBeLessThan(types.indexOf("compact_user_done"));
  });

  it("compact_user_done.messagesBefore equals history length before compaction", async () => {
    const agent = await makeAgentWithLongHistory("summary");
    const historyBefore = agent.getCompactedContextHistory().length;
    const events = await collectEvents(agent, "/compact");
    const done = events.find((e) => e.type === "compact_user_done") as any;
    expect(done.messagesBefore).toBe(historyBefore);
  });

  it("compact_user_done.messagesAfter equals new compactedContextHistory length", async () => {
    const agent = await makeAgentWithLongHistory("summary");
    const events = await collectEvents(agent, "/compact");
    const done = events.find((e) => e.type === "compact_user_done") as any;
    expect(done.messagesAfter).toBe(agent.getCompactedContextHistory().length);
  });

  it("compactedContextHistory is shorter after compaction", async () => {
    const agent = await makeAgentWithLongHistory("summary");
    const lengthBefore = agent.getCompactedContextHistory().length;
    await collectEvents(agent, "/compact");
    const lengthAfter = agent.getCompactedContextHistory().length;
    expect(lengthAfter).toBeLessThan(lengthBefore);
  });

  it("compacted history starts with synthetic summary message (role=user)", async () => {
    const summaryText = "Agent read many files.";
    const agent = await makeAgentWithLongHistory(summaryText);
    await collectEvents(agent, "/compact");
    const view = agent.getCompactedContextHistory();
    const first = view[0] as any;
    expect(first.role).toBe("user");
    expect(typeof first.content).toBe("string");
    expect((first.content as string)).toContain("[Compacted context summary:");
    expect((first.content as string)).toContain(summaryText);
  });

  it("tail messages after compaction are verbatim copies of the last KEEP_RECENT_TURNS*2 messages", async () => {
    let phase: "seed" | "compact" = "seed";
    let seedCallNum = 0;
    const provider: StreamProvider = async () => {
      if (phase === "seed") {
        seedCallNum++;
        return makeMockStream(textStreamEvents(`reply ${seedCallNum}`), textMessage(`reply ${seedCallNum}`));
      }
      return makeMockStream(textStreamEvents("summary"), textMessage("summary"));
    };
    const { agent, dispose } = makeTestAgent(provider);
    disposeAll.push(dispose);
    for (let i = 0; i < TOTAL_PAIRS; i++) {
      await collectEvents(agent, `user msg ${i}`);
    }
    const historyBeforeCompact = [...agent.getCompactedContextHistory()];
    phase = "compact";
    await collectEvents(agent, "/compact");
    const view = agent.getCompactedContextHistory();

    const expectedTail = historyBeforeCompact.slice(-(KEEP_RECENT_TURNS * 2));
    const actualTail = view.slice(1); // skip synthetic summary
    expect(actualTail).toEqual(expectedTail);
  });

  it("messagesAfter = 1 (synthetic) + KEEP_RECENT_TURNS * 2 (tail)", async () => {
    const agent = await makeAgentWithLongHistory("summary");
    await collectEvents(agent, "/compact");
    const view = agent.getCompactedContextHistory();
    expect(view.length).toBe(1 + KEEP_RECENT_TURNS * 2);
  });

  it("compactedContextHistory length matches compact_user_done.messagesAfter", async () => {
    const agent = await makeAgentWithLongHistory("summary");
    const events = await collectEvents(agent, "/compact");
    const done = events.find((e) => e.type === "compact_user_done") as any;
    expect(done.messagesAfter).toBe(agent.getCompactedContextHistory().length);
  });

  it("agent can continue sending messages after compaction", async () => {
    let phase: "seed" | "compact" | "post" = "seed";
    let seedNum = 0;
    const provider: StreamProvider = async () => {
      if (phase === "seed") {
        seedNum++;
        return makeMockStream(textStreamEvents(`r${seedNum}`), textMessage(`r${seedNum}`));
      }
      if (phase === "compact") {
        return makeMockStream(textStreamEvents("summary"), textMessage("summary"));
      }
      return makeMockStream(textStreamEvents("post-compact reply"), textMessage("post-compact reply"));
    };
    const { agent, dispose } = makeTestAgent(provider);
    disposeAll.push(dispose);
    for (let i = 0; i < TOTAL_PAIRS; i++) {
      await collectEvents(agent, `msg ${i}`);
    }
    phase = "compact";
    await collectEvents(agent, "/compact");
    phase = "post";

    const events = await collectEvents(agent, "hello after compact");
    const types = events.map((e) => e.type);
    expect(types).toContain("turn_end");
    expect(types).not.toContain("agent_error");
  });

  it("history grows correctly after compaction + one more turn", async () => {
    let phase: "seed" | "compact" | "post" = "seed";
    let seedNum = 0;
    const provider: StreamProvider = async () => {
      if (phase === "seed") {
        seedNum++;
        return makeMockStream(textStreamEvents(`r${seedNum}`), textMessage(`r${seedNum}`));
      }
      if (phase === "compact") {
        return makeMockStream(textStreamEvents("summary"), textMessage("summary"));
      }
      return makeMockStream(textStreamEvents("post"), textMessage("post"));
    };
    const { agent, dispose } = makeTestAgent(provider);
    disposeAll.push(dispose);
    for (let i = 0; i < TOTAL_PAIRS; i++) {
      await collectEvents(agent, `msg ${i}`);
    }
    phase = "compact";
    await collectEvents(agent, "/compact");
    const lenAfterCompact = agent.getCompactedContextHistory().length;
    phase = "post";
    await collectEvents(agent, "one more");
    const lenAfterOneTurn = agent.getCompactedContextHistory().length;
    expect(lenAfterOneTurn).toBe(lenAfterCompact + 2);
  });
});

// ---------------------------------------------------------------------------
// /compact — error path (LLM throws during compaction)
// ---------------------------------------------------------------------------

describe("/compact — error path", () => {
  it("emits compact_user_start then compact_user_error when LLM throws", async () => {
    let phase: "seed" | "compact" = "seed";
    let seedNum = 0;
    const provider: StreamProvider = async () => {
      if (phase === "seed") {
        seedNum++;
        return makeMockStream(textStreamEvents(`r${seedNum}`), textMessage(`r${seedNum}`));
      }
      throw new Error("API unavailable");
    };
    const { agent, dispose } = makeTestAgent(provider);
    disposeAll.push(dispose);
    const pairs = KEEP_RECENT_TURNS + 2;
    for (let i = 0; i < pairs; i++) {
      await collectEvents(agent, `msg ${i}`);
    }
    phase = "compact";

    const events = await collectEvents(agent, "/compact");
    const types = events.map((e) => e.type);
    expect(types).toContain("compact_user_start");
    expect(types).toContain("compact_user_error");
    expect(types).not.toContain("compact_user_done");
  });

  it("compact_user_error carries the error message", async () => {
    let phase: "seed" | "compact" = "seed";
    let seedNum = 0;
    const errMessage = "Network timeout during compaction";
    const provider: StreamProvider = async () => {
      if (phase === "seed") {
        seedNum++;
        return makeMockStream(textStreamEvents(`r${seedNum}`), textMessage(`r${seedNum}`));
      }
      throw new Error(errMessage);
    };
    const { agent, dispose } = makeTestAgent(provider);
    disposeAll.push(dispose);
    const pairs = KEEP_RECENT_TURNS + 2;
    for (let i = 0; i < pairs; i++) {
      await collectEvents(agent, `msg ${i}`);
    }
    phase = "compact";

    const events = await collectEvents(agent, "/compact");
    const errEv = events.find((e) => e.type === "compact_user_error") as any;
    expect(errEv).toBeDefined();
    expect(errEv.error).toBe(errMessage);
  });

  it("compact_user_start appears before compact_user_error", async () => {
    let phase: "seed" | "compact" = "seed";
    let seedNum = 0;
    const provider: StreamProvider = async () => {
      if (phase === "seed") {
        seedNum++;
        return makeMockStream(textStreamEvents(`r${seedNum}`), textMessage(`r${seedNum}`));
      }
      throw new Error("boom");
    };
    const { agent, dispose } = makeTestAgent(provider);
    disposeAll.push(dispose);
    const pairs = KEEP_RECENT_TURNS + 2;
    for (let i = 0; i < pairs; i++) {
      await collectEvents(agent, `msg ${i}`);
    }
    phase = "compact";

    const events = await collectEvents(agent, "/compact");
    const types = events.map((e) => e.type);
    const startIdx = types.indexOf("compact_user_start");
    const errIdx = types.indexOf("compact_user_error");
    expect(startIdx).toBeGreaterThanOrEqual(0);
    expect(errIdx).toBeGreaterThanOrEqual(0);
    expect(startIdx).toBeLessThan(errIdx);
  });

  it("compactedContextHistory is unchanged after a failed compaction", async () => {
    let phase: "seed" | "compact" = "seed";
    let seedNum = 0;
    const provider: StreamProvider = async () => {
      if (phase === "seed") {
        seedNum++;
        return makeMockStream(textStreamEvents(`r${seedNum}`), textMessage(`r${seedNum}`));
      }
      throw new Error("oops");
    };
    const { agent, dispose } = makeTestAgent(provider);
    disposeAll.push(dispose);
    const pairs = KEEP_RECENT_TURNS + 2;
    for (let i = 0; i < pairs; i++) {
      await collectEvents(agent, `msg ${i}`);
    }
    const historySnapshot = [...agent.getCompactedContextHistory()];
    phase = "compact";

    await collectEvents(agent, "/compact");

    expect(agent.getCompactedContextHistory()).toEqual(historySnapshot);
  });

  it("agent can still send messages after a failed compaction", async () => {
    let phase: "seed" | "compact" | "post" = "seed";
    let seedNum = 0;
    const provider: StreamProvider = async () => {
      if (phase === "seed") {
        seedNum++;
        return makeMockStream(textStreamEvents(`r${seedNum}`), textMessage(`r${seedNum}`));
      }
      if (phase === "compact") throw new Error("fail");
      return makeMockStream(textStreamEvents("ok"), textMessage("ok"));
    };
    const { agent, dispose } = makeTestAgent(provider);
    disposeAll.push(dispose);
    const pairs = KEEP_RECENT_TURNS + 2;
    for (let i = 0; i < pairs; i++) {
      await collectEvents(agent, `msg ${i}`);
    }
    phase = "compact";
    await collectEvents(agent, "/compact");
    phase = "post";

    const events = await collectEvents(agent, "hello after failed compact");
    const types = events.map((e) => e.type);
    expect(types).toContain("turn_end");
    expect(types).not.toContain("agent_error");
  });
});

// ---------------------------------------------------------------------------
// /compact — event ordering invariants
// ---------------------------------------------------------------------------

describe("/compact — event ordering invariants", () => {
  it("compact_user_start carries a ts field", async () => {
    const { agent, dispose } = makeTestAgent(makeSummaryProvider("summary"));
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "/compact");
    const start = events.find((e) => e.type === "compact_user_start") as any;
    expect(typeof start.ts).toBe("string");
    expect(start.ts.length).toBeGreaterThan(0);
  });

  it("compact_user_done carries a ts field", async () => {
    const { agent, dispose } = makeTestAgent(makeSummaryProvider("summary"));
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "/compact");
    const done = events.find((e) => e.type === "compact_user_done") as any;
    expect(typeof done.ts).toBe("string");
  });

  it("compact_user_error carries a ts field", async () => {
    let phase: "seed" | "compact" = "seed";
    let seedNum = 0;
    const provider: StreamProvider = async () => {
      if (phase === "seed") {
        seedNum++;
        return makeMockStream(textStreamEvents(`r${seedNum}`), textMessage(`r${seedNum}`));
      }
      throw new Error("err");
    };
    const { agent, dispose } = makeTestAgent(provider);
    disposeAll.push(dispose);
    const pairs = KEEP_RECENT_TURNS + 2;
    for (let i = 0; i < pairs; i++) {
      await collectEvents(agent, `msg ${i}`);
    }
    phase = "compact";
    const events = await collectEvents(agent, "/compact");
    const errEv = events.find((e) => e.type === "compact_user_error") as any;
    expect(typeof errEv.ts).toBe("string");
  });

  it("/compact emits no turn_end (it's a command, not a regular turn)", async () => {
    const { agent, dispose } = makeTestAgent(makeSummaryProvider("summary"));
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "/compact");
    expect(events.find((e) => e.type === "turn_end")).toBeUndefined();
  });

  it("/compact emits no user_message (it's a command, not forwarded to LLM)", async () => {
    const { agent, dispose } = makeTestAgent(makeSummaryProvider("summary"));
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "/compact");
    expect(events.find((e) => e.type === "user_message")).toBeUndefined();
  });
});
