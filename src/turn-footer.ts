/**
 * Pure formatting logic for the two-line turn footer printed after each API turn.
 *
 *   turn:    in: NNN  out: NNN  cost: $N.NNNN  saved: $N.NNNN  ttft: NNNms  [provider/model]
 *   session: in: NNN  out: NNN  cost: $N.NNN   saved: $N.NNN
 *
 * "in:" and "cost:" are column-aligned between the two lines.
 * "saved:" always appears for Anthropic (even when $0), never for OpenAI.
 *
 * Provider differences:
 *   Anthropic — shows cost (actual), saved, cache_write, cache_read.
 *   OpenAI    — shows cost ceiling only (prefix "<="), no saved, no cache fields.
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

export interface TurnMetrics {
  inputTokens: number;
  outputTokens: number;
  costUsd: number;
  savedUsd?: number;
  ttftMs: number | null;
  cacheCreationTokens?: number;
  cacheReadTokens?: number;
}

export interface SessionTotals {
  inputTokens: number;
  outputTokens: number;
  costUsd: number;
  savedUsd?: number;
  cacheCreationTokens?: number;
  cacheReadTokens?: number;
}

export interface TurnFooter {
  /** Dimmed line: "turn:    in: … out: … cost: … [saved: …]  ttft: … [provider/model]" */
  turnLine: string;
  /** Dimmed line: "session: in: … out: … cost: … [saved: …]" */
  sessionLine: string;
}

/**
 * Returns two plain-text (but ANSI-dimmed) lines for the turn footer.
 * The "in:" and "cost:" fields are column-aligned between both lines.
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

  const inOut = (inp: number, out: number) =>
    `in: ${inp}  out: ${out}`;

  const cacheFields = (creation?: number, read?: number): string => {
    const parts: string[] = [];
    if (creation && creation > 0) parts.push(`cache_write: ${creation}`);
    if (read && read > 0) parts.push(`cache_read: ${read}`);
    return parts.length > 0 ? "  " + parts.join("  ") : "";
  };

  const isOpenAi = provider === "openai";

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

  const turnCache    = isOpenAi ? "" : cacheFields(turn.cacheCreationTokens, turn.cacheReadTokens);
  const sessionCache = isOpenAi ? "" : cacheFields(session.cacheCreationTokens, session.cacheReadTokens);

  const costPart  = (usd: number) => `cost: ${formatCost(usd, costWidth, costPrefix)}`;
  const savedPart = (usd: number) => isOpenAi ? "" : `  saved: ${formatCost(usd, savedWidth)}`;

  const turnBody =
    `${inOut(turn.inputTokens, turn.outputTokens)}  ${costPart(turn.costUsd)}${savedPart(turnSaved)}${turnCache}  ttft: ${formatMs(turn.ttftMs)}  [${provider}/${model}]`;
  const sessionBody =
    `${inOut(session.inputTokens, session.outputTokens)}  ${costPart(session.costUsd)}${savedPart(sessionSaved)}${sessionCache}`;

  return {
    turnLine:    dim(`${TURN_LABEL} ${turnBody}`),
    sessionLine: dim(`${SESSION_LABEL} ${sessionBody}`),
  };
}
