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

// Tests that use a mock StreamProvider must never write to production files.
// These tests prove the contract: when a streamProvider is given without
// explicit paths, Agent must NOT write to any production file.
describe("Agent — test isolation (no production file pollution)", () => {
  it("does not write to sessions/ or diagnosis/ during a turn (mock provider isolation)", async () => {
    const { existsSync, readdirSync: rds } = await import("fs");

    const countFiles = (dir: string) => existsSync(dir) ? rds(dir).length : 0;
    const beforeSessions = countFiles("sessions");
    const beforeDiag = countFiles("diagnosis");

    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hi"), textMessage("hi"));

    const agent = new Agent(mockProvider);
    await collectEvents(agent, "should not write to production files");
    await Bun.sleep(100);

    expect(countFiles("sessions")).toBe(beforeSessions);
    expect(countFiles("diagnosis")).toBe(beforeDiag);
  });

  it("does not write to sessions/context.jsonl when no contextFile is given", async () => {
    const { existsSync, statSync } = await import("fs");
    const contextPath = join(process.cwd(), "sessions", "context.jsonl");

    // Record the file size before (it may already exist from a real session)
    const sizeBefore = existsSync(contextPath) ? statSync(contextPath).size : -1;

    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hi"), textMessage("hi"));

    // No contextFile passed — must not append to production context file
    const agent = new Agent(mockProvider);
    await collectEvents(agent, "should not write context");
    await Bun.sleep(100);

    const sizeAfter = existsSync(contextPath) ? statSync(contextPath).size : -1;
    expect(sizeAfter).toBe(sizeBefore);
  });

  it("does not write to sessions/events.jsonl when OpenAI caller used without explicit eventsFile=null", async () => {
    // Regression test: agent-rate-limit tests previously passed streamProvider=undefined
    // with a custom openAiCaller, bypassing the mock-provider heuristic and writing to
    // the production events file. Explicit null, null must be passed in that pattern.
    const { existsSync, statSync } = await import("fs");
    const eventsPath = join(process.cwd(), "sessions", "events.jsonl");

    const sizeBefore = existsSync(eventsPath) ? statSync(eventsPath).size : -1;

    const openAiCaller = async () => ({
      response: {
        content: [{ type: "text", text: "ok" } as any],
        stop_reason: "stop",
        usage: { input_tokens: 1, output_tokens: 2 },
      },
      text: "ok",
      raw: { usage: { input_tokens: 1, output_tokens: 2 } },
    });

    // Must pass null, null explicitly to disable production file writes
    const agent = new Agent(undefined, null, openAiCaller as any, null, null, null);
    agent.setProvider("openai");
    await collectEvents(agent, "hello");
    await Bun.sleep(100);

    const sizeAfter = existsSync(eventsPath) ? statSync(eventsPath).size : -1;
    expect(sizeAfter).toBe(sizeBefore);
  });
});

// ---------------------------------------------------------------------------
// Plain text response
// ---------------------------------------------------------------------------

describe("Agent.sendMessage — plain text response", () => {
  it("emits user_message, then text events, then turn_end", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("Hello!"), textMessage("Hello!"));

    const agent = new Agent(mockProvider);
    const events = await collectEvents(agent, "hi");

    const types = events.map((e) => e.type);
    expect(types[0]).toBe("user_message");
    expect(types).toContain("text");
    expect(types[types.length - 1]).toBe("turn_end");
    expect(types).not.toContain("status");
    expect(types).not.toContain("metrics");
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

    const history = agent.getLlmMessageLog();
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

  it("turn_end carries correct token counts", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("ok"), textMessage("ok"));

    const agent = new Agent(mockProvider);
    const events = await collectEvents(agent, "test");

    const turnEnd = events.find((e) => e.type === "turn_end") as any;
    expect(turnEnd).toBeDefined();
    expect(turnEnd.metrics.inputTokens).toBe(10);
    expect(turnEnd.metrics.outputTokens).toBe(5);
    expect(turnEnd.metrics.costUsd).toBeGreaterThan(0);
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

    const toolCallEvents = events.filter((e) => e.type === "agent_to_agent_tool_call");
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

    const resultEvents = events.filter((e) => e.type === "agent_to_agent_tool_result");
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
    const history = agent.getLlmMessageLog();
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

    const toolCallEvents = events.filter((e) => e.type === "agent_to_agent_tool_call");
    const toolResultEvents = events.filter((e) => e.type === "agent_to_agent_tool_result");
    expect(toolCallEvents.length).toBe(2);
    expect(toolResultEvents.length).toBe(2);

    // Parallel execution: both tool_call events appear before any tool_result event.
    // Sequential execution would interleave: call_A, result_A, call_B, result_B.
    const firstResultIndex = events.findIndex((e) => e.type === "agent_to_agent_tool_result");
    const lastCallIndex = events.reduce(
      (idx, e, i) => (e.type === "agent_to_agent_tool_call" ? i : idx),
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

    const errorEvents = events.filter((e) => e.type === "agent_error");
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
    const errorEvents = events.filter((e) => e.type === "agent_error");
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

    // Should emit a "turn_interrupted" event so the UI can show feedback
    const interrupted = events.find((e) => e.type === "turn_interrupted");
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
    const history = agent.getLlmMessageLog();
    expect(history.length).toBe(1);
    expect(history[0].role).toBe("user");
  });
});

// ---------------------------------------------------------------------------
// llm_call event
// ---------------------------------------------------------------------------

describe("llm_call event", () => {
  it("emits llm_call before the first API call", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hello"), textMessage("hello"));
    const agent = new Agent(mockProvider, null);
    const events = await collectEvents(agent, "hi");
    const startEvents = events.filter((e) => e.type === "llm_call");
    expect(startEvents.length).toBe(1);
  });

  it("llm_call carries provider, url, and request", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hello"), textMessage("hello"));
    const agent = new Agent(mockProvider, null);
    const events = await collectEvents(agent, "hi");
    const e = events.find((e) => e.type === "llm_call") as any;
    expect(e).toBeDefined();
    expect(e.provider).toBe("anthropic");
    expect(typeof e.url).toBe("string");
    expect(typeof e.request).toBe("object");
    expect(e.llmCallNumber).toBeUndefined();
  });

  it("llm_call exposes request messages", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hello"), textMessage("hello"));
    const agent = new Agent(mockProvider, null);
    const events = await collectEvents(agent, "hi");
    const e = events.find((e) => e.type === "llm_call") as any;
    expect(Array.isArray(e.request.messages)).toBe(true);
    expect(e.request.messages[0].role).toBe("user");
  });

  it("emits llm_call once per round-trip in a tool loop", async () => {
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
    const startEvents = events.filter((e) => e.type === "llm_call");
    expect(startEvents.length).toBe(2);
  });

  it("llm_call request snapshot is correct (not a live reference)", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("reply"), textMessage("reply"));
    const agent = new Agent(mockProvider, null);
    const events = await collectEvents(agent, "hello");
    const e = events.find((ev) => ev.type === "llm_call") as any;
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
    const toolCalls = events.filter((e) => e.type === "agent_to_agent_tool_call");
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
// llm_response event
// ---------------------------------------------------------------------------

describe("Agent — llm_response event", () => {
  it("emits llm_response after each API call with stop_reason and usage", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hello"), textMessage("hello"));
    const agent = new Agent(mockProvider, null);
    const events = await collectEvents(agent, "hi");
    const r = events.find((e) => e.type === "llm_to_agent") as any;
    expect(r).toBeDefined();
    expect(r.provider).toBe("anthropic");
    expect(r.url).toBe("https://api.anthropic.com/v1/messages");
    expect(r.stopReason).toBe("end_turn");
    expect(r.usage.input_tokens).toBe(10);
    expect(r.usage.output_tokens).toBe(5);
  });

  it("llm_response content includes text blocks", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hello"), textMessage("hello"));
    const agent = new Agent(mockProvider, null);
    const events = await collectEvents(agent, "hi");
    const r = events.find((e) => e.type === "llm_to_agent") as any;
    expect(r.content.some((b: any) => b.type === "text")).toBe(true);
  });

  it("llm_response content includes tool_use blocks when model requests tools", async () => {
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
    const responses = events.filter((e) => e.type === "llm_to_agent") as any[];
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
    const result = events.find((e) => e.type === "agent_to_agent_tool_result") as any;
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
    const result = events.find((e) => e.type === "agent_to_agent_tool_result") as any;
    expect(result.formatted).toBe("run_command: echo hi");
  });
});

// ---------------------------------------------------------------------------
// turn_end carries timing fields
// ---------------------------------------------------------------------------

describe("Agent — turn_end timing fields", () => {
  it("turn_end carries totalMs", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hi"), textMessage("hi"));
    const agent = new Agent(mockProvider, null);
    const events = await collectEvents(agent, "hi");
    const m = events.find((e) => e.type === "turn_end") as any;
    expect(typeof m.metrics.totalMs).toBe("number");
    expect(m.metrics.totalMs).toBeGreaterThanOrEqual(0);
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

  it("user_message is emitted before llm_call", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hi"), textMessage("hi"));
    const agent = new Agent(mockProvider, null);
    const events = await collectEvents(agent, "hi");
    const types = events.map((e) => e.type);
    const umIdx = types.indexOf("user_message");
    const apiIdx = types.indexOf("llm_call");
    expect(umIdx).toBeGreaterThanOrEqual(0);
    expect(umIdx).toBeLessThan(apiIdx);
  });
});



// ---------------------------------------------------------------------------
// History grows verbatim (no zone 2 compaction after manifest Step 2)
// ---------------------------------------------------------------------------

describe("Agent — verbatim history (no turn compaction)", () => {
  it("after a turn, history contains the verbatim user+assistant exchange", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("Hello!"), textMessage("Hello!"));
    const agent = new Agent(mockProvider);
    await collectEvents(agent, "hi");
    const history = agent.getLlmMessageLog();
    // History has 2 messages: user + assistant (verbatim, no compaction)
    expect(history).toHaveLength(2);
    expect(history[0].role).toBe("user");
    expect(history[1].role).toBe("assistant");
    expect(history[0].content).toBe("hi");
  });

  it("after two turns, history contains all four messages verbatim", async () => {
    let call = 0;
    const mockProvider: StreamProvider = async () => {
      call++;
      return makeMockStream(textStreamEvents(`response ${call}`), textMessage(`response ${call}`));
    };
    const agent = new Agent(mockProvider);
    await collectEvents(agent, "turn 1");
    await collectEvents(agent, "turn 2");
    const history = agent.getLlmMessageLog();
    // 4 messages: user1, asst1, user2, asst2
    expect(history).toHaveLength(4);
    expect(history[0].role).toBe("user");
    expect(history[1].role).toBe("assistant");
    expect(history[2].role).toBe("user");
    expect(history[3].role).toBe("assistant");
  });

  it("no orphaned tool_result blocks across multiple turns with tool use", async () => {
    let call = 0;
    const mockProvider: StreamProvider = async () => {
      call++;
      if (call === 1) return makeMockStream(toolUseStreamEvents("list_files"), toolUseMessage("t1", "list_files", { path: "." }));
      if (call === 2) return makeMockStream(textStreamEvents("done turn 1"), textMessage("done turn 1"));
      return makeMockStream(textStreamEvents("done turn 2"), textMessage("done turn 2"));
    };
    const agent = new Agent(mockProvider);
    await collectEvents(agent, "turn 1 with tool");
    await collectEvents(agent, "turn 2");
    const history = agent.getLlmMessageLog() as any[];
    // Check: every tool_result has a matching tool_use
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
            expect(allToolUseIds.has(block.tool_use_id)).toBe(true);
          }
        }
      }
    }
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

  it("/help is rejected as unknown (operator asks the LLM instead)", async () => {
    const agent = new Agent(null as any, null);
    const events = await collectEvents(agent, "/help");
    const err = events.find((e) => e.type === "agent_error") as any;
    expect(err).toBeDefined();
  });

  it("old /gpt command is rejected as unknown", async () => {
    const agent = new Agent(null as any, null);
    const events = await collectEvents(agent, "/gpt");
    const err = events.find((e) => e.type === "agent_error") as any;
    expect(err).toBeDefined();
  });

  it("old /openai command is rejected as unknown", async () => {
    const agent = new Agent(null as any, null);
    const events = await collectEvents(agent, "/openai");
    const err = events.find((e) => e.type === "agent_error") as any;
    expect(err).toBeDefined();
  });

  it("old /anthropic command is rejected as unknown", async () => {
    const agent = new Agent(null as any, null);
    const events = await collectEvents(agent, "/anthropic");
    const err = events.find((e) => e.type === "agent_error") as any;
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

// ---------------------------------------------------------------------------
// Unified event taxonomy — SessionEvent variants
// ---------------------------------------------------------------------------

describe("Agent — unified event taxonomy (true duals)", () => {
  it("emits llm_to_agent (not api_response or llm_response) after LLM call", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hello"), textMessage("hello"));
    const agent = new Agent(mockProvider, null);
    const events = await collectEvents(agent, "hi");
    expect(events.find((e) => e.type === "llm_to_agent")).toBeDefined();
    expect(events.find((e) => (e as any).type === "api_response")).toBeUndefined();
    expect(events.find((e) => (e as any).type === "llm_response")).toBeUndefined();
  });

  it("emits agent_to_agent_tool_call (not tool_call) when a tool is invoked", async () => {
    let call = 0;
    const mockProvider: StreamProvider = async () => {
      call++;
      if (call === 1) return makeMockStream(toolUseStreamEvents("read_file"), toolUseMessage("t1", "read_file", { path: "src/config.ts" }));
      return makeMockStream(textStreamEvents("done"), textMessage("done"));
    };
    const agent = new Agent(mockProvider, null);
    const events = await collectEvents(agent, "read it");
    expect(events.find((e) => e.type === "agent_to_agent_tool_call")).toBeDefined();
    expect(events.find((e) => (e as any).type === "tool_call")).toBeUndefined();
  });

  it("emits agent_to_agent_tool_result (not tool_result) after tool execution", async () => {
    let call = 0;
    const mockProvider: StreamProvider = async () => {
      call++;
      if (call === 1) return makeMockStream(toolUseStreamEvents("read_file"), toolUseMessage("t1", "read_file", { path: "src/config.ts" }));
      return makeMockStream(textStreamEvents("done"), textMessage("done"));
    };
    const agent = new Agent(mockProvider, null);
    const events = await collectEvents(agent, "read it");
    expect(events.find((e) => e.type === "agent_to_agent_tool_result")).toBeDefined();
    expect(events.find((e) => (e as any).type === "tool_result")).toBeUndefined();
  });
});
