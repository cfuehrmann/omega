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
 * Mock provider routes on the FIRST user message (the original trigger). The
 * agent loop iterates; within a single turn we pick which step to return
 * based on how many assistant messages have already accumulated in
 * params.messages (0 = first call, 1 = second, etc.). Resumption calls are
 * detected by inspecting `params.system` (a plain string starting with
 * "Summarise the coding session…").
 *
 * Control API (port 3004):
 *   GET  /control/ready        — health check
 *   GET  /control/llm-calls    — returns captured LLM calls (see ProjectedCall)
 *   POST /control/reset-calls  — clears capture (tests call at beforeEach)
 */

import type Anthropic from "@anthropic-ai/sdk";
import { runWebApp } from "../../src/web/server.js";
import type { CreateMessageStream } from "../../src/agent.js";
import { TEST_SESSIONS_ROOT } from "../../src/session-dir.js";

export const REAL_SERVER_PORT = 3003;
const CTRL_PORT = 3004;

// ---------------------------------------------------------------------------
// LLM call history (for test inspection)
// ---------------------------------------------------------------------------

interface CapturedCall {
  messages: Anthropic.Beta.Messages.BetaMessageParam[];
  systemKind: "task" | "resumption";
  at: number;
}

const llmCallHistory: CapturedCall[] = [];

// ---------------------------------------------------------------------------
// Routing helpers
// ---------------------------------------------------------------------------

/** First user message text content (the turn's original trigger). */
function firstUserText(messages: Anthropic.Beta.Messages.BetaMessageParam[]): string {
  const first = messages.find(m => m.role === "user");
  if (!first) return "";
  const c = first.content;
  if (typeof c === "string") return c;
  if (Array.isArray(c)) {
    return (c as any[])
      .filter(b => b.type === "text")
      .map((b: any) => b.text)
      .join(" ");
  }
  return "";
}

/** How many LLM calls have already completed in this turn (0 = first call). */
function nthCallInTurn(messages: Anthropic.Beta.Messages.BetaMessageParam[]): number {
  return messages.filter(m => m.role === "assistant").length;
}

// ---------------------------------------------------------------------------
// Mock stream builders
// ---------------------------------------------------------------------------

function makeMessage(
  content: Anthropic.Beta.Messages.BetaContentBlock[],
  stopReason: Anthropic.Beta.Messages.BetaMessage["stop_reason"],
): Anthropic.Beta.Messages.BetaMessage {
  return {
    id: "msg_" + Math.random().toString(36).slice(2, 8),
    type: "message",
    role: "assistant",
    model: "claude-sonnet-4-6",
    container: null,
    content,
    stop_reason: stopReason,
    stop_sequence: null,
    stop_details: null,
    context_management: null,
    usage: {
      input_tokens: 10,
      output_tokens: 5,
      cache_creation: null,
      cache_creation_input_tokens: null,
      cache_read_input_tokens: null,
      inference_geo: null,
      iterations: null,
      server_tool_use: null,
      speed: null,
      service_tier: null,
    },
  };
}

function streamFromEvents(events: any[], message: Anthropic.Beta.Messages.BetaMessage) {
  return {
    async *[Symbol.asyncIterator]() {
      for (const e of events) yield e;
    },
    finalMessage: async () => message,
  };
}

function makeTextStream(text: string) {
  const message = makeMessage(
    [{ type: "text", text, citations: null }],
    "end_turn",
  );
  const events = [
    { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } },
    { type: "content_block_delta", index: 0, delta: { type: "text_delta", text } },
    { type: "content_block_stop", index: 0 },
    { type: "message_delta", delta: { stop_reason: "end_turn" }, usage: { output_tokens: 5 } },
    { type: "message_stop" },
  ];
  return streamFromEvents(events, message);
}

/**
 * Text response whose deltas are emitted over `delayMs` ms each — simulates
 * a long streaming LLM call so tests can click Pause while text is still
 * streaming in.
 */
function makeSlowTextStream(text: string, chunks: number, delayMs: number) {
  const message = makeMessage(
    [{ type: "text", text, citations: null }],
    "end_turn",
  );
  const chunkSize = Math.max(1, Math.ceil(text.length / chunks));
  return {
    async *[Symbol.asyncIterator]() {
      yield { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } };
      for (let i = 0; i < text.length; i += chunkSize) {
        const chunk = text.slice(i, i + chunkSize);
        await new Promise(r => setTimeout(r, delayMs));
        yield { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: chunk } };
      }
      yield { type: "content_block_stop", index: 0 };
      yield { type: "message_delta", delta: { stop_reason: "end_turn" }, usage: { output_tokens: 20 } };
      yield { type: "message_stop" };
    },
    finalMessage: async () => message,
  };
}

function makeToolUseStream(toolId: string, toolName: string, input: any) {
  const message = makeMessage(
    [{ type: "tool_use", id: toolId, name: toolName, input, caller: { type: "direct" } } as any],
    "tool_use",
  );
  const events = [
    { type: "content_block_start", index: 0, content_block: { type: "tool_use", id: toolId, name: toolName } },
    { type: "content_block_delta", index: 0, delta: { type: "input_json_delta", partial_json: JSON.stringify(input) } },
    { type: "content_block_stop", index: 0 },
    { type: "message_delta", delta: { stop_reason: "tool_use" }, usage: { output_tokens: 10 } },
    { type: "message_stop" },
  ];
  return streamFromEvents(events, message);
}

function makeConcurrentToolsStream(
  tools: Array<{ id: string; name: string; input: any }>,
) {
  const content = tools.map(t => ({
    type: "tool_use" as const,
    id: t.id,
    name: t.name,
    input: t.input,
    caller: { type: "direct" },
  })) as any;
  const message = makeMessage(content, "tool_use");
  const events: any[] = [];
  tools.forEach((t, i) => {
    events.push({
      type: "content_block_start",
      index: i,
      content_block: { type: "tool_use", id: t.id, name: t.name },
    });
    events.push({
      type: "content_block_delta",
      index: i,
      delta: { type: "input_json_delta", partial_json: JSON.stringify(t.input) },
    });
    events.push({ type: "content_block_stop", index: i });
  });
  events.push({ type: "message_delta", delta: { stop_reason: "tool_use" }, usage: { output_tokens: 10 } });
  events.push({ type: "message_stop" });
  return streamFromEvents(events, message);
}

// ---------------------------------------------------------------------------
// Mock CreateMessageStream
// ---------------------------------------------------------------------------

const mockCreateMessageStream: CreateMessageStream = (params) => {
  const isResumption =
    typeof params.system === "string" &&
    params.system.startsWith("Summarise the coding session");

  llmCallHistory.push({
    messages: params.messages as Anthropic.Beta.Messages.BetaMessageParam[],
    systemKind: isResumption ? "resumption" : "task",
    at: Date.now(),
  });

  if (isResumption) {
    // Stage 4 scenario 7 inspects llmCallHistory to assert the basis contains
    // "User (mid-turn): …". Return a well-formed summary/description so the
    // production resumption pipeline accepts the response.
    return makeTextStream(
      "<summary>Resumed session summary.</summary>\n<description>Resumed work.</description>",
    );
  }

  const firstText = firstUserText(params.messages);
  const nth = nthCallInTurn(params.messages);

  // --- Legacy real-server-replay.spec.ts triggers ---
  if (firstText.includes("abort_sleep_test")) {
    return makeToolUseStream("toolu_sleep_abort", "run_command", { command: "sleep 10" });
  }

  // --- Stage 4 pause/resume/interject triggers ---

  // Multi-tool: three sequential sleep calls, then text. The caller clicks
  // Pause while the 2nd tool is running; the seam fires after the 2nd
  // tool_result lands.
  if (firstText.includes("MULTI_TOOL_TEST")) {
    if (nth < 3) {
      return makeToolUseStream(`toolu_mt_${nth + 1}`, "run_command", { command: "sleep 0.6" });
    }
    return makeTextStream("done multi");
  }

  // Concurrent tools: one LLM call returns two tool_use blocks. Seam waits
  // for both to complete (Promise.all-equivalent at the seam check).
  if (firstText.includes("CONCURRENT_TOOLS_TEST")) {
    if (nth === 0) {
      return makeConcurrentToolsStream([
        { id: "toolu_ct_fast", name: "run_command", input: { command: "sleep 0.1" } },
        { id: "toolu_ct_slow", name: "run_command", input: { command: "sleep 1.5" } },
      ]);
    }
    return makeTextStream("done concurrent");
  }

  // Long streaming text — no tools. Pause during stream becomes a no-op at
  // the agent level (end_turn has no seam), but the server-side turnState
  // still transitions via pause_requested and the assistant text must not
  // be truncated.
  if (firstText.includes("LONG_STREAM_TEST")) {
    return makeSlowTextStream(
      "This is a deliberately long streaming response emitted in chunks. done stream",
      8,
      100,
    );
  }

  // Two pauses in one turn — four sleep tools then text.
  if (firstText.includes("TWO_PAUSES_TEST")) {
    if (nth < 4) {
      return makeToolUseStream(`toolu_tp_${nth + 1}`, "run_command", { command: "sleep 0.6" });
    }
    return makeTextStream("done two pauses");
  }

  // Resume basis: one sleep, then text. Test pauses, interjects, continues,
  // lets the turn end, then resumes the session.
  if (firstText.includes("RESUME_BASIS_TEST")) {
    if (nth === 0) {
      return makeToolUseStream("toolu_rb_1", "run_command", { command: "sleep 0.3" });
    }
    return makeTextStream("done basis");
  }

  // Default: simple "pong" used by the legacy replay spec.
  return makeTextStream("pong");
};

// ---------------------------------------------------------------------------
// Start the real server
// ---------------------------------------------------------------------------

await runWebApp({
  streamProvider: mockCreateMessageStream,
  port: REAL_SERVER_PORT,
  sessionsRoot: TEST_SESSIONS_ROOT,
});

// ---------------------------------------------------------------------------
// Control server — health check + LLM call inspection
// ---------------------------------------------------------------------------

Bun.serve({
  port: CTRL_PORT,
  fetch(req) {
    const url = new URL(req.url);
    if (url.pathname === "/control/ready") return new Response("ok");

    if (url.pathname === "/control/llm-calls") {
      // Project content blocks to a searchable string so tests can do
      // substring matches without JSON-walking. `role` is preserved so
      // tests can filter to user/assistant messages.
      const projected = llmCallHistory.map(c => ({
        systemKind: c.systemKind,
        at: c.at,
        messages: c.messages.map((m: any) => ({
          role: m.role,
          content:
            typeof m.content === "string"
              ? m.content
              : JSON.stringify(m.content),
        })),
      }));
      return new Response(JSON.stringify(projected), {
        headers: { "content-type": "application/json" },
      });
    }

    if (url.pathname === "/control/reset-calls" && req.method === "POST") {
      llmCallHistory.length = 0;
      return new Response("ok");
    }

    return new Response("Not found", { status: 404 });
  },
});

console.log(`Real server:  http://localhost:${REAL_SERVER_PORT}`);
console.log(`Control API:  http://localhost:${CTRL_PORT}/control/ready`);
