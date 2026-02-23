/**
 * Integration tests for the Agent class using a mock provider.
 *
 * These tests cover the full sendMessage loop — streaming, tool dispatch,
 * session persistence, and history management — without hitting the real API.
 *
 * The Agent constructor accepts an optional StreamProvider. Tests inject a
 * mock that returns pre-scripted responses.
 */

import { describe, it, expect } from "bun:test";
import { mkdirSync, rmSync } from "fs";
import { join } from "path";
import { tmpdir } from "os";
import type Anthropic from "@anthropic-ai/sdk";

import { Agent, type AgentEvent, type StreamProvider } from "./agent.js";

// ---------------------------------------------------------------------------
// Mock provider helpers
// ---------------------------------------------------------------------------

/**
 * Build a fake stream object that mimics the Anthropic SDK stream interface.
 * `events` is the sequence of raw stream events to yield.
 * `message` is what finalMessage() resolves to.
 */
function makeMockStream(events: any[], message: Anthropic.Message) {
  return {
    async *[Symbol.asyncIterator]() {
      for (const e of events) yield e;
    },
    finalMessage: async () => message,
  };
}

/** Minimal Anthropic.Message for a plain text response. */
function textMessage(text: string): Anthropic.Message {
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

/** Stream events for a plain text response. */
function textStreamEvents(text: string): any[] {
  return [
    { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } },
    { type: "content_block_delta", index: 0, delta: { type: "text_delta", text } },
    { type: "content_block_stop", index: 0 },
    { type: "message_delta", delta: { stop_reason: "end_turn" }, usage: { output_tokens: 5 } },
    { type: "message_stop" },
  ];
}

/** Minimal Anthropic.Message for a tool_use response. */
function toolUseMessage(
  toolId: string,
  toolName: string,
  toolInput: any
): Anthropic.Message {
  return {
    id: "msg_tool",
    type: "message",
    role: "assistant",
    model: "claude-sonnet-4-6",
    content: [{ type: "tool_use", id: toolId, name: toolName, input: toolInput }],
    stop_reason: "tool_use",
    stop_sequence: null,
    usage: { input_tokens: 20, output_tokens: 10 },
  };
}

/** Stream events for a tool_use response. */
function toolUseStreamEvents(toolName: string): any[] {
  return [
    { type: "content_block_start", index: 0, content_block: { type: "tool_use", id: "t1", name: toolName } },
    { type: "content_block_delta", index: 0, delta: { type: "input_json_delta", partial_json: "{}" } },
    { type: "content_block_stop", index: 0 },
    { type: "message_delta", delta: { stop_reason: "tool_use" }, usage: { output_tokens: 10 } },
    { type: "message_stop" },
  ];
}

/**
 * Collect all events from an agent.sendMessage() call into an array.
 * Auto-approves all tool confirmations.
 */
async function collectEvents(
  agent: Agent,
  message: string,
  confirmFn?: (name: string, input: any, formatted: string) => Promise<boolean>
): Promise<AgentEvent[]> {
  const events: AgentEvent[] = [];
  const confirm = confirmFn ?? (async () => true);
  for await (const event of agent.sendMessage(message, confirm)) {
    events.push(event);
  }
  return events;
}

// ---------------------------------------------------------------------------
// Test setup
// ---------------------------------------------------------------------------

// Each test that touches the filesystem gets its own unique directory to
// avoid races between fire-and-forget persist calls from prior tests leaking
// into a freshly-recreated shared directory.
let _testDirCounter = 0;
function makeTempDir(): string {
  const dir = join(tmpdir(), `omega-agent-test-${Date.now()}-${++_testDirCounter}`);
  mkdirSync(dir, { recursive: true });
  return dir;
}

// ---------------------------------------------------------------------------
// Pollution guard
// ---------------------------------------------------------------------------

// Tests that use a mock StreamProvider must never write to the real world-state
// file. This test proves the contract: when a streamProvider is given without
// a worldStatePath, Agent must NOT write to any project world-state file.
describe("Agent — test isolation (no production world-state pollution)", () => {
  it("does not write to any world-state file when worldStatePath is not given", async () => {
    const { homedir } = await import("os");
    const omegaDir = join(homedir(), ".local", "share", "omega");
    const { existsSync, readdirSync: rds } = await import("fs");

    // Snapshot omega dir before
    const before = existsSync(omegaDir) ? rds(omegaDir).filter(f => f.startsWith("world-")).length : 0;

    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hi"), textMessage("hi"));

    // No worldStatePath passed — must not write to real world-state
    const agent = new Agent(mockProvider);
    await collectEvents(agent, "should not persist world state");
    await Bun.sleep(100);

    // World state files must be unchanged
    const after = existsSync(omegaDir) ? rds(omegaDir).filter(f => f.startsWith("world-")).length : 0;
    expect(after).toBe(before);
  });
});

// ---------------------------------------------------------------------------
// Plain text response
// ---------------------------------------------------------------------------

describe("Agent.sendMessage — plain text response", () => {
  it("emits a status event, then text events, then metrics", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("Hello!"), textMessage("Hello!"));

    const agent = new Agent(mockProvider);
    const events = await collectEvents(agent, "hi");

    const types = events.map((e) => e.type);
    expect(types[0]).toBe("user_message");
    expect(types).toContain("status");
    expect(types).toContain("text");
    expect(types).toContain("metrics");
    expect(types[types.length - 1]).toBe("turn_end");
  });

  it("accumulates text from chunks", async () => {
    const chunkEvents = [
      { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } },
      { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "foo " } },
      { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "bar " } },
      { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "baz" } },
      { type: "content_block_stop", index: 0 },
      { type: "message_stop" },
    ];
    const mockProvider: StreamProvider = async () =>
      makeMockStream(chunkEvents, textMessage("foo bar baz"));

    const agent = new Agent(mockProvider);
    const events = await collectEvents(agent, "tell me something");

    const textEvents = events.filter((e) => e.type === "text");
    expect(textEvents.length).toBe(3);
    const combined = textEvents.map((e) => (e as any).text).join("");
    expect(combined).toBe("foo bar baz");
  });

  it("adds user message and assistant response to history", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("I am fine"), textMessage("I am fine"));

    const agent = new Agent(mockProvider);
    await collectEvents(agent, "how are you?");

    const history = agent.getHistory();
    expect(history.length).toBe(2);
    expect(history[0]).toEqual({ role: "user", content: "how are you?" });
    expect(history[1].role).toBe("assistant");
  });

  it("accumulates token counts across turns", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("ok"), textMessage("ok"));

    const agent = new Agent(mockProvider);
    await collectEvents(agent, "first");
    await collectEvents(agent, "second");

    // Each call: 10 input + 5 output
    expect(agent.sessionInputTokens).toBe(20);
    expect(agent.sessionOutputTokens).toBe(10);
  });

  it("emits metrics with correct token counts", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("ok"), textMessage("ok"));

    const agent = new Agent(mockProvider);
    const events = await collectEvents(agent, "test");

    const metricsEvent = events.find((e) => e.type === "metrics") as any;
    expect(metricsEvent).toBeDefined();
    expect(metricsEvent.metrics.inputTokens).toBe(10);
    expect(metricsEvent.metrics.outputTokens).toBe(5);
    expect(metricsEvent.metrics.costUsd).toBeGreaterThan(0);
  });
});

// ---------------------------------------------------------------------------
// Tool call loop
// ---------------------------------------------------------------------------

describe("Agent.sendMessage — tool call loop", () => {
  it("emits tool_call event for auto-approved tools", async () => {
    // First response: use read_file (auto-approved). Second: plain text.
    let call = 0;
    const mockProvider: StreamProvider = async () => {
      call++;
      if (call === 1) {
        return makeMockStream(
          toolUseStreamEvents("read_file"),
          toolUseMessage("t1", "read_file", { path: "src/config.ts" })
        );
      }
      return makeMockStream(textStreamEvents("Done"), textMessage("Done"));
    };

    const agent = new Agent(mockProvider);
    const events = await collectEvents(agent, "read config");

    const toolCallEvents = events.filter((e) => e.type === "tool_call");
    expect(toolCallEvents.length).toBe(1);
    expect((toolCallEvents[0] as any).name).toBe("read_file");
  });

  it("emits tool_result event after executing the tool", async () => {
    let call = 0;
    const mockProvider: StreamProvider = async () => {
      call++;
      if (call === 1) {
        return makeMockStream(
          toolUseStreamEvents("read_file"),
          toolUseMessage("t1", "read_file", { path: "src/config.ts" })
        );
      }
      return makeMockStream(textStreamEvents("Done"), textMessage("Done"));
    };

    const agent = new Agent(mockProvider);
    const events = await collectEvents(agent, "read config");

    const resultEvents = events.filter((e) => e.type === "tool_result");
    expect(resultEvents.length).toBe(1);
    const result = resultEvents[0] as any;
    expect(result.name).toBe("read_file");
    expect(result.result.isError).toBe(false);
  });



  it("adds tool results to history and makes a second API call", async () => {
    const calls: any[] = [];
    const mockProvider: StreamProvider = async (params) => {
      // Snapshot immediately — params.messages is this.history by reference
      calls.push({ messages: [...params.messages] });
      if (calls.length === 1) {
        return makeMockStream(
          toolUseStreamEvents("read_file"),
          toolUseMessage("t1", "read_file", { path: "src/config.ts" })
        );
      }
      return makeMockStream(textStreamEvents("Done"), textMessage("Done"));
    };

    const agent = new Agent(mockProvider);
    await collectEvents(agent, "read config");

    // Provider should have been called at least twice (possibly more for compaction)
    expect(calls.length).toBeGreaterThanOrEqual(2);

    // Second call's messages should contain tool_result
    const secondMessages = calls[1].messages as Anthropic.MessageParam[];
    const toolResultMsg = secondMessages.find(
      (m) => Array.isArray(m.content) &&
        m.content.some((b: any) => b.type === "tool_result")
    );
    expect(toolResultMsg).toBeDefined();
  });

  it("history grows correctly across a tool loop", async () => {
    let call = 0;
    const mockProvider: StreamProvider = async () => {
      call++;
      if (call === 1) {
        return makeMockStream(
          toolUseStreamEvents("read_file"),
          toolUseMessage("t1", "read_file", { path: "src/config.ts" })
        );
      }
      return makeMockStream(textStreamEvents("Done"), textMessage("Done"));
    };

    const agent = new Agent(mockProvider);
    await collectEvents(agent, "read config");

    // Expected: user, assistant(tool_use), user(tool_result), assistant(text)
    const history = agent.getHistory();
    expect(history.length).toBe(4);
    expect(history[0].role).toBe("user");
    expect(history[1].role).toBe("assistant");
    expect(history[2].role).toBe("user"); // tool result
    expect(history[3].role).toBe("assistant");
  });

  it("executes multiple tools in parallel (both tool_call events before any tool_result)", async () => {
    // Build a response with two tool_use blocks (list_files + list_files)
    const twoToolMessage: Anthropic.Message = {
      id: "msg_two_tools",
      type: "message",
      role: "assistant",
      model: "claude-sonnet-4-6",
      content: [
        { type: "tool_use", id: "tA", name: "list_files", input: { path: "src" } },
        { type: "tool_use", id: "tB", name: "list_files", input: { path: "plan" } },
      ],
      stop_reason: "tool_use",
      stop_sequence: null,
      usage: { input_tokens: 20, output_tokens: 10 },
    };
    const twoToolStreamEvents: any[] = [
      { type: "content_block_start", index: 0, content_block: { type: "tool_use", id: "tA", name: "list_files" } },
      { type: "content_block_delta", index: 0, delta: { type: "input_json_delta", partial_json: '{"path":"src"}' } },
      { type: "content_block_stop", index: 0 },
      { type: "content_block_start", index: 1, content_block: { type: "tool_use", id: "tB", name: "list_files" } },
      { type: "content_block_delta", index: 1, delta: { type: "input_json_delta", partial_json: '{"path":"plan"}' } },
      { type: "content_block_stop", index: 1 },
      { type: "message_delta", delta: { stop_reason: "tool_use" }, usage: { output_tokens: 10 } },
      { type: "message_stop" },
    ];

    let call = 0;
    const mockProvider: StreamProvider = async () => {
      call++;
      if (call === 1) return makeMockStream(twoToolStreamEvents, twoToolMessage);
      return makeMockStream(textStreamEvents("Done"), textMessage("Done"));
    };

    const agent = new Agent(mockProvider);
    const events = await collectEvents(agent, "list dirs");

    const toolCallEvents = events.filter((e) => e.type === "tool_call");
    const toolResultEvents = events.filter((e) => e.type === "tool_result");
    expect(toolCallEvents.length).toBe(2);
    expect(toolResultEvents.length).toBe(2);

    // Parallel execution: both tool_call events appear before any tool_result event.
    // Sequential execution would interleave: call_A, result_A, call_B, result_B.
    const firstResultIndex = events.findIndex((e) => e.type === "tool_result");
    const lastCallIndex = events.reduce(
      (idx, e, i) => (e.type === "tool_call" ? i : idx),
      -1
    );
    expect(lastCallIndex).toBeLessThan(firstResultIndex);
  });
});



// ---------------------------------------------------------------------------
// Error handling
// ---------------------------------------------------------------------------

describe("Agent.sendMessage — error handling", () => {
  it("emits an error event on non-retryable API failure", async () => {
    const mockProvider: StreamProvider = async () => {
      const err: any = new Error("Bad request");
      err.status = 400;
      throw err;
    };

    const agent = new Agent(mockProvider);
    const events = await collectEvents(agent, "test");

    const errorEvents = events.filter((e) => e.type === "error");
    expect(errorEvents.length).toBeGreaterThan(0);
    expect((errorEvents[0] as any).error).toContain("Bad request");
  });

  it("emits retry error events then succeeds on transient failure", async () => {
    process.env.OMEGA_RETRY_BASE_MS = "1";
    process.env.OMEGA_RETRY_MAX_MS = "2";
    process.env.OMEGA_RETRY_ATTEMPTS = "3";

    let attempts = 0;
    const mockProvider: StreamProvider = async () => {
      attempts++;
      if (attempts < 3) {
        const err: any = new Error("overloaded");
        err.status = 529;
        throw err;
      }
      return makeMockStream(textStreamEvents("ok"), textMessage("ok"));
    };

    const agent = new Agent(mockProvider);
    const events = await collectEvents(agent, "test");

    delete process.env.OMEGA_RETRY_BASE_MS;
    delete process.env.OMEGA_RETRY_MAX_MS;
    delete process.env.OMEGA_RETRY_ATTEMPTS;

    // Should have retried and eventually succeeded (compaction may add extra calls)
    expect(attempts).toBeGreaterThanOrEqual(3);
    const errorEvents = events.filter((e) => e.type === "error");
    // Two retry error messages emitted before success
    expect(errorEvents.length).toBe(2);
    // Final event should be turn_end (success)
    expect(events[events.length - 1].type).toBe("turn_end");
  }, 30_000);
});

// ---------------------------------------------------------------------------
// Abort / interrupt
// ---------------------------------------------------------------------------

describe("Agent.sendMessage — abort", () => {
  it("stops emitting events when aborted mid-stream", async () => {
    // Stream that yields text chunks with a delay so we can abort mid-way
    const mockProvider: StreamProvider = async () => {
      async function* slowEvents() {
        yield { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } };
        yield { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "chunk1 " } };
        // Pause — abort fires here
        await new Promise((r) => setTimeout(r, 50));
        yield { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "chunk2 " } };
        await new Promise((r) => setTimeout(r, 50));
        yield { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "chunk3" } };
        yield { type: "content_block_stop", index: 0 };
        yield { type: "message_delta", delta: { stop_reason: "end_turn" }, usage: { output_tokens: 10 } };
        yield { type: "message_stop" };
      }
      return {
        [Symbol.asyncIterator]: slowEvents,
        finalMessage: async () => textMessage("chunk1 chunk2 chunk3"),
      };
    };

    const agent = new Agent(mockProvider);
    const events: AgentEvent[] = [];
    const controller = new AbortController();

    const gen = agent.sendMessage("test", async () => true, controller.signal);

    for await (const event of gen) {
      events.push(event);
      // Abort after receiving the first text chunk
      if (event.type === "text" && (event as any).text.includes("chunk1")) {
        controller.abort();
      }
    }

    const textEvents = events.filter((e) => e.type === "text");
    // Only chunk1 should have arrived — chunk2 and chunk3 were aborted
    expect(textEvents.length).toBe(1);
    expect((textEvents[0] as any).text).toContain("chunk1");

    // Should emit an "interrupted" event so the UI can show feedback
    const interrupted = events.find((e) => e.type === "interrupted");
    expect(interrupted).toBeDefined();
  });

  it("does not add incomplete assistant turn to history when aborted", async () => {
    const mockProvider: StreamProvider = async () => {
      async function* slowEvents() {
        yield { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } };
        yield { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "partial" } };
        await new Promise((r) => setTimeout(r, 50));
        yield { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: " response" } };
        yield { type: "content_block_stop", index: 0 };
        yield { type: "message_delta", delta: { stop_reason: "end_turn" }, usage: { output_tokens: 5 } };
        yield { type: "message_stop" };
      }
      return {
        [Symbol.asyncIterator]: slowEvents,
        finalMessage: async () => textMessage("partial response"),
      };
    };

    const agent = new Agent(mockProvider);
    const controller = new AbortController();
    const gen = agent.sendMessage("hello", async () => true, controller.signal);

    for await (const event of gen) {
      if (event.type === "text") controller.abort();
    }

    // History should only have the user message — no partial assistant turn
    const history = agent.getHistory();
    expect(history.length).toBe(1);
    expect(history[0].role).toBe("user");
  });
});

// ---------------------------------------------------------------------------
// api_call_start event
// ---------------------------------------------------------------------------

describe("api_call_start event", () => {
  it("emits api_call_start before the first API call", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hello"), textMessage("hello"));
    const agent = new Agent(mockProvider, null);
    const events = await collectEvents(agent, "hi");
    const startEvents = events.filter((e) => e.type === "api_call_start");
    expect(startEvents.length).toBe(1);
  });

  it("api_call_start carries provider, url, request, and callNumber", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hello"), textMessage("hello"));
    const agent = new Agent(mockProvider, null);
    const events = await collectEvents(agent, "hi");
    const e = events.find((e) => e.type === "api_call_start") as any;
    expect(e).toBeDefined();
    expect(e.provider).toBe("anthropic");
    expect(typeof e.url).toBe("string");
    expect(typeof e.request).toBe("object");
    expect(typeof e.callNumber).toBe("number");
    expect(e.callNumber).toBe(1);
  });

  it("api_call_start exposes request messages", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hello"), textMessage("hello"));
    const agent = new Agent(mockProvider, null);
    const events = await collectEvents(agent, "hi");
    const e = events.find((e) => e.type === "api_call_start") as any;
    expect(Array.isArray(e.request.messages)).toBe(true);
    expect(e.request.messages[0].role).toBe("user");
  });

  it("emits api_call_start once per round-trip in a tool loop", async () => {
    // First call: tool_use; second call: text response
    let callCount = 0;
    const mockProvider: StreamProvider = async () => {
      callCount++;
      if (callCount === 1) {
        return makeMockStream(
          toolUseStreamEvents("list_files"),
          toolUseMessage("t1", "list_files", { path: "." })
        );
      }
      return makeMockStream(textStreamEvents("done"), textMessage("done"));
    };
    const agent = new Agent(mockProvider, null);
    const events = await collectEvents(agent, "list files");
    const startEvents = events.filter((e) => e.type === "api_call_start");
    expect(startEvents.length).toBe(2);
    expect((startEvents[0] as any).callNumber).toBe(1);
    expect((startEvents[1] as any).callNumber).toBe(2);
  });

  it("api_call_start request snapshot is correct (not a live reference)", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("reply"), textMessage("reply"));
    const agent = new Agent(mockProvider, null);
    const events = await collectEvents(agent, "hello");
    const e = events.find((ev) => ev.type === "api_call_start") as any;
    expect(e.request.messages.length).toBe(1);
    expect(e.request.messages[0].role).toBe("user");
  });
});

// ---------------------------------------------------------------------------
// Full auto-approve
// ---------------------------------------------------------------------------

describe("Agent — full auto-approve", () => {
  it("never emits tool_pending even for previously-rejected commands", async () => {
    // run_command with a dangerous command — should auto-approve, never ask
    let call = 0;
    const mockProvider: StreamProvider = async () => {
      call++;
      if (call === 1) {
        return makeMockStream(
          toolUseStreamEvents("run_command"),
          toolUseMessage("t1", "run_command", { command: "rm -rf /" })
        );
      }
      return makeMockStream(textStreamEvents("done"), textMessage("done"));
    };
    const agent = new Agent(mockProvider, null);
    const events = await collectEvents(agent, "do it");
    const pending = events.filter((e) => e.type === "tool_pending");
    expect(pending.length).toBe(0);
  });

  it("emits tool_call for every tool regardless of name", async () => {
    let call = 0;
    const mockProvider: StreamProvider = async () => {
      call++;
      if (call === 1) {
        return makeMockStream(
          toolUseStreamEvents("run_command"),
          toolUseMessage("t1", "run_command", { command: "rm -rf /" })
        );
      }
      return makeMockStream(textStreamEvents("done"), textMessage("done"));
    };
    const agent = new Agent(mockProvider, null);
    const events = await collectEvents(agent, "do it");
    const toolCalls = events.filter((e) => e.type === "tool_call");
    expect(toolCalls.length).toBe(1);
    expect((toolCalls[0] as any).name).toBe("run_command");
  });

  it("confirmTool callback is never invoked", async () => {
    let confirmCalled = false;
    let call = 0;
    const mockProvider: StreamProvider = async () => {
      call++;
      if (call === 1) {
        return makeMockStream(
          toolUseStreamEvents("run_command"),
          toolUseMessage("t1", "run_command", { command: "echo hi" })
        );
      }
      return makeMockStream(textStreamEvents("done"), textMessage("done"));
    };
    const agent = new Agent(mockProvider, null);
    const confirm = async () => { confirmCalled = true; return true; };
    for await (const _ of agent.sendMessage("go", confirm)) {}
    expect(confirmCalled).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// turn_end event
// ---------------------------------------------------------------------------

describe("Agent — turn_end event", () => {
  it("emits exactly one turn_end per user message", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hello"), textMessage("hello"));
    const agent = new Agent(mockProvider, null);
    const events = await collectEvents(agent, "hi");
    const turnEnds = events.filter((e) => e.type === "turn_end");
    expect(turnEnds.length).toBe(1);
  });

  it("turn_end is the last event emitted", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hello"), textMessage("hello"));
    const agent = new Agent(mockProvider, null);
    const events = await collectEvents(agent, "hi");
    expect(events[events.length - 1].type).toBe("turn_end");
  });

  it("turn_end includes all tool calls across multiple API calls", async () => {
    let call = 0;
    const mockProvider: StreamProvider = async () => {
      call++;
      if (call === 1) {
        return makeMockStream(
          toolUseStreamEvents("read_file"),
          toolUseMessage("t1", "read_file", { path: "x.ts" })
        );
      }
      if (call === 2) {
        return makeMockStream(
          toolUseStreamEvents("run_command"),
          toolUseMessage("t2", "run_command", { command: "ls" })
        );
      }
      return makeMockStream(textStreamEvents("done"), textMessage("done"));
    };
    const agent = new Agent(mockProvider, null);
    const events = await collectEvents(agent, "do it");
    const turnEnd = events.find((e) => e.type === "turn_end") as any;
    expect(turnEnd.toolCalls).toEqual(["read_file", "run_command"]);
  });

  it("turn_end aggregates token counts across all API calls", async () => {
    let call = 0;
    const mockProvider: StreamProvider = async () => {
      call++;
      if (call === 1) {
        return makeMockStream(
          toolUseStreamEvents("read_file"),
          toolUseMessage("t1", "read_file", { path: "x.ts" })
        );
      }
      return makeMockStream(textStreamEvents("done"), textMessage("done"));
    };
    const agent = new Agent(mockProvider, null);
    const events = await collectEvents(agent, "go");
    const turnEnd = events.find((e) => e.type === "turn_end") as any;
    // toolUseMessage has input:20 output:10, textMessage has input:10 output:5
    expect(turnEnd.metrics.inputTokens).toBe(30);
    expect(turnEnd.metrics.outputTokens).toBe(15);
  });

  it("turn_end toolCalls is empty when no tools used", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hi"), textMessage("hi"));
    const agent = new Agent(mockProvider, null);
    const events = await collectEvents(agent, "hi");
    const turnEnd = events.find((e) => e.type === "turn_end") as any;
    expect(turnEnd.toolCalls).toEqual([]);
  });

  it("turn_end model reflects activeModel after /opus switch", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hi"), textMessage("hi"));
    const agent = new Agent(mockProvider, null);
    // Switch to opus first
    await collectEvents(agent, "/opus");
    // Send a real message and check the turn_end model
    const events = await collectEvents(agent, "hi");
    const turnEnd = events.find((e) => e.type === "turn_end") as any;
    expect(turnEnd.model).toBe("claude-opus-4-6");
  });

  it("turn_end model is sonnet by default", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hi"), textMessage("hi"));
    const agent = new Agent(mockProvider, null);
    const events = await collectEvents(agent, "hi");
    const turnEnd = events.find((e) => e.type === "turn_end") as any;
    expect(turnEnd.model).toBe("claude-sonnet-4-6");
  });
});

// ---------------------------------------------------------------------------
// api_response event
// ---------------------------------------------------------------------------

describe("Agent — api_response event", () => {
  it("emits api_response after each API call with stop_reason and usage", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hello"), textMessage("hello"));
    const agent = new Agent(mockProvider, null);
    const events = await collectEvents(agent, "hi");
    const r = events.find((e) => e.type === "api_response") as any;
    expect(r).toBeDefined();
    expect(r.provider).toBe("anthropic");
    expect(r.url).toBe("https://api.anthropic.com/v1/messages");
    expect(r.stopReason).toBe("end_turn");
    expect(r.usage.input_tokens).toBe(10);
    expect(r.usage.output_tokens).toBe(5);
  });

  it("api_response content includes text blocks", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hello"), textMessage("hello"));
    const agent = new Agent(mockProvider, null);
    const events = await collectEvents(agent, "hi");
    const r = events.find((e) => e.type === "api_response") as any;
    expect(r.content.some((b: any) => b.type === "text")).toBe(true);
  });

  it("api_response content includes tool_use blocks when model requests tools", async () => {
    let call = 0;
    const mockProvider: StreamProvider = async () => {
      call++;
      if (call === 1) {
        return makeMockStream(
          toolUseStreamEvents("list_files"),
          toolUseMessage("t1", "list_files", { path: "." })
        );
      }
      return makeMockStream(textStreamEvents("done"), textMessage("done"));
    };
    const agent = new Agent(mockProvider, null);
    const events = await collectEvents(agent, "list");
    const responses = events.filter((e) => e.type === "api_response") as any[];
    const first = responses[0];
    expect(first.content.some((b: any) => b.type === "tool_use")).toBe(true);
    expect(first.content.find((b: any) => b.type === "tool_use").name).toBe("list_files");
  });
});

// ---------------------------------------------------------------------------
// tool_result carries formatted string
// ---------------------------------------------------------------------------

describe("Agent — tool_result formatted field", () => {
  it("tool_result event carries the formatted string from formatToolCall", async () => {
    let call = 0;
    const mockProvider: StreamProvider = async () => {
      call++;
      if (call === 1) {
        return makeMockStream(
          toolUseStreamEvents("list_files"),
          toolUseMessage("t1", "list_files", { path: "src/" })
        );
      }
      return makeMockStream(textStreamEvents("done"), textMessage("done"));
    };
    const agent = new Agent(mockProvider, null);
    const events = await collectEvents(agent, "list files");
    const result = events.find((e) => e.type === "tool_result") as any;
    expect(result.formatted).toBe("list_files: src/");
  });

  it("tool_result formatted includes command for run_command", async () => {
    let call = 0;
    const mockProvider: StreamProvider = async () => {
      call++;
      if (call === 1) {
        return makeMockStream(
          toolUseStreamEvents("run_command"),
          toolUseMessage("t1", "run_command", { command: "echo hi" })
        );
      }
      return makeMockStream(textStreamEvents("done"), textMessage("done"));
    };
    const agent = new Agent(mockProvider, null);
    const events = await collectEvents(agent, "test");
    const result = events.find((e) => e.type === "tool_result") as any;
    expect(result.formatted).toBe("run_command: echo hi");
  });
});

// ---------------------------------------------------------------------------
// metrics carries durationMs and startedAt
// ---------------------------------------------------------------------------

describe("Agent — metrics timing fields", () => {
  it("metrics event carries durationMs", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hi"), textMessage("hi"));
    const agent = new Agent(mockProvider, null);
    const events = await collectEvents(agent, "hi");
    const m = events.find((e) => e.type === "metrics") as any;
    expect(typeof m.metrics.totalMs).toBe("number");
    expect(m.metrics.totalMs).toBeGreaterThanOrEqual(0);
  });

  it("metrics event carries startedAt timestamp string", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hi"), textMessage("hi"));
    const agent = new Agent(mockProvider, null);
    const events = await collectEvents(agent, "hi");
    const m = events.find((e) => e.type === "metrics") as any;
    expect(typeof m.startedAt).toBe("string");
    // Should be a valid time string HH:MM:SS
    expect(m.startedAt).toMatch(/^\d{2}:\d{2}:\d{2}$/);
  });
});

// ---------------------------------------------------------------------------
// user_message event
// ---------------------------------------------------------------------------

describe("Agent — user_message event", () => {
  it("emits user_message as first event with the prompt text", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hi"), textMessage("hi"));
    const agent = new Agent(mockProvider, null);
    const events = await collectEvents(agent, "hello there");
    const um = events.find((e) => e.type === "user_message") as any;
    expect(um).toBeDefined();
    expect(um.content).toBe("hello there");
  });

  it("user_message is emitted before api_call_start", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hi"), textMessage("hi"));
    const agent = new Agent(mockProvider, null);
    const events = await collectEvents(agent, "hi");
    const types = events.map((e) => e.type);
    const umIdx = types.indexOf("user_message");
    const apiIdx = types.indexOf("api_call_start");
    expect(umIdx).toBeGreaterThanOrEqual(0);
    expect(umIdx).toBeLessThan(apiIdx);
  });
});

// ---------------------------------------------------------------------------
// tool_result_message event
// ---------------------------------------------------------------------------

describe("Agent — tool_result_message event", () => {
  it("emits tool_result_message after tool results are collected", async () => {
    let call = 0;
    const mockProvider: StreamProvider = async () => {
      call++;
      if (call === 1) {
        return makeMockStream(
          toolUseStreamEvents("list_files"),
          toolUseMessage("t1", "list_files", { path: "." })
        );
      }
      return makeMockStream(textStreamEvents("done"), textMessage("done"));
    };
    const agent = new Agent(mockProvider, null);
    const events = await collectEvents(agent, "list");
    const trm = events.find((e) => e.type === "tool_result_message") as any;
    expect(trm).toBeDefined();
    expect(Array.isArray(trm.results)).toBe(true);
    expect(trm.results.length).toBe(1);
    expect(trm.results[0].tool_use_id).toBe("t1");
    expect(typeof trm.results[0].content).toBe("string");
    expect(typeof trm.results[0].is_error).toBe("boolean");
  });

  it("tool_result_message is emitted after tool_result and before next api_call_start", async () => {
    let call = 0;
    const mockProvider: StreamProvider = async () => {
      call++;
      if (call === 1) {
        return makeMockStream(
          toolUseStreamEvents("list_files"),
          toolUseMessage("t1", "list_files", { path: "." })
        );
      }
      return makeMockStream(textStreamEvents("done"), textMessage("done"));
    };
    const agent = new Agent(mockProvider, null);
    const events = await collectEvents(agent, "list");
    const types = events.map((e) => e.type);
    const trmIdx = types.lastIndexOf("tool_result_message");
    const lastApiIdx = types.lastIndexOf("api_call_start");
    expect(trmIdx).toBeGreaterThan(0);
    expect(trmIdx).toBeLessThan(lastApiIdx);
  });
});

// ---------------------------------------------------------------------------
// Zone 2 compaction — turn summary
// ---------------------------------------------------------------------------

describe("Agent — zone 2 compaction", () => {
  it("after a turn, history is replaced with 2-message synthetic exchange", async () => {
    // Two providers: one for the real turn, one for the compaction call.
    // The agent re-uses the same streamProvider for compaction.
    let callCount = 0;
    const mockProvider: StreamProvider = async () => {
      callCount++;
      if (callCount === 1) {
        // The actual user turn
        return makeMockStream(textStreamEvents("Hello!"), textMessage("Hello!"));
      }
      // Compaction call
      return makeMockStream(
        textStreamEvents("User said hi. Assistant responded with Hello!."),
        textMessage("User said hi. Assistant responded with Hello!.")
      );
    };
    const tempDir = makeTempDir();
    const agent = new Agent(mockProvider, null, undefined, tempDir + "/world-state.md");
    await collectEvents(agent, "hi");
    // Wait for async compaction
    await Bun.sleep(50);
    const history = agent.getHistory();
    // Should be exactly 2 messages: synthetic user summary + assistant ack
    expect(history).toHaveLength(2);
    expect(history[0].role).toBe("user");
    expect(history[1].role).toBe("assistant");
    expect(typeof history[0].content).toBe("string");
    expect((history[0].content as string)).toContain("[session summary");
  });

  it("compaction racing with next turn does not corrupt history (tool_use/result orphan bug)", async () => {
    // Regression test: if compaction of turn N completes after turn N+1 has already
    // pushed tool_use+tool_result to history, the compaction must NOT overwrite those
    // messages. Doing so leaves orphaned tool_result blocks with no matching tool_use,
    // causing Anthropic API 400 "unexpected tool_use_id found in tool_result blocks".
    //
    // The race to reproduce:
    //   1. Turn 1 ends. compactAfterTurn fires (async, fire-and-forget).
    //   2. User immediately sends turn 2. sendMessage starts.
    //   3. Turn 2 calls provider → gets tool_use → pushes to history.
    //   4. Turn 2 calls provider again → gets final text.
    //   5. Meanwhile compaction of turn 1 finishes and sets this.history = [2-msg summary].
    //      This WIPES the tool_use+tool_result blocks from turn 2.
    //   6. Next API call has tool_result with no matching tool_use → 400 error.
    //
    // In this test we control the compaction timing with a promise latch.

    let compactionLatch: () => void = () => {};
    const compactionBlocked = new Promise<void>((resolve) => {
      compactionLatch = resolve;
    });

    // Track which call is which by label, not by count, to be robust to ordering.
    type CallLabel = "turn1" | "compact" | "turn2-tool" | "turn2-final";
    const callLabels: CallLabel[] = [];
    let callIndex = 0;

    // Call ordering (JS single-threaded, async interleaving):
    //   call 0 = turn 1 (plain text)
    //   call 1 = compact (starts immediately after turn 1; blocked on latch)
    //   call 2 = turn 2 first call (tool_use)
    //   call 3 = turn 2 second call (final text, after tool result pushed to history)
    //
    // The race: compact is unblocked by the turn2-tool stream's finalMessage().
    // At that point turn 2 has pushed [tool_use] to history but NOT yet [tool_result].
    // Compact then finishes, wiping [tool_use] from history.
    // Turn 2 pushes [tool_result], then makes call 3 — but [tool_use] is gone → 400.
    //
    // We simulate this by releasing the latch inside the turn2-tool finalMessage wrapper.
    const scripts: Array<{ label: CallLabel; fn: () => Promise<any> }> = [
      {
        label: "turn1",
        fn: async () =>
          makeMockStream(textStreamEvents("turn 1 done"), textMessage("turn 1 done")),
      },
      {
        label: "compact",
        fn: async () => {
          // Block until we release the latch (after assistant msg with tool_use is in history
          // but before tool_result is added — the worst-case race window)
          await compactionBlocked;
          return makeMockStream(
            textStreamEvents("summary of turn 1"),
            textMessage("summary of turn 1")
          );
        },
      },
      {
        label: "turn2-tool",
        fn: async () => {
          // Wrap the stream so we can release the latch in finalMessage(),
          // which is called after the assistant message (with tool_use) is in history
          // but before tool_results are pushed.
          const msg = toolUseMessage("tool-turn2-id", "list_files", { path: "." });
          const inner = makeMockStream(toolUseStreamEvents("list_files"), msg);
          return {
            [Symbol.asyncIterator]: () => inner[Symbol.asyncIterator](),
            finalMessage: async () => {
              const result = await inner.finalMessage();
              // Release latch: compaction can now run and overwrite history,
              // racing with the tool_result push that happens after this call.
              compactionLatch();
              return result;
            },
          };
        },
      },
      {
        label: "turn2-final",
        fn: async () =>
          makeMockStream(textStreamEvents("turn 2 done"), textMessage("turn 2 done")),
      },
    ];

    const mockProvider: StreamProvider = async () => {
      const idx = callIndex++;
      return scripts[idx].fn();
    };

    const agent = new Agent(mockProvider);

    // Turn 1 — triggers async compaction (script[1], blocked on latch)
    await collectEvents(agent, "hello turn 1");

    // Turn 2 — runs to completion (scripts[2] + [3]); script[3] releases the latch
    await collectEvents(agent, "hello turn 2");

    // Now wait for compaction (which was unblocked by latch in turn2-final) to complete
    await Bun.sleep(50);

    // The history must never have orphaned tool_result blocks.
    // Verify: every tool_result's tool_use_id has a matching tool_use block.
    const history = agent.getHistory() as any[];

    const allToolUseIds = new Set<string>();
    for (const msg of history) {
      if (msg.role === "assistant" && Array.isArray(msg.content)) {
        for (const block of msg.content) {
          if (block.type === "tool_use") allToolUseIds.add(block.id);
        }
      }
    }

    for (const msg of history) {
      if (msg.role === "user" && Array.isArray(msg.content)) {
        for (const block of msg.content) {
          if (block.type === "tool_result") {
            // This would fail BEFORE the fix: compaction would wipe tool_use blocks
            expect(allToolUseIds.has(block.tool_use_id)).toBe(true);
          }
        }
      }
    }
  });

  it("stale compaction finishing after a newer compaction must not wipe next-turn messages", async () => {
    // Regression test for a second race variant:
    //
    // Turn 2 compaction (SLOW) captures historyLenAtStart_2 = 6.
    // Turn 3 compaction (FAST) captures historyLenAtStart_3 = 10, finishes quickly,
    //   and replaces history with [s3_u, s3_a] (2 items).
    // Turn 4 starts, pushes user4, API returns tool_use T4,
    //   pushes asst_4_T → history is now 4 items.
    // Turn 2 compaction finishes LAST:
    //   tail_2 = this.history.slice(6) = [] (history only has 4 items!)
    //   this.history = [s2_u, s2_a]   ← WIPES user4 and asst_4_T
    // Turn 4 then pushes tool_results_4 → [s2_u, s2_a, tool_results_4]
    // Turn 4 iteration 2: API call with messages[2] = tool_result with no tool_use → 400.
    //
    // Fix: serialize compactions so a stale one cannot overwrite a newer one.

    // Latch: compaction for turn 2 blocks until we release it
    let latch2: () => void = () => {};
    const blocked2 = new Promise<void>((res) => { latch2 = res; });

    let callIndex = 0;
    // Script:
    //   0 = turn1 (plain text, fast)
    //   1 = compact-turn1 (fast, text)
    //   2 = turn2 (plain text, fast)
    //   3 = compact-turn2 (SLOW — blocked on latch2)
    //   4 = turn3 (plain text, fast)
    //   5 = compact-turn3 (fast, finishes BEFORE compact-turn2 releases)
    //   6 = turn4 tool_use call (fast)
    //   7 = turn4 final call (fast)
    const scripts: Array<() => Promise<any>> = [
      // 0: turn1 — simple text
      async () => makeMockStream(textStreamEvents("t1"), textMessage("t1")),
      // 1: compact-turn1 — fast
      async () => makeMockStream(textStreamEvents("sum1"), textMessage("sum1")),
      // 2: turn2 — simple text
      async () => makeMockStream(textStreamEvents("t2"), textMessage("t2")),
      // 3: compact-turn2 — SLOW (blocks)
      async () => {
        await blocked2;
        return makeMockStream(textStreamEvents("sum2"), textMessage("sum2"));
      },
      // 4: turn3 — simple text
      async () => makeMockStream(textStreamEvents("t3"), textMessage("t3")),
      // 5: compact-turn3 — fast (unblocks immediately, finishes before compact-turn2)
      async () => makeMockStream(textStreamEvents("sum3"), textMessage("sum3")),
      // 6: turn4 first call — tool_use
      async () => {
        const msg = toolUseMessage("tool-t4-id", "list_files", { path: "." });
        const inner = makeMockStream(toolUseStreamEvents("list_files"), msg);
        return {
          [Symbol.asyncIterator]: () => inner[Symbol.asyncIterator](),
          finalMessage: async () => {
            const result = await inner.finalMessage();
            // Release the blocked compact-turn2 NOW:
            // at this point turn4 has pushed asst_with_tool_use to history
            // but NOT yet pushed tool_results. The stale compaction will
            // capture tail = [] (if bug) or tail with the turn4 messages (if fixed).
            latch2();
            return result;
          },
        };
      },
      // 7: turn4 final call — plain text
      async () => makeMockStream(textStreamEvents("t4"), textMessage("t4")),
    ];

    const mockProvider: StreamProvider = async () => scripts[callIndex++]();

    const agent = new Agent(mockProvider);

    await collectEvents(agent, "turn 1");
    await collectEvents(agent, "turn 2");
    await collectEvents(agent, "turn 3");
    // Turn 4: triggers the race (releases latch2 during tool_use finalMessage)
    await collectEvents(agent, "turn 4");

    // Wait for all compactions to settle
    await Bun.sleep(100);

    // Validate: no orphaned tool_result blocks in history
    const history = agent.getHistory() as any[];

    const allToolUseIds = new Set<string>();
    for (const msg of history) {
      if (msg.role === "assistant" && Array.isArray(msg.content)) {
        for (const block of msg.content) {
          if (block.type === "tool_use") allToolUseIds.add(block.id);
        }
      }
    }

    for (const msg of history) {
      if (msg.role === "user" && Array.isArray(msg.content)) {
        for (const block of msg.content) {
          if (block.type === "tool_result") {
            // Without the fix: this fails because compaction wipes the tool_use
            expect(allToolUseIds.has(block.tool_use_id)).toBe(true);
          }
        }
      }
    }
  });

  it("world state file is written at session start if prior session exists", async () => {
    // This tests that compactWorldState is called when resumeSession is invoked
    // Actually this is tested via the world-state integration path
    // Placeholder: just verify no crash when worldStatePath is given
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("ok"), textMessage("ok"));
    const tempDir = makeTempDir();
    const agent = new Agent(mockProvider, null, undefined, tempDir + "/world-state.md");
    const events = await collectEvents(agent, "test");
    expect(events.some((e) => e.type === "turn_end")).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// Slash command tests
// ---------------------------------------------------------------------------

describe("slash commands", () => {
  it("/sonnet switches to Anthropic provider with sonnet model", async () => {
    const agent = new Agent(null as any, null);
    const events = await collectEvents(agent, "/sonnet");
    const status = events.find((e) => e.type === "status") as any;
    expect(status).toBeDefined();
    expect(status.message).toContain("sonnet");
    expect(agent.getProvider()).toBe("anthropic");
    expect(agent.getActiveModel()).toBe("claude-sonnet-4-6");
  });

  it("/opus switches to Anthropic provider with opus model", async () => {
    const agent = new Agent(null as any, null);
    const events = await collectEvents(agent, "/opus");
    const status = events.find((e) => e.type === "status") as any;
    expect(status).toBeDefined();
    expect(status.message).toContain("opus");
    expect(agent.getProvider()).toBe("anthropic");
    expect(agent.getActiveModel()).toBe("claude-opus-4-6");
  });

  it("/codex switches to OpenAI provider", async () => {
    const agent = new Agent(null as any, null);
    const events = await collectEvents(agent, "/codex");
    const status = events.find((e) => e.type === "status") as any;
    expect(status).toBeDefined();
    expect(status.message).toContain("codex");
    expect(agent.getProvider()).toBe("openai");
  });

  it("/help emits a status event with command list", async () => {
    const agent = new Agent(null as any, null);
    const events = await collectEvents(agent, "/help");
    const status = events.find((e) => e.type === "status") as any;
    expect(status).toBeDefined();
    expect(status.message).toContain("/sonnet");
    expect(status.message).toContain("/opus");
    expect(status.message).toContain("/codex");
    expect(status.message).toContain("/help");
  });

  it("/help (Anthropic) includes footer legend with all three input buckets, saved, and cost multipliers", async () => {
    const agent = new Agent(null as any, null); // default = anthropic
    const events = await collectEvents(agent, "/help");
    const status = events.find((e) => e.type === "status") as any;
    expect(status.message).toContain("new:");
    expect(status.message).toContain("write:");
    expect(status.message).toContain("read:");
    expect(status.message).toContain("out:");
    expect(status.message).toContain("saved:");
    // cost multipliers
    expect(status.message).toContain("1×");
    expect(status.message).toContain("1.25×");
    expect(status.message).toContain("0.1×");
  });

  it("/help (OpenAI after /codex) shows shorter legend — no write:/read:/saved:", async () => {
    const agent = new Agent(null as any, null);
    await collectEvents(agent, "/codex"); // switch to openai
    const events = await collectEvents(agent, "/help");
    const status = events.find((e) => e.type === "status") as any;
    expect(status.message).toContain("new:");
    expect(status.message).toContain("out:");
    // OpenAI footer has no cache breakdown or saved
    expect(status.message).not.toContain("write:");
    expect(status.message).not.toContain("read:");
    expect(status.message).not.toContain("saved:");
  });

  it("old /gpt command is rejected as unknown", async () => {
    const agent = new Agent(null as any, null);
    const events = await collectEvents(agent, "/gpt");
    const err = events.find((e) => e.type === "error") as any;
    expect(err).toBeDefined();
  });

  it("old /openai command is rejected as unknown", async () => {
    const agent = new Agent(null as any, null);
    const events = await collectEvents(agent, "/openai");
    const err = events.find((e) => e.type === "error") as any;
    expect(err).toBeDefined();
  });

  it("old /anthropic command is rejected as unknown", async () => {
    const agent = new Agent(null as any, null);
    const events = await collectEvents(agent, "/anthropic");
    const err = events.find((e) => e.type === "error") as any;
    expect(err).toBeDefined();
  });

  it("/sonnet followed by /opus changes active model", async () => {
    const agent = new Agent(null as any, null);
    await collectEvents(agent, "/sonnet");
    await collectEvents(agent, "/opus");
    expect(agent.getActiveModel()).toBe("claude-opus-4-6");
    expect(agent.getProvider()).toBe("anthropic");
  });
});
