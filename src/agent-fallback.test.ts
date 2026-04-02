import { describe, it, expect, afterEach } from "bun:test";
import { type StreamProvider, type OmegaEvent, type StreamSignal } from "./agent.js";
import { makeTestAgent, type TestAgent } from "./test-utils.js";

function makeRateLimitError() {
  return { status: 429, message: "rate limit" } as any;
}

async function collectEvents(agent: TestAgent["agent"], message: string): Promise<(OmegaEvent | StreamSignal)[]> {
  const events: (OmegaEvent | StreamSignal)[] = [];
  for await (const event of agent.sendMessage(message, async () => true)) {
    events.push(event);
  }
  return events;
}

const disposeAll: (() => void)[] = [];
afterEach(() => { disposeAll.splice(0).forEach(d => d()); });

describe("Agent fallback", () => {
  it("emits api_error on rate limit", async () => {
    const mockProvider: StreamProvider = () => {
      throw makeRateLimitError();
    };

    process.env.OMEGA_RETRY_BASE_MS = "1";
    process.env.OMEGA_RETRY_MAX_MS = "2";
    process.env.OMEGA_RETRY_ATTEMPTS = "2";

    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "hello");

    const apiError = events.find((e) => e.type === "llm_error") as any;
    expect(apiError).toBeTruthy();
    expect(apiError.url).toBe("https://api.anthropic.com/v1/messages");
    expect(apiError.error).toContain("rate limit");

    delete process.env.OMEGA_RETRY_BASE_MS;
    delete process.env.OMEGA_RETRY_MAX_MS;
    delete process.env.OMEGA_RETRY_ATTEMPTS;
  });
});
