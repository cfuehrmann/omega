/**
 * Stage 2 contract tests for the pause/resume WebSocket protocol.
 *
 * The agent's pause state machine is covered end-to-end in
 * `src/agent-pause.test.ts`. These tests verify only the transport layer:
 *
 *   1. `{type:"pause"}`  invokes Agent.requestPause and broadcasts
 *      `pause_requested` + updated `session_info` (turnState transitions).
 *   2. Reconnecting while the server is in `paused` yields a
 *      `session_info` with `turnState: "paused"` on the new socket.
 *   3. Reconnecting while the server is in `pause_requested` (before the
 *      seam has fired) yields `turnState: "pause_requested"`.
 *   4. Full turnState lifecycle over a single socket:
 *      idle → running → pause_requested → paused → running → idle.
 *
 * Each test spins up a real web server with a mock stream provider, drives
 * a short tool-use/text turn, and asserts on the observed WS traffic.
 */

import { describe, it, expect, afterEach } from "bun:test";
import type Anthropic from "@anthropic-ai/sdk";
import type { BetaRawMessageStreamEvent } from "@anthropic-ai/sdk/resources/beta/messages/messages.js";
import { runWebApp } from "./server.js";
import type { ServerMessage } from "./protocol.js";
import type { CreateMessageStream } from "../agent.js";
import { TEST_SESSIONS_ROOT } from "../session-dir.js";

// ---------------------------------------------------------------------------
// Mock provider helpers (mirrored from agent-pause.test.ts — small fixtures)
// ---------------------------------------------------------------------------

function makeMockStream(events: BetaRawMessageStreamEvent[], message: Anthropic.Beta.Messages.BetaMessage) {
  return {
    async *[Symbol.asyncIterator]() { for (const e of events) yield e; },
    finalMessage: async () => message,
  };
}

/**
 * Like `makeMockStream` but `finalMessage` blocks on an external promise.
 * Tests resolve `release` once they are ready for the LLM call to "complete".
 * This gives a deterministic window where the agent's turn is *running* but
 * the seam hasn't been reached — the only time `turnState === "pause_requested"`
 * is observable.
 */
function makeBlockingStream(
  events: BetaRawMessageStreamEvent[],
  message: Anthropic.Beta.Messages.BetaMessage,
): { stream: ReturnType<typeof makeMockStream>; release: () => void } {
  let resolveGate!: () => void;
  const gate = new Promise<void>(r => { resolveGate = r; });
  return {
    stream: {
      async *[Symbol.asyncIterator]() { for (const e of events) yield e; },
      finalMessage: async () => { await gate; return message; },
    },
    release: () => resolveGate(),
  };
}

function textMessage(text: string): Anthropic.Beta.Messages.BetaMessage {
  return {
    id: "msg_test",
    type: "message",
    role: "assistant",
    model: "claude-sonnet-4-6",
    container: null,
    content: [{ type: "text", text, citations: null }],
    stop_reason: "end_turn",
    stop_sequence: null,
    stop_details: null,
    context_management: null,
    usage: { input_tokens: 10, output_tokens: 5, cache_creation: null, cache_creation_input_tokens: null, cache_read_input_tokens: null, inference_geo: null, iterations: null, server_tool_use: null, service_tier: null, speed: null },
  };
}

function textStreamEvents(text: string): BetaRawMessageStreamEvent[] {
  return [
    { type: "content_block_start", index: 0, content_block: { type: "text", text: "", citations: null } },
    { type: "content_block_delta", index: 0, delta: { type: "text_delta", text } },
    { type: "content_block_stop", index: 0 },
    { type: "message_delta", context_management: null, delta: { stop_reason: "end_turn", stop_sequence: null, stop_details: null, container: null }, usage: { output_tokens: 5, cache_creation_input_tokens: null, cache_read_input_tokens: null, input_tokens: null, iterations: null, server_tool_use: null } },
    { type: "message_stop" },
  ];
}

function toolUseMessage(
  toolId: string,
  toolName: string,
  toolInput: unknown,
): Anthropic.Beta.Messages.BetaMessage {
  return {
    id: "msg_tool",
    type: "message",
    role: "assistant",
    model: "claude-sonnet-4-6",
    container: null,
    content: [{ type: "tool_use", id: toolId, name: toolName, input: toolInput, caller: { type: "direct" } }],
    stop_reason: "tool_use",
    stop_sequence: null,
    stop_details: null,
    context_management: null,
    usage: { input_tokens: 20, output_tokens: 10, cache_creation: null, cache_creation_input_tokens: null, cache_read_input_tokens: null, inference_geo: null, iterations: null, server_tool_use: null, service_tier: null, speed: null },
  };
}

function toolUseStreamEvents(toolName: string, toolId = "t1"): BetaRawMessageStreamEvent[] {
  return [
    { type: "content_block_start", index: 0, content_block: { type: "tool_use", id: toolId, name: toolName, input: {} } },
    { type: "content_block_delta", index: 0, delta: { type: "input_json_delta", partial_json: "{}" } },
    { type: "content_block_stop", index: 0 },
    { type: "message_delta", context_management: null, delta: { stop_reason: "tool_use", stop_sequence: null, stop_details: null, container: null }, usage: { output_tokens: 10, cache_creation_input_tokens: null, cache_read_input_tokens: null, input_tokens: null, iterations: null, server_tool_use: null } },
    { type: "message_stop" },
  ];
}

/** Provider: first call → read_file tool_use; second call → plain text "done". */
function readFileThenTextProvider(): CreateMessageStream {
  let call = 0;
  return () => {
    call++;
    if (call === 1) {
      return makeMockStream(
        toolUseStreamEvents("read_file"),
        toolUseMessage("t1", "read_file", { path: "package.json" }),
      );
    }
    return makeMockStream(textStreamEvents("done"), textMessage("done"));
  };
}

// ---------------------------------------------------------------------------
// WS helpers
// ---------------------------------------------------------------------------

function waitOpen(ws: WebSocket): Promise<void> {
  return new Promise((resolve, reject) => {
    ws.onopen = () => resolve();
    ws.onerror = (e) => reject(new Error(`WS open error: ${JSON.stringify(e)}`));
  });
}

/**
 * Attach a message collector to a WebSocket. Returns a handle with:
 *   - messages:     every parsed ServerMessage received so far.
 *   - waitFor(pred): resolves with the first message matching `pred`, or
 *                   rejects on timeout (default 5 s).
 *   - close():      detach the onmessage handler (no-op on ws.close).
 */
function collect(ws: WebSocket): {
  messages: ServerMessage[];
  waitFor: (pred: (m: ServerMessage) => boolean, timeoutMs?: number) => Promise<ServerMessage>;
  close: () => void;
} {
  const messages: ServerMessage[] = [];
  const waiters: { pred: (m: ServerMessage) => boolean; resolve: (m: ServerMessage) => void; reject: (e: Error) => void; timer: ReturnType<typeof setTimeout> }[] = [];

  ws.onmessage = (ev) => {
    const msg = JSON.parse(ev.data as string) as ServerMessage;
    messages.push(msg);
    for (let i = waiters.length - 1; i >= 0; i--) {
      const w = waiters[i]!;
      if (w.pred(msg)) {
        clearTimeout(w.timer);
        waiters.splice(i, 1);
        w.resolve(msg);
      }
    }
  };

  return {
    messages,
    waitFor(pred, timeoutMs = 5000) {
      // Scan already-received messages first — avoids races where the event
      // fires before the caller can set up its waiter.
      const existing = messages.find(pred);
      if (existing) return Promise.resolve(existing);
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => {
          reject(new Error(`Timed out after ${timeoutMs}ms waiting for message`));
        }, timeoutMs);
        waiters.push({ pred, resolve, reject, timer });
      });
    },
    close() { ws.onmessage = null; },
  };
}

// ---------------------------------------------------------------------------
// Server lifecycle — each test captures the Bun.Server for clean shutdown
// ---------------------------------------------------------------------------

async function startServer(port: number, streamProvider: CreateMessageStream): Promise<{ stop: () => void }> {
  let bunServer: { stop: (force?: boolean) => void } | undefined;
  const origServe = Bun.serve.bind(Bun);
  (Bun as unknown as { serve: unknown }).serve = (opts: unknown) => {
    bunServer = (origServe as (o: unknown) => { stop: (force?: boolean) => void })(opts);
    (Bun as unknown as { serve: unknown }).serve = origServe;
    return bunServer;
  };
  await runWebApp({ port, streamProvider, sessionsRoot: TEST_SESSIONS_ROOT });
  return { stop: () => bunServer?.stop(true) };
}

/** Open a WS, wait for `ready`, send `reset`, and wait for `reset_done`. */
async function newSessionSocket(port: number): Promise<{ ws: WebSocket; c: ReturnType<typeof collect> }> {
  const ws = new WebSocket(`ws://localhost:${port}`);
  await waitOpen(ws);
  const c = collect(ws);
  await c.waitFor(m => m.type === "ready");
  ws.send(JSON.stringify({ type: "reset" }));
  await c.waitFor(m => m.type === "reset_done");
  return { ws, c };
}

function isSessionInfo(m: ServerMessage): m is Extract<ServerMessage, { type: "session_info" }> {
  return m.type === "session_info";
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("pause/resume WebSocket protocol", () => {
  let server: { stop: () => void } | undefined;
  const sockets: WebSocket[] = [];

  afterEach(async () => {
    for (const ws of sockets.splice(0)) {
      try { ws.close(); } catch { /* ignore */ }
    }
    await new Promise(r => setTimeout(r, 20));
    server?.stop();
    server = undefined;
  });

  it("forwards pause to the agent and broadcasts pause_requested + turnState", async () => {
    const port = 47830;
    server = await startServer(port, readFileThenTextProvider());
    const { ws, c } = await newSessionSocket(port);
    sockets.push(ws);

    // session_info arrives as part of the reset burst; initial turnState = idle.
    const initialInfo = c.messages.find(isSessionInfo);
    expect(initialInfo?.turnState).toBe("idle");

    ws.send(JSON.stringify({ type: "message", content: "read config" }));
    // Wait until the running transition lands.
    await c.waitFor(m => isSessionInfo(m) && m.turnState === "running");

    // Fire pause — server must broadcast pause_requested and flip turnState.
    ws.send(JSON.stringify({ type: "pause" }));
    const pauseReqEvent = await c.waitFor(m => m.type === "pause_requested");
    expect(pauseReqEvent.type).toBe("pause_requested");

    const pauseReqInfo = await c.waitFor(m => isSessionInfo(m) && m.turnState === "pause_requested");
    expect((pauseReqInfo as { turnState?: string }).turnState).toBe("pause_requested");

    // Seam fires after tool_result → turn_paused + turnState = paused.
    await c.waitFor(m => m.type === "turn_paused");
    await c.waitFor(m => isSessionInfo(m) && m.turnState === "paused");

    // Continue without interjection → turnState runs again, turn finishes.
    ws.send(JSON.stringify({ type: "continue" }));
    await c.waitFor(m => m.type === "turn_continued");
    await c.waitFor(m => isSessionInfo(m) && m.turnState === "running");
    await c.waitFor(m => m.type === "turn_end");
    await c.waitFor(m => isSessionInfo(m) && m.turnState === "idle");
  });

  it("exposes turnState='paused' on a reconnecting socket", async () => {
    const port = 47831;
    server = await startServer(port, readFileThenTextProvider());
    const { ws: ws1, c: c1 } = await newSessionSocket(port);
    sockets.push(ws1);

    ws1.send(JSON.stringify({ type: "message", content: "read config" }));
    // Wait for the running transition so the pause handler's
    // `currentTurnState === "running"` gate accepts our pause.
    await c1.waitFor(m => isSessionInfo(m) && m.turnState === "running");
    ws1.send(JSON.stringify({ type: "pause" }));
    // Let the seam fire on the server side.
    await c1.waitFor(m => isSessionInfo(m) && m.turnState === "paused");

    // Drop ws1 and reconnect.
    c1.close();
    ws1.close();
    await new Promise(r => setTimeout(r, 30));

    const ws2 = new WebSocket(`ws://localhost:${port}`);
    sockets.push(ws2);
    await waitOpen(ws2);
    const c2 = collect(ws2);

    const info = await c2.waitFor(isSessionInfo);
    expect((info as { turnState?: string }).turnState).toBe("paused");

    // Cleanup: continue so the turn ends before the test tears down.
    ws2.send(JSON.stringify({ type: "continue" }));
    await c2.waitFor(m => m.type === "turn_end");
  });

  it("exposes turnState='pause_requested' on reconnect before the seam fires", async () => {
    // Blocking provider: first call's finalMessage awaits a test-owned gate.
    // This keeps the turn in 'running' long enough to pause before the seam.
    const { stream: blocking, release } = makeBlockingStream(
      toolUseStreamEvents("read_file"),
      toolUseMessage("t1", "read_file", { path: "package.json" }),
    );
    let call = 0;
    const provider: CreateMessageStream = () => {
      call++;
      if (call === 1) return blocking;
      return makeMockStream(textStreamEvents("done"), textMessage("done"));
    };

    const port = 47832;
    server = await startServer(port, provider);
    const { ws: ws1, c: c1 } = await newSessionSocket(port);
    sockets.push(ws1);

    ws1.send(JSON.stringify({ type: "message", content: "read config" }));
    await c1.waitFor(m => m.type === "llm_call");

    // Pause now — finalMessage still blocked, so the seam has not fired.
    ws1.send(JSON.stringify({ type: "pause" }));
    await c1.waitFor(m => isSessionInfo(m) && m.turnState === "pause_requested");

    // Reconnect; session_info on the new socket must report pause_requested.
    c1.close();
    ws1.close();
    await new Promise(r => setTimeout(r, 30));

    const ws2 = new WebSocket(`ws://localhost:${port}`);
    sockets.push(ws2);
    await waitOpen(ws2);
    const c2 = collect(ws2);

    const info = await c2.waitFor(isSessionInfo);
    expect((info as { turnState?: string }).turnState).toBe("pause_requested");

    // Release the LLM gate so the turn completes cleanly.
    release();
    await c2.waitFor(m => isSessionInfo(m) && m.turnState === "paused");
    ws2.send(JSON.stringify({ type: "continue" }));
    await c2.waitFor(m => m.type === "turn_end");
  });

  it("ignores pause when no turn is running", async () => {
    const port = 47833;
    server = await startServer(port, readFileThenTextProvider());
    const { ws, c } = await newSessionSocket(port);
    sockets.push(ws);

    // idle → pause should be a no-op (no pause_requested event, no session_info transition).
    ws.send(JSON.stringify({ type: "pause" }));

    // Give the server a moment to (not) do anything.
    await new Promise(r => setTimeout(r, 80));

    const sawPauseReq = c.messages.some(m => m.type === "pause_requested");
    const sawPauseReqInfo = c.messages.some(m => isSessionInfo(m) && m.turnState === "pause_requested");
    expect(sawPauseReq).toBe(false);
    expect(sawPauseReqInfo).toBe(false);
  });
});
