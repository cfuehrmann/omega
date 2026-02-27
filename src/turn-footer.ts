/**
 * Pure formatting logic for the two-line turn footer printed after each API turn.
 *
 *   turn:    new: NNN  write: NNN  read: NNN  out: NNN  cost: $N.NNNN  saved: $N.NNNN  ttft: NNNms  [provider/model]
 *   session: new: NNN  write: NNN  read: NNN  out: NNN  cost: $N.NNN   saved: $N.NNN
 *
 * For Anthropic, the three input-token buckets are always shown:
 *   new:   — full-price (non-cached) input tokens
 *   write: — cache-creation tokens (written to cache, billed at 1.25×)
 *   read:  — cache-read tokens (served from cache, billed at 0.1×)
 * For OpenAI: only new: and out: (no cache breakdown).
 *
 * "new:" and "cost:" are column-aligned between the two lines.
 * "saved:" always appears for Anthropic (even when $0), never for OpenAI.
 *
 * Provider differences:
 *   Anthropic — shows cost (actual), saved, new/write/read buckets.
 *   OpenAI    — shows cost ceiling only (prefix "<="), no saved, no write/read.
 */

const CSI = "\x1b[";

function sgr(...codes: number[]): string {
  return `${CSI}${codes.join(";")}m`;
}

const RESET = sgr(0);

function dim(s: string): string {
  return sgr(2) + s + RESET;
}

/**
 * Format a cost in USD. Always uses 4 dp for values < $0.01, else 3 dp.
 * An optional prefix (e.g. "<=") is prepended before the dollar sign.
 * Result is padEnd'd to `width` chars when width > 0.
 */
function formatCost(usd: number, width = 0, prefix = ""): string {
  const usdStr = usd < 0.01 ? usd.toFixed(4) : usd.toFixed(3);
  const formatted = prefix + "$" + usdStr;
  return width > 0 ? formatted.padEnd(width) : formatted;
}

function formatMs(ms: number | null): string {
  if (ms === null) return "-";
  if (ms < 1000) return `${Math.round(ms)}ms`;
  return `${(ms / 1000).toFixed(1)}s`;
}

interface TurnMetrics {
  inputTokens: number;
  outputTokens: number;
  costUsd: number;
  savedUsd?: number;
  ttftMs: number | null;
  cacheCreationTokens?: number;
  cacheReadTokens?: number;
}

interface SessionTotals {
  inputTokens: number;
  outputTokens: number;
  costUsd: number;
  savedUsd?: number;
  cacheCreationTokens?: number;
  cacheReadTokens?: number;
}

interface TurnFooter {
  /** Dimmed line: "turn:    new: … write: … read: … out: … cost: … [saved: …]  ttft: … [provider/model]" */
  turnLine: string;
  /** Dimmed line: "session: new: … write: … read: … out: … cost: … [saved: …]" */
  sessionLine: string;
}

/**
 * Returns two plain-text (but ANSI-dimmed) lines for the turn footer.
 * The "new:" and "cost:" fields are column-aligned between both lines.
 * "saved:" is always shown for Anthropic (even $0.0000), never for OpenAI.
 * Provider-specific: OpenAI shows cost ceiling only; Anthropic shows full detail.
 */
export function formatTurnFooter(
  turn: TurnMetrics,
  session: SessionTotals,
  provider: "anthropic" | "openai",
  model: string,
): TurnFooter {
  // Labels — pad so "in:" lands at the same column
  const TURN_LABEL    = "turn:   ";  // 8 chars
  const SESSION_LABEL = "session:";  // 8 chars

  const isOpenAi = provider === "openai";

  // For Anthropic: always show all three input buckets inline.
  // For OpenAI: just show the single "new:" (full-price) bucket.
  const inputFields = (inp: number, write: number, read: number, out: number) =>
    isOpenAi
      ? `new: ${inp}  out: ${out}`
      : `new: ${inp}  write: ${write}  read: ${read}  out: ${out}`;

  // OpenAI: ceiling cost only, no saved, no cache fields.
  // Anthropic: full detail — actual cost, saved, cache_write, cache_read.
  const costPrefix = isOpenAi ? "<=" : "";

  // Saved amounts (Anthropic always shows; OpenAI never shows)
  const turnSaved = isOpenAi ? 0 : (turn.savedUsd ?? 0);
  const sessionSaved = isOpenAi ? 0 : (session.savedUsd ?? 0);

  // Compute cost string widths for alignment.
  // Both cost fields use the same width = max of both formatted lengths.
  const turnCostStr    = formatCost(turn.costUsd, 0, costPrefix);
  const sessionCostStr = formatCost(session.costUsd, 0, costPrefix);
  const costWidth = Math.max(turnCostStr.length, sessionCostStr.length);

  // Same for saved (Anthropic always, OpenAI never)
  const savedWidth = isOpenAi ? 0
    : Math.max(formatCost(turnSaved).length, formatCost(sessionSaved).length);

  const costPart  = (usd: number) => `cost: ${formatCost(usd, costWidth, costPrefix)}`;
  const savedPart = (usd: number) => isOpenAi ? "" : `  saved: ${formatCost(usd, savedWidth)}`;

  const turnBody =
    `${inputFields(turn.inputTokens, turn.cacheCreationTokens ?? 0, turn.cacheReadTokens ?? 0, turn.outputTokens)}  ${costPart(turn.costUsd)}${savedPart(turnSaved)}  ttft: ${formatMs(turn.ttftMs)}  [${provider}/${model}]`;
  const sessionBody =
    `${inputFields(session.inputTokens, session.cacheCreationTokens ?? 0, session.cacheReadTokens ?? 0, session.outputTokens)}  ${costPart(session.costUsd)}${savedPart(sessionSaved)}`;

  return {
    turnLine:    dim(`${TURN_LABEL} ${turnBody}`),
    sessionLine: dim(`${SESSION_LABEL} ${sessionBody}`),
  };
}
