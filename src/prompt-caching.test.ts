/**
 * Tests for Anthropic prompt caching implementation.
 *
 * Verifies:
 * 1. cache_control breakpoints are injected into streamParams (system message and last tool)
 * 2. cache_creation_input_tokens / cache_read_input_tokens are extracted from usage
 * 3. TurnMetrics includes cacheCreationTokens and cacheReadTokens
 * 4. Session-level cache totals are tracked
 */

import { describe, it, expect, afterEach } from "bun:test";
import type { OmegaEvent, StreamSignal, CreateMessageStream } from "./agent.js";
import type { TurnMetrics } from "./events.js";
import { makeTestAgent, type TestAgent } from "./test-utils.js";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function makeCreateMessageStream(overrides: {
  cacheCreationTokens?: number;
  cacheReadTokens?: number;
  captureParams?: (p: any) => void;
  captureFirstParams?: (p: any) => void;
}): CreateMessageStream {
  let callCount = 0;
  return (params) => {
    callCount++;
    if (callCount === 1) overrides.captureFirstParams?.(params);
    overrides.captureParams?.(params);
    return {
      async *[Symbol.asyncIterator]() {
        yield { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "hello" } };
      },
      async finalMessage() {
        return {
          id: "msg_test",
          type: "message",
          role: "assistant",
          content: [{ type: "text", text: "hello" }],
          model: "claude-sonnet-4-6",
          stop_reason: "end_turn",
          stop_sequence: null,
          usage: {
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_input_tokens: overrides.cacheCreationTokens ?? 0,
            cache_read_input_tokens: overrides.cacheReadTokens ?? 0,
          },
        } as any;
      },
    };
  };
}

const disposeAll: (() => void)[] = [];
afterEach(() => { disposeAll.splice(0).forEach(d => d()); });

async function runTurn(agent: TestAgent["agent"]): Promise<(OmegaEvent | StreamSignal)[]> {
  const events: (OmegaEvent | StreamSignal)[] = [];
  for await (const event of agent.sendMessage(
    "hello",
    async () => true
  )) {
    events.push(event);
  }
  return events;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("prompt caching — cache_control in streamParams", () => {
  it("prepends billing header block as first system block", async () => {
    let firstParams: any = null;
    const provider = makeCreateMessageStream({ captureFirstParams: (p) => { firstParams = p; } });
    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);
    await agent.init();
    await runTurn(agent);

    const blocks: any[] = firstParams.system;
    expect(blocks.length).toBeGreaterThanOrEqual(2);
    const first = blocks[0];
    // Must be a plain text block (no cache_control)
    expect(first.type).toBe("text");
    expect(first.cache_control).toBeUndefined();
    // Must contain the billing header keyword with all required fields
    expect(first.text).toMatch(/x-anthropic-billing-header:/);
    expect(first.text).toMatch(/cc_version=/);
    expect(first.text).toMatch(/cc_entrypoint=/);
    expect(first.text).toMatch(/cch=/);
  });

  it("injects cache_control on system message blocks", async () => {
    let firstParams: any = null;
    const provider = makeCreateMessageStream({ captureFirstParams: (p) => { firstParams = p; } });
    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);
    await agent.init();
    await runTurn(agent);

    // System should be an array (not a plain string) when caching is enabled
    expect(Array.isArray(firstParams.system)).toBe(true);
    const blocks: any[] = firstParams.system;
    // At least one block must have cache_control
    const hasCacheControl = blocks.some(
      (b: any) => b.cache_control?.type === "ephemeral"
    );
    expect(hasCacheControl).toBe(true);
  });

  it("injects cache_control on the last message in the conversation", async () => {
    let firstParams: any = null;
    const provider = makeCreateMessageStream({ captureFirstParams: (p) => { firstParams = p; } });
    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);
    await agent.init();
    await runTurn(agent);

    // The messages array should have cache_control on the last message's content
    const messages: any[] = firstParams.messages;
    expect(messages.length).toBeGreaterThan(0);
    const lastMsg = messages[messages.length - 1];
    // The last message content should be an array with cache_control on the last block
    // (the helper normalises string content to [{type:"text", text:…}])
    const content = lastMsg.content;
    expect(Array.isArray(content)).toBe(true);
    const lastBlock = content[content.length - 1];
    expect(lastBlock.cache_control?.type).toBe("ephemeral");
  });

  it("injects cache_control on the last tool definition", async () => {
    let firstParams: any = null;
    const provider = makeCreateMessageStream({ captureFirstParams: (p) => { firstParams = p; } });
    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);
    await agent.init();
    await runTurn(agent);

    const tools: any[] = firstParams.tools;
    expect(tools.length).toBeGreaterThan(0);
    const lastTool = tools[tools.length - 1];
    expect(lastTool.cache_control?.type).toBe("ephemeral");
  });
});

describe("prompt caching — cache token extraction", () => {
  it("includes cacheCreationTokens in turn_end metrics", async () => {
    const provider = makeCreateMessageStream({ cacheCreationTokens: 800, cacheReadTokens: 0 });
    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);
    await agent.init();
    const events = await runTurn(agent);

    const turnEnd = events.find((e) => e.type === "turn_end") as any;
    expect(turnEnd).toBeDefined();
    expect(turnEnd.metrics.cacheCreationTokens).toBe(800);
    expect(turnEnd.metrics.cacheReadTokens).toBe(0);
  });

  it("includes cacheReadTokens in turn_end metrics", async () => {
    const provider = makeCreateMessageStream({ cacheCreationTokens: 0, cacheReadTokens: 500 });
    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);
    await agent.init();
    const events = await runTurn(agent);

    const turnEnd = events.find((e) => e.type === "turn_end") as any;
    expect(turnEnd).toBeDefined();
    expect(turnEnd.metrics.cacheCreationTokens).toBe(0);
    expect(turnEnd.metrics.cacheReadTokens).toBe(500);
  });

  it("session totals accumulate cacheCreationTokens across turns", async () => {
    const provider = makeCreateMessageStream({ cacheCreationTokens: 300, cacheReadTokens: 0 });
    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);
    await agent.init();
    await runTurn(agent);
    await runTurn(agent);

    expect((agent as any).sessionCacheCreationTokens).toBe(600);
  });

  it("session totals accumulate cacheReadTokens across turns", async () => {
    const provider = makeCreateMessageStream({ cacheCreationTokens: 0, cacheReadTokens: 200 });
    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);
    await agent.init();
    await runTurn(agent);
    await runTurn(agent);

    expect((agent as any).sessionCacheReadTokens).toBe(400);
  });
});


