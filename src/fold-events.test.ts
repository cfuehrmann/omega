/**
 * Tests for structured events emitted by foldCurrentSessionIntoWorldState.
 *
 * When the world-state fold runs (on shutdown), it should emit the same
 * structured AgentEvents as a regular turn — api_call_start, api_response,
 * and a tool_result for the file write — so the UI can render them visibly.
 */

import { describe, it, expect, beforeEach, afterEach } from "bun:test";
import { mkdtemp, rm } from "fs/promises";
import { tmpdir } from "os";
import { join } from "path";
import type { StreamProvider, AgentEvent } from "./agent.js";
import { Agent } from "./agent.js";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

let tempDir: string;

beforeEach(async () => {
  tempDir = await mkdtemp(join(tmpdir(), "omega-fold-events-test-"));
});

afterEach(async () => {
  await rm(tempDir, { recursive: true, force: true });
});

function makeMockStream(responseText: string) {
  return {
    async *[Symbol.asyncIterator]() {
      yield { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } };
      yield { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: responseText } };
      yield { type: "content_block_stop", index: 0 };
      yield { type: "message_delta", delta: { stop_reason: "end_turn" }, usage: { output_tokens: 5 } };
      yield { type: "message_stop" };
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
  };
}

function makeMockProvider(responseText = "new world state"): StreamProvider {
  return async () => makeMockStream(responseText);
}

async function collectSendMessage(agent: Agent, message: string) {
  const events: AgentEvent[] = [];
  for await (const event of agent.sendMessage(message, async () => true)) {
    events.push(event);
  }
  return events;
}

async function collectFold(agent: Agent): Promise<AgentEvent[]> {
  const events: AgentEvent[] = [];
  for await (const event of agent.foldCurrentSessionIntoWorldState()) {
    events.push(event);
  }
  return events;
}

// ---------------------------------------------------------------------------
// foldCurrentSessionIntoWorldState is an async generator
// ---------------------------------------------------------------------------

describe("foldCurrentSessionIntoWorldState — structured events", () => {
  it("is an async generator (returns AsyncGenerator)", () => {
    const agent = new Agent(makeMockProvider(), null, undefined, null);
    const result = agent.foldCurrentSessionIntoWorldState();
    // AsyncGenerators have Symbol.asyncIterator
    expect(typeof result[Symbol.asyncIterator]).toBe("function");
    // Clean up (drain so no leak)
    result.return(undefined);
  });

  it("emits no events when worldStatePath is null", async () => {
    const agent = new Agent(makeMockProvider(), null, undefined, null);
    const events = await collectFold(agent);
    expect(events).toHaveLength(0);
  });

  it("emits no events when history is empty", async () => {
    const worldStatePath = join(tempDir, "world-state.md");
    const agent = new Agent(makeMockProvider(), null, undefined, worldStatePath);
    // Do NOT send any message — history is empty
    const events = await collectFold(agent);
    expect(events).toHaveLength(0);
  });

  it("emits api_call_start event", async () => {
    const worldStatePath = join(tempDir, "world-state.md");
    const agent = new Agent(makeMockProvider(), null, undefined, worldStatePath);
    await collectSendMessage(agent, "hello");

    const events = await collectFold(agent);
    const apiCallStart = events.find(e => e.type === "api_call_start");
    expect(apiCallStart).toBeDefined();
    expect((apiCallStart as any).provider).toBe("anthropic");
  });

  it("emits api_response event with token usage", async () => {
    const worldStatePath = join(tempDir, "world-state.md");
    const agent = new Agent(makeMockProvider(), null, undefined, worldStatePath);
    await collectSendMessage(agent, "hello");

    const events = await collectFold(agent);
    const apiResponse = events.find(e => e.type === "api_response");
    expect(apiResponse).toBeDefined();
    expect((apiResponse as any).usage).toBeDefined();
    expect(typeof (apiResponse as any).usage.input_tokens).toBe("number");
    expect(typeof (apiResponse as any).usage.output_tokens).toBe("number");
  });

  it("emits world_state_saved event for the file write (not tool_result)", async () => {
    const worldStatePath = join(tempDir, "world-state.md");
    const agent = new Agent(makeMockProvider(), null, undefined, worldStatePath);
    await collectSendMessage(agent, "hello");

    const events = await collectFold(agent);

    // Must have a dedicated world_state_saved event
    const saved = events.find(e => e.type === "world_state_saved");
    expect(saved).toBeDefined();
    expect((saved as any).path).toBe(worldStatePath);
    expect(typeof (saved as any).charCount).toBe("number");

    // Must NOT use the generic tool_result event type for this
    const toolResult = events.find(e => e.type === "tool_result");
    expect(toolResult).toBeUndefined();
  });

  it("still writes the world state file to disk", async () => {
    const worldStatePath = join(tempDir, "world-state.md");
    const agent = new Agent(makeMockProvider("the new state"), null, undefined, worldStatePath);
    await collectSendMessage(agent, "hello");

    await collectFold(agent);

    const file = Bun.file(worldStatePath);
    const exists = await file.exists();
    expect(exists).toBe(true);
    const content = await file.text();
    expect(content).toContain("the new state");
  });

  it("emits an error event (not throw) on LLM failure", async () => {
    const worldStatePath = join(tempDir, "world-state.md");
    const failProvider: StreamProvider = async () => {
      throw new Error("LLM exploded");
    };
    const agent = new Agent(failProvider, null, undefined, worldStatePath);
    await collectSendMessage(agent, "hello");

    const events = await collectFold(agent);
    const errorEvent = events.find(e => e.type === "error");
    expect(errorEvent).toBeDefined();
    expect((errorEvent as any).error).toContain("LLM exploded");
  });

  it("retries once on transient stream error ('Unexpected event order') during fold and succeeds", async () => {
    const worldStatePath = join(tempDir, "world-state.md");

    // The main turn always succeeds; only fold calls (2nd+ provider calls) are flaky on first try
    let foldCallCount = 0;
    let mainTurnDone = false;
    const flakyProvider: StreamProvider = async () => {
      if (!mainTurnDone) {
        // Main turn call — succeed normally
        return makeMockStream("main turn response");
      }
      // Fold calls
      foldCallCount++;
      if (foldCallCount === 1) {
        // Simulate the transient Anthropic SDK error on first fold attempt
        throw new Error("Unexpected event order, got message_start before receiving \"message_stop\"");
      }
      // Second fold call succeeds
      return makeMockStream("recovered world state");
    };
    const agent = new Agent(flakyProvider, null, undefined, worldStatePath);
    await collectSendMessage(agent, "hello");
    // Give compactAfterTurn a moment to finish (it also calls the provider)
    await new Promise(r => setTimeout(r, 50));
    mainTurnDone = true;

    const events = await collectFold(agent);

    // Should have retried and succeeded — no error event
    const errorEvent = events.find(e => e.type === "error");
    expect(errorEvent).toBeUndefined();

    // Should have saved the world state from the retry
    const saved = events.find(e => e.type === "world_state_saved");
    expect(saved).toBeDefined();

    const content = await Bun.file(worldStatePath).text();
    expect(content).toContain("recovered world state");
  });

  it("compactTurn uses the active model (e.g. opus) not hardcoded sonnet", async () => {
    // This tests that when /opus is active, turn compaction calls the provider
    // with model=claude-opus-4-6, not the hardcoded claude-sonnet-4-6.
    const modelsUsed: string[] = [];
    const capturingProvider: StreamProvider = async (params) => {
      modelsUsed.push(params.model);
      return makeMockStream("summary text");
    };

    const agent = new Agent(capturingProvider, null, undefined, null);
    // Switch to Opus
    for await (const _ of agent.sendMessage("/opus", async () => true)) { /* drain */ }
    // Reset captured models (the /opus command doesn't call the provider)
    modelsUsed.length = 0;

    // Send a real message — this triggers a turn + compactAfterTurn
    for await (const _ of agent.sendMessage("hello", async () => true)) { /* drain */ }

    // Give compactAfterTurn (fire-and-forget) a moment to run
    await new Promise(r => setTimeout(r, 50));

    // The main turn call uses opus; compaction should also use opus
    expect(modelsUsed.every(m => m === "claude-opus-4-6")).toBe(true);
    expect(modelsUsed.length).toBeGreaterThanOrEqual(2); // at least 1 main call + 1 compaction
  });

  it("uses the OpenAI caller for fold when OpenAI is the active provider", async () => {
    const worldStatePath = join(tempDir, "world-state.md");

    // Anthropic stream provider that must NOT be called during fold
    let anthropicCalled = false;
    const anthropicProvider: StreamProvider = async () => {
      anthropicCalled = true;
      return makeMockStream("anthropic response");
    };

    // OpenAI caller that MUST be called during fold
    let openAiCalled = false;
    const mockOpenAiCaller: any = async () => {
      openAiCalled = true;
      return {
        text: "openai world state",
        response: {
          content: [{ type: "text", text: "openai world state" }],
          stop_reason: "end_turn",
          usage: { input_tokens: 8, output_tokens: 3 },
        },
        raw: {},
      };
    };

    const agent = new Agent(anthropicProvider, null, mockOpenAiCaller, worldStatePath);
    // Switch to OpenAI provider before fold
    await collectSendMessage(agent, "/codex");
    // Send a real message so history is non-empty (sendMessage with /codex won't add to history)
    await collectSendMessage(agent, "hello");
    // Reset flags after sendMessage (the Anthropic provider may have been called during sendMessage for the hello turn)
    anthropicCalled = false;

    const events = await collectFold(agent);

    expect(openAiCalled).toBe(true);
    expect(anthropicCalled).toBe(false);

    // The saved world state should come from OpenAI
    const saved = events.find(e => e.type === "world_state_saved");
    expect(saved).toBeDefined();

    // The api_call_start event should identify OpenAI as the provider
    const apiCallStart = events.find(e => e.type === "api_call_start");
    expect((apiCallStart as any).provider).toBe("openai");
  });
});
