import { describe, it, expect, afterEach } from "bun:test";
import { Agent, type OmegaEvent, type StreamSignal } from "./agent.js";
import type { StreamProvider } from "./agent.js";
import { makeTestAgent } from "./test-utils.js";


function rateLimitError(message = "rate limit: try again in 0.01s") {
  const err: any = new Error(message);
  err.status = 429;
  return err;
}

function overloadError() {
  const err: any = new Error("Overloaded");
  err.status = 529;
  err.error = { type: "error", error: { type: "overloaded_error", message: "Overloaded" } };
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

// ---------------------------------------------------------------------------
// Basic backoff — gives up when OMEGA_RETRY_ATTEMPTS is set
// ---------------------------------------------------------------------------

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
    // Turn ends with turn_interrupted(reason=error) so streaming flag resets
    const last = events[events.length - 1] as any;
    expect(last.type).toBe("turn_interrupted");
    expect(last.reason).toBe("error");

    delete process.env.OMEGA_RETRY_BASE_MS;
    delete process.env.OMEGA_RETRY_MAX_MS;
    delete process.env.OMEGA_RETRY_ATTEMPTS;
  });
});

// ---------------------------------------------------------------------------
// Indefinite retry — succeeds after N failures (no OMEGA_RETRY_ATTEMPTS cap)
// ---------------------------------------------------------------------------

describe("overload (529) — indefinite retry", () => {
  /** Build a minimal successful stream mock (text-only, no tool use). */
  function makeSuccessProvider(): StreamProvider {
    return async () => ({
      async *[Symbol.asyncIterator]() {
        yield { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } };
        yield { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "done" } };
        yield { type: "content_block_stop", index: 0 };
        yield { type: "message_delta", delta: { stop_reason: "end_turn" }, usage: { output_tokens: 1 } };
        yield { type: "message_stop" };
      },
      finalMessage: async () => ({
        id: "msg_ok",
        type: "message",
        role: "assistant",
        model: "claude-sonnet-4-6",
        content: [{ type: "text", text: "done" }],
        stop_reason: "end_turn",
        stop_sequence: null,
        usage: { input_tokens: 10, output_tokens: 1 },
      }),
    });
  }

  it("retries indefinitely until success — no cap needed", async () => {
    process.env.OMEGA_RETRY_BASE_MS = "1";
    process.env.OMEGA_RETRY_MAX_MS = "2";
    // OMEGA_RETRY_ATTEMPTS is deliberately NOT set — production default (infinite).

    let callCount = 0;
    const failTimes = 3; // fail 3 times, then succeed

    const successStream = makeSuccessProvider();
    const mockProvider: StreamProvider = async (params) => {
      callCount++;
      if (callCount <= failTimes) throw overloadError();
      return successStream(params);
    };

    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "hello");

    // Should have made exactly failTimes+1 calls
    expect(callCount).toBe(failTimes + 1);

    // Three llm_retry events emitted (one per failure)
    const retries = events.filter(e => e.type === "llm_retry") as any[];
    expect(retries.length).toBe(failTimes);
    // Attempt numbers are 1-based and sequential
    expect(retries[0].attempt).toBe(1);
    expect(retries[1].attempt).toBe(2);
    expect(retries[2].attempt).toBe(3);

    // Turn ended cleanly (turn_end, not turn_interrupted)
    const last = events[events.length - 1] as any;
    expect(last.type).toBe("turn_end");

    delete process.env.OMEGA_RETRY_BASE_MS;
    delete process.env.OMEGA_RETRY_MAX_MS;
  });

  it("llm_retry event carries retryAt, httpStatus, and errorBody", async () => {
    process.env.OMEGA_RETRY_BASE_MS = "1";
    process.env.OMEGA_RETRY_MAX_MS = "5";
    process.env.OMEGA_RETRY_ATTEMPTS = "2"; // cap so test terminates quickly

    const mockProvider: StreamProvider = async () => {
      throw overloadError();
    };

    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "hello");

    const retries = events.filter(e => e.type === "llm_retry") as any[];
    expect(retries.length).toBeGreaterThanOrEqual(1);
    const retry = retries[0];

    // httpStatus
    expect(retry.httpStatus).toBe(529);
    // retryAt: must be a non-empty ISO string after the event's own time
    expect(typeof retry.retryAt).toBe("string");
    expect(new Date(retry.retryAt).getTime()).toBeGreaterThanOrEqual(
      new Date(retry.time).getTime(),
    );
    // errorBody: the structured body extracted from the SDK error
    expect(retry.errorBody).toBeDefined();
    expect((retry.errorBody as any).type).toBe("error");
    // waitMs is positive
    expect(retry.waitMs).toBeGreaterThan(0);

    delete process.env.OMEGA_RETRY_BASE_MS;
    delete process.env.OMEGA_RETRY_MAX_MS;
    delete process.env.OMEGA_RETRY_ATTEMPTS;
  });

  it("retry event round-trips through events.jsonl", async () => {
    const { readFileSync } = await import("fs");
    process.env.OMEGA_RETRY_BASE_MS = "1";
    process.env.OMEGA_RETRY_MAX_MS = "2";
    process.env.OMEGA_RETRY_ATTEMPTS = "2";

    const mockProvider: StreamProvider = async () => {
      throw overloadError();
    };

    const { agent, eventsFile, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    await collectEvents(agent, "hello");

    const lines = readFileSync(eventsFile, "utf-8")
      .split("\n")
      .filter(l => l.trim());
    const retryLine = lines.find(l => {
      try { return JSON.parse(l).type === "llm_retry"; } catch { return false; }
    });
    expect(retryLine).toBeDefined();
    const parsed = JSON.parse(retryLine!);
    expect(parsed.httpStatus).toBe(529);
    expect(typeof parsed.retryAt).toBe("string");
    expect(parsed.errorBody).toBeDefined();

    delete process.env.OMEGA_RETRY_BASE_MS;
    delete process.env.OMEGA_RETRY_MAX_MS;
    delete process.env.OMEGA_RETRY_ATTEMPTS;
  });
});

// ---------------------------------------------------------------------------
// Mid-stream retry: error thrown while iterating the stream (not before it)
// ---------------------------------------------------------------------------

describe("mid-stream retry", () => {
  /** Build a provider that throws mid-stream on the first N calls. */
  function makePartialThenSuccessProvider(failCount: number): StreamProvider {
    let calls = 0;

    return async () => {
      const attempt = ++calls;
      if (attempt <= failCount) {
        // Emit a partial thinking delta, then throw
        return {
          async *[Symbol.asyncIterator]() {
            yield { type: "content_block_start", index: 0, content_block: { type: "thinking", thinking: "" } };
            yield { type: "content_block_delta", index: 0, delta: { type: "thinking_delta", thinking: "partial…" } };
            // Simulate 529 mid-stream (the Anthropic SDK would throw here)
            throw overloadError();
          },
          finalMessage: async () => { throw overloadError(); },
        };
      }

      // Success
      return {
        async *[Symbol.asyncIterator]() {
          yield { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } };
          yield { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "done" } };
          yield { type: "content_block_stop", index: 0 };
          yield { type: "message_delta", delta: { stop_reason: "end_turn" }, usage: { output_tokens: 1 } };
          yield { type: "message_stop" };
        },
        finalMessage: async () => ({
          id: "msg_ok",
          type: "message",
          role: "assistant",
          model: "claude-sonnet-4-6",
          content: [{ type: "text", text: "done" }],
          stop_reason: "end_turn",
          stop_sequence: null,
          usage: { input_tokens: 10, output_tokens: 1 },
        }),
      };
    };
  }

  it("recovers cleanly when 529 fires mid-thinking-stream", async () => {
    process.env.OMEGA_RETRY_BASE_MS = "1";
    process.env.OMEGA_RETRY_MAX_MS = "2";

    const mockProvider = makePartialThenSuccessProvider(1); // fail once, succeed second

    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "hello");

    // One retry event emitted
    const retries = events.filter(e => e.type === "llm_retry");
    expect(retries.length).toBe(1);

    // Turn ended cleanly
    const last = events[events.length - 1] as any;
    expect(last.type).toBe("turn_end");

    // The thinking signals from the aborted stream AND the successful retry
    // are both in the event list (both are StreamSignals, which accumulate
    // but are cleared in the UI on llm_retry).  The llm_response.text
    // contains only the final successful response.
    const llmResponse = events.find(e => e.type === "llm_response") as any;
    expect(llmResponse).toBeDefined();
    expect(llmResponse.text).toBe("done");

    delete process.env.OMEGA_RETRY_BASE_MS;
    delete process.env.OMEGA_RETRY_MAX_MS;
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
    expect(errorEvents.some(e => e.error.includes("Context too large"))).toBe(true);
    // Turn ends with turn_interrupted(reason=error)
    const last = events[events.length - 1] as any;
    expect(last.type).toBe("turn_interrupted");
    expect(last.reason).toBe("error");
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
    // Turn ends with turn_interrupted(reason=error)
    const last = events[events.length - 1] as any;
    expect(last.type).toBe("turn_interrupted");
    expect(last.reason).toBe("error");
  });
});
