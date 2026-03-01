/**
 * Tests for Anthropic prompt caching implementation.
 *
 * Verifies:
 * 1. cache_control breakpoints are injected into streamParams (system message and last tool)
 * 2. cache_creation_input_tokens / cache_read_input_tokens are extracted from usage
 * 3. estimateCostWithCache is used for cost accounting (not estimateCost)
 * 4. TurnMetrics includes cacheCreationTokens and cacheReadTokens
 * 5. Session-level cache totals are tracked
 */

import { describe, it, expect, afterEach } from "bun:test";
import type { OmegaEvent, StreamSignal, TurnMetrics, StreamProvider } from "./agent.js";
import { makeTestAgent } from "./test-utils.js";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function makeStreamProvider(overrides: {
  cacheCreationTokens?: number;
  cacheReadTokens?: number;
  captureParams?: (p: any) => void;
  captureFirstParams?: (p: any) => void;
}): StreamProvider {
  let callCount = 0;
  return async (params) => {
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

async function runTurn(agent: ReturnType<typeof makeTestAgent>["agent"]): Promise<(OmegaEvent | StreamSignal)[]> {
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
  it("injects cache_control on system message blocks", async () => {
    let firstParams: any = null;
    const provider = makeStreamProvider({ captureFirstParams: (p) => { firstParams = p; } });
    const { agent, dispose } = makeTestAgent(provider);
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
    const provider = makeStreamProvider({ captureFirstParams: (p) => { firstParams = p; } });
    const { agent, dispose } = makeTestAgent(provider);
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
    const provider = makeStreamProvider({ captureFirstParams: (p) => { firstParams = p; } });
    const { agent, dispose } = makeTestAgent(provider);
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
    const provider = makeStreamProvider({ cacheCreationTokens: 800, cacheReadTokens: 0 });
    const { agent, dispose } = makeTestAgent(provider);
    disposeAll.push(dispose);
    await agent.init();
    const events = await runTurn(agent);

    const turnEnd = events.find((e) => e.type === "turn_end") as any;
    expect(turnEnd).toBeDefined();
    expect(turnEnd.metrics.cacheCreationTokens).toBe(800);
    expect(turnEnd.metrics.cacheReadTokens).toBe(0);
  });

  it("includes cacheReadTokens in turn_end metrics", async () => {
    const provider = makeStreamProvider({ cacheCreationTokens: 0, cacheReadTokens: 500 });
    const { agent, dispose } = makeTestAgent(provider);
    disposeAll.push(dispose);
    await agent.init();
    const events = await runTurn(agent);

    const turnEnd = events.find((e) => e.type === "turn_end") as any;
    expect(turnEnd).toBeDefined();
    expect(turnEnd.metrics.cacheCreationTokens).toBe(0);
    expect(turnEnd.metrics.cacheReadTokens).toBe(500);
  });

  it("session totals accumulate cacheCreationTokens across turns", async () => {
    const provider = makeStreamProvider({ cacheCreationTokens: 300, cacheReadTokens: 0 });
    const { agent, dispose } = makeTestAgent(provider);
    disposeAll.push(dispose);
    await agent.init();
    await runTurn(agent);
    await runTurn(agent);

    expect((agent as any).sessionCacheCreationTokens).toBe(600);
  });

  it("session totals accumulate cacheReadTokens across turns", async () => {
    const provider = makeStreamProvider({ cacheCreationTokens: 0, cacheReadTokens: 200 });
    const { agent, dispose } = makeTestAgent(provider);
    disposeAll.push(dispose);
    await agent.init();
    await runTurn(agent);
    await runTurn(agent);

    expect((agent as any).sessionCacheReadTokens).toBe(400);
  });
});

describe("prompt caching — cost accounting", () => {
  it("uses cache-aware cost when cache tokens present", async () => {
    // Sonnet: $3 input/M, $15 output/M
    // cache write: 1.25× = $3.75/M, cache read: 0.1× = $0.30/M
    // input=100, output=50, cacheCreation=800, cacheRead=0
    // = (100*3 + 50*15 + 800*3.75) / 1_000_000
    // = (300 + 750 + 3000) / 1_000_000 = 0.004050
    const provider = makeStreamProvider({ cacheCreationTokens: 800, cacheReadTokens: 0 });
    const { agent, dispose } = makeTestAgent(provider);
    disposeAll.push(dispose);
    await agent.init();
    const events = await runTurn(agent);

    const turnEnd = events.find((e) => e.type === "turn_end") as any;
    expect(turnEnd.metrics.costUsd).toBeCloseTo(0.00405, 6);
  });

  it("cache read tokens are cheaper than equivalent input tokens", async () => {
    // Scenario: 600 total prompt tokens
    // Without caching: 600 input tokens, 50 output
    //   cost = (600*3 + 50*15) / 1M = (1800+750)/1M = 0.00255
    // With caching: 100 non-cached input + 500 cache read, 50 output
    //   cost = (100*3 + 50*15 + 500*0.3) / 1M = (300+750+150)/1M = 0.00120
    //   Cache reads are 0.1x input rate = cheaper than full input rate
    const makeProvider = (inputTokens: number, cacheReadTokens: number): StreamProvider => {
      return async (params) => {
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
                input_tokens: inputTokens,
                output_tokens: 50,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: cacheReadTokens,
              },
            } as any;
          },
        };
      };
    };

    const { agent: agentWithCache, dispose: d1 } = makeTestAgent(makeProvider(100, 500));
    const { agent: agentNoCache, dispose: d2 } = makeTestAgent(makeProvider(600, 0));
    disposeAll.push(d1, d2);
    await agentWithCache.init();
    await agentNoCache.init();

    const eventsWithCache = await runTurn(agentWithCache);
    const eventsNoCache = await runTurn(agentNoCache);

    const costWithCache = (eventsWithCache.find((e) => e.type === "turn_end") as any).metrics.costUsd;
    const costNoCache = (eventsNoCache.find((e) => e.type === "turn_end") as any).metrics.costUsd;

    // 0.00120 < 0.00255
    expect(costWithCache).toBeLessThan(costNoCache);
  });
});

describe("prompt caching — savings math identity", () => {
  // The critical identity: cost + saved === hypothetical cost without caching.
  // This validates that the savings calculation is correct and consistent
  // with the cost calculation, not inflated or double-counted.

  it("cost + saved === hypothetical no-cache cost (Sonnet, pure cache read)", async () => {
    // Scenario: 500 uncached input + 40000 cache-read + 1000 output, 0 cache write
    // Sonnet: $3/M input, $15/M output
    // Actual cost = (500×3 + 1000×15 + 40000×3×0.1) / 1M = (1500+15000+12000)/1M = 0.0285
    // Savings = 40000 × 3 × 0.9 / 1M = 108000/1M = 0.108
    // Hypothetical (no caching) = (500+40000)×3/M + 1000×15/M = 121500+15000 = 136500/1M = 0.1365
    // Check: 0.0285 + 0.108 = 0.1365 ✅
    const { estimateCostWithCache, estimateCacheSavings, estimateCost } = await import("./agent.js");

    const model = "claude-sonnet-4-6";
    const inputTokens = 500;
    const outputTokens = 1000;
    const cacheCreation = 0;
    const cacheRead = 40000;

    const actualCost = estimateCostWithCache(model, inputTokens, outputTokens, cacheCreation, cacheRead);
    const saved = estimateCacheSavings(model, cacheRead);
    const hypotheticalNoCacheCost = estimateCost(model, inputTokens + cacheCreation + cacheRead, outputTokens);

    expect(actualCost).toBeCloseTo(0.0285, 6);
    expect(saved).toBeCloseTo(0.108, 6);
    expect(actualCost + saved).toBeCloseTo(hypotheticalNoCacheCost, 6);
  });

  it("cost + saved === hypothetical no-cache cost (Opus)", async () => {
    const { estimateCostWithCache, estimateCacheSavings, estimateCost } = await import("./agent.js");

    const model = "claude-opus-4-6";
    const inputTokens = 300;
    const outputTokens = 800;
    const cacheCreation = 0;
    const cacheRead = 50000;

    const actualCost = estimateCostWithCache(model, inputTokens, outputTokens, cacheCreation, cacheRead);
    const saved = estimateCacheSavings(model, cacheRead);
    const hypotheticalNoCacheCost = estimateCost(model, inputTokens + cacheCreation + cacheRead, outputTokens);

    // Opus: $5/M input, $25/M output
    // Actual = (300×5 + 800×25 + 50000×5×0.1)/1M = (1500+20000+25000)/1M = 0.0465
    // Saved = 50000×5×0.9/1M = 225000/1M = 0.225
    // Hypothetical = (50300×5 + 800×25)/1M = (251500+20000)/1M = 0.2715
    expect(actualCost).toBeCloseTo(0.0465, 6);
    expect(saved).toBeCloseTo(0.225, 6);
    expect(actualCost + saved).toBeCloseTo(hypotheticalNoCacheCost, 6);
  });

  it("savings can legitimately exceed cost when cache read tokens dominate", async () => {
    // This validates that "implausibly high savings" is expected behavior.
    // With heavy caching (typical in long sessions), savings > cost is normal
    // because you're paying 0.1× instead of 1.0× for most input tokens.
    const { estimateCostWithCache, estimateCacheSavings } = await import("./agent.js");

    const model = "claude-sonnet-4-6";
    // Realistic late-session turn: small uncached part, huge cache read
    const inputTokens = 200;       // new user message tokens
    const outputTokens = 500;      // model response
    const cacheCreation = 0;       // nothing new cached
    const cacheRead = 80000;       // system + tools + history from cache

    const cost = estimateCostWithCache(model, inputTokens, outputTokens, cacheCreation, cacheRead);
    const saved = estimateCacheSavings(model, cacheRead);

    // cost = (200×3 + 500×15 + 80000×0.3)/1M = (600+7500+24000)/1M = 0.0321
    // saved = 80000×3×0.9/1M = 216000/1M = 0.216
    // Ratio: saved/cost ≈ 6.7×
    expect(cost).toBeCloseTo(0.0321, 4);
    expect(saved).toBeCloseTo(0.216, 4);
    expect(saved).toBeGreaterThan(cost * 5); // savings > 5× cost is normal
  });

  it("cache write turns have zero savings (no double-counting)", async () => {
    const { estimateCostWithCache, estimateCacheSavings, estimateCost } = await import("./agent.js");

    const model = "claude-sonnet-4-6";
    const inputTokens = 100;
    const outputTokens = 50;
    const cacheCreation = 5000;
    const cacheRead = 0;

    const cost = estimateCostWithCache(model, inputTokens, outputTokens, cacheCreation, cacheRead);
    const saved = estimateCacheSavings(model, cacheRead);

    // No cache reads → no savings. Cache writes actually cost MORE than base.
    expect(saved).toBe(0);
    // Cache write cost > equivalent uncached cost (1.25× vs 1.0×)
    const uncachedCost = estimateCost(model, inputTokens + cacheCreation, outputTokens);
    expect(cost).toBeGreaterThan(uncachedCost);
  });
});
