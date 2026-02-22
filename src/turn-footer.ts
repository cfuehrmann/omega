/**
 * Pure formatting logic for the two-line turn footer printed after each API turn.
 *
 *   turn:    in: NNN  out: NNN  cost: $N.NNNN  ttft: NNNms  [provider/model]
 *   session: in: NNN  out: NNN  cost: $N.NNN
 *
 * "in:" is column-aligned between the two lines.
 */

const CSI = "\x1b[";

function sgr(...codes: number[]): string {
  return `${CSI}${codes.join(";")}m`;
}

const RESET = sgr(0);

function dim(s: string): string {
  return sgr(2) + s + RESET;
}

function formatCost(usd: number): string {
  if (usd < 0.01) return `$${usd.toFixed(4)}`;
  return `$${usd.toFixed(3)}`;
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
  ttftMs: number | null;
}

export interface SessionTotals {
  inputTokens: number;
  outputTokens: number;
  costUsd: number;
}

export interface TurnFooter {
  /** Dimmed line: "turn:    in: … out: … cost: … ttft: … [provider/model]" */
  turnLine: string;
  /** Dimmed line: "session: in: … out: … cost: …" */
  sessionLine: string;
}

/**
 * Returns two plain-text (but ANSI-dimmed) lines for the turn footer.
 * The "in:" field is column-aligned between both lines.
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

  const turnBody =
    `${inOut(turn.inputTokens, turn.outputTokens)}  cost: ${formatCost(turn.costUsd)}  ttft: ${formatMs(turn.ttftMs)}  [${provider}/${model}]`;
  const sessionBody =
    `${inOut(session.inputTokens, session.outputTokens)}  cost: ${formatCost(session.costUsd)}`;

  return {
    turnLine:    dim(`${TURN_LABEL} ${turnBody}`),
    sessionLine: dim(`${SESSION_LABEL} ${sessionBody}`),
  };
}
