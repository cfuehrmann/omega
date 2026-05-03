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

export const OmegaModelSchema = z.enum(["claude-sonnet-4-6", "claude-opus-4-6", "claude-opus-4-7"]);
export type OmegaModel = z.infer<typeof OmegaModelSchema>;

// ---------------------------------------------------------------------------
// Client → Server
// ---------------------------------------------------------------------------

export const OmegaEffortSchema = z.enum(["low", "medium", "high", "xhigh", "max"]);
export type OmegaEffort = z.infer<typeof OmegaEffortSchema>;

/**
 * Server-tracked turn state, surfaced on `session_info` so the client can
 * render the right UI (buttons + status) without replaying events on every
 * reconnect. Derived server-side from the agent's event stream.
 *
 *   idle             — no turn active.
 *   running          — turn in progress, no pause pending.
 *   pause_requested  — user pressed pause; agent still running until seam.
 *   paused           — agent suspended at seam, awaiting continue/abort.
 */
export const TurnStateSchema = z.enum(["idle", "running", "pause_requested", "paused"]);
export type TurnState = z.infer<typeof TurnStateSchema>;

export const ClientMessageSchema = z.discriminatedUnion("type", [
  z.object({ type: z.literal("message"), content: z.string() }),
  z.object({ type: z.literal("abort") }),
  /** User pressed Esc from Running: request pause at next clean seam. */
  z.object({ type: z.literal("pause") }),
  /**
   * Continue from Paused (or PauseRequested with pre-commit fired). Optional
   * `content` is a mid-turn interjection appended as a user_message before the
   * next LLM call. Empty/undefined resumes without a message.
   */
  z.object({ type: z.literal("continue"), content: z.string().optional() }),
  z.object({ type: z.literal("reset") }),
  z.object({ type: z.literal("set_model"), model: OmegaModelSchema }),
  z.object({ type: z.literal("set_effort"), effort: OmegaEffortSchema }),
  /**
   * Resume a previous session. `sessionDir` is the relative folder name
   * (within SESSIONS_ROOT) of the session to continue.
   */
  z.object({ type: z.literal("resume_session"), sessionDir: z.string() }),
  /**
   * Delete a session directory. `sessionDir` is the relative folder name
   * (within SESSIONS_ROOT) to remove.
   */
  z.object({ type: z.literal("delete_session"), sessionDir: z.string() }),
  /**
   * Rename a session. `sessionDir` is the relative folder name
   * (within SESSIONS_ROOT); `name` is the new display name.
   */
  z.object({ type: z.literal("rename_session"), sessionDir: z.string(), name: z.string() }),
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
  /**
   * Signals "ready to interact". When `streaming` is true the server has an
   * in-flight agent turn that survived a browser refresh; the client should
   * keep `streaming = true` rather than resetting to idle.
   */
  z.object({ type: z.literal("ready"), streaming: z.boolean().optional() }),
  z.object({ type: z.literal("reset_done") }),
  z.object({
    type: z.literal("session_info"),
    dir: z.string(),
    model: z.string(),
    effort: z.string(),
    cwd: z.string(),
    name: z.string().optional(),
    /**
     * Live turn state. Sent on WS open and whenever the state transitions.
     * Optional for backwards compatibility with older server versions (client
     * defaults to "idle" when absent).
     */
    turnState: TurnStateSchema.optional(),
    /**
     * True when the server detected uncommitted git changes at session
     * creation time.  The client shows a blocking modal until acknowledged.
     * Optional for backwards compatibility; absent means false.
     */
    hasPendingChanges: z.boolean().optional(),
  }),
  /**
   * History replay batch. When `streaming` is true the server has an
   * in-flight turn; the client must NOT add a synthetic turn_interrupted
   * — it will receive the real turn_end (or turn_interrupted) over the live socket.
   */
  z.object({ type: z.literal("history"), events: z.array(OmegaEventSchema), streaming: z.boolean().optional() }),
  /** Confirms a session was deleted. */
  z.object({ type: z.literal("session_deleted"), sessionDir: z.string() }),
  /** Confirms a session was renamed (or auto-named). */
  z.object({ type: z.literal("session_renamed"), sessionDir: z.string(), name: z.string() }),
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
]) satisfies z.ZodType<
  | OmegaEvent
  | StreamSignal
  | { type: "ready"; streaming?: boolean }
  | { type: "reset_done" }
  | { type: "session_info"; dir: string; model: string; effort: string; cwd: string; name?: string; turnState?: TurnState; hasPendingChanges?: boolean }
  | { type: "history"; events: OmegaEvent[]; streaming?: boolean }
  | { type: "session_deleted"; sessionDir: string }
  | { type: "session_renamed"; sessionDir: string; name: string }
>;

export type ServerMessage = z.infer<typeof ServerMessageSchema>;
