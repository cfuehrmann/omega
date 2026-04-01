import { describe, it, expect } from "bun:test";
import { isAutoApproved, isRetryable, isContextTooLong } from "./agent.js";
// isAutoApproved is kept exported for logging purposes; it always returns true.
import type Anthropic from "@anthropic-ai/sdk";

// ---------------------------------------------------------------------------
// Unit tests for agent.ts pure functions
// ---------------------------------------------------------------------------

// --- Auto-approve logic ---

describe("isAutoApproved", () => {
  it("always returns true — everything is auto-approved", () => {
    expect(isAutoApproved("run_command", { command: "rm -rf /" })).toBe(true);
    expect(isAutoApproved("read_file", { path: "src/agent.ts" })).toBe(true);
    expect(isAutoApproved("unknown_tool", {})).toBe(true);
  });
});

// --- Retry logic ---

describe("isRetryable", () => {
  it("retries on 429 (rate limit)", () => {
    expect(isRetryable({ status: 429 })).toBe(true);
  });

  it("retries on 529 (overload)", () => {
    expect(isRetryable({ status: 529 })).toBe(true);
  });

  it("retries on 500 (server error)", () => {
    expect(isRetryable({ status: 500 })).toBe(true);
  });

  it("retries on 503 (unavailable)", () => {
    expect(isRetryable({ status: 503 })).toBe(true);
  });

  it("does not retry on 400 (bad request)", () => {
    expect(isRetryable({ status: 400 })).toBe(false);
  });

  it("does not retry on 401 (unauthorized)", () => {
    expect(isRetryable({ status: 401 })).toBe(false);
  });

  it("does not retry on null", () => {
    expect(isRetryable(null)).toBe(false);
  });

  it("supports statusCode (alternative field)", () => {
    expect(isRetryable({ statusCode: 429 })).toBe(true);
  });

  it("retries on SDK stream ordering error (message_start before message_stop)", () => {
    const sdkStreamError = new Error(
      'Unexpected event order, got message_start before receiving "message_stop"'
    );
    expect(isRetryable(sdkStreamError)).toBe(true);
  });

  it("retries on socket-closed / TCP reset error (no HTTP status)", () => {
    // Bun throws this when the underlying TCP connection to the API server is
    // dropped mid-flight (e.g. server closes idle connection, network blip).
    // The error has no HTTP status code. Seen in the wild during world-state
    // fold at shutdown: "The socket connection was closed unexpectedly."
    // Diagnosis: 2026-02-23T18-36-29-982Z.json
    const socketErr = new Error("The socket connection was closed unexpectedly. For more information, pass `verbose: true` in the second argument to fetch()");
    expect(isRetryable(socketErr)).toBe(true);
  });

  it("retries on ECONNRESET fetch error", () => {
    const resetErr = new Error("fetch failed: read ECONNRESET");
    expect(isRetryable(resetErr)).toBe(true);
  });

  it("retries on SSE-stream overload — status=undefined, body contains overloaded_error", () => {
    // When the Anthropic API delivers a 529 overload *as an SSE stream error
    // event* (rather than as an HTTP-level 529 status), the SDK throws:
    //   new APIError(undefined, parsedBody, undefined, headers)
    // which has .status=undefined and .message=JSON.stringify(parsedBody).
    // isRetryable must catch this via the body/message rather than the status.
    // Bug reproduced in session 2026-04-01T16-02-14-529-87454cef.
    const sseOverloadErr = Object.assign(
      new Error('{"type":"error","error":{"details":null,"type":"overloaded_error","message":"Overloaded"},"request_id":"req_011CZdMFPu4gDnTXTxq63PZw"}'),
      {
        status: undefined,
        error: { type: "error", error: { type: "overloaded_error", message: "Overloaded" } },
      }
    );
    expect(isRetryable(sseOverloadErr)).toBe(true);
  });

  it("does NOT retry on 429 'Extra usage is required for long context requests'", () => {
    const longContextErr = Object.assign(
      new Error("429 {\"type\":\"error\",\"error\":{\"type\":\"rate_limit_error\",\"message\":\"Extra usage is required for long context requests.\"}}"),
      { status: 429 }
    );
    expect(isRetryable(longContextErr)).toBe(false);
  });
});

// --- isContextTooLong ---

describe("isContextTooLong", () => {
  it("returns true for 429 with 'Extra usage is required for long context requests'", () => {
    const err = Object.assign(
      new Error("429 {\"type\":\"error\",\"error\":{\"type\":\"rate_limit_error\",\"message\":\"Extra usage is required for long context requests.\"}}"),
      { status: 429 }
    );
    expect(isContextTooLong(err)).toBe(true);
  });

  it("returns false for ordinary 429 rate limit", () => {
    const err = Object.assign(new Error("Rate limit exceeded"), { status: 429 });
    expect(isContextTooLong(err)).toBe(false);
  });

  it("returns false for 400 prompt-too-long (different mechanism)", () => {
    const err = Object.assign(new Error("prompt is too long"), { status: 400 });
    expect(isContextTooLong(err)).toBe(false);
  });

  it("returns false for null", () => {
    expect(isContextTooLong(null)).toBe(false);
  });
});






