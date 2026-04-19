/**
 * Integration tests for the pause / resume / interject state machine
 * (UX-1/UX-2 replacement).
 *
 * Every test drives `Agent.sendMessage` through a mocked provider and
 * exercises `requestPause`, `requestContinue`, and `requestAbort` at
 * specific points in the event stream. The emitted event log is asserted
 * against the scenarios in the plan (`backlog/pause-resume-interject.md`
 * \u00a7 "Log scenarios").
 *
 * The tests iterate the generator manually so we can inject control calls
 * at precise seam moments. For the manual-resume path we call
 * `requestContinue` via `setTimeout` so that the generator has a chance
 * to reach its `await new Promise(...)` suspend and set `pausedResolver`
 * \u2014 that's what makes the continued event's `mode` "manual" rather than
 * "auto".
 */

import { describe, it, expect, afterEach } from "bun:test";
import { readFile } from "node:fs/promises";
import type Anthropic from "@anthropic-ai/sdk";
import type { BetaRawMessageStreamEvent } from "@anthropic-ai/sdk/resources/beta/messages/messages.js";

import { Agent, type OmegaEvent, type StreamSignal, type CreateMessageStream } from "./agent.js";
import { parseOmegaEvent } from "./events.schema.js";
import { makeTestAgent } from "./test-utils.js";

const disposeAll: (() => void)[] = [];
afterEach(() => { disposeAll.splice(0).forEach(d => d()); });

// ---------------------------------------------------------------------------
// Mock provider helpers (mirrored from agent-integration.test.ts to keep the
// pause test file self-contained; these are small fixtures, not worth extracting).
// ---------------------------------------------------------------------------

function makeMockStream(events: BetaRawMessageStreamEvent[], message: Anthropic.Beta.Messages.BetaMessage) {
  return {
    async *[Symbol.asyncIterator]() { for (const e of events) yield e; },
    finalMessage: async () => message,
  };
}

function textMessage(text: string): Anthropic.Beta.Messages.BetaMessage {
  return {
    id: "msg_test",
    type: "message",
    role: "assistant",
    model: "claude-sonnet-4-6",
    container: null,
    content: [{ type: "text", text, citations: null }],
    stop_reason: "end_turn",
    stop_sequence: null,
    stop_details: null,
    context_management: null,
    usage: { input_tokens: 10, output_tokens: 5, cache_creation: null, cache_creation_input_tokens: null, cache_read_input_tokens: null, inference_geo: null, iterations: null, server_tool_use: null, service_tier: null, speed: null },
  };
}

function textStreamEvents(text: string): BetaRawMessageStreamEvent[] {
  return [
    { type: "content_block_start", index: 0, content_block: { type: "text", text: "", citations: null } },
    { type: "content_block_delta", index: 0, delta: { type: "text_delta", text } },
    { type: "content_block_stop", index: 0 },
    { type: "message_delta", context_management: null, delta: { stop_reason: "end_turn", stop_sequence: null, stop_details: null, container: null }, usage: { output_tokens: 5, cache_creation_input_tokens: null, cache_read_input_tokens: null, input_tokens: null, iterations: null, server_tool_use: null } },
    { type: "message_stop" },
  ];
}

function toolUseMessage(
  toolId: string,
  toolName: string,
  toolInput: unknown,
): Anthropic.Beta.Messages.BetaMessage {
  return {
    id: "msg_tool",
    type: "message",
    role: "assistant",
    model: "claude-sonnet-4-6",
    container: null,
    content: [{ type: "tool_use", id: toolId, name: toolName, input: toolInput, caller: { type: "direct" } }],
    stop_reason: "tool_use",
    stop_sequence: null,
    stop_details: null,
    context_management: null,
    usage: { input_tokens: 20, output_tokens: 10, cache_creation: null, cache_creation_input_tokens: null, cache_read_input_tokens: null, inference_geo: null, iterations: null, server_tool_use: null, service_tier: null, speed: null },
  };
}

function toolUseStreamEvents(toolName: string, toolId = "t1"): BetaRawMessageStreamEvent[] {
  return [
    { type: "content_block_start", index: 0, content_block: { type: "tool_use", id: toolId, name: toolName, input: {} } },
    { type: "content_block_delta", index: 0, delta: { type: "input_json_delta", partial_json: "{}" } },
    { type: "content_block_stop", index: 0 },
    { type: "message_delta", context_management: null, delta: { stop_reason: "tool_use", stop_sequence: null, stop_details: null, container: null }, usage: { output_tokens: 10, cache_creation_input_tokens: null, cache_read_input_tokens: null, input_tokens: null, iterations: null, server_tool_use: null } },
    { type: "message_stop" },
  ];
}

/**
 * Provider that returns `read_file` on call 1 and plain text on call 2.
 * Produces a single clean pause seam after the tool_result.
 */
function readFileThenTextProvider(): CreateMessageStream {
  let call = 0;
  return () => {
    call++;
    if (call === 1) {
      return makeMockStream(
        toolUseStreamEvents("read_file"),
        toolUseMessage("t1", "read_file", { path: "src/config.ts" }),
      );
    }
    return makeMockStream(textStreamEvents("Done"), textMessage("Done"));
  };
}

/** Read events.jsonl and return parsed events in file order. */
async function readEventLog(eventsFile: string): Promise<OmegaEvent[]> {
  const text = await readFile(eventsFile, "utf8");
  const events: OmegaEvent[] = [];
  for (const line of text.split("\n")) {
    if (line.length === 0) continue;
    const parsed = parseOmegaEvent(JSON.parse(line));
    if (!parsed.success) {
      throw new Error(`Failed to parse log line: ${line} \u2014 ${parsed.error.message}`);
    }
    events.push(parsed.data);
  }
  return events;
}

/** Wait a tick so the generator can reach an `await` suspension. */
function tick(ms = 10): Promise<void> {
  return new Promise(resolve => setTimeout(resolve, ms));
}

// ---------------------------------------------------------------------------
// Scenario 1: Pause \u2192 Paused \u2192 Continue (no interjection, manual mode)
// ---------------------------------------------------------------------------

describe("Agent pause/resume \u2014 manual continue", () => {
  it("emits turn_paused after tool_result, suspends, then turn_continued with mode=\"manual\"", async () => {
    const { agent, eventsFile, dispose } = await makeTestAgent(readFileThenTextProvider());
    disposeAll.push(dispose);

    const events: (OmegaEvent | StreamSignal)[] = [];
    let pauseCalled = false;

    for await (const e of agent.sendMessage("read config", async () => true)) {
      events.push(e);
      if (e.type === "tool_result" && !pauseCalled) {
        pauseCalled = true;
        agent.requestPause();
      } else if (e.type === "turn_paused") {
        // Let the generator reach `await new Promise(...)` so pausedResolver
        // is set \u2014 then mode="manual" when we continue.
        setTimeout(() => agent.requestContinue(), 5);
      }
    }

    const types = events.map(e => e.type);
    // Expected shape: user_message, llm_call, llm_response, tool_call,
    // tool_result, turn_paused, [suspend], turn_continued, llm_call,
    // text (streamed), llm_response (settled), turn_end.
    // Text yields are streamed from content_block_delta events during the
    // LLM stream; llm_response is the settled summary after finalMessage().
    expect(types).toEqual([
      "user_message",
      "llm_call",
      "llm_response",
      "tool_call",
      "tool_result",
      "turn_paused",
      "turn_continued",
      "llm_call",
      "text",
      "llm_response",
      "turn_end",
    ]);

    const cont = events.find(e => e.type === "turn_continued");
    expect(cont).toBeDefined();
    expect((cont as Extract<OmegaEvent, { type: "turn_continued" }>).mode).toBe("manual");

    // pause_requested is fired from outside the generator so it's not
    // yielded \u2014 it must still be in the log.
    const logged = await readEventLog(eventsFile);
    const loggedTypes = logged.map(e => e.type);
    expect(loggedTypes).toContain("pause_requested");
    // And both seam events are persisted.
    expect(loggedTypes).toContain("turn_paused");
    expect(loggedTypes).toContain("turn_continued");

    // No interjection was given \u2014 history must not contain a mid-turn user msg.
    // Expected: user, assistant(tool_use), user(tool_result), assistant(text).
    const history = agent.getCompactedContextHistory();
    expect(history.length).toBe(4);
    expect(history[0]).toEqual({ role: "user", content: "read config" });
  });
});

// ---------------------------------------------------------------------------
// Scenario 2: Pause + interjection delivers user_message before continue
// ---------------------------------------------------------------------------

describe("Agent pause/resume \u2014 interjection", () => {
  it("appends user_message between turn_paused and turn_continued and feeds it to the next LLM call", async () => {
    let call = 0;
    const seenMessages: Anthropic.Beta.Messages.BetaMessageParam[][] = [];
    const provider: CreateMessageStream = (params) => {
      seenMessages.push([...params.messages]);
      call++;
      if (call === 1) {
        return makeMockStream(
          toolUseStreamEvents("read_file"),
          toolUseMessage("t1", "read_file", { path: "src/config.ts" }),
        );
      }
      return makeMockStream(textStreamEvents("got it"), textMessage("got it"));
    };

    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);

    const events: (OmegaEvent | StreamSignal)[] = [];
    let pauseCalled = false;
    const interjection = "also look at src/agent.ts";

    for await (const e of agent.sendMessage("read config", async () => true)) {
      events.push(e);
      if (e.type === "tool_result" && !pauseCalled) {
        pauseCalled = true;
        agent.requestPause();
      } else if (e.type === "turn_paused") {
        setTimeout(() => agent.requestContinue(interjection), 5);
      }
    }

    // Between turn_paused and turn_continued we must see a user_message
    // carrying the interjection.
    const pausedIdx = events.findIndex(e => e.type === "turn_paused");
    const contIdx = events.findIndex(e => e.type === "turn_continued");
    expect(pausedIdx).toBeGreaterThanOrEqual(0);
    expect(contIdx).toBeGreaterThan(pausedIdx);
    const between = events.slice(pausedIdx + 1, contIdx);
    const interject = between.find(e => e.type === "user_message");
    expect(interject).toBeDefined();
    expect((interject as Extract<OmegaEvent, { type: "user_message" }>).content).toBe(interjection);

    expect((events[contIdx] as Extract<OmegaEvent, { type: "turn_continued" }>).mode).toBe("manual");

    // The second LLM call must have seen the interjection in history.
    // addCacheControlToLastMessage wraps the last message's content as an
    // array of blocks, so accept both string and block-array shapes.
    expect(seenMessages.length).toBeGreaterThanOrEqual(2);
    const lastMessages = seenMessages[seenMessages.length - 1]!;
    const containsInterjection = (content: unknown): boolean => {
      if (typeof content === "string") return content === interjection;
      if (Array.isArray(content)) {
        return content.some(
          block => typeof block === "object" && block !== null && "text" in block && (block as { text?: unknown }).text === interjection,
        );
      }
      return false;
    };
    const hasInterjection = lastMessages.some(
      m => m.role === "user" && containsInterjection(m.content),
    );
    expect(hasInterjection).toBe(true);

    // Also: the agent's in-memory history must contain the interjection as a
    // separate user message (not merged into an existing one).
    const history = agent.getCompactedContextHistory();
    const interjectionInHistory = history.some(
      m => m.role === "user" && containsInterjection(m.content),
    );
    expect(interjectionInHistory).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// Scenario 3: Pre-commit \u2014 continue before the seam fires \u2192 mode="auto"
// ---------------------------------------------------------------------------

describe("Agent pause/resume \u2014 pre-commit", () => {
  it("fires turn_continued with mode=\"auto\" when continue lands before the seam", async () => {
    const { agent, dispose } = await makeTestAgent(readFileThenTextProvider());
    disposeAll.push(dispose);

    const events: (OmegaEvent | StreamSignal)[] = [];
    let pauseCalled = false;

    for await (const e of agent.sendMessage("read config", async () => true)) {
      events.push(e);
      if (e.type === "tool_result" && !pauseCalled) {
        pauseCalled = true;
        // Both calls happen synchronously before the generator reaches the
        // seam. At continue-time, pausedResolver is null \u2192 mode="auto".
        agent.requestPause();
        agent.requestContinue();
      }
    }

    const cont = events.find(e => e.type === "turn_continued");
    expect(cont).toBeDefined();
    expect((cont as Extract<OmegaEvent, { type: "turn_continued" }>).mode).toBe("auto");

    // turn_paused is still emitted: the log always shows both bookends.
    expect(events.map(e => e.type)).toContain("turn_paused");
  });
});

// ---------------------------------------------------------------------------
// Scenario 4: Abort while pause is pending (before seam) \u2192
//             turn_interrupted{aborted}, no turn_paused
// ---------------------------------------------------------------------------

describe("Agent pause/resume \u2014 abort from PauseRequested", () => {
  it("fires turn_interrupted with no turn_paused when abort lands before the seam", async () => {
    const { agent, eventsFile, dispose } = await makeTestAgent(readFileThenTextProvider());
    disposeAll.push(dispose);

    const events: (OmegaEvent | StreamSignal)[] = [];
    let fired = false;
    for await (const e of agent.sendMessage("read config", async () => true)) {
      events.push(e);
      if (e.type === "tool_result" && !fired) {
        fired = true;
        // Fire pause first so pause_requested gets logged, then abort
        // before the generator advances to the seam.
        agent.requestPause();
        agent.requestAbort();
      }
    }

    const types = events.map(e => e.type);
    expect(types).toContain("turn_interrupted");
    expect(types).not.toContain("turn_paused");
    expect(types).not.toContain("turn_continued");

    const interrupt = events.find(e => e.type === "turn_interrupted");
    expect((interrupt as Extract<OmegaEvent, { type: "turn_interrupted" }>).reason).toBe("aborted");

    // pause_requested was still logged. logEvent writes are fire-and-forget
    // queued through a private logQueue; wait a tick so the append lands
    // before reading the file.
    await tick(25);
    const logged = await readEventLog(eventsFile);
    expect(logged.map(e => e.type)).toContain("pause_requested");
    expect(logged.map(e => e.type)).toContain("turn_interrupted");
  });
});

// ---------------------------------------------------------------------------
// Scenario 5: Abort while Paused (after seam, during suspension) \u2192
//             turn_paused then turn_interrupted{aborted}, no turn_continued
// ---------------------------------------------------------------------------

describe("Agent pause/resume \u2014 abort from Paused", () => {
  it("fires turn_paused then turn_interrupted when abort lands during suspension", async () => {
    const { agent, dispose } = await makeTestAgent(readFileThenTextProvider());
    disposeAll.push(dispose);

    const events: (OmegaEvent | StreamSignal)[] = [];
    let pauseCalled = false;
    for await (const e of agent.sendMessage("read config", async () => true)) {
      events.push(e);
      if (e.type === "tool_result" && !pauseCalled) {
        pauseCalled = true;
        agent.requestPause();
      } else if (e.type === "turn_paused") {
        // Generator reaches await suspend, then we abort.
        setTimeout(() => agent.requestAbort(), 5);
      }
    }

    const types = events.map(e => e.type);
    const pausedIdx = types.indexOf("turn_paused");
    const interruptIdx = types.indexOf("turn_interrupted");
    expect(pausedIdx).toBeGreaterThanOrEqual(0);
    expect(interruptIdx).toBeGreaterThan(pausedIdx);
    expect(types).not.toContain("turn_continued");

    const interrupt = events[interruptIdx] as Extract<OmegaEvent, { type: "turn_interrupted" }>;
    expect(interrupt.reason).toBe("aborted");
  });
});

// ---------------------------------------------------------------------------
// Scenario 6: Concurrent tools \u2014 pause waits for the whole batch
// ---------------------------------------------------------------------------

describe("Agent pause/resume \u2014 concurrent tools", () => {
  it("waits for all tool_results in a batch before firing turn_paused", async () => {
    // Two tool_use blocks in one response, then a plain text continuation.
    let call = 0;
    const twoToolMessage: Anthropic.Beta.Messages.BetaMessage = {
      id: "msg_two",
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
      stop_details: null,
      context_management: null,
      usage: { input_tokens: 20, output_tokens: 10, cache_creation: null, cache_creation_input_tokens: null, cache_read_input_tokens: null, inference_geo: null, iterations: null, server_tool_use: null, service_tier: null, speed: null },
    };
    const twoToolEvents: BetaRawMessageStreamEvent[] = [
      { type: "content_block_start", index: 0, content_block: { type: "tool_use", id: "tA", name: "list_files", input: {} } },
      { type: "content_block_delta", index: 0, delta: { type: "input_json_delta", partial_json: "{}" } },
      { type: "content_block_stop", index: 0 },
      { type: "content_block_start", index: 1, content_block: { type: "tool_use", id: "tB", name: "list_files", input: {} } },
      { type: "content_block_delta", index: 1, delta: { type: "input_json_delta", partial_json: "{}" } },
      { type: "content_block_stop", index: 1 },
      { type: "message_delta", context_management: null, delta: { stop_reason: "tool_use", stop_sequence: null, stop_details: null, container: null }, usage: { output_tokens: 10, cache_creation_input_tokens: null, cache_read_input_tokens: null, input_tokens: null, iterations: null, server_tool_use: null } },
      { type: "message_stop" },
    ];
    const provider: CreateMessageStream = () => {
      call++;
      if (call === 1) return makeMockStream(twoToolEvents, twoToolMessage);
      return makeMockStream(textStreamEvents("done"), textMessage("done"));
    };

    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);

    const events: (OmegaEvent | StreamSignal)[] = [];
    let firstResultSeen = false;
    for await (const e of agent.sendMessage("list stuff", async () => true)) {
      events.push(e);
      if (e.type === "tool_result" && !firstResultSeen) {
        // Pause after the FIRST tool_result \u2014 the seam must wait for the
        // SECOND tool_result before emitting turn_paused.
        firstResultSeen = true;
        agent.requestPause();
      } else if (e.type === "turn_paused") {
        setTimeout(() => agent.requestContinue(), 5);
      }
    }

    // Find positions of each event type.
    const types = events.map(e => e.type);
    const toolResultIdxs: number[] = [];
    types.forEach((t, i) => { if (t === "tool_result") toolResultIdxs.push(i); });
    const pausedIdx = types.indexOf("turn_paused");

    expect(toolResultIdxs.length).toBe(2);
    expect(pausedIdx).toBeGreaterThan(toolResultIdxs[1]!);
  });
});

// ---------------------------------------------------------------------------
// Scenario 7: No pause requested \u2014 agent runs to completion unchanged
// ---------------------------------------------------------------------------

describe("Agent pause/resume \u2014 no-op baseline", () => {
  it("emits no pause events and completes normally when nothing is requested", async () => {
    const { agent, dispose } = await makeTestAgent(readFileThenTextProvider());
    disposeAll.push(dispose);

    const events: (OmegaEvent | StreamSignal)[] = [];
    for await (const e of agent.sendMessage("read config", async () => true)) {
      events.push(e);
    }

    const types = events.map(e => e.type);
    expect(types).not.toContain("pause_requested");
    expect(types).not.toContain("turn_paused");
    expect(types).not.toContain("turn_continued");
    expect(types[types.length - 1]).toBe("turn_end");
  });
});

// Satisfy ts-unused-locals import when only one tick-like helper is kept.
void tick;
