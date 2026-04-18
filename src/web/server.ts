/**
 * Web application server — Bun HTTP + WebSocket.
 *
 * Serves the static web UI from src/web/public/ and streams OmegaEvent
 * objects as JSON over a WebSocket connection. Accepts user messages back
 * over the same socket.
 *
 * Protocol (client → server):  see src/web/protocol.ts (ClientMessageSchema)
 *
 * Protocol (server → client):
 *   All OmegaEvent shapes from events.ts, JSON-serialised.
 *   Extra: { type: "ready" }               — sent after history replay batch
 *
 * Persistence: identical to the terminal UI. Agent writes context.jsonl and
 * events.jsonl into .omega/sessions/<timestamp>/. History replay on reconnect
 * reads events.jsonl from the current session dir — no separate session-store.
 */

import { join, basename } from "path";
import { readFileSync, existsSync } from "fs";
import { readFile, readdir } from "fs/promises";
import { z } from "zod";
import type { ServerWebSocket } from "bun";
import { Agent, makeDefaultCreateMessageStream, type CreateMessageStream, type OmegaEvent } from "../agent.js";
import {
  makeSessionDir,
  readSessionMetadata,
  updateSessionMetadata,
  SESSIONS_ROOT,
  SESSION_DIR_RE,
  type SessionMetadata,
  type SessionPaths,
} from "../session-dir.js";
import { appendEvent } from "../event-store.js";
import type { ContextRecord } from "../context-store.js";
import { parseOmegaEvent } from "../events.schema.js";
import { ContextRecordSchema } from "../context-store.schema.js";
import { readEnvPort } from "../env.js";
import { config } from "../config.js";
import { ClientMessageSchema, type ClientMessage } from "./protocol.js";
import { now } from "../iso-timestamp.js";
import {
  extractResumptionBasis,
  extractDescriptionFromResponse,
  extractLastModelAndEffort,
} from "../session-resume.js";

// ---------------------------------------------------------------------------
// Port resolution: --port flag > PORT env > 3000
// ---------------------------------------------------------------------------

const PortNumber = z.coerce.number().int().min(1).max(65535);

function resolvePort(): number {
  const flagIdx = process.argv.indexOf("--port");
  if (flagIdx !== -1) {
    const val = process.argv[flagIdx + 1];
    const parsed = PortNumber.safeParse(val);
    if (!parsed.success) {
      console.error(`Error: --port requires a valid port number (got: ${val ?? "(nothing)"})`);
      process.exit(1);
    }
    return parsed.data;
  }
  return readEnvPort(3000);
}

const PORT = resolvePort();
const PUBLIC_DIR = join(import.meta.dir, "public");

/**
 * Active sessions root — set by `runWebApp()` at startup.
 * Defaults to the production root; tests override via `opts.sessionsRoot`.
 */
let activeSessionsRoot: string = SESSIONS_ROOT;

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
export async function lookupContextRecords(
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
      let raw: unknown;
      try { raw = JSON.parse(trimmed); } catch { continue; }
      const result = ContextRecordSchema.safeParse(raw);
      if (result.success && hashSet.has(result.data.hash)) {
        map.set(result.data.hash, result.data);
      }
    }
  } catch { /* file unreadable */ }
  return hashes.flatMap(h => { const r = map.get(h); return r ? [r] : []; });
}

function serveStatic(pathname: string): Response | null {
  const rel = pathname === "/" ? "/index.html" : pathname;
  const safe = rel.replace(/\.\./g, "");
  const fullPath = join(PUBLIC_DIR, safe);
  if (!existsSync(fullPath)) return null;
  const ext = fullPath.match(/(\.[^.]+)$/)?.[1] ?? ".html";
  const mime = MIME[ext] ?? "application/octet-stream";
  // HTML: always revalidate so browsers pick up new asset hashes after a build.
  // Hashed assets (/assets/...): immutable — content hash guarantees freshness.
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
// Session integrity helpers (exported for tests)
// ---------------------------------------------------------------------------

/**
 * Events that should not be replayed to a reconnecting browser.
 *   ready     — server-sent after history batch; meaningless to replay
 *   text      — streaming text fragments; assembled response is in context.jsonl
 */
const REPLAY_EXCLUDE = new Set(["ready", "text"]);

/**
 * Returns true if the event should be included in history replay.
 * Mirrors the set of events Agent persists to events.jsonl — streaming
 * text fragments and transient transport signals are excluded.
 */
export function shouldLogEvent(event: object): boolean {
  return "type" in event && !REPLAY_EXCLUDE.has((event as { type: string }).type);
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
    const entry = log[i]!;
    if (!("type" in entry)) continue;
    const t = (entry as { type: string }).type;
    if (t === "turn_end" || t === "turn_interrupted") return log;
    if (t === "user_message") {
      return [...log, { type: "turn_interrupted", time: now() }];
    }
  }
  return log;
}

// ---------------------------------------------------------------------------
// History replay — read events.jsonl written by Agent
// ---------------------------------------------------------------------------

/**
 * Read all parseable OmegaEvents from an events file (no filtering).
 * Used to load a previous session for resumption basis extraction.
 * Returns [] if the file is absent or unreadable.
 */
export async function loadAllEvents(eventsFile: string): Promise<OmegaEvent[]> {
  if (!existsSync(eventsFile)) return [];
  try {
    const text = await readFile(eventsFile, "utf-8");
    const events: OmegaEvent[] = [];
    for (const line of text.split("\n")) {
      const trimmed = line.trim();
      if (!trimmed) continue;
      let raw: unknown;
      try { raw = JSON.parse(trimmed); } catch { continue; }
      const result = parseOmegaEvent(raw);
      if (result.success) events.push(result.data);
    }
    return events;
  } catch {
    return [];
  }
}

/**
 * Read the current session's events.jsonl and return the subset of events
 * suitable for history replay (excludes streaming text, transient signals).
 * Does NOT apply closeOpenTurn — callers decide whether to apply it based on
 * the current isStreaming state (running turns must not be falsely closed).
 */
async function loadReplayEvents(eventsFile: string): Promise<object[]> {
  if (!existsSync(eventsFile)) return [];
  try {
    const text = await readFile(eventsFile, "utf-8");
    const events: OmegaEvent[] = [];
    for (const line of text.split("\n")) {
      const trimmed = line.trim();
      if (!trimmed) continue;
      let raw: unknown;
      try { raw = JSON.parse(trimmed); } catch { continue; }
      const result = parseOmegaEvent(raw);
      if (result.success && shouldLogEvent(result.data)) events.push(result.data);
    }
    return closeOpenTurn(events);
  } catch {
    return [];
  }
}

// ---------------------------------------------------------------------------
// Session listing — GET /sessions
// ---------------------------------------------------------------------------

/** Convert a session folder name to an ISO timestamp string. */
function folderNameToTimestamp(name: string): string {
  // "2025-07-11T09-14-22-037-a8c3f1b2" → "2025-07-11T09:14:22.037Z"
  const m = name.match(/^(\d{4}-\d{2}-\d{2})T(\d{2})-(\d{2})-(\d{2})(?:-(\d{3}))?/);
  if (!m) return name;
  const [, date, h, min, s, ms] = m;
  return `${date}T${h}:${min}:${s}${ms ? `.${ms}` : ""}Z`;
}

interface SessionListItem extends SessionMetadata {
  dir: string;
  lastActivity: string;
}

async function listSessions(): Promise<SessionListItem[]> {
  let entries: string[];
  try {
    entries = await readdir(activeSessionsRoot);
  } catch {
    return [];
  }

  const dirs = entries
    .filter(e => SESSION_DIR_RE.test(e))
    .sort()
    .reverse(); // newest first

  const items: SessionListItem[] = [];
  for (const dir of dirs) {
    const fullDir = join(activeSessionsRoot, dir);
    const meta = await readSessionMetadata(fullDir);
    items.push({
      dir,
      ...(meta.name !== undefined ? { name: meta.name } : {}),
      ...(meta.description !== undefined ? { description: meta.description } : {}),
      ...(meta.resumedFrom !== undefined ? { resumedFrom: meta.resumedFrom } : {}),
      lastActivity: folderNameToTimestamp(dir),
    });
  }
  return items;
}

// ---------------------------------------------------------------------------
// Persistent agent (survives WebSocket reconnects)
// ---------------------------------------------------------------------------

/**
 * The agent is created when the user first creates or resumes a session and
 * reused across all WebSocket reconnects. Starts as undefined — no session is
 * created at server startup. The client sees `ready` with no `session_info`
 * and is forced to choose (new or resume) before any work can begin.
 *
 * Replaced when the client sends { type: "reset" } or { type: "resume_session" }.
 */
let persistentAgent: Agent | undefined;
let currentSessionPaths: SessionPaths | undefined;

// ---------------------------------------------------------------------------
// WebSocket session (transport layer — one active WS at a time)
// ---------------------------------------------------------------------------

interface Session {
  ws: ServerWebSocket<unknown>;
}

let activeSession: Session | null = null;

/**
 * Module-level streaming state — persists across WebSocket reconnects.
 *
 * Keeping these at module scope (rather than on the Session object) means that
 * when a browser refreshes mid-turn the new WebSocket session inherits the
 * correct streaming state. Without this, a refresh would create a new Session
 * with isStreaming=false and allow a second concurrent turn to start, while
 * events from the running turn would be sent to the now-closed old socket.
 */
let isStreaming = false;
let activeAbortController: AbortController | null = null;

/**
 * Send an event to the currently active WebSocket, if any.
 *
 * Using broadcast() instead of a captured `ws` reference ensures that events
 * from a long-running turn reach a browser that reconnected mid-turn (e.g.
 * after a page refresh). The captured `ws` would be the now-closed old socket;
 * `activeSession?.ws` is always the current live connection.
 */
function broadcast(event: object): void {
  if (activeSession) send(activeSession.ws, event);
}

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
 * send is never blocked. If no session is active yet, only the WS send runs.
 */
function sendTransportError(ws: ServerWebSocket<unknown>, error: string, context?: string): void {
  const event = {
    type: "transport_error" as const,
    time: now(),
    error,
    ...(context !== undefined ? { context } : {}),
  };
  send(ws, event);
  if (currentSessionPaths) {
    appendEvent(event, currentSessionPaths.eventsFile).catch(() => {});
  }
}

async function handleMessage(
  session: Session,
  data: string,
  createMessageStream: CreateMessageStream,
): Promise<void> {
  let msg: ClientMessage;
  try {
    msg = ClientMessageSchema.parse(JSON.parse(data));
  } catch (err) {
    const detail = err instanceof z.ZodError ? z.prettifyError(err) : "Invalid JSON from client";
    sendTransportError(session.ws, detail, "handleMessage");
    return;
  }

  if (msg.type === "abort") {
    if (persistentAgent) activeAbortController?.abort();
    return;
  }

  if (msg.type === "reset") {
    activeAbortController?.abort();
    isStreaming = false;
    activeAbortController = null;

    // Replace the persistent agent with a fresh one in a new session dir
    currentSessionPaths = await makeSessionDir(new Date(), activeSessionsRoot);
    persistentAgent = new Agent(
      createMessageStream,
      currentSessionPaths.contextFile,
      currentSessionPaths.eventsFile,
      currentSessionPaths.dir,
    );

    // After the await we're outside the auto-corked message callback —
    // cork explicitly so all three frames are batched reliably.
    session.ws.cork(() => {
      session.ws.send(JSON.stringify({ type: "session_info", dir: currentSessionPaths!.dir, model: persistentAgent!.getActiveModel(), effort: persistentAgent!.getActiveEffort(), cwd: process.cwd() }));
      session.ws.send(JSON.stringify({ type: "history", events: [] }));
      session.ws.send(JSON.stringify({ type: "reset_done" }));
    });

    persistentAgent.init()
      .then(async () => {
        await persistentAgent!.loadSystemPromptAppend().catch(() => {});
        // Forward the init events (server_started + session_started) that init()
        // just persisted to events.jsonl. Without this they are invisible until a
        // browser refresh triggers the history-replay path.
        const initEvents = await loadReplayEvents(currentSessionPaths!.eventsFile);
        for (const ev of initEvents) {
          send(session.ws, ev);
        }
      })
      .catch((err: unknown) => {
        send(session.ws, { type: "agent_error", time: now(), error: `Init failed: ${err instanceof Error ? err.message : String(err)}` });
      });
    return;
  }

  if (msg.type === "resume_session") {
    if (isStreaming) {
      sendTransportError(session.ws, "Cannot resume session during an active turn", "handleMessage");
      return;
    }

    activeAbortController?.abort();
    isStreaming = false;
    activeAbortController = null;

    // Create new session dir + agent
    currentSessionPaths = await makeSessionDir(new Date(), activeSessionsRoot);
    persistentAgent = new Agent(
      createMessageStream,
      currentSessionPaths.contextFile,
      currentSessionPaths.eventsFile,
      currentSessionPaths.dir,
    );

    await persistentAgent.init();
    await persistentAgent.loadSystemPromptAppend().catch(() => {});

    // Read previous session events and extract the basis for summarisation.
    const prevSessionDir = join(activeSessionsRoot, msg.sessionDir);
    const prevEventsFile = join(prevSessionDir, "events.jsonl");
    const [prevEvents, prevMeta] = await Promise.all([
      loadAllEvents(prevEventsFile),
      readSessionMetadata(prevSessionDir).catch(() => ({})),
    ]);
    const basis = extractResumptionBasis(prevEvents);
    const resumedSessionName = (prevMeta as { name?: string }).name;

    // Restore the model and effort that were active at the end of the previous
    // session so the resumed session starts in the same state.
    const { model: prevModel, effort: prevEffort } = extractLastModelAndEffort(prevEvents);
    if (prevModel !== undefined && prevModel !== persistentAgent.getActiveModel()) {
      persistentAgent.setModel(prevModel);
    }
    if (prevEffort !== undefined && prevEffort !== persistentAgent.getActiveEffort()) {
      persistentAgent.setEffort(prevEffort);
    }
    // Flush so model_changed/effort_changed are on disk before loadReplayEvents
    // reads the file — otherwise those events are missing from the history
    // payload and only appear on the next page refresh.
    await persistentAgent.flushEventLog();

    // Send session_info and the init events (server_started + session_started)
    // to the client immediately — before the LLM call — so the feed clears and
    // the new session directory is visible right away.
    const initEvents = await loadReplayEvents(currentSessionPaths.eventsFile);
    session.ws.cork(() => {
      session.ws.send(JSON.stringify({
        type: "session_info",
        dir: currentSessionPaths!.dir,
        model: persistentAgent!.getActiveModel(),
        effort: persistentAgent!.getActiveEffort(),
        cwd: process.cwd(),
      }));
      session.ws.send(JSON.stringify({ type: "history", events: initEvents }));
    });

    // Guard against concurrent messages while the resumption LLM call is
    // in flight — same pattern as the normal turn handler.
    isStreaming = true;
    activeAbortController = new AbortController();

    // Stream resumption events live as they are generated, exactly like a
    // normal turn. The generator yields: resuming_session → llm_call →
    // llm_response → session_resumed (or llm_error on failure).
    let description: string | undefined;
    try {
      for await (const event of persistentAgent.performResumption(
        basis,
        msg.sessionDir,
        activeAbortController.signal,
        resumedSessionName,
      )) {
        // Use broadcast() so that a browser refresh mid-resumption still
        // receives the remaining events on the new socket.
        broadcast(event);
        if (event.type === "llm_response" && event.text) {
          description = extractDescriptionFromResponse(event.text);
        }
      }
    } catch (err: unknown) {
      // llm_error is already logged and sent inside the generator.
      // Surface the failure to the client as a transport error too.
      if (activeSession) {
        sendTransportError(
          activeSession.ws,
          `Session resumption failed: ${err instanceof Error ? err.message : String(err)}`,
          "resume_session",
        );
      }
    } finally {
      isStreaming = false;
      activeAbortController = null;
    }

    // Write description back to the *source* session's metadata (retroactive labelling).
    if (description) {
      await updateSessionMetadata(prevSessionDir, { description }).catch(() => {});
    }

    // Update new session's metadata with the resumedFrom link.
    await updateSessionMetadata(currentSessionPaths!.dir, {
      resumedFrom: msg.sessionDir,
    });

    send(session.ws, { type: "ready" });
    return;
  }

  if (msg.type === "delete_session") {
    // Safety: only delete directories matching the session dir pattern
    if (!SESSION_DIR_RE.test(msg.sessionDir)) {
      sendTransportError(session.ws, `Invalid session dir: ${msg.sessionDir}`, "handleMessage");
      return;
    }
    const fullDir = join(activeSessionsRoot, msg.sessionDir);
    try {
      const { rm } = await import("fs/promises");
      await rm(fullDir, { recursive: true, force: true });
      send(session.ws, { type: "session_deleted", sessionDir: msg.sessionDir });
    } catch (err: unknown) {
      sendTransportError(session.ws, `Delete failed: ${err instanceof Error ? err.message : String(err)}`, "handleMessage");
    }
    return;
  }

  if (msg.type === "rename_session") {
    if (!SESSION_DIR_RE.test(msg.sessionDir)) {
      sendTransportError(session.ws, `Invalid session dir: ${msg.sessionDir}`, "handleMessage");
      return;
    }
    const fullDir = join(activeSessionsRoot, msg.sessionDir);
    try {
      await updateSessionMetadata(fullDir, { name: msg.name });
      send(session.ws, { type: "session_renamed", sessionDir: msg.sessionDir, name: msg.name });
    } catch (err: unknown) {
      sendTransportError(session.ws, `Rename failed: ${err instanceof Error ? err.message : String(err)}`, "handleMessage");
    }
    return;
  }

  if (msg.type === "set_model") {
    if (!persistentAgent) return; // no session yet
    if (isStreaming) {
      sendTransportError(session.ws, "Cannot switch model during an active turn", "handleMessage");
      return;
    }
    const ev = persistentAgent.setModel(msg.model);
    send(session.ws, ev);
    // "max" effort is only valid on Opus. If the user switches to Sonnet while
    // effort is "max", auto-reset to "high" so the UI stays consistent.
    if (msg.model !== "claude-opus-4-6" && persistentAgent.getActiveEffort() === "max") {
      const effortEv = persistentAgent.setEffort("medium");
      send(session.ws, effortEv);
    }
    return;
  }

  if (msg.type === "set_effort") {
    if (!persistentAgent) return; // no session yet
    if (isStreaming) {
      sendTransportError(session.ws, "Cannot change effort during an active turn", "handleMessage");
      return;
    }
    const ev = persistentAgent.setEffort(msg.effort);
    send(session.ws, ev);
    return;
  }

  if (msg.type === "message") {
    if (!persistentAgent || !currentSessionPaths) {
      sendTransportError(session.ws, "No active session — create or resume a session first", "handleMessage");
      return;
    }
    if (isStreaming) {
      sendTransportError(session.ws, "Turn already in progress", "handleMessage");
      return;
    }
    const content = msg.content.trim();
    if (!content) return;

    isStreaming = true;
    activeAbortController = new AbortController();

    const turnEvents: OmegaEvent[] = [];
    try {
      const confirmTool = async () => true;
      for await (const event of persistentAgent.sendMessage(
        content,
        confirmTool,
        activeAbortController.signal,
      )) {
        // Use broadcast() instead of a captured `ws` so that events reach a
        // browser that reconnected mid-turn (e.g. after a page refresh). The
        // captured `ws` would be the now-closed old socket.
        broadcast(event);
        if ("type" in event) turnEvents.push(event as OmegaEvent);
      }
    } catch (err: unknown) {
      // Route errors to the current active session (may differ from the ws
      // that initiated the turn if the browser refreshed mid-turn).
      if (activeSession) {
        sendTransportError(activeSession.ws, err instanceof Error ? err.message : String(err), "handleMessage");
      }
    } finally {
      isStreaming = false;
      activeAbortController = null;
    }

  }
}

// ---------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------

export interface WebAppOptions {
  /** Injectable LLM stream function (used in tests to avoid real API calls). */
  streamProvider?: CreateMessageStream;
  /** Override the HTTP port (default: resolved from --port flag / PORT env / 3000). */
  port?: number;
  /** Root directory for session folders (default: `.omega/sessions`). Tests pass `.omega/test-sessions`. */
  sessionsRoot?: string;
}

export async function runWebApp(opts: WebAppOptions = {}): Promise<void> {
  // Always resolve a concrete stream provider so it can be shared between
  // the Agent (for normal turns) and auto-naming (for session name generation).
  const streamProvider: CreateMessageStream =
    opts.streamProvider ?? makeDefaultCreateMessageStream();

  activeSessionsRoot = opts.sessionsRoot ?? SESSIONS_ROOT;
  // No session is created at startup — the client is forced to choose (new or
  // resume) before any work begins. persistentAgent / currentSessionPaths are
  // set when the user sends { type: "reset" } or { type: "resume_session" }.

  // Graceful shutdown: emit server_stopped if a session is active, then exit
  const handleShutdown = () => {
    if (persistentAgent) {
      persistentAgent.emitServerStopped("clean")
        .catch(() => {})
        .finally(() => process.exit(0));
    } else {
      process.exit(0);
    }
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

      // Session listing: GET /sessions
      if (url.pathname === "/sessions" && req.method === "GET") {
        const sessions = await listSessions();
        return new Response(JSON.stringify(sessions), {
          headers: { "Content-Type": "application/json", "Cache-Control": "no-cache" },
        });
      }

      // Context record lookup: GET /context?hashes=abc123,def456,...
      if (url.pathname === "/context" && req.method === "GET") {
        const raw = url.searchParams.get("hashes") ?? "";
        const hashes = raw.split(",").map(h => h.trim()).filter(Boolean);
        if (hashes.length === 0 || !currentSessionPaths) {
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
        const session: Session = { ws };
        activeSession = session;

        if (currentSessionPaths && persistentAgent) {
          // Existing session (reconnect after user already chose): replay history.
          // After the await we're outside the auto-corked open callback, so we
          // must cork explicitly (Bun docs: "use cork in async functions").
          const [rawReplayEvents, sessionMeta] = await Promise.all([
            loadReplayEvents(currentSessionPaths.eventsFile),
            readSessionMetadata(currentSessionPaths.dir),
          ]);

          // If an agent turn is actively running (isStreaming), do NOT apply
          // closeOpenTurn — the turn is not interrupted, it is still in
          // progress. The browser will receive the remaining live events via
          // broadcast(). Applying closeOpenTurn here would inject a false
          // turn_interrupted into the replay, making the UI think the turn
          // ended when it didn't.
          const replayEvents = isStreaming ? rawReplayEvents : closeOpenTurn(rawReplayEvents);

          ws.cork(() => {
            const sessionInfoMsg: Record<string, unknown> = { type: "session_info", dir: currentSessionPaths!.dir, model: persistentAgent!.getActiveModel(), effort: persistentAgent!.getActiveEffort(), cwd: process.cwd() };
            if (sessionMeta.name) sessionInfoMsg.name = sessionMeta.name;
            ws.send(JSON.stringify(sessionInfoMsg));
            // Always send history (even empty) when streaming so the client
            // receives the streaming=true flag and stays in streaming mode.
            if (replayEvents.length > 0 || isStreaming) {
              ws.send(JSON.stringify({ type: "history", events: replayEvents, ...(isStreaming ? { streaming: true } : {}) }));
            }
            ws.send(JSON.stringify({ type: "ready", ...(isStreaming ? { streaming: true } : {}) }));
          });

          persistentAgent.init()
            .then(() => persistentAgent!.loadSystemPromptAppend().catch(() => {}))
            .catch((err: unknown) => {
              send(ws, { type: "agent_error", time: now(), error: `Init failed: ${err instanceof Error ? err.message : String(err)}` });
            });
        } else {
          // No session yet — signal ready; client will show the session picker
          // and force the user to create or resume before any work begins.
          ws.send(JSON.stringify({ type: "ready" }));
        }
      },

      message(ws, data) {
        if (activeSession?.ws !== ws) return;
        handleMessage(activeSession, String(data), streamProvider).catch((err: unknown) => {
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
  } catch (err: unknown) {
    const msg: string = err instanceof Error ? err.message : String(err);
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
