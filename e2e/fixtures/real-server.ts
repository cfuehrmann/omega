/**
 * Real-server fixture for Playwright e2e tests.
 *
 * Wraps the production runWebApp() with a mock CreateMessageStream so no real
 * Anthropic API calls are made. Runs on port 3003 (distinct from the
 * test-server on 3001).
 *
 * This fixture exists specifically to catch bugs in the production server
 * code path (src/web/server.ts) that the test-server bypasses — e.g.
 * incorrect Agent constructor arguments, wrong file paths, etc.
 *
 * Control API (port 3004):
 *   GET /control/ready — health check
 */

import type Anthropic from "@anthropic-ai/sdk";
import { runWebApp } from "../../src/web/server.js";
import type { CreateMessageStream } from "../../src/agent.js";
import { TEST_SESSIONS_ROOT } from "../../src/session-dir.js";

export const REAL_SERVER_PORT = 3003;
const CTRL_PORT = 3004;

// ---------------------------------------------------------------------------
// Mock CreateMessageStream — routes by trigger message content
// ---------------------------------------------------------------------------

function makeMockStream(events: any[], message: Anthropic.Beta.Messages.BetaMessage) {
  return {
    async *[Symbol.asyncIterator]() {
      for (const e of events) yield e;
    },
    finalMessage: async () => message,
  };
}

function makePongStream() {
  const message: Anthropic.Beta.Messages.BetaMessage = {
    id: "msg_test",
    type: "message",
    role: "assistant",
    model: "claude-sonnet-4-6",
    container: null,
    content: [{ type: "text", text: "pong", citations: null }],
    stop_reason: "end_turn",
    stop_sequence: null,
    context_management: null,
    usage: { input_tokens: 10, output_tokens: 5, cache_creation: null, cache_creation_input_tokens: null, cache_read_input_tokens: null, inference_geo: null, iterations: null, server_tool_use: null, service_tier: null, speed: null },
  };
  const events = [
    { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } },
    { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "pong" } },
    { type: "content_block_stop", index: 0 },
    { type: "message_delta", delta: { stop_reason: "end_turn" }, usage: { output_tokens: 5 } },
    { type: "message_stop" },
  ];
  return makeMockStream(events, message);
}

/**
 * Returns a tool_use stream that invokes run_command with "sleep 10".
 * Used by the abort-during-tool-execution Playwright test.
 */
function makeSleepToolUseStream() {
  const TOOL_ID = "toolu_sleep_abort_test";
  const message: Anthropic.Beta.Messages.BetaMessage = {
    id: "msg_sleep_test",
    type: "message",
    role: "assistant",
    model: "claude-sonnet-4-6",
    container: null,
    content: [{ type: "tool_use", id: TOOL_ID, name: "run_command", input: { command: "sleep 10" }, caller: { type: "direct" } }],
    stop_reason: "tool_use",
    stop_sequence: null,
    context_management: null,
    usage: { input_tokens: 20, output_tokens: 10, cache_creation: null, cache_creation_input_tokens: null, cache_read_input_tokens: null, inference_geo: null, iterations: null, server_tool_use: null, service_tier: null, speed: null },
  };
  const events = [
    { type: "content_block_start", index: 0, content_block: { type: "tool_use", id: TOOL_ID, name: "run_command" } },
    { type: "content_block_delta", index: 0, delta: { type: "input_json_delta", partial_json: '{"command":"sleep 10"}' } },
    { type: "content_block_stop", index: 0 },
    { type: "message_delta", delta: { stop_reason: "tool_use" }, usage: { output_tokens: 10 } },
    { type: "message_stop" },
  ];
  return makeMockStream(events, message);
}

const mockCreateMessageStream: CreateMessageStream = (params) => {
  // Inspect the last user message to route to the right mock response.
  // Simple messages arrive as a plain string; tool_result turns arrive as
  // an array of blocks — in that case we look at the most recent text block.
  const lastUserMsg = [...params.messages].reverse().find(m => m.role === "user");
  const rawContent = lastUserMsg?.content ?? "";
  const textContent =
    typeof rawContent === "string"
      ? rawContent
      : (rawContent as any[]).find((b: any) => b.type === "text")?.text ?? "";

  if (textContent.includes("abort_sleep_test")) {
    return makeSleepToolUseStream();
  }

  return makePongStream();
};

// ---------------------------------------------------------------------------
// Start the real server
// ---------------------------------------------------------------------------

await runWebApp({ streamProvider: mockCreateMessageStream, port: REAL_SERVER_PORT, sessionsRoot: TEST_SESSIONS_ROOT });

// ---------------------------------------------------------------------------
// Control server — health check only
// ---------------------------------------------------------------------------

Bun.serve({
  port: CTRL_PORT,
  fetch(req) {
    const url = new URL(req.url);
    if (url.pathname === "/control/ready") return new Response("ok");
    return new Response("Not found", { status: 404 });
  },
});

console.log(`Real server:  http://localhost:${REAL_SERVER_PORT}`);
console.log(`Control API:  http://localhost:${CTRL_PORT}/control/ready`);
