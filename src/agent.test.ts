import { describe, it, expect } from "bun:test";
import { estimateCost, estimateCostWithCache, isAutoApproved, isRetryable, isAuthExpired, isContextTooLong, PRICING } from "./agent.js";
// isAutoApproved is kept exported for logging purposes; it always returns true.
import type Anthropic from "@anthropic-ai/sdk";

// ---------------------------------------------------------------------------
// Unit tests for agent.ts pure functions
// ---------------------------------------------------------------------------

// --- Cost estimation ---

describe("estimateCost", () => {
  it("calculates cost for sonnet correctly", () => {
    // 1000 input tokens at $3/M + 500 output at $15/M = $0.003 + $0.0075 = $0.0105
    const cost = estimateCost("claude-sonnet-4-6", 1000, 500);
    expect(cost).toBeCloseTo(0.0105, 6);
  });

  it("calculates cost for opus correctly", () => {
    // 1000 input at $5/M + 1000 output at $25/M = $0.005 + $0.025 = $0.03
    const cost = estimateCost("claude-opus-4-6", 1000, 1000);
    expect(cost).toBeCloseTo(0.03, 6);
  });

  it("falls back to opus pricing for unknown model", () => {
    const cost = estimateCost("unknown-model", 1000, 0);
    expect(cost).toBeCloseTo(0.005, 6);
  });

  it("returns 0 for zero tokens", () => {
    expect(estimateCost("claude-sonnet-4-6", 0, 0)).toBe(0);
  });
});

// --- Prompt caching cost estimation ---

describe("estimateCostWithCache", () => {
  it("accounts for cache read and creation tokens", () => {
    // For Sonnet: input=$3/M, output=$15/M, cache write=1.25x input, cache read=0.1x input
    // base input: 1000 tokens, cache creation: 200, cache read: 300, output: 500
    // cost = input(1000)*3 + output(500)*15 + cache_creation(200)*3.75 + cache_read(300)*0.3
    // = 0.003 + 0.0075 + 0.00075 + 0.00009 = 0.01134
    const cost = estimateCostWithCache("claude-sonnet-4-6", 1000, 500, 200, 300);
    expect(cost).toBeCloseTo(0.01134, 6);
  });

  it("falls back to estimateCost when cache tokens are zero", () => {
    const base = estimateCost("claude-sonnet-4-6", 1000, 500);
    const cost = estimateCostWithCache("claude-sonnet-4-6", 1000, 500, 0, 0);
    expect(cost).toBeCloseTo(base, 6);
  });
});

// --- Pricing table ---

describe("PRICING", () => {
  it("has entries for all supported models", () => {
    expect(PRICING["claude-opus-4-6"]).toBeDefined();
    expect(PRICING["claude-sonnet-4-6"]).toBeDefined();
  });
});

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
    // The Anthropic SDK throws this from within the stream iterator when the
    // server restarts a stream mid-flight. It has no HTTP status code — it's
    // a plain AnthropicError thrown by MessageStream.#accumulateMessage().
    // We must treat it as retryable so the agent retries instead of surfacing
    // a hard error to the user.
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

  it("does NOT retry on 429 'Extra usage is required for long context requests'", () => {
    // Claude Max OAuth returns 429 with this message when the prompt exceeds
    // the context window for the account tier. Retrying with the same payload
    // is futile — treat as non-retryable so we fall through to graceful handling.
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

// --- Auth expiry detection ---

describe("isAuthExpired", () => {
  it("returns true for 401 with authentication_error in message", () => {
    const err: any = new Error('401 {"type":"error","error":{"type":"authentication_error","message":"OAuth token has expired."}}');
    err.status = 401;
    expect(isAuthExpired(err)).toBe(true);
  });

  it("returns true for 401 with 'OAuth token has expired' in message", () => {
    const err: any = new Error("OAuth token has expired. Please obtain a new token.");
    err.status = 401;
    expect(isAuthExpired(err)).toBe(true);
  });

  it("returns true for 403 permission_error with 'revoked' in message", () => {
    const err: any = new Error('403 {"type":"error","error":{"type":"permission_error","message":"OAuth token has been revoked. Please obtain a new token."}}');
    err.status = 403;
    expect(isAuthExpired(err)).toBe(true);
  });

  it("returns true for 403 with 'OAuth token has been revoked' text", () => {
    const err: any = new Error("OAuth token has been revoked. Please obtain a new token.");
    err.status = 403;
    expect(isAuthExpired(err)).toBe(true);
  });

  it("returns false for 403 without revoked/auth keyword", () => {
    const err: any = new Error("Forbidden: insufficient permissions");
    err.status = 403;
    expect(isAuthExpired(err)).toBe(false);
  });

  it("returns false for 429 (rate limit)", () => {
    expect(isAuthExpired({ status: 429 })).toBe(false);
  });

  it("returns false for null", () => {
    expect(isAuthExpired(null)).toBe(false);
  });

  it("returns false for 401 without auth_error keyword", () => {
    const err: any = new Error("Not found");
    err.status = 401;
    expect(isAuthExpired(err)).toBe(false);
  });
});




