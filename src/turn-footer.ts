/**
 * Pure formatting logic for the two-line turn footer printed after each API turn.
 *
 *   turn:    new: NNN  write: NNN  read: NNN  out: NNN  ttft: NNNms  [provider/model]
 *   session: new: NNN  write: NNN  read: NNN  out: NNN
 *
 * For Anthropic, the three input-token buckets are always shown:
 *   new:   — full-price (non-cached) input tokens
 *   write: — cache-creation tokens (written to cache, billed at 1.25×)
 *   read:  — cache-read tokens (served from cache, billed at 0.1×)
 * For OpenAI: only new: and out: (no cache breakdown).
 *
 * "new:" is column-aligned between the two lines.
 */

const CSI = "\x1b[";

function sgr(...codes: number[]): string {
  return `${CSI}${codes.join(";")}m`;
}

const RESET = sgr(0);

function dim(s: string): string {
  return sgr(2) + s + RESET;
}

interface TurnMetrics {
  inputTokens: number;
  outputTokens: number;
  cacheCreationTokens?: number;
  cacheReadTokens?: number;
}

interface SessionTotals {
  inputTokens: number;
  outputTokens: number;
  cacheCreationTokens?: number;
  cacheReadTokens?: number;
}

interface TurnFooter {
  /** Dimmed line: "turn:    new: … write: … read: … out: …  ttft: … [provider/model]" */
  turnLine: string;
  /** Dimmed line: "session: new: … write: … read: … out: …" */
  sessionLine: string;
}

/**
 * Returns two plain-text (but ANSI-dimmed) lines for the turn footer.
 * The "new:" field is column-aligned between both lines.
 * Provider-specific: OpenAI shows only new:/out:; Anthropic shows full cache breakdown.
 */
export function formatTurnFooter(
  turn: TurnMetrics,
  session: SessionTotals,
  provider: "anthropic" | "openai",
  model: string,
): TurnFooter {
  const TURN_LABEL    = "turn:   ";  // 8 chars
  const SESSION_LABEL = "session:";  // 8 chars

  const isOpenAi = provider === "openai";

  const inputFields = (inp: number, write: number, read: number, out: number) =>
    isOpenAi
      ? `new: ${inp}  out: ${out}`
      : `new: ${inp}  write: ${write}  read: ${read}  out: ${out}`;

  const turnBody =
    `${inputFields(turn.inputTokens, turn.cacheCreationTokens ?? 0, turn.cacheReadTokens ?? 0, turn.outputTokens)}  [${provider}/${model}]`;
  const sessionBody =
    `${inputFields(session.inputTokens, session.cacheCreationTokens ?? 0, session.cacheReadTokens ?? 0, session.outputTokens)}`;

  return {
    turnLine:    dim(`${TURN_LABEL} ${turnBody}`),
    sessionLine: dim(`${SESSION_LABEL} ${sessionBody}`),
  };
}
