/**
 * Zod schema for ContextRecord.
 *
 * Replaces `JSON.parse(line) as ContextRecord` casts at the persistence
 * boundary (reading context.jsonl from disk) with runtime-validated parsing.
 *
 * The `content` field is Anthropic's `BetaMessageParam["content"]` — a union
 * of `string` and a complex SDK block array type. We validate the outer shape
 * (string or array) using `z.custom`, which also sets the inferred output type
 * to `ContextRecord["content"]`. This lets the schema `satisfies
 * z.ZodType<ContextRecord>` without weakening downstream types.
 */

import { z } from "zod";
import type { ContextRecord } from "./context-store.js";

export const ContextRecordSchema = z.object({
  hash: z.string(),
  time: z.string().datetime(),
  role: z.enum(["user", "assistant"]),
  /**
   * Validates that content is a string or array; the inner block types are
   * complex SDK types that are not worth re-specifying here. `z.custom` lets
   * us assert the output type as `ContextRecord["content"]` so the schema
   * satisfies `z.ZodType<ContextRecord>` end-to-end.
   */
  content: z.custom<ContextRecord["content"]>(
    (val) => typeof val === "string" || Array.isArray(val),
    "content must be a string or an array of content blocks",
  ),
}) satisfies z.ZodType<ContextRecord>;
