/**
 * Tests for the two-line turn footer (turn: / session:) rendered after each API turn.
 */

import { describe, it, expect } from "bun:test";
import { formatTurnFooter } from "./turn-footer.js";

// Strip ANSI escape codes for plain-text assertions
function stripAnsi(s: string): string {
  // eslint-disable-next-line no-control-regex
  return s.replace(/\x1b\[[0-9;]*m/g, "");
}

describe("formatTurnFooter", () => {
  const metrics = {
    inputTokens: 1234,
    outputTokens: 567,
    costUsd: 0.00312,
    ttftMs: 850,
  };
  const session = {
    inputTokens: 9999,
    outputTokens: 1111,
    costUsd: 0.1234,
  };
  const provider = "anthropic" as const;
  const model = "claude-sonnet-4-6";

  it("turn line starts with 'turn:'", () => {
    const { turnLine } = formatTurnFooter(metrics, session, provider, model);
    expect(stripAnsi(turnLine)).toMatch(/^turn:/);
  });

  it("session line starts with 'session:'", () => {
    const { sessionLine } = formatTurnFooter(metrics, session, provider, model);
    expect(stripAnsi(sessionLine)).toMatch(/^session:/);
  });

  it("turn line contains per-turn in/out tokens", () => {
    const { turnLine } = formatTurnFooter(metrics, session, provider, model);
    const plain = stripAnsi(turnLine);
    expect(plain).toContain("in: 1234");
    expect(plain).toContain("out: 567");
  });

  it("session line contains cumulative in/out tokens", () => {
    const { sessionLine } = formatTurnFooter(metrics, session, provider, model);
    const plain = stripAnsi(sessionLine);
    expect(plain).toContain("in: 9999");
    expect(plain).toContain("out: 1111");
  });

  it("turn line contains cost", () => {
    const { turnLine } = formatTurnFooter(metrics, session, provider, model);
    expect(stripAnsi(turnLine)).toContain("$");
  });

  it("session line contains cost", () => {
    const { sessionLine } = formatTurnFooter(metrics, session, provider, model);
    expect(stripAnsi(sessionLine)).toContain("$");
  });

  it("turn line contains ttft", () => {
    const { turnLine } = formatTurnFooter(metrics, session, provider, model);
    expect(stripAnsi(turnLine)).toContain("ttft:");
  });

  it("session line does NOT contain ttft", () => {
    const { sessionLine } = formatTurnFooter(metrics, session, provider, model);
    expect(stripAnsi(sessionLine)).not.toContain("ttft:");
  });

  it("turn line contains model", () => {
    const { turnLine } = formatTurnFooter(metrics, session, provider, model);
    expect(stripAnsi(turnLine)).toContain(model);
  });

  it("session line does NOT contain model", () => {
    const { sessionLine } = formatTurnFooter(metrics, session, provider, model);
    expect(stripAnsi(sessionLine)).not.toContain(model);
  });

  it("'in:' appears at the same column offset in both lines", () => {
    const { turnLine, sessionLine } = formatTurnFooter(metrics, session, provider, model);
    const tPlain = stripAnsi(turnLine);
    const sPlain = stripAnsi(sessionLine);
    expect(tPlain.indexOf("in:")).toBe(sPlain.indexOf("in:"));
  });
});
