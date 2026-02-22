/**
 * Integration tests for the Agent class using a mock provider.
 *
 * These tests cover the full sendMessage loop — streaming, tool dispatch,
 * session persistence, and history management — without hitting the real API.
 *
 * The Agent constructor accepts an optional StreamProvider. Tests inject a
 * mock that returns pre-scripted responses.
 */

import { describe, it, expect, beforeEach, afterEach } from "bun:test";
import { mkdirSync, rmSync, readdirSync } from "fs";
import { join } from "path";
import { tmpdir } from "os";
import type Anthropic from "@anthropic-ai/sdk";

import { Agent, type AgentEvent, type StreamProvider } from "./agent.js";
import { loadLatestSession } from "./session.js";

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

// Tests that use a mock StreamProvider must never write to the real session
// directory (~/.local/share/omega/sessions/). This test proves the contract:
// when a streamProvider is given without a sessionDir, Agent must NOT fall
// back to the real default directory.
describe("Agent — test isolation (no production session pollution)", () => {
  it("does not write to the real session dir when no sessionDir is given", async () => {
    const { homedir } = await import("os");
    const realDir = join(homedir(), ".local", "share", "omega", "sessions");
    const { existsSync } = await import("fs");

    // Snapshot real dir before
    const before = existsSync(realDir) ? readdirSync(realDir).length : 0;

    const mockProvider: StreamProvider = async () =>
      makeMockStream(textStreamEvents("hi"), textMessage("hi"));

    // No sessionDir passed — this is the pollution case
    const agent = new Agent(mockProvider);
    await collectEvents(agent, "should not persist");
    await Bun.sleep(100);

    // Real dir must be unchanged
    const after = existsSync(realDir) ? readdirSync(realDir).length : 0;
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

    // Provider should have been called twice
    expect(calls.length).toBe(2);

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
});

// ---------------------------------------------------------------------------
// Session persistence
// ---------------------------------------------------------------------------

describe("Agent — session persistence", () => {
  it("persists session to disk after a turn", async () => {
    const dir = makeTempDir();
    try {
      const mockProvider: StreamProvider = async () =>
        makeMockStream(textStreamEvents("hi"), textMessage("hi"));

      const agent = new Agent(mockProvider, dir);
      await collectEvents(agent, "hello");

      // Give the fire-and-forget persist a moment to complete
      await Bun.sleep(100);

      const saved = await loadLatestSession(dir);
      expect(saved).not.toBeNull();
      expect(saved!.id).toBe(agent.sessionId);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("saved session contains the full history", async () => {
    const dir = makeTempDir();
    try {
      const mockProvider: StreamProvider = async () =>
        makeMockStream(textStreamEvents("world"), textMessage("world"));

      const agent = new Agent(mockProvider, dir);
      await collectEvents(agent, "hello");
      await Bun.sleep(100);

      const saved = await loadLatestSession(dir);
      expect(saved!.history.length).toBe(2);
      expect(saved!.history[0]).toEqual({ role: "user", content: "hello" });
      expect(saved!.history[1].role).toBe("assistant");
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("overwrites the session file on each turn, not accumulates", async () => {
    const dir = makeTempDir();
    try {
      const mockProvider: StreamProvider = async () =>
        makeMockStream(textStreamEvents("ok"), textMessage("ok"));

      const agent = new Agent(mockProvider, dir);
      await collectEvents(agent, "first");
      await Bun.sleep(100);
      await collectEvents(agent, "second");
      await Bun.sleep(100);

      const saved = await loadLatestSession(dir);
      // 4 messages: user, assistant, user, assistant
      expect(saved!.history.length).toBe(4);

      // Should still be just one file (overwritten, not duplicated)
      const files = readdirSync(dir);
      expect(files.length).toBe(1);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("saved session includes model and savedAt", async () => {
    const dir = makeTempDir();
    try {
      const mockProvider: StreamProvider = async () =>
        makeMockStream(textStreamEvents("ok"), textMessage("ok"));

      const agent = new Agent(mockProvider, dir);
      await collectEvents(agent, "hello");
      await Bun.sleep(100);

      const saved = await loadLatestSession(dir);
      expect(typeof saved!.model).toBe("string");
      expect(saved!.model.length).toBeGreaterThan(0);
      expect(typeof saved!.savedAt).toBe("string");
      expect(new Date(saved!.savedAt).getTime()).toBeGreaterThan(0);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});

// ---------------------------------------------------------------------------
// Session resume
// ---------------------------------------------------------------------------

describe("Agent — session resume", () => {
  it("checkPriorSession returns null when no sessions exist", async () => {
    const dir = makeTempDir();
    try {
      const agent = new Agent(async () => makeMockStream([], textMessage("")), dir);
      const prior = await agent.checkPriorSession();
      expect(prior).toBeNull();
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("checkPriorSession returns a session after one has been saved", async () => {
    const dir = makeTempDir();
    try {
      const mockProvider: StreamProvider = async () =>
        makeMockStream(textStreamEvents("ok"), textMessage("ok"));

      const agent = new Agent(mockProvider, dir);
      await collectEvents(agent, "hello");
      await Bun.sleep(100);

      const agent2 = new Agent(mockProvider, dir);
      const prior = await agent2.checkPriorSession();
      expect(prior).not.toBeNull();
      expect(prior!.id).toBe(agent.sessionId);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("resumeSession restores history so next sendMessage uses it", async () => {
    const dir = makeTempDir();
    try {
      // Agent 1: have a conversation and persist it
      const provider1: StreamProvider = async () =>
        makeMockStream(textStreamEvents("ok"), textMessage("ok"));

      const agent1 = new Agent(provider1, dir);
      await collectEvents(agent1, "first message");
      await Bun.sleep(100);

      // Agent 2: separate provider to capture only its calls.
      // Snapshot params.messages immediately — it's this.history by reference
      // and will be mutated (assistant push) after the call returns.
      const agent2Messages: Anthropic.MessageParam[][] = [];
      const provider2: StreamProvider = async (params) => {
        agent2Messages.push([...params.messages] as Anthropic.MessageParam[]);
        return makeMockStream(textStreamEvents("ok"), textMessage("ok"));
      };

      const agent2 = new Agent(provider2, dir);
      const prior = await agent2.checkPriorSession();
      expect(prior).not.toBeNull();
      expect(prior!.history.length).toBe(2);
      agent2.resumeSession(prior!);

      await collectEvents(agent2, "second message");

      // Agent2 should have made exactly one API call
      expect(agent2Messages.length).toBe(1);
      const sentMessages = agent2Messages[0];

      // Should contain: user:"first message", assistant(from turn1), user:"second message"
      expect(sentMessages.length).toBe(3);
      expect(sentMessages[0]).toEqual({ role: "user", content: "first message" });
      expect(sentMessages[sentMessages.length - 1]).toEqual({
        role: "user",
        content: "second message",
      });
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("resumeSession replaces (not appends to) current history", async () => {
    const dir = makeTempDir();
    try {
      const mockProvider: StreamProvider = async () =>
        makeMockStream(textStreamEvents("ok"), textMessage("ok"));

      const session = {
        id: "prior-session",
        savedAt: new Date().toISOString(),
        model: "claude-sonnet-4-6",
        history: [
          { role: "user" as const, content: "old message" },
          { role: "assistant" as const, content: "old response" },
        ],
      };

      const agent = new Agent(mockProvider, dir);
      agent.resumeSession(session);

      const history = agent.getHistory();
      expect(history.length).toBe(2);
      expect(history[0].content).toBe("old message");
      expect(history[1].content).toBe("old response");
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
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

    // Should have retried and eventually succeeded
    expect(attempts).toBe(3);
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
