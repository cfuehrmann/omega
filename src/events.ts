/**
 * OmegaEvent — the single unified event type for Omega.
 *
 * Replaces the former split between `AgentEvent` (streamed, ephemeral) and
 * `SessionEvent` (persisted). Every `OmegaEvent` is both streamed to UI
 * consumers and written to `sessions/events.jsonl`.
 *
 * Naming authority: persisted names win (EU-3 design rule). The `events.jsonl`
 * file is the single source of truth. Stream-facing names conform to it, not
 * the other way around.
 *
 * `StreamSignal` is the separate union for genuinely ephemeral rendering
 * primitives that are never persisted: currently only `text` (streaming token
 * fragments). The `sendMessage` generator yields `OmegaEvent | StreamSignal`.
 */

import type { TurnMetrics, ProviderName } from "./agent.js";

// ---------------------------------------------------------------------------
// StreamSignal — ephemeral, never persisted
// ---------------------------------------------------------------------------

/** A raw streaming text fragment from the LLM. Never written to events.jsonl. */
export interface TextSignal {
  type: "text";
  text: string;
}

export type StreamSignal = TextSignal;

// ---------------------------------------------------------------------------
// OmegaEvent variants — all persisted, all rendered
// ---------------------------------------------------------------------------

/** The session started (first event in every session). */
export interface SessionStartEvent {
  type: "session_start";
  ts: string;
  sessionId: string;
  model: string;
  provider: ProviderName;
  authMode: string;
}

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
  provider: ProviderName;
  url: string;
  model: string;
  /**
   * Ordered SHA-256 hashes (8 hex chars each) of every MessageParam in the
   * `buildApiMessages()` view actually sent with this call. Cross-references
   * entries in `context.jsonl`. Reflects the truncated view, not the full log.
   */
  contextHashes: string[];
  /** Snapshot of the full request object (Anthropic or OpenAI). UI only — not persisted. */
  request?: any;
}

/** An LLM response received by the agent. */
export interface LlmResponseEvent {
  type: "llm_response";
  ts: string;
  provider: ProviderName;
  url: string;
  stopReason: string;
  model: string;
  usage: {
    input_tokens: number;
    output_tokens: number;
    /** Tokens written to the prompt cache this call (billed at 1.25× base). */
    cache_creation_input_tokens?: number | null;
    /** Tokens served from the prompt cache this call (billed at 0.1× base). */
    cache_read_input_tokens?: number | null;
    /** Service tier used for this request; absent or "standard" is the baseline. */
    service_tier?: string | null;
  };
  /**
   * FK into `context.jsonl` — the hash of the assistant record written for
   * this response. Content is intentionally omitted from the event itself;
   * look it up via this hash.
   */
  contextHash: string;
  /** Full content blocks from the response. UI only — not persisted. */
  content?: any[];
  /** Raw provider response. UI only — not persisted. */
  raw?: any;
}

/** A tool invocation by the agent. */
export interface ToolCallEvent {
  type: "tool_call";
  ts: string;
  id: string;
  name: string;
  /** Hash of the assistant context.jsonl record containing this tool_use block. */
  contextHash: string;
  /** Tool input. UI only — not persisted (content is in context.jsonl). */
  input?: any;
  /** Formatted call string. UI only — not persisted. */
  formatted?: string;
}

/** The result of a tool invocation. */
export interface ToolResultEvent {
  type: "tool_result";
  ts: string;
  id: string;
  name: string;
  isError: boolean;
  durationMs?: number;
  /** Hash of the user context.jsonl record containing this tool_result block. */
  contextHash: string;
  /** Full tool result. UI only — not persisted (content is in context.jsonl). */
  result?: any;
  /** Formatted call string. UI only — not persisted. */
  formatted?: string;
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

/** The context view sent to the LLM was trimmed to fit within the token budget. */
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

/** The operator switched the active model/provider via a slash command. */
export interface ModelChangedEvent {
  type: "model_changed";
  ts: string;
  provider: ProviderName;
  model: string;
}

// ---------------------------------------------------------------------------
// Exhaustiveness helper
// ---------------------------------------------------------------------------

/**
 * Call this in the `default` branch of an exhaustive switch over `OmegaEvent`
 * (or any other discriminated union) to get a compile-time error if any
 * variant is not handled.
 *
 * Usage:
 *   switch (event.type) {
 *     case "foo": ...; break;
 *     // ...all cases...
 *     default: exhaustiveCheck(event);
 *   }
 */
export function exhaustiveCheck(x: never): never {
  throw new Error(`Unhandled event type: ${(x as any).type}`);
}

// ---------------------------------------------------------------------------
// OmegaEvent — the unified discriminated union
// ---------------------------------------------------------------------------

export type OmegaEvent =
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
  | ContextViewTrimmedEvent
  | ModelChangedEvent;
