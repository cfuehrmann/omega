/**
 * Pure UI logic — no React, no Ink, fully unit-testable.
 */

/**
 * Format the token count delta between consecutive API calls for display
 * in the status bar.
 *
 * @param current  Estimated token count for the most recent API call.
 * @param previous Estimated token count for the previous API call, or null
 *                 if this is the first call in the session.
 * @returns A compact string like "Δ+342 tok", "Δ-1204 tok", or "" if no
 *          previous call to compare against.
 */
export function formatTokenDelta(current: number, previous: number | null): string {
  if (previous === null) return "";
  const delta = current - previous;
  const sign = delta >= 0 ? "+" : "";
  return `Δ${sign}${delta} tok`;
}
