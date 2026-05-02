/**
 * OmegaEvent — the single unified event type for Omega.
 *
 * All types are generated from the Rust source via ts-rs (→ rust/bindings/).
 * This file re-exports them, augmenting each event struct with the `type`
 * discriminator field for backward compatibility with TypeScript consumers
 * that construct typed event objects.
 *
 * Background: the generated struct types don't include the `type` field
 * (it lives on the Rust enum, not on each variant struct).  We intersect it
 * back in here so existing code like:
 *   const e: SessionStartedEvent = { type: "session_started", ... }
 * continues to type-check without modification.
 *
 * `StreamSignal` is the separate union for genuinely ephemeral rendering
 * primitives that are never persisted: currently only `text` / `thinking`
 * (streaming token fragments). The agent yields `OmegaEvent | StreamSignal`.
 *
 * ─── Regenerate bindings ──────────────────────────────────────────────────
 *   just rust-bindings          # re-runs ts-rs, writes rust/bindings/
 *   git diff --exit-code rust/bindings/   # drift check (also in rust-gate)
 * ──────────────────────────────────────────────────────────────────────────
 */

// ---------------------------------------------------------------------------
// StreamSignal — ephemeral, never persisted
// ---------------------------------------------------------------------------

/** A raw streaming text fragment from the LLM. Never written to events.jsonl. */
export type TextSignal = { type: "text"; text: string };

/** A raw streaming thinking fragment from the LLM. Never written to events.jsonl. */
export type ThinkingSignal = { type: "thinking"; text: string };

/**
 * Streaming signal — union of ephemeral fragment types.
 * Defined locally so that TextSignal / ThinkingSignal are referenced here
 * (the generated rust/bindings/StreamSignal.ts has the same shape).
 */
export type StreamSignal = TextSignal | ThinkingSignal;

// ---------------------------------------------------------------------------
// Sub-types (not enum variants — no discriminator needed)
// ---------------------------------------------------------------------------

export type { TurnMetrics } from "../rust/bindings/TurnMetrics.js";
export type { LlmResponseUsage } from "../rust/bindings/LlmResponseUsage.js";
export type { ServerStopOutcome } from "../rust/bindings/ServerStopOutcome.js";
export type { InterruptReason } from "../rust/bindings/InterruptReason.js";
export type { ContinueMode } from "../rust/bindings/ContinueMode.js";
export type { LlmRetryReason } from "../rust/bindings/LlmRetryReason.js";

// ---------------------------------------------------------------------------
// OmegaEvent variant types
//
// Each generated struct type is intersected with { type: "<discriminator>" }
// to restore the discriminator field that lives on the Rust enum.
// ---------------------------------------------------------------------------

import type { SessionStartedEvent as _SessionStartedEvent } from "../rust/bindings/SessionStartedEvent.js";
export type SessionStartedEvent = { type: "session_started" } & _SessionStartedEvent;

import type { ServerStartedEvent as _ServerStartedEvent } from "../rust/bindings/ServerStartedEvent.js";
export type ServerStartedEvent = { type: "server_started" } & _ServerStartedEvent;

import type { ServerStoppedEvent as _ServerStoppedEvent } from "../rust/bindings/ServerStoppedEvent.js";
export type ServerStoppedEvent = { type: "server_stopped" } & _ServerStoppedEvent;

import type { UserMessageEvent as _UserMessageEvent } from "../rust/bindings/UserMessageEvent.js";
export type UserMessageEvent = { type: "user_message" } & _UserMessageEvent;

import type { LlmCallEvent as _LlmCallEvent } from "../rust/bindings/LlmCallEvent.js";
export type LlmCallEvent = { type: "llm_call" } & _LlmCallEvent;

import type { LlmResponseEvent as _LlmResponseEvent } from "../rust/bindings/LlmResponseEvent.js";
export type LlmResponseEvent = { type: "llm_response" } & _LlmResponseEvent;

import type { ToolCallEvent as _ToolCallEvent } from "../rust/bindings/ToolCallEvent.js";
export type ToolCallEvent = { type: "tool_call" } & _ToolCallEvent;

import type { ToolResultEvent as _ToolResultEvent } from "../rust/bindings/ToolResultEvent.js";
export type ToolResultEvent = { type: "tool_result" } & _ToolResultEvent;

import type { TurnEndEvent as _TurnEndEvent } from "../rust/bindings/TurnEndEvent.js";
export type TurnEndEvent = { type: "turn_end" } & _TurnEndEvent;

import type { LlmErrorEvent as _LlmErrorEvent } from "../rust/bindings/LlmErrorEvent.js";
export type LlmErrorEvent = { type: "llm_error" } & _LlmErrorEvent;

import type { AgentErrorEvent as _AgentErrorEvent } from "../rust/bindings/AgentErrorEvent.js";
export type AgentErrorEvent = { type: "agent_error" } & _AgentErrorEvent;

import type { TurnInterruptedEvent as _TurnInterruptedEvent } from "../rust/bindings/TurnInterruptedEvent.js";
export type TurnInterruptedEvent = { type: "turn_interrupted" } & _TurnInterruptedEvent;

import type { CompactedEvent as _CompactedEvent } from "../rust/bindings/CompactedEvent.js";
export type CompactedEvent = { type: "compacted" } & _CompactedEvent;

import type { LlmRetryEvent as _LlmRetryEvent } from "../rust/bindings/LlmRetryEvent.js";
export type LlmRetryEvent = { type: "llm_retry" } & _LlmRetryEvent;

import type { ModelChangedEvent as _ModelChangedEvent } from "../rust/bindings/ModelChangedEvent.js";
export type ModelChangedEvent = { type: "model_changed" } & _ModelChangedEvent;

import type { EffortChangedEvent as _EffortChangedEvent } from "../rust/bindings/EffortChangedEvent.js";
export type EffortChangedEvent = { type: "effort_changed" } & _EffortChangedEvent;

import type { TransportErrorEvent as _TransportErrorEvent } from "../rust/bindings/TransportErrorEvent.js";
export type TransportErrorEvent = { type: "transport_error" } & _TransportErrorEvent;

import type { ResumingSessionEvent as _ResumingSessionEvent } from "../rust/bindings/ResumingSessionEvent.js";
export type ResumingSessionEvent = { type: "resuming_session" } & _ResumingSessionEvent;

import type { SessionResumedEvent as _SessionResumedEvent } from "../rust/bindings/SessionResumedEvent.js";
export type SessionResumedEvent = { type: "session_resumed" } & _SessionResumedEvent;

import type { PauseRequestedEvent as _PauseRequestedEvent } from "../rust/bindings/PauseRequestedEvent.js";
export type PauseRequestedEvent = { type: "pause_requested" } & _PauseRequestedEvent;

import type { TurnPausedEvent as _TurnPausedEvent } from "../rust/bindings/TurnPausedEvent.js";
export type TurnPausedEvent = { type: "turn_paused" } & _TurnPausedEvent;

import type { TurnContinuedEvent as _TurnContinuedEvent } from "../rust/bindings/TurnContinuedEvent.js";
export type TurnContinuedEvent = { type: "turn_continued" } & _TurnContinuedEvent;

// ---------------------------------------------------------------------------
// OmegaEvent — the unified discriminated union, generated from Rust
// ---------------------------------------------------------------------------

export type { OmegaEvent } from "../rust/bindings/OmegaEvent.js";
