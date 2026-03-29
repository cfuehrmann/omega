/**
 * Zod schemas for OmegaEvent variants and the full discriminated union.
 *
 * These replace unsafe `JSON.parse(line) as OmegaEvent` casts at the
 * persistence boundary: reading events.jsonl back from disk now either
 * succeeds with a properly typed OmegaEvent or surfaces a parse error
 * rather than silently producing a structurally wrong object.
 *
 * The `satisfies z.ZodType<OmegaEvent>` annotation on the exported schema
 * is a compile-time guarantee: if a variant is missing or a field type
 * drifts from the TypeScript interface, TypeScript will error here.
 */

import { z } from "zod";
import type { OmegaEvent } from "./events.js";

// ---------------------------------------------------------------------------
// Shared sub-schemas
// ---------------------------------------------------------------------------

const TurnMetricsSchema = z.object({
  inputTokens: z.number(),
  outputTokens: z.number(),
  cacheCreationTokens: z.number().optional(),
  cacheReadTokens: z.number().optional(),
});

const LlmResponseUsageSchema = z.object({
  input_tokens: z.number(),
  output_tokens: z.number(),
  cache_creation_input_tokens: z.number().nullable().optional(),
  cache_read_input_tokens: z.number().nullable().optional(),
  service_tier: z.string().nullable().optional(),
});

// ---------------------------------------------------------------------------
// Per-variant schemas (one per OmegaEvent member)
// ---------------------------------------------------------------------------

const SessionStartSchema = z.object({
  type: z.literal("session_start"),
  ts: z.string(),
  sessionId: z.string(),
  model: z.string(),
  authMode: z.string(),
  systemPrompt: z.string(),
});

const SessionEndSchema = z.object({
  type: z.literal("session_end"),
  ts: z.string(),
  outcome: z.enum(["clean", "error"]),
  reason: z.string().optional(),
});

const UserMessageSchema = z.object({
  type: z.literal("user_message"),
  ts: z.string(),
  content: z.string(),
});

const LlmCallSchema = z.object({
  type: z.literal("llm_call"),
  ts: z.string(),
  url: z.string(),
  model: z.string(),
  contextHashes: z.array(z.string()),
  cacheBreakpointIndex: z.number().nullable(),
  requestBytes: z.number(),
  requestSummary: z.record(z.string(), z.unknown()).optional(),
});

const LlmResponseSchema = z.object({
  type: z.literal("llm_response"),
  ts: z.string(),
  stopReason: z.string(),
  usage: LlmResponseUsageSchema,
  contextHash: z.string(),
  text: z.string().optional(),
  thinking: z.string().optional(),
  streamingStart: z.string().optional(),
  responseSummary: z.record(z.string(), z.unknown()).optional(),
});

const ToolCallSchema = z.object({
  type: z.literal("tool_call"),
  ts: z.string(),
  id: z.string(),
  name: z.string(),
  input: z.unknown(),
  contextHash: z.string(),
});

const ToolResultSchema = z.object({
  type: z.literal("tool_result"),
  ts: z.string(),
  id: z.string(),
  name: z.string(),
  isError: z.boolean(),
  durationMs: z.number(),
  output: z.string(),
  contextHash: z.string(),
});

const TurnEndSchema = z.object({
  type: z.literal("turn_end"),
  ts: z.string(),
  metrics: TurnMetricsSchema,
});

const LlmErrorSchema = z.object({
  type: z.literal("llm_error"),
  ts: z.string(),
  url: z.string(),
  error: z.string(),
  httpStatus: z.number().optional(),
});

const AgentErrorSchema = z.object({
  type: z.literal("agent_error"),
  ts: z.string(),
  error: z.string(),
});

const TurnInterruptedSchema = z.object({
  type: z.literal("turn_interrupted"),
  ts: z.string(),
  reason: z.enum(["aborted", "error"]).optional(),
});

const CompactedSchema = z.object({
  type: z.literal("compacted"),
  ts: z.string(),
  // usage is the raw Anthropic usage object — not further constrained
  usage: z.unknown(),
});

const LlmRetrySchema = z.object({
  type: z.literal("llm_retry"),
  ts: z.string(),
  attempt: z.number(),
  httpStatus: z.number().optional(),
  waitMs: z.number(),
  error: z.string(),
});

const ModelChangedSchema = z.object({
  type: z.literal("model_changed"),
  ts: z.string(),
  model: z.string(),
});

const TransportErrorSchema = z.object({
  type: z.literal("transport_error"),
  ts: z.string(),
  error: z.string(),
  context: z.string().optional(),
});

// ---------------------------------------------------------------------------
// Full discriminated union
// ---------------------------------------------------------------------------

/**
 * Parse and validate a value as an OmegaEvent.
 *
 * Use `.safeParse()` at the persistence boundary (reading from disk) so that
 * malformed lines are reported rather than silently producing wrong types:
 *
 *   const result = OmegaEventSchema.safeParse(JSON.parse(line));
 *   if (result.success) { use(result.data); } // result.data is OmegaEvent
 *
 * In tests, `.parse()` is appropriate — a failure means a test bug and should
 * throw immediately.
 *
 * The `satisfies z.ZodType<OmegaEvent>` annotation is a compile-time guard:
 * TypeScript will error here if the schema's inferred output type diverges
 * from the OmegaEvent union (missing variant, wrong field type, etc.).
 */
export const OmegaEventSchema = z.discriminatedUnion("type", [
  SessionStartSchema,
  SessionEndSchema,
  UserMessageSchema,
  LlmCallSchema,
  LlmResponseSchema,
  ToolCallSchema,
  ToolResultSchema,
  TurnEndSchema,
  LlmErrorSchema,
  AgentErrorSchema,
  TurnInterruptedSchema,
  CompactedSchema,
  LlmRetrySchema,
  ModelChangedSchema,
  TransportErrorSchema,
]) satisfies z.ZodType<OmegaEvent>;
