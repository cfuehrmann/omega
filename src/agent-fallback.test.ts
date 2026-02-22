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
    });

    const agent = new Agent(mockProvider, null, openAiCaller as any);
    const events = await collectEvents(agent, "hello");

    const apiError = events.find((e) => e.type === "api_error") as any;
    expect(apiError).toBeTruthy();
    expect(apiError.model).toBe("claude-sonnet-4-6");
    expect(apiError.error).toContain("rate limit");
  });
});
