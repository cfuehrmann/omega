/**
 * Web application server — Bun HTTP + WebSocket.
 *
 * Serves the static web UI from src/web/public/ and streams AgentEvent
 * objects as JSON over a WebSocket connection. Accepts user messages back
 * over the same socket.
 *
 * Protocol (client → server):
 *   { type: "message", content: string }   — send a user prompt
 *   { type: "abort" }                       — abort the current turn
 *
 * Protocol (server → client):
 *   All AgentEvent shapes from agent.ts, JSON-serialised.
 *   Extra: { type: "connected" }            — sent on WebSocket open
 *          { type: "auth", mode: string }   — auth result
 *          { type: "turn_ready" }           — server ready for next message
 */

import { join } from "path";
import { readFileSync, existsSync } from "fs";
import type { ServerWebSocket } from "bun";
import { Agent } from "../agent.js";
import type { AgentEvent } from "../agent.js";
import { loadSession, saveSession, clearSession } from "./session-store.js";

const PORT = Number(process.env.PORT ?? 3000);
const PUBLIC_DIR = join(import.meta.dir, "public");

// ---------------------------------------------------------------------------
// Graceful shutdown — mirrors terminal/app.ts shutdown()
// ---------------------------------------------------------------------------

/**
 * Drain foldCurrentSessionIntoWorldState() on the active agent so that the
 * world-state file is updated before the process exits.  Safe to call with a
 * null/undefined agent (no-op).
 */
export async function performWebShutdown(agent: Agent | null | undefined): Promise<void> {
  if (!agent) return;
  for await (const _event of agent.foldCurrentSessionIntoWorldState()) {
    // Drain all events; the side-effect (writing world-state.md) is what matters.
  }
}

// ---------------------------------------------------------------------------
// Static file helpers
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
  // Prevent path traversal
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
// Event log — replayed to each new client on connect
// ---------------------------------------------------------------------------

/**
 * Events that are meaningful for replay (history).
 * Transient connection events are excluded:
 *   connected — synthesised from WebSocket open; meaningless to replay
 *   turn_ready — transient "server ready" signal; replaying it would unlock
 *                the input before the reconnect sequence completes
 * auth IS included so the model label re-appears after a browser refresh.
 */
const REPLAY_EXCLUDE = new Set(["connected", "turn_ready"]);

let eventLog: object[] = [];

function logEvent(event: object): void {
  const t = (event as any).type as string;
  if (REPLAY_EXCLUDE.has(t)) return;
  // Some events should only appear once — deduplicate by removing older copy first
  if (t === "auth") {
    const idx = eventLog.findIndex((e: any) => e.type === "auth");
    if (idx !== -1) eventLog.splice(idx, 1);
  }
  eventLog.push(event);
  // Persist after each turn_end — cheap, no LLM calls, protects against crashes
  if (t === "turn_end") {
    saveSession(eventLog).catch(() => {}); // fire-and-forget; never blocks a turn
  }
}

// ---------------------------------------------------------------------------
// WebSocket session (one active session per server — single-user for now)
// ---------------------------------------------------------------------------

interface Session {
  ws: ServerWebSocket<unknown>;
  agent: Agent;
  abortController: AbortController | null;
  isStreaming: boolean;
}

let activeSession: Session | null = null;

function send(ws: ServerWebSocket<unknown>, event: object): void {
  try {
    ws.send(JSON.stringify(event));
    logEvent(event);
  } catch {
    // WebSocket may have closed
  }
}

async function handleMessage(session: Session, data: string): Promise<void> {
  let msg: any;
  try {
    msg = JSON.parse(data);
  } catch {
    send(session.ws, { type: "error", error: "Invalid JSON from client" });
    return;
  }

  if (msg.type === "abort") {
    session.abortController?.abort();
    return;
  }

  if (msg.type === "message") {
    if (session.isStreaming) {
      send(session.ws, { type: "error", error: "Turn already in progress" });
      return;
    }
    const content: string = String(msg.content ?? "").trim();
    if (!content) return;

    session.isStreaming = true;
    session.abortController = new AbortController();
    const { ws } = session;

    try {
      const confirmTool = async () => true;
      for await (const event of session.agent.sendMessage(
        content,
        confirmTool,
        session.abortController.signal,
      )) {
        send(ws, event);
      }
    } catch (err: any) {
      send(ws, { type: "error", error: err.message ?? String(err) });
    } finally {
      session.isStreaming = false;
      session.abortController = null;
      send(ws, { type: "turn_ready" });
    }
  }
}

// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------

export async function runWebApp(): Promise<void> {
  // Load persisted session log — enables history replay after crashes/restarts
  eventLog = await loadSession();

  // Graceful shutdown: persist on SIGINT (Ctrl+C) and SIGTERM
  const handleShutdown = () => {
    const agent = activeSession?.agent ?? null;
    saveSession(eventLog)
      .catch(() => {})
      .then(() => performWebShutdown(agent))
      .catch(() => {})
      .finally(() => process.exit(0));
  };
  process.on("SIGINT", handleShutdown);
  process.on("SIGTERM", handleShutdown);

  const server = Bun.serve({
    port: PORT,

    fetch(req, srv) {
      // Upgrade WebSocket connections
      if (srv.upgrade(req)) return undefined as any;

      const url = new URL(req.url);
      const res = serveStatic(url.pathname);
      if (res) return res;

      return new Response("Not found", { status: 404 });
    },

    websocket: {
      open(ws) {
        const agent = new Agent();
        const session: Session = {
          ws,
          agent,
          abortController: null,
          isStreaming: false,
        };
        activeSession = session;

        // Replay past events before signalling connected — client rebuilds state
        if (eventLog.length > 0) {
          try {
            ws.send(JSON.stringify({ type: "history", events: eventLog }));
          } catch {
            // ignore
          }
        }
        send(ws, { type: "connected" });

        // Init auth + world state in background; send result over socket
        agent.init()
          .then(mode => {
            send(ws, { type: "auth", mode });
            return agent.loadWorldState().catch(() => {});
          })
          .catch((err: any) => {
            send(ws, { type: "auth", mode: `error: ${err.message}` });
          });
      },

      message(ws, data) {
        if (activeSession?.ws !== ws) return;
        handleMessage(activeSession, String(data)).catch((err: any) => {
          send(ws, { type: "error", error: String(err) });
        });
      },

      close(ws) {
        if (activeSession?.ws === ws) {
          activeSession.abortController?.abort();
          activeSession = null;
        }
      },
    },
  });

  console.log(`Omega web UI  →  http://localhost:${server.port}`);
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

if (import.meta.main) {
  runWebApp().catch(err => {
    console.error("Failed to start web server:", err);
    process.exit(1);
  });
}
