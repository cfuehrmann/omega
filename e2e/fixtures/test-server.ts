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
 */

import { join } from "path";
import { readFileSync, existsSync } from "fs";
import { readFile, writeFile, unlink, mkdir } from "fs/promises";
import type { ServerWebSocket } from "bun";

const MAIN_PORT = 3001;
const CTRL_PORT = 3002;
const PUBLIC_DIR = join(import.meta.dir, "../../src/web/public");

// Session persistence (mirrors src/web/session-store.ts logic)
const SESSIONS_DIR = join(import.meta.dir, "../../sessions-test");
const SESSION_FILE = join(SESSIONS_DIR, "current.jsonl");

async function persistToDisk(events: object[]): Promise<void> {
  await mkdir(SESSIONS_DIR, { recursive: true });
  const lines = events.map(e => JSON.stringify(e)).join("\n");
  await writeFile(SESSION_FILE, lines, "utf8");
}

async function loadFromDisk(): Promise<object[]> {
  if (!existsSync(SESSION_FILE)) return [];
  try {
    const text = await readFile(SESSION_FILE, "utf8");
    const result: object[] = [];
    for (const line of text.split("\n")) {
      const t = line.trim();
      if (t) {
        try { result.push(JSON.parse(t)); } catch { /* skip */ }
      }
    }
    return result;
  } catch {
    return [];
  }
}

async function clearDisk(): Promise<void> {
  if (existsSync(SESSION_FILE)) await unlink(SESSION_FILE);
}

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
let eventLog: object[] = [];

function sendEvent(event: object): void {
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
      // Replay event log first, then signal connected
      if (eventLog.length > 0) {
        ws.send(JSON.stringify({ type: "history", events: eventLog }));
      }
      ws.send(JSON.stringify({ type: "connected" }));
    },
    message(_ws, data) {
      receivedMessages.push(String(data));
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
      // Log persistent events (same exclusion as real server)
      const t = (event as any).type as string;
      if (!["connected", "turn_ready"].includes(t)) {
        if (t === "auth") {
          const idx = eventLog.findIndex((e: any) => e.type === "auth");
          if (idx !== -1) eventLog.splice(idx, 1);
        }
        eventLog.push(event);
        // Persist after turn_end (mirrors real server behaviour)
        if (t === "turn_end") {
          await persistToDisk(eventLog);
        }
      }
      sendEvent(event);
      return new Response("ok");
    }

    if (req.method === "POST" && url.pathname === "/control/reset") {
      eventLog.length = 0;
      eventLog = [];
      receivedMessages.length = 0;
      await clearDisk();
      return new Response("ok");
    }

    if (req.method === "POST" && url.pathname === "/control/save") {
      await persistToDisk(eventLog);
      return new Response("ok");
    }

    if (req.method === "POST" && url.pathname === "/control/load") {
      // Simulate restart: replace in-memory log with disk contents
      eventLog = await loadFromDisk();
      return new Response("ok");
    }

    if (req.method === "GET" && url.pathname === "/control/disk-snapshot") {
      const disk = await loadFromDisk();
      return new Response(JSON.stringify(disk), {
        headers: { "Content-Type": "application/json" },
      });
    }

    return new Response("Not found", { status: 404 });
  },
});

console.log(`Test server:   http://localhost:${MAIN_PORT}`);
console.log(`Control API:   http://localhost:${CTRL_PORT}/control/ready`);
