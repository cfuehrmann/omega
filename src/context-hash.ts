import { z } from "zod";

/**
 * Zod schema for a ContextHash — 12 lowercase hex characters (6 random bytes).
 * Used as the primary key of context.jsonl records and as a FK in events.jsonl.
 */
export const ContextHashSchema = z
  .string()
  .regex(/^[0-9a-f]{12}$/)
  .brand<"ContextHash">();

/** A branded ContextHash: 12 lowercase hex characters encoding 6 random bytes. */
export type ContextHash = z.infer<typeof ContextHashSchema>;

/**
 * Generate a fresh ContextHash from 6 cryptographically random bytes.
 * Synchronous — uses the Web Crypto synchronous API (crypto.getRandomValues).
 */
export function randomHash(): ContextHash {
  const bytes = new Uint8Array(6);
  crypto.getRandomValues(bytes);
  return Array.from(bytes)
    .map(b => b.toString(16).padStart(2, "0"))
    .join("") as ContextHash;
}

/**
 * Cast a plain string to ContextHash — for test fixtures only.
 * Not for production use.
 */
export function asHash(s: string): ContextHash {
  return s as ContextHash;
}
