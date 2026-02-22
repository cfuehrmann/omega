/**
 * Pure formatting logic for the two-line turn footer printed after each API turn.
 *
 *   turn:    in: NNN  out: NNN  cost: $N.NNNN  saved: $N.NNNN  ttft: NNNms  [provider/model]
 *   session: in: NNN  out: NNN  cost: $N.NNN   saved: $N.NNN
 *
 * "in:" and "cost:" are column-aligned between the two lines.
 * "saved:" only appears when caching produced savings (savedUsd > 0).
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
 * Format a cost in USD to a fixed-width string so columns align.
 * We always use 4 decimal places. The result is left-padded to `width` chars.
 */
function formatCost(usd: number, width = 0): string {
  const formatted = usd < 0.01 ? `$${usd.toFixed(4)}` : `$${usd.toFixed(3)}`;
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
 * "saved:" is shown only when savedUsd > 0 (on either line), and also aligned.
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

  // Determine if either line has savings to show
  const turnSaved = turn.savedUsd ?? 0;
  const sessionSaved = session.savedUsd ?? 0;
  const showSaved = turnSaved > 0 || sessionSaved > 0;

  // Compute cost string widths for alignment.
  // Both cost fields use the same width = max of both formatted lengths.
  const turnCostStr   = formatCost(turn.costUsd);
  const sessionCostStr = formatCost(session.costUsd);
  const costWidth = Math.max(turnCostStr.length, sessionCostStr.length);

  // Same for saved (only when shown)
  let savedWidth = 0;
  if (showSaved) {
    const turnSavedStr   = formatCost(turnSaved);
    const sessionSavedStr = formatCost(sessionSaved);
    savedWidth = Math.max(turnSavedStr.length, sessionSavedStr.length);
  }

  const turnCache = cacheFields(turn.cacheCreationTokens, turn.cacheReadTokens);
  const sessionCache = cacheFields(session.cacheCreationTokens, session.cacheReadTokens);

  const costPart = (usd: number) => `cost: ${formatCost(usd, costWidth)}`;
  const savedPart = (usd: number) => showSaved ? `  saved: ${formatCost(usd, savedWidth)}` : "";

  const turnBody =
    `${inOut(turn.inputTokens, turn.outputTokens)}  ${costPart(turn.costUsd)}${savedPart(turnSaved)}${turnCache}  ttft: ${formatMs(turn.ttftMs)}  [${provider}/${model}]`;
  const sessionBody =
    `${inOut(session.inputTokens, session.outputTokens)}  ${costPart(session.costUsd)}${savedPart(sessionSaved)}${sessionCache}`;

  return {
    turnLine:    dim(`${TURN_LABEL} ${turnBody}`),
    sessionLine: dim(`${SESSION_LABEL} ${sessionBody}`),
  };
}
