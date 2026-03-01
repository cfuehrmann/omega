/**
 * Lightweight WebSocket test server for Playwright e2e tests.
 *
 * Run by Playwright's webServer config as a Bun subprocess.
 * Speaks the same protocol as src/web/server.ts but without a real Agent.
 * Serves the built static frontend from src/web/public/.
 *
 * Two ports:
 *   3001 — main HTTP + WebSocket (what the browser connects to)
 *   3002 — control HTTP API (used by tests to inject events / read messages)
 *
 * Control API:
 *   POST /control/send   { event: object }   — send a WS event to the client
 *   POST /control/reset                      — reset event log + disconnect
 *   GET  /control/messages                   — drain received client messages
 *   GET  /control/ready                      — health check
 *
 * Persistence: in-memory event log only (no real Agent, no disk writes).
 * History replay on reconnect is served from the in-memory log — same
 * protocol as the real server, which reads events.jsonl written by Agent.
 */

import { join } from "path";
import { readFileSync, existsSync } from "fs";
import type { ServerWebSocket } from "bun";
import { closeOpenTurn, shouldLogEvent } from "../../src/web/server.js";

const MAIN_PORT = 3001;
const CTRL_PORT = 3002;
const PUBLIC_DIR = join(import.meta.dir, "../../src/web/public");

// ---------------------------------------------------------------------------
// Static file serving
// ---------------------------------------------------------------------------

const MIME: Record<string, string> = {
  ".html":  "text/html; charset=utf-8",
  ".js":    "application/javascript; charset=utf-8",
  ".mjs":   "application/javascript; charset=utf-8",
  ".css":   "text/css; charset=utf-8",
  ".json":  "application/json",
  ".ico":   "image/x-icon",
  ".svg":   "image/svg+xml",
  ".png":   "image/png",
  ".woff2": "font/woff2",
};

function serveStatic(pathname: string): Response | null {
  const rel = pathname === "/" ? "/index.html" : pathname;
  const safe = rel.replace(/\.\./g, "");
  const fullPath = join(PUBLIC_DIR, safe);
  if (!existsSync(fullPath)) return null;
  const ext = fullPath.match(/(\.[^.]+)$/)?.[1] ?? ".html";
  const mime = MIME[ext] ?? "application/octet-stream";
  return new Response(readFileSync(fullPath), {
    headers: { "Content-Type": mime },
  });
}

// ---------------------------------------------------------------------------
// Session state
// ---------------------------------------------------------------------------

let activeWs: ServerWebSocket<unknown> | null = null;
const receivedMessages: string[] = [];

/**
 * In-memory event log — mirrors the role of events.jsonl in the real server.
 * Only events that pass shouldLogEvent() are stored (same filter as real server).
 * Served as a `history` packet on reconnect, with closeOpenTurn applied.
 */
let eventLog: object[] = [];

/**
 * Monotonically increasing agent instance counter.
 * Increments on reset to let tests assert the agent was replaced.
 */
let currentAgentId = 1;

function resetState(): void {
  currentAgentId += 1;
  eventLog = [];
}

function sendWs(event: object): void {
  try { activeWs?.send(JSON.stringify(event)); } catch { /* ignore */ }
}

// ---------------------------------------------------------------------------
// Main server (browser-facing)
// ---------------------------------------------------------------------------

Bun.serve({
  port: MAIN_PORT,
  fetch(req, srv) {
    if (srv.upgrade(req)) return undefined as any;
    const url = new URL(req.url);
    const res = serveStatic(url.pathname);
    if (res) return res;
    return new Response("Not found", { status: 404 });
  },
  websocket: {
    open(ws) {
      activeWs = ws;
      // Replay event log (with crash recovery) before signalling connected
      const replay = closeOpenTurn(eventLog.filter(shouldLogEvent));
      if (replay.length > 0) {
        ws.send(JSON.stringify({ type: "history", events: replay }));
      }
      ws.send(JSON.stringify({ type: "connected" }));
    },
    message(ws, data) {
      const str = String(data);
      let msg: any;
      try { msg = JSON.parse(str); } catch { receivedMessages.push(str); return; }

      if (msg.type === "reset") {
        resetState();
        ws.send(JSON.stringify({ type: "history", events: [] }));
        ws.send(JSON.stringify({ type: "reset_done" }));
        ws.send(JSON.stringify({ type: "turn_ready" }));
        return;
      }

      receivedMessages.push(str);
    },
    close() {
      activeWs = null;
    },
  },
});

// ---------------------------------------------------------------------------
// Control server (test-facing)
// ---------------------------------------------------------------------------

Bun.serve({
  port: CTRL_PORT,
  async fetch(req) {
    const url = new URL(req.url);

    if (req.method === "GET" && url.pathname === "/control/ready") {
      return new Response("ok");
    }

    if (req.method === "GET" && url.pathname === "/control/agent-id") {
      return new Response(JSON.stringify({ agentId: currentAgentId }), {
        headers: { "Content-Type": "application/json" },
      });
    }

    if (req.method === "GET" && url.pathname === "/control/messages") {
      const msgs = [...receivedMessages];
      receivedMessages.length = 0;
      return new Response(JSON.stringify(msgs), {
        headers: { "Content-Type": "application/json" },
      });
    }

    if (req.method === "POST" && url.pathname === "/control/send") {
      const body = await req.json() as { event: object };
      const event = body.event;
      // Store in log (same filter the real server uses)
      if (shouldLogEvent(event)) {
        const t = (event as any).type as string;
        // auth deduplication — same as real server
        if (t === "auth") {
          const idx = eventLog.findIndex((e: any) => e.type === "auth");
          if (idx !== -1) eventLog.splice(idx, 1);
        }
        eventLog.push(event);
      }
      sendWs(event);
      return new Response("ok");
    }

    if (req.method === "POST" && url.pathname === "/control/reset") {
      eventLog = [];
      receivedMessages.length = 0;
      currentAgentId = 1;
      return new Response("ok");
    }

    if (req.method === "POST" && url.pathname === "/control/save") {
      // No-op: persistence is in-memory only. Exists so e2e tests that call
      // /control/save continue to work without modification.
      return new Response("ok");
    }

    if (req.method === "POST" && url.pathname === "/control/load") {
      // No-op: in-memory log is already the source of truth.
      // The real server reads events.jsonl; here the in-memory log IS events.jsonl.
      return new Response("ok");
    }

    if (req.method === "GET" && url.pathname === "/control/disk-snapshot") {
      // Return the in-memory log — equivalent to reading events.jsonl in the real server.
      return new Response(JSON.stringify(eventLog), {
        headers: { "Content-Type": "application/json" },
      });
    }

    return new Response("Not found", { status: 404 });
  },
});

console.log(`Test server:   http://localhost:${MAIN_PORT}`);
console.log(`Control API:   http://localhost:${CTRL_PORT}/control/ready`);
