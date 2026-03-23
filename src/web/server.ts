/**
 * Web application server — Bun HTTP + WebSocket.
 *
 * Serves the static web UI from src/web/public/ and streams OmegaEvent
 * objects as JSON over a WebSocket connection. Accepts user messages back
 * over the same socket.
 *
 * Protocol (client → server):
 *   { type: "message", content: string }            — send a user prompt
 *   { type: "abort" }                               — abort the current turn
 *   { type: "set_model", model: string }            — switch LLM model
 *   { type: "set_auth_mode", mode: string }         — switch auth mode
 *   { type: "cancel_oauth" }                        — cancel OAuth flow
 *
 * Protocol (server → client):
 *   All OmegaEvent shapes from events.ts, JSON-serialised.
 *   Extra: { type: "connected" }            — sent on WebSocket open
 *          { type: "auth", mode: string }   — auth mode at session start
 *          { type: "oauth_url", url: string } — open in browser; completes automatically
 *          { type: "oauth_cancelled" }      — OAuth flow cancelled
 *
 * Persistence: identical to the terminal UI. Agent writes context.jsonl and
 * events.jsonl into .omega/sessions/<timestamp>/. History replay on reconnect
 * reads events.jsonl from the current session dir — no separate session-store.
 */

import { join } from "path";
import { readFileSync, existsSync } from "fs";
import { readFile } from "fs/promises";
import type { ServerWebSocket } from "bun";
import { Agent, type StreamProvider } from "../agent.js";
import { makeSessionDir, type SessionPaths } from "../session-dir.js";
import { appendEvent } from "../event-store.js";
import type { ContextRecord } from "../context-store.js";

// ---------------------------------------------------------------------------
// Port resolution: --port flag > PORT env > 3000
// ---------------------------------------------------------------------------

function resolvePort(): number {
  const flagIdx = process.argv.indexOf("--port");
  if (flagIdx !== -1) {
    const val = process.argv[flagIdx + 1];
    const n = Number(val);
    if (!val || isNaN(n) || !Number.isInteger(n) || n < 1 || n > 65535) {
      console.error(`Error: --port requires a valid port number (got: ${val ?? "(nothing)"})`);
      process.exit(1);
    }
    return n;
  }
  return Number(process.env.PORT ?? 3000);
}

const PORT = resolvePort();
const PUBLIC_DIR = join(import.meta.dir, "public");

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

// ---------------------------------------------------------------------------
// Context record lookup — serves GET /context?hashes=abc,def,...
// ---------------------------------------------------------------------------

/**
 * Read context.jsonl and return the records whose hash appears in the
 * requested set, preserving the order of the requested hashes array.
 */
async function lookupContextRecords(
  contextFile: string,
  hashes: string[],
): Promise<ContextRecord[]> {
  if (!existsSync(contextFile)) return [];
  const hashSet = new Set(hashes);
  const map = new Map<string, ContextRecord>();
  try {
    const text = await readFile(contextFile, "utf-8");
    for (const line of text.split("\n")) {
      const trimmed = line.trim();
      if (!trimmed) continue;
      try {
        const rec = JSON.parse(trimmed) as ContextRecord;
        if (hashSet.has(rec.hash)) map.set(rec.hash, rec);
      } catch { /* skip malformed lines */ }
    }
  } catch { /* file unreadable */ }
  return hashes.map(h => map.get(h)).filter(Boolean) as ContextRecord[];
}

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
// Session integrity helpers (exported for tests)
// ---------------------------------------------------------------------------

/**
 * Events that should not be replayed to a reconnecting browser.
 *   connected — synthesised from WebSocket open; meaningless to replay
 *   text      — streaming text fragments; assembled response is in context.jsonl
 */
const REPLAY_EXCLUDE = new Set(["connected", "text"]);

/**
 * Returns true if the event should be included in history replay.
 * Mirrors the set of events Agent persists to events.jsonl — streaming
 * text fragments and transient transport signals are excluded.
 */
export function shouldLogEvent(event: object): boolean {
  const t = (event as any).type as string;
  return !REPLAY_EXCLUDE.has(t);
}

/**
 * Ensures the event log has no open (un-closed) turn at the tail.
 *
 * A turn is "open" when a `user_message` appears after the last
 * `turn_end` / `turn_interrupted` marker — the server crashed mid-turn.
 * Replaying such a log leaves `streaming = true` in the client with no
 * recovery path. We append a synthetic `turn_interrupted` to close it.
 *
 * Returns a new array (does not mutate the input).
 */
export function closeOpenTurn(log: object[]): object[] {
  for (let i = log.length - 1; i >= 0; i--) {
    const t = (log[i] as any).type as string;
    if (t === "turn_end" || t === "turn_interrupted") return log;
    if (t === "user_message") {
      return [...log, { type: "turn_interrupted" }];
    }
  }
  return log;
}

// ---------------------------------------------------------------------------
// History replay — read events.jsonl written by Agent
// ---------------------------------------------------------------------------

/**
 * Read the current session's events.jsonl and return the subset of events
 * suitable for history replay (excludes streaming text, transient signals).
 * Applies closeOpenTurn so a crashed session doesn't lock the browser UI.
 */
async function loadReplayEvents(eventsFile: string): Promise<object[]> {
  if (!existsSync(eventsFile)) return [];
  try {
    const text = await readFile(eventsFile, "utf-8");
    const events: object[] = [];
    for (const line of text.split("\n")) {
      const trimmed = line.trim();
      if (!trimmed) continue;
      try {
        const e = JSON.parse(trimmed);
        if (shouldLogEvent(e)) events.push(e);
      } catch {
        // skip malformed lines
      }
    }
    return closeOpenTurn(events);
  } catch {
    return [];
  }
}

// ---------------------------------------------------------------------------
// Persistent agent (survives WebSocket reconnects)
// ---------------------------------------------------------------------------

/**
 * The agent is created once at server start and reused across all WebSocket
 * connections. Browser refreshes / reconnects reuse the same agent context.
 * The agent is only replaced when the client sends { type: "reset" }.
 */
let persistentAgent: Agent;
let currentSessionPaths: SessionPaths;

// ---------------------------------------------------------------------------
// WebSocket session (transport layer — one active WS at a time)
// ---------------------------------------------------------------------------

interface Session {
  ws: ServerWebSocket<unknown>;
  abortController: AbortController | null;
  isStreaming: boolean;
  /** Cancel function for the in-progress OAuth flow, if any. */
  pendingOAuthCancel?: () => void;
}

let activeSession: Session | null = null;

function send(ws: ServerWebSocket<unknown>, event: object): void {
  try {
    ws.send(JSON.stringify(event));
  } catch {
    // WebSocket may have closed
  }
}

/**
 * Send a transport_error event to the client and best-effort persist it to
 * the current session's events.jsonl.
 *
 * Persistence is best-effort: if the write fails (e.g. because the error IS
 * a file I/O failure) the exception is silently swallowed so the WebSocket
 * send is never blocked.
 */
function sendTransportError(ws: ServerWebSocket<unknown>, error: string, context?: string): void {
  const event = {
    type: "transport_error" as const,
    ts: new Date().toISOString(),
    error,
    ...(context !== undefined ? { context } : {}),
  };
  send(ws, event);
  appendEvent(event, currentSessionPaths.eventsFile).catch(() => {});
}

async function handleMessage(session: Session, data: string, streamProvider?: StreamProvider): Promise<void> {
  let msg: any;
  try {
    msg = JSON.parse(data);
  } catch {
    sendTransportError(session.ws, "Invalid JSON from client", "handleMessage");
    return;
  }

  if (msg.type === "abort") {
    session.abortController?.abort();
    return;
  }

  if (msg.type === "reset") {
    session.abortController?.abort();
    session.isStreaming = false;
    session.abortController = null;

    // Replace the persistent agent with a fresh one in a new session dir
    currentSessionPaths = await makeSessionDir();
    persistentAgent = new Agent(
      streamProvider,
      currentSessionPaths.contextFile,
      currentSessionPaths.eventsFile,
    );

    // After the await we're outside the auto-corked message callback —
    // cork explicitly so all three frames are batched reliably.
    session.ws.cork(() => {
      session.ws.send(JSON.stringify({ type: "session_info", dir: currentSessionPaths.dir }));
      session.ws.send(JSON.stringify({ type: "history", events: [] }));
      session.ws.send(JSON.stringify({ type: "reset_done" }));
    });

    persistentAgent.init()
      .then(mode => {
        send(session.ws, { type: "auth", mode });
        return persistentAgent.loadSystemPromptAppend().catch(() => {});
      })
      .catch((err: any) => {
        send(session.ws, { type: "auth", mode: `error: ${err.message}` });
      });
    return;
  }

  if (msg.type === "set_model") {
    if (session.isStreaming) {
      sendTransportError(session.ws, "Cannot switch model during an active turn", "handleMessage");
      return;
    }
    const model: string = String(msg.model ?? "");
    if (model !== "claude-sonnet-4-6" && model !== "claude-opus-4-6") {
      send(session.ws, { type: "agent_error", ts: new Date().toISOString(), error: `Unknown model: ${model}` });
      return;
    }
    const ev = persistentAgent.setModel(model);
    send(session.ws, ev);
    return;
  }

  if (msg.type === "set_auth_mode") {
    if (session.isStreaming) {
      sendTransportError(session.ws, "Cannot switch auth mode during an active turn", "handleMessage");
      return;
    }
    const mode: string = String(msg.mode ?? "");
    if (mode === "api-key") {
      try {
        const ev = await persistentAgent.switchToApiKey();
        send(session.ws, ev);
      } catch (err: any) {
        send(session.ws, { type: "agent_error", ts: new Date().toISOString(), error: err.message });
      }
    } else if (mode === "claude-max") {
      try {
        const result = await persistentAgent.requestClaudeMaxSwitch();
        session.pendingOAuthCancel = result.cancel;
        send(session.ws, { type: "oauth_url", url: result.url });
        // complete() waits for the localhost callback and does the exchange.
        result.complete().then(ev => {
          session.pendingOAuthCancel = undefined;
          send(session.ws, ev);
        }).catch((err: any) => {
          session.pendingOAuthCancel = undefined;
          if (err?.message !== "OAuth cancelled") {
            send(session.ws, { type: "agent_error", ts: new Date().toISOString(), error: `OAuth failed: ${err.message}` });
          }
        });
      } catch (err: any) {
        send(session.ws, { type: "agent_error", ts: new Date().toISOString(), error: err.message });
      }
    } else {
      send(session.ws, { type: "agent_error", ts: new Date().toISOString(), error: `Unknown auth mode: ${mode}` });
    }
    return;
  }

  if (msg.type === "cancel_oauth") {
    session.pendingOAuthCancel?.();
    session.pendingOAuthCancel = undefined;
    send(session.ws, { type: "oauth_cancelled" });
    return;
  }

  if (msg.type === "message") {
    if (session.isStreaming) {
      sendTransportError(session.ws, "Turn already in progress", "handleMessage");
      return;
    }
    const content: string = String(msg.content ?? "").trim();
    if (!content) return;

    session.isStreaming = true;
    session.abortController = new AbortController();
    const { ws } = session;

    try {
      const confirmTool = async () => true;
      for await (const event of persistentAgent.sendMessage(
        content,
        confirmTool,
        session.abortController.signal,
      )) {
        send(ws, event);
      }
    } catch (err: any) {
      sendTransportError(ws, err.message ?? String(err), "handleMessage");
    } finally {
      session.isStreaming = false;
      session.abortController = null;
    }
  }
}

// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------

export interface WebAppOptions {
  /** Injectable LLM stream provider (used in tests to avoid real API calls). */
  streamProvider?: StreamProvider;
  /** Override the HTTP port (default: resolved from --port flag / PORT env / 3000). */
  port?: number;
}

export async function runWebApp(opts: WebAppOptions = {}): Promise<void> {
  currentSessionPaths = await makeSessionDir();
  persistentAgent = new Agent(
    opts.streamProvider,
    currentSessionPaths.contextFile,
    currentSessionPaths.eventsFile,
  );

  // Graceful shutdown: mirrors terminal app — emit session_end then exit
  const handleShutdown = () => {
    persistentAgent.emitSessionEnd("clean")
      .catch(() => {})
      .finally(() => process.exit(0));
  };
  process.on("SIGINT", handleShutdown);
  process.on("SIGTERM", handleShutdown);

  let server: ReturnType<typeof Bun.serve>;
  try {
    server = Bun.serve({
    port: opts.port ?? PORT,

    async fetch(req, srv) {
      if (srv.upgrade(req, { data: undefined })) return undefined as any;
      const url = new URL(req.url);

      // Context record lookup: GET /context?hashes=abc123,def456,...
      if (url.pathname === "/context" && req.method === "GET") {
        const raw = url.searchParams.get("hashes") ?? "";
        const hashes = raw.split(",").map(h => h.trim()).filter(Boolean);
        if (hashes.length === 0) {
          return new Response(JSON.stringify([]), {
            headers: { "Content-Type": "application/json" },
          });
        }
        const records = await lookupContextRecords(currentSessionPaths.contextFile, hashes);
        return new Response(JSON.stringify(records), {
          headers: { "Content-Type": "application/json" },
        });
      }

      const res = serveStatic(url.pathname);
      if (res) return res;
      return new Response("Not found", { status: 404 });
    },

    websocket: {
      async open(ws) {
        const session: Session = {
          ws,
          abortController: null,
          isStreaming: false,
        };
        activeSession = session;

        // Replay past events from events.jsonl — same file Agent writes to.
        // After the await we're outside the auto-corked open callback, so we
        // must cork explicitly (Bun docs: "use cork in async functions").
        const replayEvents = await loadReplayEvents(currentSessionPaths.eventsFile);
        ws.cork(() => {
          ws.send(JSON.stringify({ type: "session_info", dir: currentSessionPaths.dir }));
          if (replayEvents.length > 0) {
            ws.send(JSON.stringify({ type: "history", events: replayEvents }));
          }
          ws.send(JSON.stringify({ type: "connected" }));
        });

        persistentAgent.init()
          .then(mode => {
            send(ws, { type: "auth", mode });
            return persistentAgent.loadSystemPromptAppend().catch(() => {});
          })
          .catch((err: any) => {
            send(ws, { type: "auth", mode: `error: ${err.message}` });
          });
      },

      message(ws, data) {
        if (activeSession?.ws !== ws) return;
        handleMessage(activeSession, String(data), opts.streamProvider).catch((err: any) => {
          sendTransportError(ws, String(err), "websocket_message_handler");
        });
      },

      close(ws) {
        if (activeSession?.ws === ws) {
          activeSession = null;
        }
      },
    },
    });
  } catch (err: any) {
    const msg: string = err?.message ?? String(err);
    if (msg.toLowerCase().includes("address already in use")) {
      console.error(`Error: port ${PORT} is already in use. Choose a different port with --port <n>.`);
    } else {
      console.error("Error: failed to start server:", msg);
    }
    process.exit(1);
  }

  console.log(`Omega web UI  →  http://localhost:${server!.port}`);
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
