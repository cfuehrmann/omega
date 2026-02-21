import { describe, it, expect } from "bun:test";
import { estimateCost, isAutoApproved, isRetryable, truncateHistory, PRICING, formatPayloadSummary } from "./agent.js";
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
    // 1000 input at $15/M + 1000 output at $75/M = $0.015 + $0.075 = $0.09
    const cost = estimateCost("claude-opus-4-6", 1000, 1000);
    expect(cost).toBeCloseTo(0.09, 6);
  });

  it("falls back to opus pricing for unknown model", () => {
    const cost = estimateCost("unknown-model", 1000, 0);
    expect(cost).toBeCloseTo(0.015, 6);
  });

  it("returns 0 for zero tokens", () => {
    expect(estimateCost("claude-sonnet-4-6", 0, 0)).toBe(0);
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

// --- formatPayloadSummary ---

describe("formatPayloadSummary", () => {
  const basePayload = {
    model: "claude-sonnet-4-6",
    system: "You are Omega. ".repeat(50), // ~750 chars
    tools: [{ name: "read_file" }, { name: "write_file" }, { name: "run_command" }] as any[],
    messages: [
      { role: "user" as const, content: "Hello" },
      { role: "assistant" as const, content: "Hi there" },
      { role: "user" as const, content: "Do something" },
    ],
  };

  it("returns an object with estimatedTokens, systemChars, toolCount, messageCount", () => {
    const s = formatPayloadSummary(basePayload);
    expect(typeof s.estimatedTokens).toBe("number");
    expect(typeof s.systemChars).toBe("number");
    expect(s.toolCount).toBe(3);
    expect(s.messageCount).toBe(3);
  });

  it("estimatedTokens is positive and proportional to content size", () => {
    const s = formatPayloadSummary(basePayload);
    expect(s.estimatedTokens).toBeGreaterThan(0);
    // system is ~750 chars → ~187 tokens; messages are small; tools are small
    // total should be well under 1000 for this small payload
    expect(s.estimatedTokens).toBeLessThan(1000);
  });

  it("systemChars matches the system prompt length", () => {
    const s = formatPayloadSummary(basePayload);
    expect(s.systemChars).toBe(basePayload.system.length);
  });

  it("returns message summaries with role and a short preview", () => {
    const s = formatPayloadSummary(basePayload);
    expect(Array.isArray(s.messageSummaries)).toBe(true);
    expect(s.messageSummaries.length).toBe(3);
    expect(s.messageSummaries[0].role).toBe("user");
    expect(typeof s.messageSummaries[0].preview).toBe("string");
    expect(s.messageSummaries[0].preview.length).toBeGreaterThan(0);
  });

  it("preview is truncated for long messages", () => {
    const longPayload = {
      ...basePayload,
      messages: [{ role: "user" as const, content: "x".repeat(500) }],
    };
    const s = formatPayloadSummary(longPayload);
    expect(s.messageSummaries[0].preview.length).toBeLessThan(500);
  });

  it("tool_use blocks are described in the message summary", () => {
    const withToolUse = {
      ...basePayload,
      messages: [
        {
          role: "assistant" as const,
          content: [
            { type: "text", text: "I'll read the file." },
            { type: "tool_use", id: "t1", name: "read_file", input: { path: "src/agent.ts" } },
          ],
        },
      ],
    };
    const s = formatPayloadSummary(withToolUse);
    expect(s.messageSummaries[0].preview).toContain("read_file");
  });

  it("tool_result blocks are described in the message summary", () => {
    const withToolResult = {
      ...basePayload,
      messages: [
        {
          role: "user" as const,
          content: [
            { type: "tool_result", tool_use_id: "t1", content: "file contents here" },
          ],
        },
      ],
    };
    const s = formatPayloadSummary(withToolResult);
    expect(s.messageSummaries[0].preview).toContain("tool_result");
  });
});
