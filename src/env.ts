/**
 * Environment variable validation helpers.
 *
 * Each helper reads a single env var from `process.env`, validates it with
 * Zod, and returns a typed value.  They read on demand (not at module load
 * time) so that tests can safely set `process.env.X = "..."` in `beforeAll`
 * before the Agent or server is constructed.
 *
 * On invalid input the helpers throw with a human-readable message that
 * includes the variable name and the offending value — much clearer than
 * `NaN` silently propagating from `Number("oops")`.
 */

import { z } from "zod";

const PositiveInt = z.coerce.number().int().positive();

/**
 * Read a positive-integer environment variable.
 * Returns `defaultVal` when the variable is absent or empty.
 * Throws a descriptive error when present but not a positive integer.
 */
export function readEnvPositiveInt(name: string, defaultVal: number): number {
  const raw = process.env[name];
  if (raw === undefined || raw === "") return defaultVal;
  const result = PositiveInt.safeParse(raw);
  if (!result.success) {
    throw new Error(
      `Invalid env var ${name}: expected a positive integer, got "${raw}"`,
    );
  }
  return result.data;
}

/**
 * Read a positive-integer environment variable.
 * Returns `undefined` when the variable is absent or empty (caller treats
 * undefined as "no limit" / "use default logic").
 * Throws a descriptive error when present but not a positive integer.
 */
export function readEnvOptionalPositiveInt(name: string): number | undefined {
  const raw = process.env[name];
  if (raw === undefined || raw === "") return undefined;
  const result = PositiveInt.safeParse(raw);
  if (!result.success) {
    throw new Error(
      `Invalid env var ${name}: expected a positive integer, got "${raw}"`,
    );
  }
  return result.data;
}

