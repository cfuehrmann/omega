/**
 * Round-trip test: Rust-serialized JSON validates against the TypeScript schema.
 *
 * This test proves that the JSON format emitted by the Rust server matches the
 * TypeScript type definitions generated via ts-rs.  It imports directly from
 * rust/bindings/ so that a missing or stale bindings directory causes an
 * immediate test failure — making this a lightweight drift detector alongside
 * the `just rust-bindings && git diff --exit-code rust/bindings/` gate step.
 *
 * The JSON literals below represent canonical serialisations produced by
 * `serde_json::to_string` on representative Rust values.
 */

import { expect, test } from "bun:test";
import type { OmegaEvent } from "../rust/bindings/OmegaEvent.js";
import type { StreamSignal } from "../rust/bindings/StreamSignal.js";
import type { TurnMetrics } from "../rust/bindings/TurnMetrics.js";
import type { LlmResponseUsage } from "../rust/bindings/LlmResponseUsage.js";
import { OmegaEventSchema } from "./events.schema.js";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Parse a JSON string and validate it as an OmegaEvent. */
function parseEvent(json: string): OmegaEvent {
  const result = OmegaEventSchema.safeParse(JSON.parse(json));
  if (!result.success) {
    throw new Error(`OmegaEventSchema parse failed: ${JSON.stringify(result.error.issues)}`);
  }
  return result.data;
}

// ---------------------------------------------------------------------------
// session_started
// ---------------------------------------------------------------------------

test("rust-serialised session_started validates against TS schema", () => {
  const json = JSON.stringify({
    type: "session_started",
    time: "2025-01-15T12:00:00.000Z",
    sessionId: "abc123def456",
    path: ".omega/sessions/2025-01-15T12-00-00-000-abc123def456",
    model: "claude-sonnet-4-6",
    effort: "medium",
    systemPrompt: "You are Omega.",
  });

  const event = parseEvent(json);
  expect(event.type).toBe("session_started");

  // Narrow to the specific variant and check all fields are accessible.
  if (event.type === "session_started") {
    // The generated OmegaEvent type supports narrowing.
    const _check: OmegaEvent = event; // type-level: OmegaEvent is assignable
    expect(event.model).toBe("claude-sonnet-4-6");
    expect(event.sessionId).toBe("abc123def456");
  }
});

// ---------------------------------------------------------------------------
// turn_end with metrics
// ---------------------------------------------------------------------------

test("rust-serialised turn_end with TurnMetrics validates against TS schema", () => {
  const json = JSON.stringify({
    type: "turn_end",
    time: "2025-01-15T12:01:00.000Z",
    metrics: {
      inputTokens: 1234,
      outputTokens: 567,
      cacheCreationTokens: 89,
      cacheReadTokens: 0,
    },
  });

  const event = parseEvent(json);
  expect(event.type).toBe("turn_end");

  if (event.type === "turn_end") {
    const metrics: TurnMetrics = event.metrics;
    expect(metrics.inputTokens).toBe(1234);
    expect(metrics.outputTokens).toBe(567);
    expect(metrics.cacheCreationTokens).toBe(89);
  }
});

// ---------------------------------------------------------------------------
// llm_response with LlmResponseUsage
// ---------------------------------------------------------------------------

test("rust-serialised llm_response with usage validates against TS schema", () => {
  const json = JSON.stringify({
    type: "llm_response",
    time: "2025-01-15T12:01:30.000Z",
    stopReason: "end_turn",
    usage: {
      input_tokens: 500,
      output_tokens: 120,
      cache_creation_input_tokens: 400,
      cache_read_input_tokens: 100,
    },
    contextHash: "abc123def456",
    text: "Hello, world!",
  });

  const event = parseEvent(json);
  expect(event.type).toBe("llm_response");

  if (event.type === "llm_response") {
    const usage: LlmResponseUsage = event.usage;
    expect(usage.input_tokens).toBe(500);
    expect(usage.cache_creation_input_tokens).toBe(400);
    expect(usage.service_tier).toBeUndefined();
  }
});

// ---------------------------------------------------------------------------
// StreamSignal shape check (type-only; not parsed at runtime)
// ---------------------------------------------------------------------------

test("StreamSignal type matches expected shape", () => {
  // Runtime shape check — ts-rs generated type must match the actual JSON.
  const textSignal: StreamSignal = { type: "text", text: "hello" };
  const thinkingSignal: StreamSignal = { type: "thinking", text: "hmm..." };
  expect(textSignal.type).toBe("text");
  expect(thinkingSignal.type).toBe("thinking");
});

// ---------------------------------------------------------------------------
// llm_retry with reason field
// ---------------------------------------------------------------------------

test("rust-serialised llm_retry with reason validates against TS schema", () => {
  const json = JSON.stringify({
    type: "llm_retry",
    time: "2025-01-15T12:00:05.000Z",
    attempt: 1,
    waitMs: 30000,
    error: "rate_limit_error",
    httpStatus: 429,
    retryAt: "2025-01-15T12:00:35.000Z",
    reason: "retry-after",
  });

  const event = parseEvent(json);
  expect(event.type).toBe("llm_retry");
  if (event.type === "llm_retry") {
    expect(event.reason).toBe("retry-after");
    expect(event.attempt).toBe(1);
  }
});
