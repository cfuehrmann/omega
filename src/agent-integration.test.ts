/**
 * Integration tests for the Agent class using a mock provider.
 *
 * These tests cover the full sendMessage loop — streaming, tool dispatch,
 * session persistence, and history management — without hitting the real API.
 *
 * The Agent constructor accepts an optional StreamProvider. Tests inject a
 * mock that returns pre-scripted responses.
 */

import { describe, it, expect, afterEach } from "bun:test";
import type Anthropic from "@anthropic-ai/sdk";

import { Agent, type OmegaEvent, type StreamSignal, type StreamProvider } from "./agent.js";
import { makeTestAgent } from "./test-utils.js";

const disposeAll: (() => void)[] = [];
afterEach(() => { disposeAll.splice(0).forEach(d => d()); });

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
    container: null,
    content: [{ type: "text", text, citations: null }],
    stop_reason: "end_turn",
    stop_sequence: null,
    usage: { input_tokens: 10, output_tokens: 5, cache_creation: null, cache_creation_input_tokens: null, cache_read_input_tokens: null, inference_geo: null, server_tool_use: null, service_tier: null },
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
    container: null,
    content: [{ type: "tool_use", id: toolId, name: toolName, input: toolInput, caller: { type: "direct" } }],
    stop_reason: "tool_use",
    stop_sequence: null,
    usage: { input_tokens: 20, output_tokens: 10, cache_creation: null, cache_creation_input_tokens: null, cache_read_input_tokens: null, inference_geo: null, server_tool_use: null, service_tier: null },
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
  confirmFn?: (name: string, input: any) => Promise<boolean>
): Promise<(OmegaEvent | StreamSignal)[]> {
  const events: (OmegaEvent | StreamSignal)[] = [];
  const confirm = confirmFn ?? (async () => true);
  for await (const event of agent.sendMessage(message, confirm)) {
    events.push(event);
  }
  return events;
}



// ---------------------------------------------------------------------------
// Plain text response
// ---------------------------------------------------------------------------

describe("Agent.sendMessage — plain text response", () => {
  it.concurrent("emits user_message, then text events, then turn_end", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("Hello!"), textMessage("Hello!"));

    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "hi");

    const types = events.map((e) => e.type);
    expect(types[0]).toBe("user_message");
    expect(types).toContain("text");
    expect(types[types.length - 1]).toBe("turn_end");
    expect(types).not.toContain("status");
    expect(types).not.toContain("metrics");
  });

  it.concurrent("accumulates text from chunks", async () => {
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

    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "tell me something");

    const textEvents = events.filter((e) => e.type === "text");
    expect(textEvents.length).toBe(3);
    const combined = textEvents.map((e) => (e as any).text).join("");
    expect(combined).toBe("foo bar baz");
  });

  it.concurrent("adds user message and assistant response to history", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("I am fine"), textMessage("I am fine"));

    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    await collectEvents(agent, "how are you?");

    const history = agent.getCompactedContextHistory();
    expect(history.length).toBe(2);
    expect(history[0]!).toEqual({ role: "user", content: "how are you?" });
    expect(history[1]!.role).toBe("assistant");
  });

  it.concurrent("accumulates token counts across turns", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("ok"), textMessage("ok"));

    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    await collectEvents(agent, "first");
    await collectEvents(agent, "second");

    // Each call: 10 input + 5 output
    expect(agent.sessionInputTokens).toBe(20);
    expect(agent.sessionOutputTokens).toBe(10);
  });

  it.concurrent("turn_end carries correct token counts", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("ok"), textMessage("ok"));

    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "test");

    const turnEnd = events.find((e) => e.type === "turn_end") as any;
    expect(turnEnd).toBeDefined();
    expect(turnEnd.metrics.inputTokens).toBe(10);
    expect(turnEnd.metrics.outputTokens).toBe(5);
    expect(turnEnd.metrics.inputTokens).toBeGreaterThan(0);
  });
});

// ---------------------------------------------------------------------------
// Tool call loop
// ---------------------------------------------------------------------------

describe("Agent.sendMessage — tool call loop", () => {
  it.concurrent("emits tool_call event for auto-approved tools", async () => {
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

    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "read config");

    const toolCallEvents = events.filter((e) => e.type === "tool_call");
    expect(toolCallEvents.length).toBe(1);
    expect((toolCallEvents[0] as any).name).toBe("read_file");
  });

  it.concurrent("emits tool_result event after executing the tool", async () => {
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

    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "read config");

    const resultEvents = events.filter((e) => e.type === "tool_result");
    expect(resultEvents.length).toBe(1);
    const result = resultEvents[0] as any;
    expect(result.name).toBe("read_file");
    expect(result.isError).toBe(false);
  });



  it.concurrent("adds tool results to history and makes a second API call", async () => {
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

    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
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

  it.concurrent("history grows correctly across a tool loop", async () => {
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

    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    await collectEvents(agent, "read config");

    // Expected: user, assistant(tool_use), user(tool_result), assistant(text)
    const history = agent.getCompactedContextHistory();
    expect(history.length).toBe(4);
    expect(history[0]!.role).toBe("user");
    expect(history[1]!.role).toBe("assistant");
    expect(history[2]!.role).toBe("user"); // tool result
    expect(history[3]!.role).toBe("assistant");
  });

  it.concurrent("executes multiple tools in parallel (both tool_call events before any tool_result)", async () => {
    // Build a response with two tool_use blocks (list_files + list_files)
    const twoToolMessage: Anthropic.Message = {
      id: "msg_two_tools",
      type: "message",
      role: "assistant",
      model: "claude-sonnet-4-6",
      container: null,
      content: [
        { type: "tool_use", id: "tA", name: "list_files", input: { path: "src" }, caller: { type: "direct" } },
        { type: "tool_use", id: "tB", name: "list_files", input: { path: "plan" }, caller: { type: "direct" } },
      ],
      stop_reason: "tool_use",
      stop_sequence: null,
      usage: { input_tokens: 20, output_tokens: 10, cache_creation: null, cache_creation_input_tokens: null, cache_read_input_tokens: null, inference_geo: null, server_tool_use: null, service_tier: null },
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

    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
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
  it.concurrent("emits an error event on non-retryable API failure", async () => {
    const mockProvider: StreamProvider = async () => {
      const err: any = new Error("Bad request");
      err.status = 400;
      throw err;
    };

    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "test");

    const errorEvents = events.filter((e) => e.type === "agent_error");
    expect(errorEvents.length).toBeGreaterThan(0);
    expect((errorEvents[0] as any).error).toContain("Bad request");
    // Turn ends with turn_interrupted(reason=error) so the UI streaming flag resets
    const last = events[events.length - 1] as any;
    expect(last.type).toBe("turn_interrupted");
    expect(last.reason).toBe("error");
  });

  it.concurrent("emits retry error events then succeeds on transient failure", async () => {
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

    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "test");

    delete process.env.OMEGA_RETRY_BASE_MS;
    delete process.env.OMEGA_RETRY_MAX_MS;
    delete process.env.OMEGA_RETRY_ATTEMPTS;

    // Should have retried and eventually succeeded (compaction may add extra calls)
    expect(attempts).toBeGreaterThanOrEqual(3);
    // No agent_error emitted during retries — llm_retry carries the info
    const errorEvents = events.filter((e) => e.type === "agent_error");
    expect(errorEvents.length).toBe(0);
    // llm_retry events were emitted (one per failed attempt)
    const retryEvents = events.filter((e) => e.type === "llm_retry");
    expect(retryEvents.length).toBe(2);
    // Final event should be turn_end (success)
    expect(events[events.length - 1]!.type).toBe("turn_end");
  }, 30_000);
});

// ---------------------------------------------------------------------------
// Abort / interrupt
// ---------------------------------------------------------------------------

describe("Agent.sendMessage — abort", () => {
  it.concurrent("stops emitting events when aborted mid-stream", async () => {
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

    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const events: (OmegaEvent | StreamSignal)[] = [];
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
    const interrupted = events.find((e) => e.type === "turn_interrupted") as any;
    expect(interrupted).toBeDefined();
    expect(interrupted.reason).toBe("aborted");
  });

  it.concurrent("does not add incomplete assistant turn to history when aborted", async () => {
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

    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const controller = new AbortController();
    const gen = agent.sendMessage("hello", async () => true, controller.signal);

    for await (const event of gen) {
      if (event.type === "text") controller.abort();
    }

    // History should only have the user message — no partial assistant turn
    const history = agent.getCompactedContextHistory();
    expect(history.length).toBe(1);
    expect(history[0]!.role).toBe("user");
  });
});

// ---------------------------------------------------------------------------
// llm_call event
// ---------------------------------------------------------------------------

describe("llm_call event", () => {
  it.concurrent("emits llm_call before the first API call", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hello"), textMessage("hello"));
    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "hi");
    const startEvents = events.filter((e) => e.type === "llm_call");
    expect(startEvents.length).toBe(1);
  });

  it.concurrent("llm_call carries url and requestSummary", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hello"), textMessage("hello"));
    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "hi");
    const e = events.find((e) => e.type === "llm_call") as any;
    expect(e).toBeDefined();
    expect(typeof e.url).toBe("string");
    expect(typeof e.requestSummary).toBe("object");
    expect(e.llmCallNumber).toBeUndefined();
  });

  it.concurrent("llm_call requestSummary has elided messages descriptor", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hello"), textMessage("hello"));
    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "hi");
    const e = events.find((e) => e.type === "llm_call") as any;
    // messages is elided to a string descriptor, not a live array
    expect(typeof e.requestSummary.messages).toBe("string");
    expect(e.requestSummary.messages).toMatch(/1 message/);
  });

  it.concurrent("llm_call requestSummary includes all top-level request fields (pass-through)", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hello"), textMessage("hello"));
    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "hi");
    const e = events.find((e) => e.type === "llm_call") as any;
    // thinking and betas must appear — they are part of the actual API payload
    expect(e.requestSummary.thinking).toEqual({ type: "adaptive" });
    expect(Array.isArray(e.requestSummary.betas)).toBe(true);
    expect(e.requestSummary.context_management).toBeDefined();
  });

  it.concurrent("emits llm_call once per round-trip in a tool loop", async () => {
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
    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "list files");
    const startEvents = events.filter((e) => e.type === "llm_call");
    expect(startEvents.length).toBe(2);
  });

  it.concurrent("llm_call requestSummary reflects the number of messages sent", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("reply"), textMessage("reply"));
    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "hello");
    const e = events.find((ev) => ev.type === "llm_call") as any;
    // descriptor shows the message count
    expect(e.requestSummary.messages).toMatch(/1 message/);
  });
});

// ---------------------------------------------------------------------------
// Full auto-approve
// ---------------------------------------------------------------------------

describe("Agent — full auto-approve", () => {
  it.concurrent("never emits tool_pending even for previously-rejected commands", async () => {
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
    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "do it");
    const pending = events.filter((e) => (e.type as string) === "tool_pending");
    expect(pending.length).toBe(0);
  });

  it.concurrent("emits tool_call for every tool regardless of name", async () => {
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
    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "do it");
    const toolCalls = events.filter((e) => e.type === "tool_call");
    expect(toolCalls.length).toBe(1);
    expect((toolCalls[0] as any).name).toBe("run_command");
  });

  it.concurrent("confirmTool callback is never invoked", async () => {
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
    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const confirm = async () => { confirmCalled = true; return true; };
    for await (const _ of agent.sendMessage("go", confirm)) {}
    expect(confirmCalled).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// turn_end event
// ---------------------------------------------------------------------------

describe("Agent — turn_end event", () => {
  it.concurrent("emits exactly one turn_end per user message", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hello"), textMessage("hello"));
    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "hi");
    const turnEnds = events.filter((e) => e.type === "turn_end");
    expect(turnEnds.length).toBe(1);
  });

  it.concurrent("turn_end is the last event emitted", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hello"), textMessage("hello"));
    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "hi");
    expect(events[events.length - 1]!.type).toBe("turn_end");
  });

  it.concurrent("turn_end aggregates token counts across all API calls", async () => {
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
    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "go");
    const turnEnd = events.find((e) => e.type === "turn_end") as any;
    // toolUseMessage has input:20 output:10, textMessage has input:10 output:5
    expect(turnEnd.metrics.inputTokens).toBe(30);
    expect(turnEnd.metrics.outputTokens).toBe(15);
  });
});

// ---------------------------------------------------------------------------
// llm_response event
// ---------------------------------------------------------------------------

describe("Agent — llm_response event", () => {
  it.concurrent("emits llm_response after each API call with stop_reason and usage", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hello"), textMessage("hello"));
    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "hi");
    const r = events.find((e) => e.type === "llm_response") as any;
    expect(r).toBeDefined();
    expect(r.stopReason).toBe("end_turn");
    expect(r.usage.input_tokens).toBe(10);
    expect(r.usage.output_tokens).toBe(5);
  });

  it.concurrent("llm_response responseSummary has stop_reason and usage", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hello"), textMessage("hello"));
    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "hi");
    const r = events.find((e) => e.type === "llm_response") as any;
    expect(r.responseSummary).toBeDefined();
    expect(r.responseSummary.stop_reason).toBe("end_turn");
    expect(typeof r.responseSummary.usage).toBe("object");
  });

  it.concurrent("llm_response responseSummary content is elided", async () => {
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
    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "list");
    const responses = events.filter((e) => e.type === "llm_response") as any[];
    const first = responses[0];
    // content is replaced with a string descriptor, not an array
    expect(typeof first.responseSummary.content).toBe("string");
    expect(first.responseSummary.content).toMatch(/elided/);
  });
});

// ---------------------------------------------------------------------------
// tool_call carries input, tool_result carries output
// ---------------------------------------------------------------------------

describe("Agent — tool_call input and tool_result output fields", () => {
  it.concurrent("tool_call event carries the input object", async () => {
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
    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "list files");
    const toolCall = events.find((e) => e.type === "tool_call") as any;
    expect(toolCall.input).toEqual({ path: "src/" });
  });

  it.concurrent("tool_result event carries the output string", async () => {
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
    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "test");
    const result = events.find((e) => e.type === "tool_result") as any;
    expect(typeof result.output).toBe("string");
    expect(result.output).toContain("hi");
  });
});

// ---------------------------------------------------------------------------
// user_message event
// ---------------------------------------------------------------------------

describe("Agent — user_message event", () => {
  it.concurrent("emits user_message as first event with the prompt text", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hi"), textMessage("hi"));
    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "hello there");
    const um = events.find((e) => e.type === "user_message") as any;
    expect(um).toBeDefined();
    expect(um.content).toBe("hello there");
  });

  it.concurrent("user_message is emitted before llm_call", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hi"), textMessage("hi"));
    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
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
  it.concurrent("after a turn, history contains the verbatim user+assistant exchange", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("Hello!"), textMessage("Hello!"));
    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    await collectEvents(agent, "hi");
    const history = agent.getCompactedContextHistory();
    // History has 2 messages: user + assistant (verbatim, no compaction)
    expect(history).toHaveLength(2);
    expect(history[0]!.role).toBe("user");
    expect(history[1]!.role).toBe("assistant");
    expect(history[0]!.content).toBe("hi");
  });

  it.concurrent("after two turns, history contains all four messages verbatim", async () => {
    let call = 0;
    const mockProvider: StreamProvider = async () => {
      call++;
      return makeMockStream(textStreamEvents(`response ${call}`), textMessage(`response ${call}`));
    };
    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    await collectEvents(agent, "turn 1");
    await collectEvents(agent, "turn 2");
    const history = agent.getCompactedContextHistory();
    // 4 messages: user1, asst1, user2, asst2
    expect(history).toHaveLength(4);
    expect(history[0]!.role).toBe("user");
    expect(history[1]!.role).toBe("assistant");
    expect(history[2]!.role).toBe("user");
    expect(history[3]!.role).toBe("assistant");
  });

  it.concurrent("no orphaned tool_result blocks across multiple turns with tool use", async () => {
    let call = 0;
    const mockProvider: StreamProvider = async () => {
      call++;
      if (call === 1) return makeMockStream(toolUseStreamEvents("list_files"), toolUseMessage("t1", "list_files", { path: "." }));
      if (call === 2) return makeMockStream(textStreamEvents("done turn 1"), textMessage("done turn 1"));
      return makeMockStream(textStreamEvents("done turn 2"), textMessage("done turn 2"));
    };
    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    await collectEvents(agent, "turn 1 with tool");
    await collectEvents(agent, "turn 2");
    const history = agent.getCompactedContextHistory() as any[];
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
  it.concurrent("/sonnet switches to sonnet model", async () => {
    const { agent, dispose } = await makeTestAgent(undefined);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "/sonnet");
    const mc = events.find((e) => e.type === "model_changed") as any;
    expect(mc).toBeDefined();
    expect(mc.model).toBe("claude-sonnet-4-6");
    expect(agent.getActiveModel()).toBe("claude-sonnet-4-6");
  });

  it.concurrent("/opus switches to opus model", async () => {
    const { agent, dispose } = await makeTestAgent(undefined);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "/opus");
    const mc = events.find((e) => e.type === "model_changed") as any;
    expect(mc).toBeDefined();
    expect(mc.model).toBe("claude-opus-4-6");
    expect(agent.getActiveModel()).toBe("claude-opus-4-6");
  });

  it.concurrent("/codex is rejected as unknown", async () => {
    const { agent, dispose } = await makeTestAgent(undefined);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "/codex");
    const err = events.find((e) => e.type === "agent_error") as any;
    expect(err).toBeDefined();
  });

  it.concurrent("/help is rejected as unknown (operator asks the LLM instead)", async () => {
    const { agent, dispose } = await makeTestAgent(undefined);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "/help");
    const err = events.find((e) => e.type === "agent_error") as any;
    expect(err).toBeDefined();
  });

  it.concurrent("old /anthropic command is rejected as unknown", async () => {
    const { agent, dispose } = await makeTestAgent(undefined);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "/anthropic");
    const err = events.find((e) => e.type === "agent_error") as any;
    expect(err).toBeDefined();
  });

  it.concurrent("/sonnet followed by /opus changes active model", async () => {
    const { agent, dispose } = await makeTestAgent(undefined);
    disposeAll.push(dispose);
    await collectEvents(agent, "/sonnet");
    await collectEvents(agent, "/opus");
    expect(agent.getActiveModel()).toBe("claude-opus-4-6");
  });
});

// ---------------------------------------------------------------------------
// Unified event taxonomy — SessionEvent variants
// ---------------------------------------------------------------------------

describe("Agent — unified event taxonomy (true duals)", () => {
  it.concurrent("emits llm_response (not api_response or llm_to_agent) after LLM call", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hello"), textMessage("hello"));
    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "hi");
    expect(events.find((e) => e.type === "llm_response")).toBeDefined();
    expect(events.find((e) => (e as any).type === "api_response")).toBeUndefined();
    expect(events.find((e) => (e as any).type === "llm_to_agent")).toBeUndefined();
  });

  it.concurrent("emits tool_call (not agent_to_agent_tool_call) when a tool is invoked", async () => {
    let call = 0;
    const mockProvider: StreamProvider = async () => {
      call++;
      if (call === 1) return makeMockStream(toolUseStreamEvents("read_file"), toolUseMessage("t1", "read_file", { path: "src/config.ts" }));
      return makeMockStream(textStreamEvents("done"), textMessage("done"));
    };
    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "read it");
    expect(events.find((e) => e.type === "tool_call")).toBeDefined();
    expect(events.find((e) => (e as any).type === "agent_to_agent_tool_call")).toBeUndefined();
  });

  it.concurrent("emits tool_result (not agent_to_agent_tool_result) after tool execution", async () => {
    let call = 0;
    const mockProvider: StreamProvider = async () => {
      call++;
      if (call === 1) return makeMockStream(toolUseStreamEvents("read_file"), toolUseMessage("t1", "read_file", { path: "src/config.ts" }));
      return makeMockStream(textStreamEvents("done"), textMessage("done"));
    };
    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "read it");
    expect(events.find((e) => e.type === "tool_result")).toBeDefined();
    expect(events.find((e) => (e as any).type === "agent_to_agent_tool_result")).toBeUndefined();
  });
});

// ---------------------------------------------------------------------------
// llm_response.text — text field on the response event
// ---------------------------------------------------------------------------

describe("Agent — llm_response text field", () => {
  it.concurrent("llm_response carries text when response has text", async () => {
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("Hello world"), textMessage("Hello world"));
    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "hi");
    const r = events.find((e) => e.type === "llm_response") as any;
    expect(r).toBeDefined();
    expect(r.text).toBe("Hello world");
  });

  it.concurrent("llm_response.text is written to events.jsonl", async () => {
    const { readFile } = await import("fs/promises");
    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("Persisted text"), textMessage("Persisted text"));
    const { agent, eventsFile, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    await collectEvents(agent, "hi");
    const raw = await readFile(eventsFile, "utf-8");
    const lines = raw.trim().split("\n").map(l => JSON.parse(l));
    const r = lines.find((e: any) => e.type === "llm_response");
    expect(r).toBeDefined();
    expect(r.text).toBe("Persisted text");
  });

  it.concurrent("llm_response has no text field when response is tool-only", async () => {
    let call = 0;
    const mockProvider: StreamProvider = async () => {
      call++;
      if (call === 1) return makeMockStream(toolUseStreamEvents("read_file"), toolUseMessage("t1", "read_file", { path: "README.md" }));
      return makeMockStream(textStreamEvents("done"), textMessage("done"));
    };
    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "use tool");
    const responses = events.filter((e) => e.type === "llm_response") as any[];
    // First call is tool-only — no text field.
    expect(responses[0].text).toBeUndefined();
    // Second call has text.
    expect(responses[1].text).toBe("done");
  });
});

// ---------------------------------------------------------------------------
// session_start dedup — not re-emitted on reconnect
// ---------------------------------------------------------------------------

describe("Agent — session_start dedup on reconnect", () => {
  it.concurrent("init() logs session_start only once even if called multiple times", async () => {
    const { readFile } = await import("fs/promises");
    const origKey = process.env.ANTHROPIC_API_KEY;
    process.env.ANTHROPIC_API_KEY = "test-key-dummy";
    try {
      const { agent, eventsFile, dispose } = await makeTestAgent();
      disposeAll.push(dispose);
      // Call init() twice (simulating reconnect)
      await agent.init();
      await agent.init();
      const raw = await readFile(eventsFile, "utf-8");
      const lines = raw.trim().split("\n").map(l => JSON.parse(l));
      const starts = lines.filter((e: any) => e.type === "session_start");
      expect(starts.length).toBe(1);
    } finally {
      if (origKey === undefined) delete process.env.ANTHROPIC_API_KEY;
      else process.env.ANTHROPIC_API_KEY = origKey;
    }
  });
});

// ---------------------------------------------------------------------------
// BUG-2 guard — dangling tool_use from interrupted previous session
// ---------------------------------------------------------------------------

describe("Agent — BUG-2 guard: dangling tool_use repair", () => {
  it.concurrent(
    "injects synthetic tool_result before user message when context has a dangling tool_use",
    async () => {
      // Track the messages the mock provider receives so we can verify the
      // context is valid (no dangling tool_use block without a tool_result).
      let capturedMessages: any[] | null = null;

      const mockProvider: StreamProvider = async (params) => {
        capturedMessages = params.messages as any[];
        return makeMockStream(textStreamEvents("All good!"), textMessage("All good!"));
      };

      const { agent, dispose } = await makeTestAgent(mockProvider);
      disposeAll.push(dispose);

      // Simulate a crashed previous turn: inject an assistant message with a
      // dangling tool_use directly into the in-memory context (bypassing the
      // normal sendMessage flow, as would happen after a browser refresh where
      // the old generator was still running when the tool result never arrived).
      const history = (agent as any).compactedContextHistory as any[];
      const hashes = (agent as any).compactedContextHashes as string[];
      history.push({ role: "user", content: "please run git rebase" });
      history.push({
        role: "assistant",
        content: [
          {
            type: "tool_use",
            id: "toolu_dangling_001",
            name: "run_command",
            input: { command: "git rebase --continue" },
          },
        ],
      });
      hashes.push("fake-user-hash");
      hashes.push("fake-assistant-hash");

      // Now send a new message (simulating the user's action after browser refresh).
      const events = await collectEvents(agent, "what happened?");

      // The BUG-2 guard must have emitted a synthetic tool_result event.
      const toolResultEvents = events.filter((e) => e.type === "tool_result") as any[];
      expect(toolResultEvents).toHaveLength(1);
      expect(toolResultEvents[0].id).toBe("toolu_dangling_001");
      expect(toolResultEvents[0].name).toBe("run_command");
      expect(toolResultEvents[0].isError).toBe(true);
      expect(toolResultEvents[0].output).toContain("[not executed:");

      // The synthetic tool_result must appear BEFORE the user_message event.
      const toolResultIdx = events.findIndex((e) => e.type === "tool_result");
      const userMsgIdx = events.findIndex((e) => e.type === "user_message");
      expect(toolResultIdx).toBeGreaterThanOrEqual(0);
      expect(userMsgIdx).toBeGreaterThanOrEqual(0);
      expect(toolResultIdx).toBeLessThan(userMsgIdx);

      // The LLM must have received a valid message sequence: the last three
      // messages should be assistant(tool_use), user(tool_result), user(text).
      expect(capturedMessages).not.toBeNull();
      const msgs = capturedMessages!;
      expect(msgs.length).toBeGreaterThanOrEqual(3);
      const last3 = msgs.slice(-3);

      // assistant message contains the tool_use
      expect(last3[0].role).toBe("assistant");
      const assistantContent = Array.isArray(last3[0].content) ? last3[0].content : [];
      expect(assistantContent.some((b: any) => b.type === "tool_use")).toBe(true);

      // synthetic tool_result message
      expect(last3[1].role).toBe("user");
      const toolResultContent = Array.isArray(last3[1].content) ? last3[1].content : [];
      expect(toolResultContent.some((b: any) => b.type === "tool_result")).toBe(true);

      // new user text message (content may be wrapped by addCacheControlToLastMessage)
      expect(last3[2].role).toBe("user");
      const rawContent = last3[2].content;
      const userText =
        typeof rawContent === "string"
          ? rawContent
          : Array.isArray(rawContent)
            ? (rawContent as any[]).map((b: any) => b.text ?? "").join("")
            : "";
      expect(userText).toBe("what happened?");

      // Turn must complete successfully (no API error).
      const lastEvent = events[events.length - 1];
      expect(lastEvent?.type).toBe("turn_end");
    },
  );

  it.concurrent(
    "does NOT fire guard when context already has a proper tool_result",
    async () => {
      let callCount = 0;
      const mockProvider: StreamProvider = async () => {
        callCount++;
        return makeMockStream(textStreamEvents("done"), textMessage("done"));
      };

      const { agent, dispose } = await makeTestAgent(mockProvider);
      disposeAll.push(dispose);

      // Properly balanced history: tool_use followed by tool_result.
      const history = (agent as any).compactedContextHistory as any[];
      const hashes = (agent as any).compactedContextHashes as string[];
      history.push({ role: "user", content: "run something" });
      history.push({
        role: "assistant",
        content: [{ type: "tool_use", id: "toolu_ok", name: "run_command", input: { command: "ls" } }],
      });
      history.push({
        role: "user",
        content: [{ type: "tool_result", tool_use_id: "toolu_ok", content: "file.txt", is_error: false }],
      });
      hashes.push("h1", "h2", "h3");

      const events = await collectEvents(agent, "next step");

      // No synthetic tool_result should have been emitted — the guard should not fire.
      const syntheticEvents = events.filter((e) => e.type === "tool_result");
      expect(syntheticEvents).toHaveLength(0);

      // Turn completes normally.
      const lastEvent = events[events.length - 1];
      expect(lastEvent?.type).toBe("turn_end");
    },
  );
});

// ---------------------------------------------------------------------------
// Abort during tool execution
// ---------------------------------------------------------------------------

describe("Agent.sendMessage — abort during tool execution", () => {
  it.concurrent(
    "emits tool_result and turn_interrupted(aborted) when abort fires after tool_call but before loop continues",
    async () => {
      // First LLM call returns a tool_use; second would return text (but must
      // never be reached because the abort fires after the tool_call is emitted
      // and before we try to continue the loop with a second LLM call.
      let callCount = 0;
      const mockProvider: StreamProvider = async () => {
        callCount++;
        if (callCount === 1) {
          return makeMockStream(
            toolUseStreamEvents("run_command"),
            // Use a real shell command with a tiny delay so the abort has time
            // to be registered before the tool completes and the check fires.
            toolUseMessage("toolu_abort_test", "run_command", { command: "sleep 0.05" }),
          );
        }
        // Should never reach here
        return makeMockStream(textStreamEvents("should not appear"), textMessage("should not appear"));
      };

      const controller = new AbortController();
      const { agent, dispose } = await makeTestAgent(mockProvider);
      disposeAll.push(dispose);

      const events: (OmegaEvent | StreamSignal)[] = [];
      for await (const ev of agent.sendMessage("run something", async () => true, controller.signal)) {
        events.push(ev);
        // Fire abort immediately after the tool_call is dispatched.
        // At this point the tool is executing (sleep 0.05); the abort will
        // be detected after Promise.all resolves.
        if (ev.type === "tool_call") {
          controller.abort();
        }
      }

      // Should have a tool_call event (tool was dispatched)
      expect(events.some((e) => e.type === "tool_call")).toBe(true);

      // Should have a tool_result event (tool ran and result was recorded)
      expect(events.some((e) => e.type === "tool_result")).toBe(true);

      // Should have turn_interrupted with reason "aborted" — NOT turn_end
      const interrupted = events.find((e) => e.type === "turn_interrupted") as any;
      expect(interrupted).toBeDefined();
      expect(interrupted?.reason).toBe("aborted");
      expect(events.some((e) => e.type === "turn_end")).toBe(false);

      // Second LLM call must NOT have happened (abort fired before loop continued)
      expect(callCount).toBe(1);
    },
  );
});
