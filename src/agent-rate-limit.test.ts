import { describe, it, expect, afterEach } from "bun:test";
import { Agent, type OmegaEvent, type StreamSignal } from "./agent.js";
import type { CreateMessageStream } from "./agent.js";
import type { BetaRawMessageStreamEvent } from "@anthropic-ai/sdk/resources/beta/messages/messages.js";
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

/**
 * Simulates the error shape the Anthropic SDK throws when an overload arrives
 * as an SSE stream 'error' event (HTTP 200 body) rather than as an HTTP 529.
 * In that case streaming.js:63 calls:
 *   new APIError(undefined, parsedBody, undefined, headers)
 * so .status is undefined and .message is JSON.stringify(parsedBody).
 * Bug: this was not retried — session 2026-04-01T16-02-14-529-87454cef.
 */
function sseOverloadError() {
  const body = { type: "error", error: { details: null, type: "overloaded_error", message: "Overloaded" }, request_id: "req_test" };
  const err: any = new Error(JSON.stringify(body));
  // status is intentionally absent (undefined) — this is the bug-triggering shape
  err.error = body;
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

    const mockProvider: CreateMessageStream = () => {
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
  function makeSuccessProvider(): CreateMessageStream {
    return () => ({
      async *[Symbol.asyncIterator](): AsyncGenerator<BetaRawMessageStreamEvent> {
        yield { type: "content_block_start", index: 0, content_block: { type: "text", text: "", citations: null } };
        yield { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "done" } };
        yield { type: "content_block_stop", index: 0 };
        yield { type: "message_delta", context_management: null, delta: { stop_reason: "end_turn", stop_sequence: null, stop_details: null, container: null }, usage: { output_tokens: 1, cache_creation_input_tokens: null, cache_read_input_tokens: null, input_tokens: null, iterations: null, server_tool_use: null } };
        yield { type: "message_stop" };
      },
      finalMessage: async () => ({
        id: "msg_ok",
        type: "message",
        role: "assistant",
        model: "claude-sonnet-4-6",
        container: null,
        context_management: null,
        content: [{ type: "text", text: "done", citations: null }],
        stop_reason: "end_turn",
        stop_sequence: null,
        usage: { input_tokens: 10, output_tokens: 1 },
      } as any),
    });
  }

  it("retries indefinitely until success — no cap needed", async () => {
    process.env.OMEGA_RETRY_BASE_MS = "1";
    process.env.OMEGA_RETRY_MAX_MS = "2";
    // OMEGA_RETRY_ATTEMPTS is deliberately NOT set — production default (infinite).

    let callCount = 0;
    const failTimes = 3; // fail 3 times, then succeed

    const successStream = makeSuccessProvider();
    const mockProvider: CreateMessageStream = (params) => {
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

  it("SSE-stream overload (status=undefined) retries and eventually succeeds", async () => {
    // Regression test for session 2026-04-01T16-02-14-529-87454cef.
    // The SDK throws APIError(undefined, body) for SSE stream error events;
    // isRetryable must recognise "overloaded_error" in the body even without a
    // numeric .status.
    process.env.OMEGA_RETRY_BASE_MS = "1";
    process.env.OMEGA_RETRY_MAX_MS = "2";
    // No OMEGA_RETRY_ATTEMPTS cap — production default (infinite).

    let callCount = 0;
    const failTimes = 2;
    const successStream = makeSuccessProvider();
    const mockProvider: CreateMessageStream = (params) => {
      callCount++;
      if (callCount <= failTimes) throw sseOverloadError();
      return successStream(params);
    };

    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "hello");

    expect(callCount).toBe(failTimes + 1);

    const retries = events.filter(e => e.type === "llm_retry") as any[];
    expect(retries.length).toBe(failTimes);

    // httpStatus is undefined for this error shape
    expect(retries[0].httpStatus).toBeUndefined();

    // Turn ends cleanly
    const last = events[events.length - 1] as any;
    expect(last.type).toBe("turn_end");

    delete process.env.OMEGA_RETRY_BASE_MS;
    delete process.env.OMEGA_RETRY_MAX_MS;
  });

  it("llm_retry event carries retryAt, httpStatus, and errorBody", async () => {
    process.env.OMEGA_RETRY_BASE_MS = "1";
    process.env.OMEGA_RETRY_MAX_MS = "5";
    process.env.OMEGA_RETRY_ATTEMPTS = "2"; // cap so test terminates quickly

    const mockProvider: CreateMessageStream = () => {
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

    const mockProvider: CreateMessageStream = () => {
      throw overloadError();
    };

    const { agent, eventsFile, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    await collectEvents(agent, "hello");
    await agent.flushEventLog();

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
  const THINKING_BEFORE_ERROR = "Let me think carefully about this problem…";
  const TEXT_BEFORE_ERROR     = "I was about to say something when";

  /**
   * Build a provider that emits partial content then throws on the first N
   * calls, then succeeds. The partial content is configurable so tests can
   * verify both thinking and text fragments independently.
   */
  function makePartialThenSuccessProvider(
    failCount: number,
    opts: { thinkingBeforeError?: string; textBeforeError?: string } = {},
  ): CreateMessageStream {
    let calls = 0;

    return () => {
      const attempt = ++calls;
      if (attempt <= failCount) {
        const events: BetaRawMessageStreamEvent[] = [];

        if (opts.thinkingBeforeError) {
          events.push(
            { type: "content_block_start", index: 0, content_block: { type: "thinking", thinking: "", signature: "" } },
            { type: "content_block_delta", index: 0, delta: { type: "thinking_delta", thinking: opts.thinkingBeforeError } },
          );
        }
        if (opts.textBeforeError) {
          const idx = opts.thinkingBeforeError ? 1 : 0;
          events.push(
            { type: "content_block_start", index: idx, content_block: { type: "text", text: "", citations: null } },
            { type: "content_block_delta", index: idx, delta: { type: "text_delta", text: opts.textBeforeError } },
          );
        }

        return {
          async *[Symbol.asyncIterator](): AsyncGenerator<BetaRawMessageStreamEvent> {
            for (const e of events) yield e;
            throw overloadError();
          },
          finalMessage: async () => { throw overloadError(); },
        };
      }

      // Success
      return {
        async *[Symbol.asyncIterator](): AsyncGenerator<BetaRawMessageStreamEvent> {
          yield { type: "content_block_start", index: 0, content_block: { type: "text", text: "", citations: null } };
          yield { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "done" } };
          yield { type: "content_block_stop", index: 0 };
          yield { type: "message_delta", context_management: null, delta: { stop_reason: "end_turn", stop_sequence: null, stop_details: null, container: null }, usage: { output_tokens: 1, cache_creation_input_tokens: null, cache_read_input_tokens: null, input_tokens: null, iterations: null, server_tool_use: null } };
          yield { type: "message_stop" };
        },
        finalMessage: async () => ({
          id: "msg_ok",
          type: "message",
          role: "assistant",
          model: "claude-sonnet-4-6",
          container: null,
          context_management: null,
          content: [{ type: "text", text: "done", citations: null }],
          stop_reason: "end_turn",
          stop_sequence: null,
          usage: { input_tokens: 10, output_tokens: 1 },
        } as any),
      };
    };
  }

  it("thinkingFragment captured when 529 fires mid-thinking-stream", async () => {
    process.env.OMEGA_RETRY_BASE_MS = "1";
    process.env.OMEGA_RETRY_MAX_MS = "2";

    const provider = makePartialThenSuccessProvider(1, {
      thinkingBeforeError: THINKING_BEFORE_ERROR,
    });
    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "hello");

    const retry = events.find(e => e.type === "llm_retry") as any;
    expect(retry).toBeDefined();
    expect(retry.thinkingFragment).toBe(THINKING_BEFORE_ERROR);
    expect(retry.textFragment).toBeUndefined();

    // Turn ended cleanly; final response carries only post-retry content
    const last = events[events.length - 1] as any;
    expect(last.type).toBe("turn_end");
    const llmResponse = events.find(e => e.type === "llm_response") as any;
    expect(llmResponse.text).toBe("done");
    // No thinking in the successful response (mock doesn't emit thinking on retry)
    expect(llmResponse.thinking).toBeUndefined();

    delete process.env.OMEGA_RETRY_BASE_MS;
    delete process.env.OMEGA_RETRY_MAX_MS;
  });

  it("textFragment captured when 529 fires mid-text-stream", async () => {
    process.env.OMEGA_RETRY_BASE_MS = "1";
    process.env.OMEGA_RETRY_MAX_MS = "2";

    const provider = makePartialThenSuccessProvider(1, {
      textBeforeError: TEXT_BEFORE_ERROR,
    });
    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "hello");

    const retry = events.find(e => e.type === "llm_retry") as any;
    expect(retry).toBeDefined();
    expect(retry.textFragment).toBe(TEXT_BEFORE_ERROR);
    expect(retry.thinkingFragment).toBeUndefined();

    delete process.env.OMEGA_RETRY_BASE_MS;
    delete process.env.OMEGA_RETRY_MAX_MS;
  });

  it("both fragments captured when 529 fires after thinking and partial text", async () => {
    process.env.OMEGA_RETRY_BASE_MS = "1";
    process.env.OMEGA_RETRY_MAX_MS = "2";

    const provider = makePartialThenSuccessProvider(1, {
      thinkingBeforeError: THINKING_BEFORE_ERROR,
      textBeforeError:     TEXT_BEFORE_ERROR,
    });
    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "hello");

    const retry = events.find(e => e.type === "llm_retry") as any;
    expect(retry.thinkingFragment).toBe(THINKING_BEFORE_ERROR);
    expect(retry.textFragment).toBe(TEXT_BEFORE_ERROR);

    delete process.env.OMEGA_RETRY_BASE_MS;
    delete process.env.OMEGA_RETRY_MAX_MS;
  });

  it("fragments absent when 529 fires before any content arrives", async () => {
    process.env.OMEGA_RETRY_BASE_MS = "1";
    process.env.OMEGA_RETRY_MAX_MS = "2";
    process.env.OMEGA_RETRY_ATTEMPTS = "2";

    // Provider always fails before yielding anything
    const provider: CreateMessageStream = () => { throw overloadError(); };
    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "hello");

    const retry = events.find(e => e.type === "llm_retry") as any;
    expect(retry).toBeDefined();
    expect(retry.thinkingFragment).toBeUndefined();
    expect(retry.textFragment).toBeUndefined();

    delete process.env.OMEGA_RETRY_BASE_MS;
    delete process.env.OMEGA_RETRY_MAX_MS;
    delete process.env.OMEGA_RETRY_ATTEMPTS;
  });

  it("fragments survive events.jsonl round-trip", async () => {
    const { readFileSync } = await import("fs");
    process.env.OMEGA_RETRY_BASE_MS = "1";
    process.env.OMEGA_RETRY_MAX_MS = "2";

    const provider = makePartialThenSuccessProvider(1, {
      thinkingBeforeError: THINKING_BEFORE_ERROR,
      textBeforeError:     TEXT_BEFORE_ERROR,
    });
    const { agent, eventsFile, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);
    await collectEvents(agent, "hello");
    await agent.flushEventLog();

    const lines = readFileSync(eventsFile, "utf-8")
      .split("\n")
      .filter(l => l.trim());
    const retryLine = lines.find(l => {
      try { return JSON.parse(l).type === "llm_retry"; } catch { return false; }
    });
    expect(retryLine).toBeDefined();
    const parsed = JSON.parse(retryLine!);
    expect(parsed.thinkingFragment).toBe(THINKING_BEFORE_ERROR);
    expect(parsed.textFragment).toBe(TEXT_BEFORE_ERROR);

    delete process.env.OMEGA_RETRY_BASE_MS;
    delete process.env.OMEGA_RETRY_MAX_MS;
  });

  it("context is clean after mid-stream retry — no partial thinking in subsequent API call", async () => {
    // Verifies that assembledThinking/assembledText from a failed stream
    // never reach compactedContextHistory or the next API call's messages.
    process.env.OMEGA_RETRY_BASE_MS = "1";
    process.env.OMEGA_RETRY_MAX_MS = "2";

    let capturedMessages: any[] | undefined;
    let callCount = 0;

    const provider: CreateMessageStream = (params) => {
      callCount++;
      if (callCount === 1) {
        // First call: yield partial thinking, then throw
        return {
          async *[Symbol.asyncIterator](): AsyncGenerator<BetaRawMessageStreamEvent> {
            yield { type: "content_block_start", index: 0, content_block: { type: "thinking", thinking: "", signature: "" } };
            yield { type: "content_block_delta", index: 0, delta: { type: "thinking_delta", thinking: THINKING_BEFORE_ERROR } };
            throw overloadError();
          },
          finalMessage: async () => { throw overloadError(); },
        };
      }
      // Retry: capture the messages to verify they don't contain the fragment
      capturedMessages = params.messages as any[];
      return {
        async *[Symbol.asyncIterator](): AsyncGenerator<BetaRawMessageStreamEvent> {
          yield { type: "content_block_start", index: 0, content_block: { type: "text", text: "", citations: null } };
          yield { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "done" } };
          yield { type: "content_block_stop", index: 0 };
          yield { type: "message_delta", context_management: null, delta: { stop_reason: "end_turn", stop_sequence: null, stop_details: null, container: null }, usage: { output_tokens: 1, cache_creation_input_tokens: null, cache_read_input_tokens: null, input_tokens: null, iterations: null, server_tool_use: null } };
          yield { type: "message_stop" };
        },
        finalMessage: async () => ({
          id: "msg_retry_ok",
          type: "message",
          role: "assistant",
          model: "claude-sonnet-4-6",
          container: null,
          context_management: null,
          content: [{ type: "text", text: "done", citations: null }],
          stop_reason: "end_turn",
          stop_sequence: null,
          usage: { input_tokens: 10, output_tokens: 1 },
        } as any),
      };
    };

    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);
    await collectEvents(agent, "hello");

    expect(callCount).toBe(2);
    expect(capturedMessages).toBeDefined();
    // The retry must send exactly the same messages as the first call (user
    // message only — no partial assistant turn contaminating context).
    const assistantMsgs = capturedMessages!.filter((m: any) => m.role === "assistant");
    expect(assistantMsgs.length).toBe(0);
    // And the partial thinking string must not appear anywhere in the payload
    const payloadStr = JSON.stringify(capturedMessages);
    expect(payloadStr).not.toContain(THINKING_BEFORE_ERROR);

    delete process.env.OMEGA_RETRY_BASE_MS;
    delete process.env.OMEGA_RETRY_MAX_MS;
  });
});

// ---------------------------------------------------------------------------
// retry-after header: authoritative wait time overrides exponential backoff
// ---------------------------------------------------------------------------

describe("retry-after header", () => {
  /** Shared success stream used across tests in this describe block. */
  function makeSuccessProvider(): CreateMessageStream {
    return () => ({
      async *[Symbol.asyncIterator](): AsyncGenerator<BetaRawMessageStreamEvent> {
        yield { type: "content_block_start", index: 0, content_block: { type: "text", text: "", citations: null } };
        yield { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "done" } };
        yield { type: "content_block_stop", index: 0 };
        yield { type: "message_delta", context_management: null, delta: { stop_reason: "end_turn", stop_sequence: null, stop_details: null, container: null }, usage: { output_tokens: 1, cache_creation_input_tokens: null, cache_read_input_tokens: null, input_tokens: null, iterations: null, server_tool_use: null } };
        yield { type: "message_stop" };
      },
      finalMessage: async () => ({
        id: "msg_ok", type: "message", role: "assistant", model: "claude-sonnet-4-6",
        container: null, context_management: null,
        content: [{ type: "text", text: "done", citations: null }],
        stop_reason: "end_turn", stop_sequence: null,
        usage: { input_tokens: 10, output_tokens: 1 },
      } as any),
    });
  }

  it("uses retry-after header value instead of exponential backoff", async () => {
    // Set a very high base to prove exponential backoff is NOT used.
    // If the header is ignored, waitMs would be ~10 000 ms; with it, 1 ms.
    process.env.OMEGA_RETRY_BASE_MS = "10000";
    process.env.OMEGA_RETRY_MAX_MS = "10000";

    let callCount = 0;
    const successStream = makeSuccessProvider();
    const mockProvider: CreateMessageStream = (params) => {
      callCount++;
      if (callCount === 1) {
        const err: any = new Error("429 rate limited");
        err.status = 429;
        err.headers = new Headers({ "retry-after": "0.001" }); // 1 ms
        throw err;
      }
      return successStream(params);
    };

    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "hello");

    expect(callCount).toBe(2);

    const retries = events.filter(e => e.type === "llm_retry") as any[];
    expect(retries).toHaveLength(1);

    // waitMs must come from the header (1 ms), not from our backoff (10 000 ms).
    expect(retries[0].waitMs).toBe(1);

    const last = events[events.length - 1] as any;
    expect(last.type).toBe("turn_end");

    delete process.env.OMEGA_RETRY_BASE_MS;
    delete process.env.OMEGA_RETRY_MAX_MS;
  });

  it("fractional retry-after seconds are rounded up to whole milliseconds", async () => {
    process.env.OMEGA_RETRY_BASE_MS = "10000";
    process.env.OMEGA_RETRY_MAX_MS = "10000";

    let callCount = 0;
    const successStream = makeSuccessProvider();
    const mockProvider: CreateMessageStream = (params) => {
      callCount++;
      if (callCount === 1) {
        const err: any = new Error("429 rate limited");
        err.status = 429;
        err.headers = new Headers({ "retry-after": "0.0005" }); // 0.5 ms → ceil → 1 ms
        throw err;
      }
      return successStream(params);
    };

    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "hello");

    const retries = events.filter(e => e.type === "llm_retry") as any[];
    expect(retries[0].waitMs).toBe(1); // Math.ceil(0.5) = 1

    delete process.env.OMEGA_RETRY_BASE_MS;
    delete process.env.OMEGA_RETRY_MAX_MS;
  });

  it("retry-after header is honoured even when it exceeds retryMaxMs cap", async () => {
    // retryMaxMs is tiny (5 ms) but the server asks for 500 ms.
    // The cap must NOT reduce the header-recommended wait — otherwise we'd
    // hammer an already-overloaded API.
    process.env.OMEGA_RETRY_BASE_MS = "1";
    process.env.OMEGA_RETRY_MAX_MS = "5"; // cap well below the header value

    let callCount = 0;
    const successStream = makeSuccessProvider();
    const mockProvider: CreateMessageStream = (params) => {
      callCount++;
      if (callCount === 1) {
        const err: any = new Error("529 overloaded");
        err.status = 529;
        err.headers = new Headers({ "retry-after": "0.5" }); // 500 ms
        throw err;
      }
      return successStream(params);
    };

    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "hello");

    const retries = events.filter(e => e.type === "llm_retry") as any[];
    expect(retries).toHaveLength(1);
    // Must use the header value (500 ms), not the cap (5 ms).
    expect(retries[0].waitMs).toBe(500);

    delete process.env.OMEGA_RETRY_BASE_MS;
    delete process.env.OMEGA_RETRY_MAX_MS;
  });

  it("server-wins: retry-after on a non-retryable error (400) still retries", async () => {
    // Server-wins retry-after: even if the classifier would say "don't
    // retry" (e.g. a 400), the presence of retry-after means the server
    // is explicitly asking us to wait and try again. Honour that.
    process.env.OMEGA_RETRY_BASE_MS = "1";
    process.env.OMEGA_RETRY_MAX_MS = "5";

    let callCount = 0;
    const successStream = makeSuccessProvider();
    const mockProvider: CreateMessageStream = (params) => {
      callCount++;
      if (callCount === 1) {
        // 400 is not in isRetryable's list — but retry-after must win.
        const err: any = new Error("400 try again");
        err.status = 400;
        err.headers = new Headers({ "retry-after": "0.001" });
        throw err;
      }
      return successStream(params);
    };

    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "hello");

    expect(callCount).toBe(2);
    const retries = events.filter(e => e.type === "llm_retry") as any[];
    expect(retries).toHaveLength(1);
    expect(retries[0].reason).toBe("retry-after");
    expect(retries[0].httpStatus).toBe(400);
    const last = events[events.length - 1] as any;
    expect(last.type).toBe("turn_end");

    delete process.env.OMEGA_RETRY_BASE_MS;
    delete process.env.OMEGA_RETRY_MAX_MS;
  });

  it("retry-after is capped at 5 minutes (absurd duration sanity limit)", async () => {
    process.env.OMEGA_RETRY_BASE_MS = "1";
    process.env.OMEGA_RETRY_MAX_MS = "5";

    let callCount = 0;
    const successProvider = makeSuccessProvider();
    // Override sleep by intercepting via a provider that succeeds on attempt 2
    // We can't easily stub sleep; instead verify waitMs in the emitted event
    // (the test doesn't actually wait 5 min — we capture the value and then
    // a separate success attempt lets the turn complete quickly... but sleep
    // still runs. So use a smaller "600 s" that still exceeds the 5 min cap,
    // and verify the cap via the emitted waitMs only. The real sleep must be
    // short enough that the test doesn't hang.)
    //
    // Strategy: emit retry-after="600" (600 s = 600 000 ms, exceeds 300 000 ms
    // cap). The cap reduces waitMs to 300 000 ms — but the test would then
    // wait 5 minutes, which is unacceptable. So we patch a tiny window by
    // using a value that exceeds the cap but after capping is still tiny:
    // impossible for the current cap. Instead, assert the cap behaviour at a
    // lower fake cap by directly inspecting getRetryAfterMs — but that's a
    // unit-level peek.
    //
    // Cleanest: stub global.setTimeout so sleep resolves immediately, and
    // verify the emitted waitMs reflects the 5 min cap.
    const origSetTimeout = globalThis.setTimeout;
    (globalThis as any).setTimeout = (fn: () => void, _ms: number) =>
      origSetTimeout(fn, 0);

    try {
      const mockProvider: CreateMessageStream = (params) => {
        callCount++;
        if (callCount === 1) {
          const err: any = new Error("429 rate limited");
          err.status = 429;
          err.headers = new Headers({ "retry-after": "600" }); // 600 s
          throw err;
        }
        return successProvider(params);
      };

      const { agent, dispose } = await makeTestAgent(mockProvider);
      disposeAll.push(dispose);
      const events = await collectEvents(agent, "hello");

      expect(callCount).toBe(2);
      const retries = events.filter(e => e.type === "llm_retry") as any[];
      expect(retries).toHaveLength(1);
      // 600 s would be 600 000 ms; cap reduces to 300 000 ms (5 min).
      expect(retries[0].waitMs).toBe(5 * 60 * 1000);
      expect(retries[0].reason).toBe("retry-after");
    } finally {
      (globalThis as any).setTimeout = origSetTimeout;
    }
  });

  it("unbounded retries while server keeps sending retry-after (no attempt cap)", async () => {
    // Retry-after bypasses the policy attempt cap. Set a low
    // OMEGA_RETRY_ATTEMPTS to prove the cap does NOT terminate us: if the
    // server sends retry-after 3 times (more than the cap of 2), we must
    // still retry and eventually succeed.
    process.env.OMEGA_RETRY_BASE_MS = "1";
    process.env.OMEGA_RETRY_MAX_MS = "5";
    process.env.OMEGA_RETRY_ATTEMPTS = "2"; // below the 3 retry-after events

    let callCount = 0;
    const failTimes = 3;
    const successStream = makeSuccessProvider();
    const mockProvider: CreateMessageStream = (params) => {
      callCount++;
      if (callCount <= failTimes) {
        const err: any = new Error("429 rate limited");
        err.status = 429;
        err.headers = new Headers({ "retry-after": "0.001" });
        throw err;
      }
      return successStream(params);
    };

    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "hello");

    // All three retry-after errors honoured; turn succeeds on the 4th call.
    expect(callCount).toBe(failTimes + 1);
    const retries = events.filter(e => e.type === "llm_retry") as any[];
    expect(retries).toHaveLength(failTimes);
    expect(retries.every(r => r.reason === "retry-after")).toBe(true);
    // Emitted attempt numbers are 1-based and sequential across all retries.
    expect(retries.map(r => r.attempt)).toEqual([1, 2, 3]);
    const last = events[events.length - 1] as any;
    expect(last.type).toBe("turn_end");

    delete process.env.OMEGA_RETRY_BASE_MS;
    delete process.env.OMEGA_RETRY_MAX_MS;
    delete process.env.OMEGA_RETRY_ATTEMPTS;
  });

  it("retry-after first, then a non-retry-after overload: policy cap applies to the latter only", async () => {
    // After retry-after retries (which don't consume the policy cap), a
    // subsequent non-retry-after error with OMEGA_RETRY_ATTEMPTS=1 should
    // terminate the turn immediately (no policy retry allowed). This
    // verifies retry-after retries don't accidentally consume policy budget.
    process.env.OMEGA_RETRY_BASE_MS = "1";
    process.env.OMEGA_RETRY_MAX_MS = "5";
    process.env.OMEGA_RETRY_ATTEMPTS = "1";

    let callCount = 0;
    const mockProvider: CreateMessageStream = () => {
      callCount++;
      if (callCount === 1) {
        const err: any = new Error("429 rate limited");
        err.status = 429;
        err.headers = new Headers({ "retry-after": "0.001" });
        throw err;
      }
      // Second call: plain overload, no retry-after.
      throw overloadError();
    };

    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "hello");

    // One retry-after retry + one terminal overload = 2 calls total.
    expect(callCount).toBe(2);
    const retries = events.filter(e => e.type === "llm_retry") as any[];
    expect(retries).toHaveLength(1);
    expect(retries[0].reason).toBe("retry-after");
    // Turn terminates via the policy path — maxAttempts=1 means no policy
    // retry permitted, so the overload becomes terminal.
    const last = events[events.length - 1] as any;
    expect(last.type).toBe("turn_interrupted");
    expect(last.reason).toBe("error");

    delete process.env.OMEGA_RETRY_BASE_MS;
    delete process.env.OMEGA_RETRY_MAX_MS;
    delete process.env.OMEGA_RETRY_ATTEMPTS;
  });

  it("falls back to exponential backoff when no retry-after header is present", async () => {
    process.env.OMEGA_RETRY_BASE_MS = "1";
    process.env.OMEGA_RETRY_MAX_MS = "5000";
    process.env.OMEGA_RETRY_ATTEMPTS = "2"; // cap so test terminates quickly

    const mockProvider: CreateMessageStream = () => {
      const err: any = new Error("429 rate limited");
      err.status = 429;
      // no .headers — must fall back to backoff
      throw err;
    };

    const { agent, dispose } = await makeTestAgent(mockProvider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "hello");

    const retries = events.filter(e => e.type === "llm_retry") as any[];
    expect(retries.length).toBeGreaterThan(0);
    // waitMs must be from backoff (≥ 1 ms, but well below 10 000)
    expect(retries[0].waitMs).toBeGreaterThan(0);
    expect(retries[0].waitMs).toBeLessThan(10_000);

    delete process.env.OMEGA_RETRY_BASE_MS;
    delete process.env.OMEGA_RETRY_MAX_MS;
    delete process.env.OMEGA_RETRY_ATTEMPTS;
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

    const mockProvider: CreateMessageStream = () => {
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

    const mockProvider: CreateMessageStream = () => {
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
