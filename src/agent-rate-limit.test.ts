import { describe, it, expect } from "bun:test";
import { Agent, type AgentEvent } from "./agent.js";
import type { StreamProvider } from "./agent.js";

function rateLimitError(message = "rate limit: try again in 0.01s") {
  const err: any = new Error(message);
  err.status = 429;
  return err;
}

async function collectEvents(agent: Agent, message: string, signal?: AbortSignal): Promise<AgentEvent[]> {
  const events: AgentEvent[] = [];
  for await (const event of agent.sendMessage(message, async () => true, signal)) {
    events.push(event);
  }
  return events;
}

describe("rate limit backoff", () => {
  it("OpenAI retries on rate limit and succeeds", async () => {
    process.env.OMEGA_RETRY_BASE_MS = "1";
    process.env.OMEGA_RETRY_MAX_MS = "2";
    process.env.OMEGA_RETRY_ATTEMPTS = "3";

    let openAiCalls = 0;
    const openAiCaller = async () => {
      openAiCalls += 1;
      if (openAiCalls < 3) throw rateLimitError("Please try again in 0.01s");
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

    const agent = new Agent(undefined, null, openAiCaller as any);
    agent.setProvider("openai");
    const events = await collectEvents(agent, "hello");

    expect(openAiCalls).toBe(3);
    const errors = events.filter((e) => e.type === "error") as any[];
    if (errors.length > 0) {
      expect(errors[errors.length - 1].error).not.toContain("/opus");
    }

    delete process.env.OMEGA_RETRY_BASE_MS;
    delete process.env.OMEGA_RETRY_MAX_MS;
    delete process.env.OMEGA_RETRY_ATTEMPTS;
  });

  it("OpenAI gives up after retries and suggests /opus", async () => {
    process.env.OMEGA_RETRY_BASE_MS = "1";
    process.env.OMEGA_RETRY_MAX_MS = "2";
    process.env.OMEGA_RETRY_ATTEMPTS = "2";

    const openAiCaller = async () => {
      throw rateLimitError("Please try again in 0.01s");
    };

    const agent = new Agent(undefined, null, openAiCaller as any);
    agent.setProvider("openai");
    const events = await collectEvents(agent, "hello");

    const errors = events.filter((e) => e.type === "error") as any[];
    const error = errors[errors.length - 1];
    expect(error).toBeTruthy();
    expect(error.error).toContain("/opus");

    delete process.env.OMEGA_RETRY_BASE_MS;
    delete process.env.OMEGA_RETRY_MAX_MS;
    delete process.env.OMEGA_RETRY_ATTEMPTS;
  });

  it("Anthropic gives up after retries and suggests /gpt", async () => {
    process.env.OMEGA_RETRY_BASE_MS = "1";
    process.env.OMEGA_RETRY_MAX_MS = "2";
    process.env.OMEGA_RETRY_ATTEMPTS = "2";

    const mockProvider: StreamProvider = async () => {
      throw rateLimitError();
    };

    const agent = new Agent(mockProvider, null);
    agent.setProvider("anthropic");
    const events = await collectEvents(agent, "hello");

    const errors = events.filter((e) => e.type === "error") as any[];
    const error = errors[errors.length - 1];
    expect(error).toBeTruthy();
    expect(error.error).toContain("/gpt");

    delete process.env.OMEGA_RETRY_BASE_MS;
    delete process.env.OMEGA_RETRY_MAX_MS;
    delete process.env.OMEGA_RETRY_ATTEMPTS;
  });
});
