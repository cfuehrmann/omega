import { describe, it, expect } from "bun:test";
import { estimateCost, estimateCostWithCache, isAutoApproved, isRetryable, isAuthExpired, isContextTooLong, truncateHistory, PRICING } from "./agent.js";
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

// --- Context window truncation ---

function makeMsg(role: "user" | "assistant", content: string): Anthropic.MessageParam {
  return { role, content };
}

// ~4 chars per token, so 400 chars ≈ 100 tokens
const SHORT = "a".repeat(400);   // ~100 tokens
const LONG  = "a".repeat(4000);  // ~1000 tokens

describe("truncateHistory", () => {
  it("returns history unchanged if under budget", () => {
    const history = [makeMsg("user", SHORT), makeMsg("assistant", SHORT)];
    const result = truncateHistory(history, 100_000);
    expect(result).toEqual(history);
  });

  it("caps history length at 100 messages", () => {
    const msgs: Anthropic.MessageParam[] = [];
    for (let i = 0; i < 160; i++) {
      msgs.push(makeMsg(i % 2 === 0 ? "user" : "assistant", SHORT));
    }
    const result = truncateHistory(msgs, 100_000);
    expect(result.length).toBeLessThanOrEqual(100);
    expect(result[0]).toEqual(msgs[0]);
  });

  it("preserves the first message", () => {
    // Create a history that's well over budget
    const msgs: Anthropic.MessageParam[] = [];
    for (let i = 0; i < 30; i++) {
      msgs.push(makeMsg(i % 2 === 0 ? "user" : "assistant", LONG));
    }
    const result = truncateHistory(msgs, 5000);
    expect(result[0]).toEqual(msgs[0]);
  });

  it("preserves the most recent messages", () => {
    const msgs: Anthropic.MessageParam[] = [];
    for (let i = 0; i < 30; i++) {
      msgs.push(makeMsg(i % 2 === 0 ? "user" : "assistant", LONG));
    }
    const result = truncateHistory(msgs, 5000);
    const lastOriginal = msgs[msgs.length - 1];
    const lastResult = result[result.length - 1];
    expect(lastResult).toEqual(lastOriginal);
  });

  it("reduces length when over budget", () => {
    const msgs: Anthropic.MessageParam[] = [];
    for (let i = 0; i < 30; i++) {
      msgs.push(makeMsg(i % 2 === 0 ? "user" : "assistant", LONG));
    }
    const result = truncateHistory(msgs, 5000);
    expect(result.length).toBeLessThan(msgs.length);
  });

  it("returns all messages if fewer than KEEP_RECENT_TURNS*2", () => {
    const msgs = [
      makeMsg("user", SHORT),
      makeMsg("assistant", SHORT),
    ];
    const result = truncateHistory(msgs, 1); // tiny budget
    // With only 2 messages, nothing can be dropped
    expect(result.length).toBe(2);
  });

  it("never produces orphaned tool_result without matching tool_use", () => {
    // The bug: truncation drops messages one-by-one from the middle.
    // If an assistant(tool_use) gets dropped but the following user(tool_result)
    // lands in the "kept tail", the API rejects with:
    // "unexpected tool_use_id found in tool_result blocks"
    //
    // To trigger this, we place a tool_use/tool_result pair right at the
    // boundary between "droppable middle" and "kept tail" (last 20 messages).
    const msgs: Anthropic.MessageParam[] = [
      { role: "user", content: "Start" },
    ];

    // Fill middle with enough messages to force truncation
    for (let i = 0; i < 20; i++) {
      msgs.push(makeMsg(i % 2 === 0 ? "assistant" : "user", LONG));
    }

    // Now add a tool_use/tool_result pair right before the "tail" section
    msgs.push({
      role: "assistant",
      content: [
        { type: "text", text: "I'll update the plan." },
        { type: "tool_use", id: "tool_boundary", name: "write_file", input: { path: "x", content: LONG } },
      ],
    });
    msgs.push({
      role: "user",
      content: [
        { type: "tool_result", tool_use_id: "tool_boundary", content: "Wrote file" },
      ],
    });

    // Add exactly KEEP_RECENT_TURNS*2 - 1 more messages so the tool_result
    // above is at the very start of the tail
    for (let i = 0; i < 19; i++) {
      msgs.push(makeMsg(i % 2 === 0 ? "assistant" : "user", LONG));
    }

    const result = truncateHistory(msgs, 8000);

    // Check: every tool_result must have a preceding tool_use with matching ID
    for (let i = 0; i < result.length; i++) {
      const msg = result[i];
      if (typeof msg.content === "string") continue;
      if (!Array.isArray(msg.content)) continue;

      for (const block of msg.content) {
        if ((block as any).type === "tool_result") {
          const toolResultId = (block as any).tool_use_id;
          // Search ALL preceding messages for matching tool_use
          let found = false;
          for (let j = 0; j < i; j++) {
            const prev = result[j];
            if (prev.role !== "assistant") continue;
            if (typeof prev.content === "string") continue;
            if (!Array.isArray(prev.content)) continue;
            for (const prevBlock of prev.content) {
              if ((prevBlock as any).type === "tool_use" && (prevBlock as any).id === toolResultId) {
                found = true;
              }
            }
          }
          expect(found).withContext(`tool_result for ${toolResultId} has no matching tool_use`).toBe(true);
        }
      }
    }
  });

  it("handles prompt-too-long by actually reducing token count", () => {
    // Simulate what causes 1.17M tokens: tool results with huge file contents
    const msgs: Anthropic.MessageParam[] = [
      { role: "user", content: "Read the big file" },
    ];
    // Add many turns with large tool results
    for (let i = 0; i < 50; i++) {
      msgs.push({
        role: "assistant",
        content: [
          { type: "tool_use", id: `tool_${i}`, name: "read_file", input: { path: "big.txt" } },
        ],
      });
      msgs.push({
        role: "user",
        content: [
          { type: "tool_result", tool_use_id: `tool_${i}`, content: "x".repeat(20000) },
        ],
      });
    }
    msgs.push({ role: "assistant", content: "Here's the summary." });

    // Total is ~50 * 20000 / 4 = 250K estimated tokens, way over 100K budget
    const result = truncateHistory(msgs, 100_000);

    // Result must be significantly smaller
    const resultSize = result.reduce((sum, m) => {
      const text = typeof m.content === "string" ? m.content : JSON.stringify(m.content);
      return sum + text.length;
    }, 0);

    // Must be under budget (100K tokens * 4 chars/token = 400K chars)
    expect(resultSize).toBeLessThan(400_000);
  });
});


