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
 *   POST /control/reset                      — reset event log + create new session dir
 *   GET  /control/messages                   — drain received client messages
 *   GET  /control/ready                      — health check
 *   GET  /control/disk-snapshot              — return events.jsonl as parsed JSON array
 *
 * Persistence: real disk writes to .omega/test-sessions/<timestamp>/ using the
 * same appendEvent() + makeSessionDir() machinery as the production server.
 * The test-sessions root is clearly distinct from .omega/sessions/ so test
 * data can never be confused with production session data.
 *
 * Each /control/reset creates a fresh timestamped session directory, exactly
 * as a real server restart would. History replay on reconnect reads the current
 * session's events.jsonl from disk — same protocol as src/web/server.ts.
 */

import { join } from "path";
import { readFileSync, existsSync } from "fs";
import { readFile, writeFile } from "fs/promises";
import type { ServerWebSocket } from "bun";
import { closeOpenTurn, shouldLogEvent } from "../../src/web/server.js";
import { appendEvent } from "../../src/event-store.js";
import { makeSessionDir, TEST_SESSIONS_ROOT, type SessionPaths } from "../../src/session-dir.js";
import type { OmegaEvent } from "../../src/events.js";
import { OmegaEventSchema } from "../../src/events.schema.js";

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
  const isHtml = ext === ".html";
  const isHashedAsset = pathname.startsWith("/assets/");
  const cacheControl = isHtml
    ? "no-cache"
    : isHashedAsset
      ? "public, max-age=31536000, immutable"
      : "no-cache";
  return new Response(readFileSync(fullPath), {
    headers: { "Content-Type": mime, "Cache-Control": cacheControl },
  });
}

// ---------------------------------------------------------------------------
// Session state
// ---------------------------------------------------------------------------

let activeWs: ServerWebSocket<unknown> | null = null;
const receivedMessages: string[] = [];

/**
 * Current session directory paths — created by makeSessionDir() on startup
 * and on each /control/reset. Events are persisted to eventsFile on disk.
 *
 * makeSessionDir() appends an 8-char random hex suffix to the timestamp, so
 * rapid resets within the same second produce distinct directory names without
 * any counter bookkeeping.
 */

let sessionPaths: SessionPaths = await makeSessionDir(new Date(), TEST_SESSIONS_ROOT);

/**
 * Monotonically increasing agent instance counter.
 * Increments on reset to let tests assert the agent was replaced.
 */
let currentAgentId = 1;

/**
 * Read the current session's events.jsonl and return the subset of events
 * suitable for history replay (same logic as the real server).
 */
async function loadReplayEvents(): Promise<object[]> {
  const { eventsFile } = sessionPaths;
  if (!existsSync(eventsFile)) return [];
  try {
    const text = await readFile(eventsFile, "utf-8");
    const events: OmegaEvent[] = [];
    for (const line of text.split("\n")) {
      const trimmed = line.trim();
      if (!trimmed) continue;
      try {
        const raw = JSON.parse(trimmed);
        const result = OmegaEventSchema.safeParse(raw);
        if (result.success && shouldLogEvent(result.data)) events.push(result.data);
      } catch { /* skip malformed lines */ }
    }
    return closeOpenTurn(events);
  } catch {
    return [];
  }
}

async function resetState(): Promise<void> {
  currentAgentId += 1;
  // Create a fresh uniquely-named session directory for the next session
  sessionPaths = await makeSessionDir(new Date(), TEST_SESSIONS_ROOT);
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
    async open(ws) {
      activeWs = ws;
      // Replay event log from disk (with crash recovery) before signalling connected.
      // After the await we're outside the auto-corked open callback — cork
      // explicitly so the history + connected frames are batched reliably.
      const replay = await loadReplayEvents();
      ws.cork(() => {
        if (replay.length > 0) {
          ws.send(JSON.stringify({ type: "history", events: replay }));
        }
        ws.send(JSON.stringify({ type: "ready" }));
      });
    },
    message(ws, data) {
      const str = String(data);
      let msg: any;
      try { msg = JSON.parse(str); } catch { receivedMessages.push(str); return; }

      if (msg.type === "reset") {
        resetState().then(() => {
          // After await (inside .then) — cork explicitly.
          ws.cork(() => {
            ws.send(JSON.stringify({ type: "history", events: [] }));
            ws.send(JSON.stringify({ type: "reset_done" }));
          });
        }).catch(() => {});
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
      const body = await req.json() as { event: Record<string, unknown> };
      // Ensure every event has a ts field — tests focus on semantics, not timestamps.
      // This mirrors what the real agent does: every emitted OmegaEvent has a ts.
      const event: Record<string, unknown> = {
        ts: new Date().toISOString(),
        ...body.event,
      };
      // Persist to disk (same filter the real server uses), then forward to browser
      if (shouldLogEvent(event)) {
        // appendEvent strips UI-only fields before writing
        await appendEvent(event as unknown as OmegaEvent, sessionPaths.eventsFile);
      }
      sendWs(event);
      return new Response("ok");
    }

    if (req.method === "POST" && url.pathname === "/control/reset") {
      receivedMessages.length = 0;
      currentAgentId = 1;
      // Create a fresh uniquely-named session directory (old one left on disk for inspection)
      sessionPaths = await makeSessionDir(new Date(), TEST_SESSIONS_ROOT);
      return new Response("ok");
    }

    if (req.method === "POST" && url.pathname === "/control/save") {
      // Events are written incrementally on each /control/send, so this is a
      // true no-op — disk is already up to date. Exists for API compatibility.
      return new Response("ok");
    }

    if (req.method === "POST" && url.pathname === "/control/load") {
      // In-memory state is always derived from disk on reconnect (loadReplayEvents).
      // No separate in-memory log to refresh. Exists for API compatibility.
      return new Response("ok");
    }

    if (req.method === "POST" && url.pathname === "/control/load-fixture") {
      // Write raw JSONL content directly to the current session's events.jsonl.
      // Does NOT send anything over the WebSocket — the browser only sees the
      // events after it reconnects (page reload) and the server replays from disk.
      const body = await req.json() as { lines: string[] };
      const jsonl = body.lines.join("\n") + "\n";
      await writeFile(sessionPaths.eventsFile, jsonl, "utf-8");
      return new Response("ok");
    }

    if (req.method === "GET" && url.pathname === "/control/disk-snapshot") {
      // Read and parse the current session's events.jsonl from disk
      const { eventsFile } = sessionPaths;
      if (!existsSync(eventsFile)) {
        return new Response(JSON.stringify([]), {
          headers: { "Content-Type": "application/json" },
        });
      }
      try {
        const text = await readFile(eventsFile, "utf-8");
        const events: object[] = [];
        for (const line of text.split("\n")) {
          const trimmed = line.trim();
          if (!trimmed) continue;
          try { events.push(JSON.parse(trimmed)); } catch { /* skip malformed */ }
        }
        return new Response(JSON.stringify(events), {
          headers: { "Content-Type": "application/json" },
        });
      } catch {
        return new Response(JSON.stringify([]), {
          headers: { "Content-Type": "application/json" },
        });
      }
    }

    return new Response("Not found", { status: 404 });
  },
});

console.log(`Test server:   http://localhost:${MAIN_PORT}`);
console.log(`Control API:   http://localhost:${CTRL_PORT}/control/ready`);
console.log(`Session dir:   ${sessionPaths.dir}`);
