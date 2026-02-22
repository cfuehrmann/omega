import { describe, it, expect } from "bun:test";
import type Anthropic from "@anthropic-ai/sdk";
import { Agent, type StreamProvider, type AgentEvent } from "./agent.js";

function makeRateLimitError() {
  return { status: 429, message: "rate limit" } as any;
}

function makeMockStream(events: any[], message: Anthropic.Message) {
  return {
    async *[Symbol.asyncIterator]() {
      for (const e of events) yield e;
    },
    finalMessage: async () => message,
  };
}

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

async function collectEvents(agent: Agent, message: string): Promise<AgentEvent[]> {
  const events: AgentEvent[] = [];
  for await (const event of agent.sendMessage(message, async () => true)) {
    events.push(event);
  }
  return events;
}

describe("Agent fallback", () => {
  it("emits api_error before falling back", async () => {
    process.env.OPENAI_API_KEY = "test-key";

    const mockProvider: StreamProvider = async () => {
      throw makeRateLimitError();
    };

    const openAiCaller = async () => ({
      response: {
        content: [{ type: "text", text: "hi" } as any],
        stop_reason: "stop",
        usage: { input_tokens: 1, output_tokens: 2 },
      },
      text: "hi",
      raw: { usage: { input_tokens: 1, output_tokens: 2 } },
    });

    const agent = new Agent(mockProvider, null, openAiCaller as any);
    const events = await collectEvents(agent, "hello");

    const apiError = events.find((e) => e.type === "api_error") as any;
    expect(apiError).toBeTruthy();
    expect(apiError.provider).toBe("anthropic");
    expect(apiError.url).toBe("https://api.anthropic.com/v1/messages");
    expect(apiError.error).toContain("rate limit");
  });

  it("sticks to fallback for subsequent calls", async () => {
    process.env.OPENAI_API_KEY = "test-key";

    let providerCalls = 0;
    let openAiCalls = 0;

    const mockProvider: StreamProvider = async () => {
      providerCalls += 1;
      throw makeRateLimitError();
    };

    const openAiCaller = async () => {
      openAiCalls += 1;
      return {
        response: {
          content: [{ type: "text", text: "ok" } as any],
          stop_reason: "stop",
          usage: { input_tokens: 1, output_tokens: 2 },
        },
        text: "ok",
        raw: { usage: { input_tokens: 1, output_tokens: 2 } },
      };
    };

    const agent = new Agent(mockProvider, null, openAiCaller as any);
    await collectEvents(agent, "first");
    expect(providerCalls).toBe(1);
    expect(openAiCalls).toBe(1);

    await collectEvents(agent, "second");
    expect(providerCalls).toBe(1); // no more Anthropic calls
    expect(openAiCalls).toBe(2);
  });

  it("emits fallback api_call_start with provider/url/request", async () => {
    process.env.OPENAI_API_KEY = "test-key";

    const mockProvider: StreamProvider = async () => {
      throw makeRateLimitError();
    };

    const openAiCaller = async () => ({
      response: {
        content: [{ type: "text", text: "ok" } as any],
        stop_reason: "stop",
        usage: { input_tokens: 1, output_tokens: 2 },
      },
      text: "ok",
    });

    const agent = new Agent(mockProvider, null, openAiCaller as any);
    const events = await collectEvents(agent, "hello");

    const fallbackCall = events.find(
      (e) => e.type === "api_call_start" && (e as any).provider === "openai"
    ) as any;
    expect(fallbackCall).toBeTruthy();
    expect(fallbackCall.url).toContain("/v1/responses");
    expect(fallbackCall.request).toBeTruthy();
  });
});
