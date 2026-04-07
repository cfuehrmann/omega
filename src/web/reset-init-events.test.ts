/**
 * Regression test: server_started and session_started events must be
 * forwarded to the WebSocket client immediately after a "reset" (new session)
 * — not only visible after a browser refresh.
 *
 * Root cause: the reset handler fires init() as a fire-and-forget promise
 * AFTER sending reset_done. The init events were written to events.jsonl but
 * never pushed to the open WebSocket. History replay (reconnect) read them
 * from disk, which is why a refresh appeared to "fix" the problem.
 */

import { describe, it, expect, afterEach } from "bun:test";
import { runWebApp } from "./server.js";
import type { ServerMessage } from "./protocol.js";
import type { StreamProvider } from "../agent.js";
import { TEST_SESSIONS_ROOT } from "../session-dir.js";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** A stream provider that should never be called in this test. */
const noopProvider: StreamProvider = () => {
  throw new Error("StreamProvider should not be called in this test");
};

/**
 * Collect WebSocket messages until a predicate returns true or timeout elapses.
 * Returns all collected messages.
 */
function collectMessages(
  ws: WebSocket,
  until: (msgs: ServerMessage[]) => boolean,
  timeoutMs = 5000,
): Promise<ServerMessage[]> {
  return new Promise((resolve, reject) => {
    const msgs: ServerMessage[] = [];
    const timer = setTimeout(() => {
      resolve(msgs); // return what we have; the test will assert on it
    }, timeoutMs);

    ws.onmessage = (ev) => {
      const msg = JSON.parse(ev.data as string) as ServerMessage;
      msgs.push(msg);
      if (until(msgs)) {
        clearTimeout(timer);
        resolve(msgs);
      }
    };

    ws.onerror = (ev) => {
      clearTimeout(timer);
      reject(new Error(`WebSocket error: ${JSON.stringify(ev)}`));
    };
  });
}

/** Resolve once the WebSocket is open. */
function waitOpen(ws: WebSocket): Promise<void> {
  return new Promise((resolve, reject) => {
    ws.onopen = () => resolve();
    ws.onerror = (e) => reject(new Error(`WS open error: ${JSON.stringify(e)}`));
  });
}

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

describe("reset — init events forwarded without reconnect", () => {
  let server: { stop: () => void } | undefined;
  let ws: WebSocket | undefined;

  afterEach(async () => {
    ws?.close();
    // Give the server a moment to handle the close before stopping
    await new Promise(r => setTimeout(r, 20));
    server?.stop?.();
    server = undefined;
    ws = undefined;
  });

  it("delivers server_started and session_started over the open socket after reset", async () => {
    // Pick an ephemeral port
    const port = 47821;

    // Start the server. runWebApp() doesn't return a handle — we'll stop it
    // via the returned Bun.Server reference stored in a module-level variable
    // inside server.ts. Instead, rely on the process-level cleanup or just
    // close the socket. We capture the Bun.Server by monkey-patching Bun.serve.
    let bunServer: { stop: (force?: boolean) => void } | undefined;
    const origServe = Bun.serve.bind(Bun);
    (Bun as any).serve = (opts: any) => {
      bunServer = origServe(opts);
      (Bun as any).serve = origServe; // restore
      return bunServer;
    };

    await runWebApp({
      port,
      streamProvider: noopProvider,
      sessionsRoot: TEST_SESSIONS_ROOT,
    });
    server = bunServer as any;

    // Open WebSocket connection
    ws = new WebSocket(`ws://localhost:${port}`);
    await waitOpen(ws);

    // First batch: wait for the initial "ready" (no session yet)
    const initialMsgs = await collectMessages(
      ws,
      msgs => msgs.some(m => m.type === "ready"),
    );
    expect(initialMsgs.some(m => m.type === "ready")).toBe(true);

    // Now send reset — this triggers new session creation
    ws.send(JSON.stringify({ type: "reset" }));

    // Collect until we see session_started (or timeout)
    const afterReset = await collectMessages(
      ws,
      msgs => msgs.some(m => m.type === "session_started"),
      5000,
    );

    const types = afterReset.map(m => m.type);

    // Core assertions: both init events must arrive WITHOUT reconnecting
    expect(types).toContain("server_started");
    expect(types).toContain("session_started");

    // reset_done must also arrive (sanity)
    expect(types).toContain("reset_done");
  });
});
