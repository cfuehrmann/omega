import { describe, it, expect } from "bun:test";
import { Agent, type StreamProvider, type AgentEvent } from "./agent.js";

function makeRateLimitError() {
  return { status: 429, message: "rate limit" } as any;
}

async function collectEvents(agent: Agent, message: string): Promise<AgentEvent[]> {
  const events: AgentEvent[] = [];
  for await (const event of agent.sendMessage(message, async () => true)) {
    events.push(event);
  }
  return events;
}

describe("Agent fallback", () => {
  it("emits api_error on rate limit", async () => {
    process.env.OPENAI_API_KEY = "test-key";

    const mockProvider: StreamProvider = async () => {
      throw makeRateLimitError();
    };

    process.env.OMEGA_RETRY_BASE_MS = "1";
    process.env.OMEGA_RETRY_MAX_MS = "2";
    process.env.OMEGA_RETRY_ATTEMPTS = "2";

    const agent = new Agent(mockProvider, null);
    const events = await collectEvents(agent, "hello");

    const apiError = events.find((e) => e.type === "api_error") as any;
    expect(apiError).toBeTruthy();
    expect(apiError.provider).toBe("anthropic");
    expect(apiError.url).toBe("https://api.anthropic.com/v1/messages");
    expect(apiError.error).toContain("rate limit");

    delete process.env.OMEGA_RETRY_BASE_MS;
    delete process.env.OMEGA_RETRY_MAX_MS;
    delete process.env.OMEGA_RETRY_ATTEMPTS;
  });
});
