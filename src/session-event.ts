/**
 * SessionEvent — canonical persistent event log (Step 3c).
 *
 * Every significant event in a session is appended to `sessions/events.jsonl`
 * as it happens. This file is the single source of truth for diagnostics,
 * session replay, and (eventually) UI visualisation.
 *
 * Design:
 * - All events carry an ISO `ts` timestamp.
 * - Streaming `text` fragments and ephemeral `status` messages are NOT
 *   persisted — they are UI concerns. The full assembled response is
 *   captured in `llm_response`.
 * - The agent writes this file directly (same pattern as context-store.ts).
 *   The UI layer renders; it does not persist agent state.
 * - `filePath: string | null` — null is an explicit no-op for test isolation.
 */

import { appendFile, writeFile, mkdir } from "fs/promises";
import { dirname } from "path";
import { rotateFile } from "./context-store.js";
import type Anthropic from "@anthropic-ai/sdk";
import type { ToolResult } from "./tools.js";
import type { TurnMetrics, ProviderName } from "./agent.js";

export const DEFAULT_EVENTS_FILE = "sessions/events.jsonl";

// ---------------------------------------------------------------------------
// SessionEvent discriminated union
// ---------------------------------------------------------------------------

/** A user message submitted to the agent. */
export interface UserMessageEvent {
  type: "user_message";
  ts: string;
  content: string;
}

/** An outgoing API call to an LLM. */
export interface LlmCallEvent {
  type: "llm_call";
  ts: string;
  llmCallNumber: number;
  provider: ProviderName;
  url: string;
  model: string;
  messageCount: number;
  /**
   * Step 3e-iii: ordered SHA-256 hashes (8 hex chars each) of every
   * MessageParam in the `buildApiMessages()` view actually sent with this
   * call. Hashes are computed from the stored ContextRecord (including `ts`),
   * so they cross-reference entries in `context.jsonl` exactly.
   *
   * Note: this reflects the truncated view sent to the LLM, NOT the full
   * `llmMessageLog`. Messages dropped by truncation will be absent.
   */
  contextHashes: string[];
}

/** An LLM response received by the agent. */
export interface LlmResponseEvent {
  type: "llm_response";
  ts: string;
  provider: ProviderName;
  url: string;
  stopReason: string;
  model: string;
  content: Anthropic.ContentBlock[];
  usage: {
    input_tokens: number;
    output_tokens: number;
    cache_creation_input_tokens?: number;
    cache_read_input_tokens?: number;
  };
}

/** A tool invocation by the agent. */
export interface ToolCallEvent {
  type: "tool_call";
  ts: string;
  id: string;
  name: string;
  input: unknown;
}

/** The result of a tool invocation. */
export interface ToolResultEvent {
  type: "tool_result";
  ts: string;
  id: string;
  name: string;
  isError: boolean;
  durationMs?: number;
  outputLength: number;
}

/** End of a user turn — aggregate metrics. */
export interface TurnEndEvent {
  type: "turn_end";
  ts: string;
  provider: ProviderName;
  model: string;
  metrics: TurnMetrics;
  toolCalls: string[];
}

/** A non-retryable LLM provider call error. */
export interface LlmErrorEvent {
  type: "llm_error";
  ts: string;
  provider: ProviderName;
  url: string;
  error: string;
  httpStatus?: number;
}

/** A generic agent-level error (slash-command failures, etc.). */
export interface AgentErrorEvent {
  type: "agent_error";
  ts: string;
  error: string;
}

/** The user interrupted an in-flight turn. */
export interface TurnInterruptedEvent {
  type: "turn_interrupted";
  ts: string;
}

/** History was compacted via /compact. */
export interface SessionCompactedEvent {
  type: "session_compacted";
  ts: string;
  originalCount: number;
  newCount: number;
}

/** The session started (first event in every session). */
export interface SessionStartEvent {
  type: "session_start";
  ts: string;
  sessionId: string;
  model: string;
  provider: ProviderName;
  authMode: string;
}

/** OAuth token was successfully refreshed mid-session. */
export interface OauthRefreshedEvent {
  type: "oauth_refreshed";
  ts: string;
}

/** OAuth token expired, triggering a refresh attempt. */
export interface OauthTokenExpiredEvent {
  type: "oauth_token_expired";
  ts: string;
  attempt: number;
  httpStatus?: number;
}

/** LLM provider call retried after a transient error. */
export interface LlmRetryEvent {
  type: "llm_retry";
  ts: string;
  attempt: number;
  provider: ProviderName;
  httpStatus?: number;
  waitMs: number;
  error: string;
}

/** A diagnostic snapshot was written to disk. */
export interface DiagnosticWrittenEvent {
  type: "diagnostic_written";
  ts: string;
  path: string;
}

/** The context view sent to the LLM was trimmed to fit within the token budget.
 *  The canonical history (llmMessageLog) is never modified. */
export interface ContextViewTrimmedEvent {
  type: "context_view_trimmed";
  ts: string;
  originalMessages: number;
  keptMessages: number;
  droppedMessages: number;
  estimatedTokensBefore: number;
  estimatedTokensAfter: number;
  reason: string;
}

export type SessionEvent =
  | SessionStartEvent
  | UserMessageEvent
  | LlmCallEvent
  | LlmResponseEvent
  | ToolCallEvent
  | ToolResultEvent
  | TurnEndEvent
  | LlmErrorEvent
  | AgentErrorEvent
  | TurnInterruptedEvent
  | SessionCompactedEvent
  | OauthRefreshedEvent
  | OauthTokenExpiredEvent
  | LlmRetryEvent
  | DiagnosticWrittenEvent
  | ContextViewTrimmedEvent;

// ---------------------------------------------------------------------------
// Persistence helpers
// ---------------------------------------------------------------------------

/**
 * Append a single SessionEvent to the events JSONL file.
 * Creates the file (and parent directories) if they don't exist.
 * Pass `null` to disable the write (test isolation).
 */
export async function appendSessionEvent(
  event: SessionEvent,
  filePath: string | null = DEFAULT_EVENTS_FILE
): Promise<void> {
  if (filePath === null) return;
  await mkdir(dirname(filePath), { recursive: true });
  await appendFile(filePath, JSON.stringify(event) + "\n", "utf-8");
}

/**
 * Rotate events.jsonl → events.prev.jsonl, then start fresh.
 * Called at session start so the previous session's events are preserved
 * for diagnostics while the current session starts clean.
 * No-op if filePath is null (test isolation).
 */
export async function clearSessionEvents(
  filePath: string | null = DEFAULT_EVENTS_FILE
): Promise<void> {
  if (filePath === null) return;
  await rotateFile(filePath);
}
