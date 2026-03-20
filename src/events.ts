/**
 * OmegaEvent — the single unified event type for Omega.
 *
 * Replaces the former split between `AgentEvent` (streamed, ephemeral) and
 * `SessionEvent` (persisted). Every `OmegaEvent` is both streamed to UI
 * consumers and written to `.omega/sessions/<timestamp>/events.jsonl`.
 *
 * Naming authority: persisted names win (EU-3 design rule). The `events.jsonl`
 * file is the single source of truth. Stream-facing names conform to it, not
 * the other way around.
 *
 * `StreamSignal` is the separate union for genuinely ephemeral rendering
 * primitives that are never persisted: currently only `text` (streaming token
 * fragments). The `sendMessage` generator yields `OmegaEvent | StreamSignal`.
 */

import type { TurnMetrics } from "./agent.js";
import type { CompactionUsage } from "./compaction.js";

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
  provider: "anthropic";
  authMode: string;
  /** The full system prompt text at session start. */
  systemPrompt: string;
}

/** The session ended cleanly. Absence of this event means the session crashed. */
export interface SessionEndEvent {
  type: "session_end";
  ts: string;
  /** "clean" = normal shutdown; "error" = session ended due to a hard error. */
  outcome: "clean" | "error";
  /** Human-readable reason, e.g. the error message on "error" outcome. */
  reason?: string;
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
  provider: "anthropic";
  url: string;
  model: string;
  /**
   * Ordered SHA-256 hashes (8 hex chars each) of every MessageParam in the
   * `buildSentContext()` view actually sent with this call. Cross-references
   * entries in `context.jsonl`. Reflects the truncated view, not the full log.
   */
  contextHashes: string[];
  /**
   * Index (0-based) of the message in the sent context that received the
   * `cache_control: { type: "ephemeral" }` breakpoint for Anthropic prompt
   * caching. Always the last message index (contextHashes.length - 1).
   */
  cacheBreakpointIndex: number | null;
  /**
   * Serialized byte size of the full request payload sent to the provider.
   * Measured as JSON.stringify(payload).length at the call site, before any
   * elision. Useful for estimating upstream network cost.
   */
  requestBytes: number;
  /**
   * Elided summary of the request sent to the provider. Large repetitive
   * fields (system prompt, messages, tool definitions) are replaced with
   * compact descriptors. Persisted to events.jsonl.
   */
  requestSummary?: Record<string, unknown>;
}

/** An LLM response received by the agent. */
export interface LlmResponseEvent {
  type: "llm_response";
  ts: string;
  stopReason: string;
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
  /**
   * The full assembled assistant text for this response, if any.
   * Absent for tool-only responses (stop_reason "tool_use" with no text block).
   * Replaces the former separate `assistant_text` event.
   */
  text?: string;
  /** ISO timestamp of the first streaming text delta — when text visibly began arriving. */
  streamingStart?: string;
  /**
   * Elided summary of the provider response envelope. The content field is
   * omitted (it lives in context.jsonl via contextHash); all other envelope
   * fields (id, model, stop_reason, usage, type, role) are kept verbatim.
   * Persisted to events.jsonl.
   */
  responseSummary?: Record<string, unknown>;
}

/** A tool invocation by the agent. */
export interface ToolCallEvent {
  type: "tool_call";
  ts: string;
  id: string;
  name: string;
  /** Tool input parameters. */
  input: unknown;
  /** Hash of the assistant context.jsonl record containing this tool_use block. */
  contextHash: string;
}

/** The result of a tool invocation. */
export interface ToolResultEvent {
  type: "tool_result";
  ts: string;
  id: string;
  name: string;
  isError: boolean;
  durationMs: number;
  /** Full text output of the tool. */
  output: string;
  /** Hash of the user context.jsonl record containing this tool_result block. */
  contextHash: string;
}

/** End of a user turn — aggregate metrics. */
export interface TurnEndEvent {
  type: "turn_end";
  ts: string;
  metrics: TurnMetrics;
}

/** A non-retryable LLM provider call error. */
export interface LlmErrorEvent {
  type: "llm_error";
  ts: string;
  provider: "anthropic";
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

/** Operator triggered /compact — compaction LLM call about to start. */
export interface CompactUserStartEvent {
  type: "compact_user_start";
  ts: string;
}

/** Compaction completed successfully. */
export interface CompactUserDoneEvent {
  type: "compact_user_done";
  ts: string;
  messagesBefore: number;
  messagesAfter: number;
  usage?: CompactionUsage;
}

/** Compaction failed. */
export interface CompactUserErrorEvent {
  type: "compact_user_error";
  ts: string;
  error: string;
}

/** Automatic compaction triggered (context grew beyond threshold). */
export interface CompactAutoStartEvent {
  type: "compact_auto_start";
  ts: string;
  messagesBefore: number;
}

/** Automatic compaction completed successfully. */
export interface CompactAutoDoneEvent {
  type: "compact_auto_done";
  ts: string;
  messagesBefore: number;
  messagesAfter: number;
  usage?: CompactionUsage;
}

/** Automatic compaction failed — session continues with rolling truncation as fallback. */
export interface CompactAutoErrorEvent {
  type: "compact_auto_error";
  ts: string;
  error: string;
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
  provider: "anthropic";
  httpStatus?: number;
  waitMs: number;
  error: string;
}



/** The operator switched the active model via a slash command. */
export interface ModelChangedEvent {
  type: "model_changed";
  ts: string;
  model: string;
}



// ---------------------------------------------------------------------------
// Exhaustiveness helper
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// OmegaEvent — the unified discriminated union
// ---------------------------------------------------------------------------

export type OmegaEvent =
  | SessionStartEvent
  | SessionEndEvent
  | UserMessageEvent
  | LlmCallEvent
  | LlmResponseEvent
  | ToolCallEvent
  | ToolResultEvent
  | TurnEndEvent
  | LlmErrorEvent
  | AgentErrorEvent
  | TurnInterruptedEvent
  | CompactUserStartEvent
  | CompactUserDoneEvent
  | CompactUserErrorEvent
  | CompactAutoStartEvent
  | CompactAutoDoneEvent
  | CompactAutoErrorEvent
  | OauthRefreshedEvent
  | OauthTokenExpiredEvent
  | LlmRetryEvent
  | ModelChangedEvent;
