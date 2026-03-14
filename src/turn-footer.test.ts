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
    ttftMs: 850,
  };
  const session = {
    inputTokens: 9999,
    outputTokens: 1111,
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

  it("turn line contains per-turn new/out tokens", () => {
    const { turnLine } = formatTurnFooter(metrics, session, provider, model);
    const plain = stripAnsi(turnLine);
    expect(plain).toContain("new: 1234");
    expect(plain).toContain("out: 567");
  });

  it("turn line does NOT use old 'in:' label", () => {
    const { turnLine } = formatTurnFooter(metrics, session, provider, model);
    expect(stripAnsi(turnLine)).not.toMatch(/\bin:/);
  });

  it("session line contains cumulative new/out tokens", () => {
    const { sessionLine } = formatTurnFooter(metrics, session, provider, model);
    const plain = stripAnsi(sessionLine);
    expect(plain).toContain("new: 9999");
    expect(plain).toContain("out: 1111");
  });

  it("turn line does NOT contain cost", () => {
    const { turnLine } = formatTurnFooter(metrics, session, provider, model);
    expect(stripAnsi(turnLine)).not.toContain("cost:");
    expect(stripAnsi(turnLine)).not.toContain("$");
  });

  it("session line does NOT contain cost", () => {
    const { sessionLine } = formatTurnFooter(metrics, session, provider, model);
    expect(stripAnsi(sessionLine)).not.toContain("cost:");
    expect(stripAnsi(sessionLine)).not.toContain("$");
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

  it("'new:' appears at the same column offset in both lines", () => {
    const { turnLine, sessionLine } = formatTurnFooter(metrics, session, provider, model);
    const tPlain = stripAnsi(turnLine);
    const sPlain = stripAnsi(sessionLine);
    expect(tPlain.indexOf("new:")).toBe(sPlain.indexOf("new:"));
  });
});

describe("formatTurnFooter — cache token display", () => {
  const provider = "anthropic" as const;
  const model = "claude-sonnet-4-6";

  const metricsWithCache = {
    inputTokens: 100,
    outputTokens: 50,
    ttftMs: 400,
    cacheCreationTokens: 800,
    cacheReadTokens: 0,
  };
  const metricsWithRead = {
    inputTokens: 100,
    outputTokens: 50,
    ttftMs: 300,
    cacheCreationTokens: 0,
    cacheReadTokens: 500,
  };
  const sessionWithCache = {
    inputTokens: 500,
    outputTokens: 200,
    cacheCreationTokens: 800,
    cacheReadTokens: 500,
  };
  const sessionNoCache = {
    inputTokens: 500,
    outputTokens: 200,
  };

  it("turn line shows write: token count when cacheCreationTokens > 0", () => {
    const { turnLine } = formatTurnFooter(metricsWithCache, sessionNoCache, provider, model);
    expect(stripAnsi(turnLine)).toContain("write: 800");
  });

  it("turn line shows read: token count when cacheReadTokens > 0", () => {
    const { turnLine } = formatTurnFooter(metricsWithRead, sessionNoCache, provider, model);
    expect(stripAnsi(turnLine)).toContain("read: 500");
  });

  it("turn line shows write: 0 and read: 0 even when both are zero (always-on labels)", () => {
    const metricsZero = { inputTokens: 100, outputTokens: 50, ttftMs: 300, cacheCreationTokens: 0, cacheReadTokens: 0 };
    const { turnLine } = formatTurnFooter(metricsZero, sessionNoCache, provider, model);
    const plain = stripAnsi(turnLine);
    expect(plain).toContain("write: 0");
    expect(plain).toContain("read: 0");
  });

  it("turn line shows write: 0 and read: 0 when cache fields absent (defaults to 0)", () => {
    const metricsNone = { inputTokens: 100, outputTokens: 50, ttftMs: 300 };
    const { turnLine } = formatTurnFooter(metricsNone, sessionNoCache, provider, model);
    const plain = stripAnsi(turnLine);
    expect(plain).toContain("write: 0");
    expect(plain).toContain("read: 0");
  });

  it("session line shows cumulative read: token count when present", () => {
    const { sessionLine } = formatTurnFooter(metricsWithRead, sessionWithCache, provider, model);
    expect(stripAnsi(sessionLine)).toContain("read: 500");
  });

  it("session line shows write: 0 and read: 0 when session has no cache tokens", () => {
    const { sessionLine } = formatTurnFooter(metricsWithRead, sessionNoCache, provider, model);
    const plain = stripAnsi(sessionLine);
    expect(plain).toContain("write: 0");
    expect(plain).toContain("read: 0");
  });

  it("old 'cache_write:' and 'cache_read:' labels are gone for Anthropic", () => {
    const { turnLine, sessionLine } = formatTurnFooter(metricsWithCache, sessionWithCache, provider, model);
    expect(stripAnsi(turnLine)).not.toContain("cache_write");
    expect(stripAnsi(turnLine)).not.toContain("cache_read");
    expect(stripAnsi(sessionLine)).not.toContain("cache_write");
    expect(stripAnsi(sessionLine)).not.toContain("cache_read");
  });
});

describe("formatTurnFooter — OpenAI provider", () => {
  const provider = "openai" as const;
  const model = "gpt-5.2-codex";

  const metrics = {
    inputTokens: 2000,
    outputTokens: 300,
    ttftMs: 600,
    cacheCreationTokens: 100,   // should be ignored for OpenAI
    cacheReadTokens: 400,       // should be ignored for OpenAI
  };
  const session = {
    inputTokens: 8000,
    outputTokens: 900,
    cacheCreationTokens: 200,   // should be ignored for OpenAI
    cacheReadTokens: 1000,      // should be ignored for OpenAI
  };

  it("turn line does NOT show cost for OpenAI", () => {
    const { turnLine } = formatTurnFooter(metrics, session, provider, model);
    expect(stripAnsi(turnLine)).not.toContain("cost:");
    expect(stripAnsi(turnLine)).not.toContain("$");
  });

  it("session line does NOT show cost for OpenAI", () => {
    const { sessionLine } = formatTurnFooter(metrics, session, provider, model);
    expect(stripAnsi(sessionLine)).not.toContain("cost:");
    expect(stripAnsi(sessionLine)).not.toContain("$");
  });

  it("turn line does NOT show 'saved:' for OpenAI", () => {
    const { turnLine } = formatTurnFooter(metrics, session, provider, model);
    expect(stripAnsi(turnLine)).not.toContain("saved:");
  });

  it("session line does NOT show 'saved:' for OpenAI", () => {
    const { sessionLine } = formatTurnFooter(metrics, session, provider, model);
    expect(stripAnsi(sessionLine)).not.toContain("saved:");
  });

  it("turn line does NOT show write: or read: fields for OpenAI", () => {
    const { turnLine } = formatTurnFooter(metrics, session, provider, model);
    const plain = stripAnsi(turnLine);
    expect(plain).not.toContain("write:");
    expect(plain).not.toContain("read:");
  });

  it("session line does NOT show write: or read: fields for OpenAI", () => {
    const { sessionLine } = formatTurnFooter(metrics, session, provider, model);
    const plain = stripAnsi(sessionLine);
    expect(plain).not.toContain("write:");
    expect(plain).not.toContain("read:");
  });

  it("turn line still shows new:, out:, ttft:, and provider/model for OpenAI", () => {
    const { turnLine } = formatTurnFooter(metrics, session, provider, model);
    const plain = stripAnsi(turnLine);
    expect(plain).toContain("new: 2000");
    expect(plain).toContain("out: 300");
    expect(plain).toContain("ttft:");
    expect(plain).toContain(model);
  });
});
