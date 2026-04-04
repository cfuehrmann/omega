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
import { ISOTimestampSchema } from "./iso-timestamp.js";
import { ContextHashSchema } from "./context-hash.js";

// ---------------------------------------------------------------------------
// Shared sub-schemas
// ---------------------------------------------------------------------------

const TurnMetricsSchema = z.object({
  inputTokens: z.number().int(),
  outputTokens: z.number().int(),
  cacheCreationTokens: z.number().int().optional(),
  cacheReadTokens: z.number().int().optional(),
});

const LlmResponseUsageSchema = z.object({
  input_tokens: z.number().int(),
  output_tokens: z.number().int(),
  cache_creation_input_tokens: z.number().int().nullable().optional(),
  cache_read_input_tokens: z.number().int().nullable().optional(),
  service_tier: z.string().nullable().optional(),
});

// ---------------------------------------------------------------------------
// Per-variant schemas (one per OmegaEvent member)
// ---------------------------------------------------------------------------

const SessionStartSchema = z.object({
  type: z.literal("session_started"),
  time: ISOTimestampSchema,
  sessionId: z.string(),
  model: z.string(),
  authMode: z.string(),
  systemPrompt: z.string(),
});

const ServerStartedSchema = z.object({
  type: z.literal("server_started"),
  time: ISOTimestampSchema,
});

const ServerStoppedSchema = z.object({
  type: z.literal("server_stopped"),
  time: ISOTimestampSchema,
  outcome: z.enum(["clean", "error"]),
  reason: z.string().optional(),
});

const UserMessageSchema = z.object({
  type: z.literal("user_message"),
  time: ISOTimestampSchema,
  content: z.string(),
});

const LlmCallSchema = z.object({
  type: z.literal("llm_call"),
  time: ISOTimestampSchema,
  url: z.string(),
  model: z.string(),
  contextHashes: z.array(ContextHashSchema),
  cacheBreakpointIndex: z.number().int().nullable(),
  requestBytes: z.number().int(),
  requestSummary: z.record(z.string(), z.unknown()).optional(),
});

const LlmResponseSchema = z.object({
  type: z.literal("llm_response"),
  time: ISOTimestampSchema,
  stopReason: z.string(),
  usage: LlmResponseUsageSchema,
  contextHash: ContextHashSchema,
  text: z.string().optional(),
  thinking: z.string().optional(),
  streamingStart: ISOTimestampSchema.optional(),
  responseSummary: z.record(z.string(), z.unknown()).optional(),
});

const ToolCallSchema = z.object({
  type: z.literal("tool_call"),
  time: ISOTimestampSchema,
  id: z.string(),
  name: z.string(),
  input: z.unknown(),
  contextHash: ContextHashSchema,
});

const ToolResultSchema = z.object({
  type: z.literal("tool_result"),
  time: ISOTimestampSchema,
  id: z.string(),
  name: z.string(),
  isError: z.boolean(),
  durationMs: z.number().int(),
  output: z.string(),
});

const TurnEndSchema = z.object({
  type: z.literal("turn_end"),
  time: ISOTimestampSchema,
  metrics: TurnMetricsSchema,
});

const LlmErrorSchema = z.object({
  type: z.literal("llm_error"),
  time: ISOTimestampSchema,
  url: z.string(),
  error: z.string(),
  httpStatus: z.number().int().min(100).max(599).optional(),
});

const AgentErrorSchema = z.object({
  type: z.literal("agent_error"),
  time: ISOTimestampSchema,
  error: z.string(),
});

const TurnInterruptedSchema = z.object({
  type: z.literal("turn_interrupted"),
  time: ISOTimestampSchema,
  reason: z.enum(["aborted", "error"]).optional(),
});

const CompactedSchema = z.object({
  type: z.literal("compacted"),
  time: ISOTimestampSchema,
  // usage is the raw Anthropic usage object — not further constrained
  usage: z.unknown(),
});

const LlmRetrySchema = z.object({
  type: z.literal("llm_retry"),
  time: ISOTimestampSchema,
  attempt: z.number().int(),
  httpStatus: z.number().int().min(100).max(599).optional(),
  waitMs: z.number().int(),
  error: z.string(),
  retryAt: ISOTimestampSchema.optional(),
  errorBody: z.unknown().optional(),
  thinkingFragment: z.string().optional(),
  textFragment: z.string().optional(),
});

const ModelChangedSchema = z.object({
  type: z.literal("model_changed"),
  time: ISOTimestampSchema,
  model: z.string(),
});

const TransportErrorSchema = z.object({
  type: z.literal("transport_error"),
  time: ISOTimestampSchema,
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
  ServerStartedSchema,
  ServerStoppedSchema,
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
