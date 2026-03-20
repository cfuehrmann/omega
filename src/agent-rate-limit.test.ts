import { describe, it, expect, afterEach } from "bun:test";
import { Agent, type OmegaEvent, type StreamSignal } from "./agent.js";
import type { StreamProvider } from "./agent.js";
import { makeTestAgent } from "./test-utils.js";


function authExpiredError() {
  const err: any = new Error('401 {"type":"error","error":{"type":"authentication_error","message":"OAuth token has expired."}}');
  err.status = 401;
  return err;
}

function rateLimitError(message = "rate limit: try again in 0.01s") {
  const err: any = new Error(message);
  err.status = 429;
  return err;
}

async function collectEvents(agent: Agent, message: string, signal?: AbortSignal): Promise<(OmegaEvent | StreamSignal)[]> {
  const events: (OmegaEvent | StreamSignal)[] = [];
  for await (const event of agent.sendMessage(message, async () => true, signal)) {
    events.push(event);
  }
  return events;
}

const disposeAll: (() => void)[] = [];
afterEach(() => { disposeAll.splice(0).forEach(d => d()); });

describe("rate limit backoff", () => {
  it("Anthropic gives up after retries and emits agent_error", async () => {
    process.env.OMEGA_RETRY_BASE_MS = "1";
    process.env.OMEGA_RETRY_MAX_MS = "2";
    process.env.OMEGA_RETRY_ATTEMPTS = "2";

    const mockProvider: StreamProvider = async () => {
      throw rateLimitError();
    };

    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "hello");

    const errors = events.filter((e) => e.type === "agent_error") as any[];
    const error = errors[errors.length - 1];
    expect(error).toBeTruthy();
    expect(error.error).toContain("rate limit");

    delete process.env.OMEGA_RETRY_BASE_MS;
    delete process.env.OMEGA_RETRY_MAX_MS;
    delete process.env.OMEGA_RETRY_ATTEMPTS;
  });
});

describe("OAuth token expiry reauth", () => {
  it("retries after 401 when reinitAuth succeeds", async () => {
    let llmCallCount = 0;
    let hadAuthError = false;
    let hadSuccess = false;

    const mockProvider: StreamProvider = async () => {
      llmCallCount += 1;
      if (llmCallCount === 1) {
        hadAuthError = true;
        throw authExpiredError();
      }
      hadSuccess = true;
      return {
        async *[Symbol.asyncIterator]() {
          yield { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "ok" } };
        },
        async finalMessage() {
          return {
            id: "msg_ok",
            type: "message",
            role: "assistant",
            content: [{ type: "text", text: "ok" }],
            model: "claude-sonnet-4-6",
            stop_reason: "end_turn",
            stop_sequence: null,
            usage: { input_tokens: 5, output_tokens: 2 },
          } as any;
        },
      };
    };

    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    // Override reinitAuth so it doesn't touch the real token file
    (agent as any).reinitAuth = async () => {
      (agent as any).authMode = "oauth";
      return true;
    };

    const events = await collectEvents(agent, "hello");
    expect(hadAuthError).toBe(true);
    expect(hadSuccess).toBe(true);
    const texts = events.filter(e => e.type === "text").map((e: any) => e.text);
    expect(texts.join("")).toContain("ok");
    const errors = events.filter(e => e.type === "agent_error") as any[];
    expect(errors).toHaveLength(0);
  });

  it("reports login required when reinitAuth fails", async () => {
    const mockProvider: StreamProvider = async () => {
      throw authExpiredError();
    };

    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    (agent as any).reinitAuth = async () => false;

    const events = await collectEvents(agent, "hello");
    const errors = events.filter(e => e.type === "agent_error") as any[];
    expect(errors.length).toBeGreaterThan(0);
    const lastError = errors[errors.length - 1];
    expect(lastError.error).toContain("login");
  });
});

// ---------------------------------------------------------------------------
// Context overflow: errors out immediately, no retry
// ---------------------------------------------------------------------------

describe("context overflow (prompt too long)", () => {
  function promptTooLongError() {
    const err: any = new Error('400 {"type":"error","error":{"type":"invalid_request_error","message":"prompt is too long: 250000 tokens"}}');
    err.status = 400;
    return err;
  }

  it("emits llm_error + actionable agent_error — no retry", async () => {
    let callCount = 0;

    const mockProvider: StreamProvider = async () => {
      callCount++;
      throw promptTooLongError();
    };

    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "hello");

    expect(callCount).toBe(1);

    const llmErrors = events.filter(e => e.type === "llm_error");
    expect(llmErrors.length).toBe(1);

    const errorEvents = events.filter(e => e.type === "agent_error") as any[];
    expect(errorEvents.length).toBeGreaterThanOrEqual(1);
    expect(errorEvents.some(e => e.error.includes("compact"))).toBe(true);
  });

  it("also errors out cleanly for isContextTooLong (429 extra usage required)", async () => {
    function contextTooLongError() {
      const err: any = new Error("429 Extra usage is required for long context requests");
      err.status = 429;
      return err;
    }

    const mockProvider: StreamProvider = async () => {
      throw contextTooLongError();
    };

    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "hello");

    const errorEvents = events.filter(e => e.type === "agent_error") as any[];
    expect(errorEvents.length).toBeGreaterThanOrEqual(1);
  });
});
