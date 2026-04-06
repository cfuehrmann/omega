/**
 * WebSocket protocol — client ↔ server message contracts.
 *
 * Single source of truth for all WebSocket message shapes. Both sides
 * import from here:
 *
 * Client → Server:
 *   - Client (App.tsx): uses `ClientMessage` for compile-time enforcement.
 *   - Server (server.ts): calls `ClientMessageSchema.parse()` for runtime enforcement.
 *
 * Server → Client:
 *   - Server (server.ts): constructs messages that conform to `ServerMessage`.
 *   - Client (App.tsx): calls `ServerMessageSchema.parse()` for runtime enforcement.
 */

import { z } from "zod";
import { OmegaEventSchema } from "../events.schema.js";
import type { OmegaEvent, StreamSignal } from "../events.js";

// ---------------------------------------------------------------------------
// Model enum (shared by both directions)
// ---------------------------------------------------------------------------

export const OmegaModelSchema = z.enum(["claude-sonnet-4-6", "claude-opus-4-6"]);
export type OmegaModel = z.infer<typeof OmegaModelSchema>;

// ---------------------------------------------------------------------------
// Client → Server
// ---------------------------------------------------------------------------

export const OmegaEffortSchema = z.enum(["low", "medium", "high", "max"]);
export type OmegaEffort = z.infer<typeof OmegaEffortSchema>;

export const ClientMessageSchema = z.discriminatedUnion("type", [
  z.object({ type: z.literal("message"), content: z.string() }),
  z.object({ type: z.literal("abort") }),
  z.object({ type: z.literal("reset") }),
  z.object({ type: z.literal("set_model"), model: OmegaModelSchema }),
  z.object({ type: z.literal("set_effort"), effort: OmegaEffortSchema }),
  /**
   * Resume a previous session. `sessionDir` is the relative folder name
   * (within SESSIONS_ROOT) of the session to continue.
   */
  z.object({ type: z.literal("resume_session"), sessionDir: z.string() }),
]);

export type ClientMessage = z.infer<typeof ClientMessageSchema>;

// ---------------------------------------------------------------------------
// Server → Client
// ---------------------------------------------------------------------------

/**
 * Streaming text/thinking fragments — ephemeral, never persisted.
 * Sent during a live turn; cleared when the settled llm_response arrives.
 */
const StreamSignalSchema = z.discriminatedUnion("type", [
  z.object({ type: z.literal("text"),     text: z.string() }),
  z.object({ type: z.literal("thinking"), text: z.string() }),
]) satisfies z.ZodType<StreamSignal>;

/**
 * Protocol envelopes — connection and session management signals sent by the
 * server at connection open and after a reset. Never written to events.jsonl.
 */
const ProtocolEnvelopeSchema = z.discriminatedUnion("type", [
  z.object({ type: z.literal("ready") }),
  z.object({ type: z.literal("reset_done") }),
  z.object({ type: z.literal("session_info"), dir: z.string(), model: z.string(), effort: z.string(), cwd: z.string() }),
  z.object({ type: z.literal("history"),      events: z.array(OmegaEventSchema) }),
]);

/**
 * Union of every message the server can send over the WebSocket.
 *
 * Composed of three groups:
 *   - OmegaEvent        — agent events (persisted to events.jsonl, replayed on reconnect)
 *   - StreamSignal      — ephemeral streaming text/thinking fragments
 *   - ProtocolEnvelope  — connection/session management signals
 *
 * Use `ServerMessageSchema.parse()` at the client's ws.onmessage boundary.
 */
export const ServerMessageSchema = z.union([
  OmegaEventSchema,
  StreamSignalSchema,
  ProtocolEnvelopeSchema,
]) satisfies z.ZodType<OmegaEvent | StreamSignal | { type: "ready" } | { type: "reset_done" } | { type: "session_info"; dir: string; model: string; effort: string } | { type: "history"; events: OmegaEvent[] }>;

export type ServerMessage = z.infer<typeof ServerMessageSchema>;
