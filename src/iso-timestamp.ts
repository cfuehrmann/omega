/**
 * ISOTimestamp — a branded string type for ISO 8601 datetime values.
 *
 * Prevents arbitrary strings from being assigned to `time` fields on
 * OmegaEvent and ContextRecord without an explicit, intentional cast.
 *
 * Production code: use now() to capture the current moment.
 * Test fixtures:   use iso("2024-01-15T...") to assert a known string.
 */

import { z } from "zod";

/**
 * Zod schema for ISO 8601 datetime strings, with a TypeScript brand applied.
 * Use this wherever an event schema needs a `time` field.
 */
export const ISOTimestampSchema = z.string().datetime().brand<"ISOTimestamp">();

/**
 * TypeScript type for ISO 8601 datetime strings.
 * A branded string: assignable to `string`, but a plain `string` is not
 * assignable to `ISOTimestamp` without an explicit cast.
 */
export type ISOTimestamp = z.infer<typeof ISOTimestampSchema>;

/**
 * Returns the current wall-clock time as an ISOTimestamp.
 * Use this at every production callsite that creates an event or context record.
 */
export function now(): ISOTimestamp {
  return new Date().toISOString() as ISOTimestamp;
}

/**
 * Converts an arbitrary Date to an ISOTimestamp.
 * Use this when the timestamp is not "now" — e.g. a computed future instant.
 */
export function fromDate(d: Date): ISOTimestamp {
  return d.toISOString() as ISOTimestamp;
}

/**
 * Type-asserts a known-valid ISO string literal as ISOTimestamp.
 *
 * Intended for test fixtures and static constants where the string is
 * visually verified to be a valid ISO 8601 datetime. Do NOT use in
 * production logic — use now() there instead.
 */
export function iso(s: string): ISOTimestamp {
  return s as ISOTimestamp;
}
