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
import { readFile, writeFile, readdir, rm, mkdir } from "fs/promises";
import type { ServerWebSocket } from "bun";
import { closeOpenTurn, shouldLogEvent, listFilesForCompletion } from "../../src/web/server-helpers.js";
import { appendEvent } from "../../src/event-store.js";
import {
  makeSessionDir,
  readSessionMetadata,
  writeSessionMetadata,
  updateSessionMetadata,
  SESSION_DIR_RE,
  SESSION_METADATA_FILE,
  TEST_SESSIONS_ROOT,
  type SessionMetadata,
  type SessionPaths,
} from "../../src/session-dir.js";
import type { OmegaEvent } from "../../src/events.js";
import { parseOmegaEvent } from "../../src/events.schema.js";
import { now as isoNow } from "../../src/iso-timestamp.js";
import type { TurnState } from "../../src/web/protocol.js";

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
 * Derived turn state, mirroring src/web/server.ts's deriveTurnState(). Kept
 * in sync with events as they flow through /control/send so the test server
 * broadcasts a session_info update on every transition — exactly what the
 * real server does.
 */
let currentTurnState: TurnState = "idle";

function deriveTurnState(prev: TurnState, event: { type: string }): TurnState {
  switch (event.type) {
    case "user_message":     return "running";
    case "pause_requested":  return "pause_requested";
    case "turn_paused":      return "paused";
    case "turn_continued":   return "running";
    case "turn_end":
    case "turn_interrupted": return "idle";
    default:                 return prev;
  }
}

function buildSessionInfo(): Record<string, unknown> {
  return {
    type: "session_info",
    dir: sessionPaths.dir,
    model: "claude-sonnet-4-6",
    effort: "medium",
    cwd: process.cwd(),
    turnState: currentTurnState,
    hasPendingChanges,
  };
}

/**
 * Configurable delay (ms) injected into resume_session handling so tests
 * can observe the "resuming…" state. 0 = instant.
 */
let resumeDelayMs = 0;

/**
 * When true, `buildSessionInfo()` includes `hasPendingChanges: true`.
 * Set via `POST /control/set-pending-changes`.
 */
let hasPendingChanges = false;

/**
 * Current session directory paths — created by makeSessionDir() on startup
 * and on each /control/reset. Events are persisted to eventsFile on disk.
 *
 * makeSessionDir() appends an 8-char random hex suffix to the timestamp, so
 * rapid resets within the same second produce distinct directory names without
 * any counter bookkeeping.
 */

// Wipe any sessions left over from previous test-server runs so the
// directory cannot grow without bound across repeated gate / test-browser
// invocations.  Safe to do at startup because no tests have run yet.
await rm(TEST_SESSIONS_ROOT, { recursive: true, force: true });
await mkdir(TEST_SESSIONS_ROOT, { recursive: true });

let sessionPaths: SessionPaths = await makeSessionDir(new Date(), TEST_SESSIONS_ROOT);

/** Track all session dirs created by this server so reset can clean only those. */
const ownedSessionDirs: Set<string> = new Set([sessionPaths.dir]);

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
        const result = parseOmegaEvent(raw);
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
  currentTurnState = "idle";
  // Create a fresh uniquely-named session directory for the next session
  sessionPaths = await makeSessionDir(new Date(), TEST_SESSIONS_ROOT);
  ownedSessionDirs.add(sessionPaths.dir);
}

function sendWs(event: object): void {
  try { activeWs?.send(JSON.stringify(event)); } catch { /* ignore */ }
}

// ---------------------------------------------------------------------------
// Session listing — mirrors real server's GET /api/sessions
// ---------------------------------------------------------------------------

/** Convert a session folder name to an ISO timestamp string. */
function folderNameToTimestamp(name: string): string {
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
    entries = await readdir(TEST_SESSIONS_ROOT);
  } catch {
    return [];
  }

  const dirs = entries
    .filter(e => SESSION_DIR_RE.test(e))
    .sort()
    .reverse(); // newest first

  const items: SessionListItem[] = [];
  for (const dir of dirs) {
    const fullDir = join(TEST_SESSIONS_ROOT, dir);
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
// Main server (browser-facing)
// ---------------------------------------------------------------------------

Bun.serve({
  port: MAIN_PORT,
  async fetch(req, srv) {
    if (srv.upgrade(req)) return undefined as any;
    const url = new URL(req.url);

    // File completion: GET /api/files (matches Rust omega-server)
    if (url.pathname === "/api/files" && req.method === "GET") {
      const prefix = url.searchParams.get("prefix") ?? "";
      const items = await listFilesForCompletion(prefix);
      return new Response(JSON.stringify(items), {
        headers: { "Content-Type": "application/json", "Cache-Control": "no-cache" },
      });
    }

    // Session listing: GET /api/sessions (matches Rust omega-server)
    if (url.pathname === "/api/sessions" && req.method === "GET") {
      const sessions = await listSessions();
      return new Response(JSON.stringify(sessions), {
        headers: { "Content-Type": "application/json", "Cache-Control": "no-cache" },
      });
    }

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
      // Re-derive currentTurnState from the replayed events: after closeOpenTurn()
      // any mid-turn crash has a synthetic turn_interrupted appended, so the
      // derived state matches what a real server would see on restart. This also
      // keeps the server's broadcast turnState consistent with the disk log after
      // page reloads within the same server process.
      let derived: TurnState = "idle";
      for (const e of replay) derived = deriveTurnState(derived, e as { type: string });
      currentTurnState = derived;
      ws.cork(() => {
        ws.send(JSON.stringify(buildSessionInfo()));
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
            ws.send(JSON.stringify(buildSessionInfo()));
            ws.send(JSON.stringify({ type: "history", events: [] }));
            ws.send(JSON.stringify({ type: "reset_done" }));
          });
        }).catch(() => {});
        return;
      }

      if (msg.type === "resume_session") {
        const dir = msg.sessionDir as string;
        // Immediately signal that resumption has started
        ws.send(JSON.stringify({ type: "resuming_session", sessionDir: dir }));

        (async () => {
          // Simulate the real server's async work with a configurable delay
          if (resumeDelayMs > 0) {
            await new Promise(r => setTimeout(r, resumeDelayMs));
          }

          // Create a new session dir (as the real server does)
          await resetState();
          const resumed: OmegaEvent = {
            type: "session_resumed",
            time: isoNow(),
            resumedFrom: dir,
            summary: "(test summary of previous session)",
          };
          await appendEvent(resumed, sessionPaths.eventsFile);

          // Replay events from new session (typically just the session_resumed
          // event we just wrote). Re-derive turnState for consistency.
          const replay = await loadReplayEvents();
          let derived: TurnState = "idle";
          for (const e of replay) derived = deriveTurnState(derived, e as { type: string });
          currentTurnState = derived;
          ws.cork(() => {
            ws.send(JSON.stringify(buildSessionInfo()));
            ws.send(JSON.stringify({ type: "history", events: replay }));
            ws.send(JSON.stringify({ type: "ready" }));
          });
        })().catch(() => {});
        return;
      }

      if (msg.type === "delete_session") {
        const dir = msg.sessionDir as string;
        // Safety: only delete directories matching the session pattern
        if (!SESSION_DIR_RE.test(dir)) {
          ws.send(JSON.stringify({ type: "transport_error", time: new Date().toISOString(), error: `Invalid session dir: ${dir}` }));
          return;
        }
        const fullDir = join(TEST_SESSIONS_ROOT, dir);
        rm(fullDir, { recursive: true, force: true })
          .then(() => {
            ws.send(JSON.stringify({ type: "session_deleted", sessionDir: dir }));
          })
          .catch((err: unknown) => {
            ws.send(JSON.stringify({ type: "transport_error", time: new Date().toISOString(), error: `Delete failed: ${err}` }));
          });
        return;
      }

      if (msg.type === "rename_session") {
        const dir = msg.sessionDir as string;
        const name = msg.name as string;
        if (!SESSION_DIR_RE.test(dir)) {
          ws.send(JSON.stringify({ type: "transport_error", time: new Date().toISOString(), error: `Invalid session dir: ${dir}` }));
          return;
        }
        const fullDir = join(TEST_SESSIONS_ROOT, dir);
        updateSessionMetadata(fullDir, { name })
          .then(() => {
            ws.send(JSON.stringify({ type: "session_renamed", sessionDir: dir, name }));
          })
          .catch((err: unknown) => {
            ws.send(JSON.stringify({ type: "transport_error", time: new Date().toISOString(), error: `Rename failed: ${err}` }));
          });
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
      // Ensure every event has a time field — tests focus on semantics, not timestamps.
      // This mirrors what the real agent does: every emitted OmegaEvent has a time.
      const event: Record<string, unknown> = {
        time: new Date().toISOString(),
        ...body.event,
      };
      // Persist to disk (same filter the real server uses), then forward to browser
      if (shouldLogEvent(event)) {
        // appendEvent strips UI-only fields before writing
        await appendEvent(event as unknown as OmegaEvent, sessionPaths.eventsFile);
      }
      sendWs(event);
      // Broadcast a session_info update if this event transitions turnState —
      // mirrors what the real server does from its agent event loop.
      const nextTurnState = deriveTurnState(currentTurnState, event as { type: string });
      if (nextTurnState !== currentTurnState) {
        currentTurnState = nextTurnState;
        sendWs(buildSessionInfo());
      }
      return new Response("ok");
    }

    if (req.method === "POST" && url.pathname === "/control/reset") {
      receivedMessages.length = 0;
      currentAgentId = 1;
      currentTurnState = "idle";
      resumeDelayMs = 0;
      // Note: hasPendingChanges is NOT reset here — it is configured
      // independently via POST /control/set-pending-changes and
      // reflects per-test setup, not per-session teardown.
      // Delete only sessions created by this server — not those belonging to
      // concurrent bun-test agents sharing .omega/test-sessions/.
      for (const dir of ownedSessionDirs) {
        await rm(dir, { recursive: true, force: true }).catch(() => {});
      }
      ownedSessionDirs.clear();
      // Create a fresh uniquely-named session directory for the new session
      sessionPaths = await makeSessionDir(new Date(), TEST_SESSIONS_ROOT);
      ownedSessionDirs.add(sessionPaths.dir);
      return new Response("ok");
    }

    // Create a past session with metadata + optional events (for session picker tests)
    if (req.method === "POST" && url.pathname === "/control/create-past-session") {
      const body = await req.json() as {
        metadata?: SessionMetadata;
        events?: Array<Record<string, unknown>>;
      };
      const pastPaths = await makeSessionDir(new Date(), TEST_SESSIONS_ROOT);
      ownedSessionDirs.add(pastPaths.dir);
      if (body.metadata) {
        await writeSessionMetadata(pastPaths.dir, body.metadata);
      }
      if (body.events) {
        for (const event of body.events) {
          const ev: Record<string, unknown> = {
            time: new Date().toISOString(),
            ...event,
          };
          await appendEvent(ev as unknown as OmegaEvent, pastPaths.eventsFile);
        }
      }
      return new Response(JSON.stringify({ dir: pastPaths.dir }), {
        headers: { "Content-Type": "application/json" },
      });
    }

    // Configure the resume delay for testing the "resuming…" state
    if (req.method === "POST" && url.pathname === "/control/set-resume-delay") {
      const body = await req.json() as { delayMs: number };
      resumeDelayMs = body.delayMs;
      return new Response("ok");
    }

    // Configure whether the next session_info reports pending git changes
    if (req.method === "POST" && url.pathname === "/control/set-pending-changes") {
      const body = await req.json() as { hasPendingChanges: boolean };
      hasPendingChanges = body.hasPendingChanges;
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
