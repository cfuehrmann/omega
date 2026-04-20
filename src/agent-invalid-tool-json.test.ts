/**
 * Invalid-tool-JSON recovery tests.
 *
 * When the model emits tool_use input that is not valid JSON, the Anthropic
 * SDK throws `AnthropicError("Unable to parse tool parameter JSON from model…")`
 * during stream finalisation. The agent's recovery policy:
 *
 *   1. Transparent retry up to twice (2 retries = 3 total calls per
 *      streamLlmCall cycle) with exponential backoff.
 *   2. On exhaustion, append a synthetic corrective `user_message` to history
 *      and restart the agentic loop so the model can self-correct.
 *   3. Bounded at 2 feedback cycles per turn; then fall through to
 *      `agent_error` + `turn_interrupted{reason:"error"}`.
 *
 * These tests exercise the three recovery modes end-to-end through a mock
 * `CreateMessageStream`. They use the same pattern as `agent-rate-limit.test.ts`.
 */

import { describe, it, expect, afterEach } from "bun:test";
import { Agent, type OmegaEvent, type StreamSignal } from "./agent.js";
import type { CreateMessageStream } from "./agent.js";
import type { BetaRawMessageStreamEvent } from "@anthropic-ai/sdk/resources/beta/messages/messages.js";
import { makeTestAgent } from "./test-utils.js";

/** Mimic the exact error shape thrown by BetaMessageStream on malformed JSON. */
function invalidToolJsonError() {
  return new Error(
    'Unable to parse tool parameter JSON from model. Please retry your request or adjust your prompt. Error: SyntaxError: JSON Parse error: Unterminated string. JSON: {"path": "a.txt", "content": "unescaped\nnewline"}',
  );
}

/** Minimal success stream — text-only, no tool use. */
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

async function collectEvents(agent: Agent, message: string): Promise<(OmegaEvent | StreamSignal)[]> {
  const events: (OmegaEvent | StreamSignal)[] = [];
  for await (const event of agent.sendMessage(message, async () => true, undefined)) {
    events.push(event);
  }
  return events;
}

const disposeAll: (() => void)[] = [];
afterEach(() => { disposeAll.splice(0).forEach(d => d()); });

describe("invalid tool JSON recovery", () => {
  // -----------------------------------------------------------------------
  // 1. Transparent retry succeeds
  // -----------------------------------------------------------------------
  it("two failures then success — 2 × llm_retry, no feedback", async () => {
    process.env.OMEGA_RETRY_BASE_MS = "1";
    process.env.OMEGA_RETRY_MAX_MS  = "2";

    let callCount = 0;
    const success = makeSuccessProvider();
    const provider: CreateMessageStream = (params) => {
      callCount++;
      if (callCount <= 2) throw invalidToolJsonError();
      return success(params);
    };

    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "hello");

    expect(callCount).toBe(3);

    const retries = events.filter(e => e.type === "llm_retry") as any[];
    expect(retries).toHaveLength(2);
    // Ordinary policy-driven retries — no `reason` set (that field is
    // reserved for retry-after server-wins).
    expect(retries.every(r => r.reason === undefined)).toBe(true);

    // No llm_error emitted (retries swallowed the failures).
    expect(events.filter(e => e.type === "llm_error")).toHaveLength(0);
    // Exactly one user_message — the original "hello". No feedback synthesised.
    const userMsgs = events.filter(e => e.type === "user_message") as any[];
    expect(userMsgs).toHaveLength(1);
    expect(userMsgs[0].content).toBe("hello");

    const last = events[events.length - 1] as any;
    expect(last.type).toBe("turn_end");

    delete process.env.OMEGA_RETRY_BASE_MS;
    delete process.env.OMEGA_RETRY_MAX_MS;
  });

  // -----------------------------------------------------------------------
  // 2. Feedback retry succeeds (one feedback cycle, then success)
  // -----------------------------------------------------------------------
  it("three failures then success — feedback nudge appended, 4th call succeeds", async () => {
    process.env.OMEGA_RETRY_BASE_MS = "1";
    process.env.OMEGA_RETRY_MAX_MS  = "2";

    let callCount = 0;
    const success = makeSuccessProvider();
    const provider: CreateMessageStream = (params) => {
      callCount++;
      if (callCount <= 3) throw invalidToolJsonError();
      return success(params);
    };

    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "hello");

    // 3 failures exhaust the first streamLlmCall cycle (1 initial + 2 retries).
    // Feedback nudge is appended; the 4th call succeeds.
    expect(callCount).toBe(4);

    // Exactly 2 llm_retry events (from the first cycle).
    const retries = events.filter(e => e.type === "llm_retry") as any[];
    expect(retries).toHaveLength(2);

    // Exactly 1 llm_error event (the first cycle's terminal failure).
    expect(events.filter(e => e.type === "llm_error")).toHaveLength(1);

    // user_messages: the original "hello" plus the synthetic feedback.
    const userMsgs = events.filter(e => e.type === "user_message") as any[];
    expect(userMsgs).toHaveLength(2);
    expect(userMsgs[0].content).toBe("hello");
    expect(userMsgs[1].content).toContain("could not be parsed");
    expect(userMsgs[1].content).toContain("JSON string escaping");

    // No agent_error / turn_interrupted — we recovered.
    expect(events.filter(e => e.type === "agent_error")).toHaveLength(0);
    expect(events.filter(e => e.type === "turn_interrupted")).toHaveLength(0);

    const last = events[events.length - 1] as any;
    expect(last.type).toBe("turn_end");

    // History: the original user msg + the feedback user msg (+ the
    // assistant's final success response). Exactly one feedback entry.
    const history = agent.getCompactedContextHistory();
    const feedbackInHistory = history.filter(m => {
      if (m.role !== "user") return false;
      const content = m.content;
      if (typeof content === "string") return content.includes("could not be parsed");
      if (Array.isArray(content)) {
        return content.some((b: any) =>
          b?.type === "text" && typeof b.text === "string" && b.text.includes("could not be parsed"),
        );
      }
      return false;
    });
    expect(feedbackInHistory).toHaveLength(1);

    delete process.env.OMEGA_RETRY_BASE_MS;
    delete process.env.OMEGA_RETRY_MAX_MS;
  });

  // -----------------------------------------------------------------------
  // 3. Full exhaustion — indefinite failures, 2 feedback cycles then give up
  // -----------------------------------------------------------------------
  it("indefinite failure — 2 feedback cycles then agent_error + turn_interrupted", async () => {
    process.env.OMEGA_RETRY_BASE_MS = "1";
    process.env.OMEGA_RETRY_MAX_MS  = "2";

    let callCount = 0;
    const provider: CreateMessageStream = () => {
      callCount++;
      throw invalidToolJsonError();
    };

    const { agent, dispose } = await makeTestAgent(provider);
    disposeAll.push(dispose);
    const events = await collectEvents(agent, "hello");

    // 3 cycles × 3 API calls each = 9 total calls
    //   (cycle = 1 initial + 2 retries before the policy gives up).
    expect(callCount).toBe(9);

    // 6 llm_retry events (2 per cycle × 3 cycles).
    const retries = events.filter(e => e.type === "llm_retry") as any[];
    expect(retries).toHaveLength(6);

    // 3 llm_error events (one per cycle).
    expect(events.filter(e => e.type === "llm_error")).toHaveLength(3);

    // user_messages: original "hello" + 2 feedback nudges.
    const userMsgs = events.filter(e => e.type === "user_message") as any[];
    expect(userMsgs).toHaveLength(3);
    expect(userMsgs[0].content).toBe("hello");
    expect(userMsgs[1].content).toContain("could not be parsed");
    expect(userMsgs[2].content).toContain("could not be parsed");

    // Terminal path: generic agent_error ("API error: …") + turn_interrupted.
    const agentErrors = events.filter(e => e.type === "agent_error") as any[];
    expect(agentErrors).toHaveLength(1);
    expect(agentErrors[0].error).toContain("API error");
    expect(agentErrors[0].error).toContain("Unable to parse tool parameter JSON");

    const last = events[events.length - 1] as any;
    expect(last.type).toBe("turn_interrupted");
    expect(last.reason).toBe("error");

    // Exactly 2 feedback messages in history (plus the original user message).
    const history = agent.getCompactedContextHistory();
    const feedbackInHistory = history.filter(m => {
      if (m.role !== "user") return false;
      const content = m.content;
      if (Array.isArray(content)) {
        return content.some((b: any) =>
          b?.type === "text" && typeof b.text === "string" && b.text.includes("could not be parsed"),
        );
      }
      return false;
    });
    expect(feedbackInHistory).toHaveLength(2);

    delete process.env.OMEGA_RETRY_BASE_MS;
    delete process.env.OMEGA_RETRY_MAX_MS;
  });
});
