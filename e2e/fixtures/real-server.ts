/**
 * Real-server fixture for Playwright e2e tests.
 *
 * Wraps the production runWebApp() with a mock StreamProvider so no real
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
import type { StreamProvider } from "../../src/agent.js";

export const REAL_SERVER_PORT = 3003;
const CTRL_PORT = 3004;

// ---------------------------------------------------------------------------
// Mock StreamProvider — returns a fixed "pong" text response
// ---------------------------------------------------------------------------

function makeMockStream(events: any[], message: Anthropic.Message) {
  return {
    async *[Symbol.asyncIterator]() {
      for (const e of events) yield e;
    },
    finalMessage: async () => message,
  };
}

const mockStreamProvider: StreamProvider = async () => {
  const message: Anthropic.Message = {
    id: "msg_test",
    type: "message",
    role: "assistant",
    model: "claude-sonnet-4-6",
    container: null,
    content: [{ type: "text", text: "pong", citations: null }],
    stop_reason: "end_turn",
    stop_sequence: null,
    usage: { input_tokens: 10, output_tokens: 5, cache_creation: null, cache_creation_input_tokens: null, cache_read_input_tokens: null, inference_geo: null, server_tool_use: null, service_tier: null },
  };
  const events = [
    { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } },
    { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "pong" } },
    { type: "content_block_stop", index: 0 },
    { type: "message_delta", delta: { stop_reason: "end_turn" }, usage: { output_tokens: 5 } },
    { type: "message_stop" },
  ];
  return makeMockStream(events, message);
};

// ---------------------------------------------------------------------------
// Start the real server
// ---------------------------------------------------------------------------

await runWebApp({ streamProvider: mockStreamProvider, port: REAL_SERVER_PORT });

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
