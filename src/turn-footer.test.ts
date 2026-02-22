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

describe("formatTurnFooter — cache savings", () => {
  const provider = "anthropic" as const;
  const model = "claude-sonnet-4-6";

  const metricsWithCache = {
    inputTokens: 100,
    outputTokens: 50,
    costUsd: 0.0015,
    ttftMs: 400,
    cacheCreationTokens: 800,
    cacheReadTokens: 0,
  };
  const metricsWithRead = {
    inputTokens: 100,
    outputTokens: 50,
    costUsd: 0.0005,
    ttftMs: 300,
    cacheCreationTokens: 0,
    cacheReadTokens: 500,
  };
  const sessionWithCache = {
    inputTokens: 500,
    outputTokens: 200,
    costUsd: 0.005,
    cacheCreationTokens: 800,
    cacheReadTokens: 500,
  };
  const sessionNoCache = {
    inputTokens: 500,
    outputTokens: 200,
    costUsd: 0.005,
  };

  it("turn line shows cache write tokens when cacheCreationTokens > 0", () => {
    const { turnLine } = formatTurnFooter(metricsWithCache, sessionNoCache, provider, model);
    const plain = stripAnsi(turnLine);
    expect(plain).toContain("cache_write: 800");
  });

  it("turn line shows cache read tokens when cacheReadTokens > 0", () => {
    const { turnLine } = formatTurnFooter(metricsWithRead, sessionNoCache, provider, model);
    const plain = stripAnsi(turnLine);
    expect(plain).toContain("cache_read: 500");
  });

  it("turn line does NOT show cache fields when both are zero", () => {
    const metricsZero = {
      inputTokens: 100,
      outputTokens: 50,
      costUsd: 0.0005,
      ttftMs: 300,
      cacheCreationTokens: 0,
      cacheReadTokens: 0,
    };
    const { turnLine } = formatTurnFooter(metricsZero, sessionNoCache, provider, model);
    const plain = stripAnsi(turnLine);
    expect(plain).not.toContain("cache_write");
    expect(plain).not.toContain("cache_read");
  });

  it("turn line does NOT show cache fields when cache fields are absent", () => {
    const metricsNone = {
      inputTokens: 100,
      outputTokens: 50,
      costUsd: 0.0005,
      ttftMs: 300,
    };
    const { turnLine } = formatTurnFooter(metricsNone, sessionNoCache, provider, model);
    const plain = stripAnsi(turnLine);
    expect(plain).not.toContain("cache_write");
    expect(plain).not.toContain("cache_read");
  });

  it("session line shows cumulative cache read tokens when present", () => {
    const { sessionLine } = formatTurnFooter(metricsWithRead, sessionWithCache, provider, model);
    const plain = stripAnsi(sessionLine);
    expect(plain).toContain("cache_read: 500");
  });

  it("session line does NOT show cache fields when session has none", () => {
    const { sessionLine } = formatTurnFooter(metricsWithRead, sessionNoCache, provider, model);
    const plain = stripAnsi(sessionLine);
    expect(plain).not.toContain("cache_write");
    expect(plain).not.toContain("cache_read");
  });
});

describe("formatTurnFooter — cost savings display", () => {
  const provider = "anthropic" as const;
  const model = "claude-sonnet-4-6";

  const metricsWithSavings = {
    inputTokens: 100,
    outputTokens: 50,
    costUsd: 0.00105,    // actual cost paid (with cache read discount)
    savedUsd: 0.00135,   // what would have been paid without caching
    ttftMs: 300,
    cacheCreationTokens: 0,
    cacheReadTokens: 500,
  };
  const sessionWithSavings = {
    inputTokens: 500,
    outputTokens: 200,
    costUsd: 0.005,
    savedUsd: 0.0027,
    cacheCreationTokens: 0,
    cacheReadTokens: 500,
  };
  const metricsNoSavings = {
    inputTokens: 100,
    outputTokens: 50,
    costUsd: 0.00075,
    ttftMs: 300,
  };
  const sessionNoSavings = {
    inputTokens: 500,
    outputTokens: 200,
    costUsd: 0.005,
  };

  it("turn line shows 'saved:' field when savedUsd > 0", () => {
    const { turnLine } = formatTurnFooter(metricsWithSavings, sessionWithSavings, provider, model);
    const plain = stripAnsi(turnLine);
    expect(plain).toContain("saved:");
  });

  it("session line shows 'saved:' field when savedUsd > 0", () => {
    const { sessionLine } = formatTurnFooter(metricsWithSavings, sessionWithSavings, provider, model);
    const plain = stripAnsi(sessionLine);
    expect(plain).toContain("saved:");
  });

  it("turn line does NOT show 'saved:' when savedUsd is absent or zero", () => {
    const { turnLine } = formatTurnFooter(metricsNoSavings, sessionNoSavings, provider, model);
    const plain = stripAnsi(turnLine);
    expect(plain).not.toContain("saved:");
  });

  it("session line does NOT show 'saved:' when savedUsd is absent or zero", () => {
    const { sessionLine } = formatTurnFooter(metricsNoSavings, sessionNoSavings, provider, model);
    const plain = stripAnsi(sessionLine);
    expect(plain).not.toContain("saved:");
  });

  it("'cost:' field in both lines reflects actual cost paid (not inflated)", () => {
    const { turnLine, sessionLine } = formatTurnFooter(metricsWithSavings, sessionWithSavings, provider, model);
    const tPlain = stripAnsi(turnLine);
    const sPlain = stripAnsi(sessionLine);
    // costUsd=0.00105 → $0.0011 (4dp), not a larger number
    expect(tPlain).toContain("cost:");
    expect(sPlain).toContain("cost:");
    // The saved field should be distinct from cost
    expect(tPlain).toContain("saved:");
    expect(sPlain).toContain("saved:");
  });

  it("'cost:' column aligns between turn and session lines regardless of digit count", () => {
    // Use costs with different digit counts to verify padding
    const m1 = { inputTokens: 1, outputTokens: 1, costUsd: 0.0001, savedUsd: 0.002, ttftMs: 100 };
    const s1 = { inputTokens: 1, outputTokens: 1, costUsd: 0.12345, savedUsd: 0.0001 };
    const { turnLine, sessionLine } = formatTurnFooter(m1, s1, provider, model);
    const tPlain = stripAnsi(turnLine);
    const sPlain = stripAnsi(sessionLine);
    expect(tPlain.indexOf("cost:")).toBe(sPlain.indexOf("cost:"));
  });

  it("'saved:' column aligns between turn and session lines regardless of digit count", () => {
    const m1 = { inputTokens: 1, outputTokens: 1, costUsd: 0.0001, savedUsd: 0.002, ttftMs: 100 };
    const s1 = { inputTokens: 1, outputTokens: 1, costUsd: 0.12345, savedUsd: 0.0001 };
    const { turnLine, sessionLine } = formatTurnFooter(m1, s1, provider, model);
    const tPlain = stripAnsi(turnLine);
    const sPlain = stripAnsi(sessionLine);
    expect(tPlain.indexOf("saved:")).toBe(sPlain.indexOf("saved:"));
  });
});

describe("formatTurnFooter — OpenAI provider", () => {
  const provider = "openai" as const;
  const model = "gpt-5.2-codex";

  const metrics = {
    inputTokens: 2000,
    outputTokens: 300,
    costUsd: 0.0042,
    savedUsd: 0.001,            // should be ignored for OpenAI
    ttftMs: 600,
    cacheCreationTokens: 100,   // should be ignored for OpenAI
    cacheReadTokens: 400,       // should be ignored for OpenAI
  };
  const session = {
    inputTokens: 8000,
    outputTokens: 900,
    costUsd: 0.015,
    savedUsd: 0.003,            // should be ignored for OpenAI
    cacheCreationTokens: 200,   // should be ignored for OpenAI
    cacheReadTokens: 1000,      // should be ignored for OpenAI
  };

  it("turn line cost shows '<=' prefix", () => {
    const { turnLine } = formatTurnFooter(metrics, session, provider, model);
    const plain = stripAnsi(turnLine);
    expect(plain).toContain("cost: <=$");
  });

  it("session line cost shows '<=' prefix", () => {
    const { sessionLine } = formatTurnFooter(metrics, session, provider, model);
    const plain = stripAnsi(sessionLine);
    expect(plain).toContain("cost: <=$");
  });

  it("turn line does NOT show 'saved:' for OpenAI", () => {
    const { turnLine } = formatTurnFooter(metrics, session, provider, model);
    expect(stripAnsi(turnLine)).not.toContain("saved:");
  });

  it("session line does NOT show 'saved:' for OpenAI", () => {
    const { sessionLine } = formatTurnFooter(metrics, session, provider, model);
    expect(stripAnsi(sessionLine)).not.toContain("saved:");
  });

  it("turn line does NOT show cache fields for OpenAI", () => {
    const { turnLine } = formatTurnFooter(metrics, session, provider, model);
    const plain = stripAnsi(turnLine);
    expect(plain).not.toContain("cache_write");
    expect(plain).not.toContain("cache_read");
  });

  it("session line does NOT show cache fields for OpenAI", () => {
    const { sessionLine } = formatTurnFooter(metrics, session, provider, model);
    const plain = stripAnsi(sessionLine);
    expect(plain).not.toContain("cache_write");
    expect(plain).not.toContain("cache_read");
  });

  it("'cost:' column aligns between turn and session lines for OpenAI", () => {
    const { turnLine, sessionLine } = formatTurnFooter(metrics, session, provider, model);
    const tPlain = stripAnsi(turnLine);
    const sPlain = stripAnsi(sessionLine);
    expect(tPlain.indexOf("cost:")).toBe(sPlain.indexOf("cost:"));
  });

  it("turn line still shows in:, out:, ttft:, and provider/model", () => {
    const { turnLine } = formatTurnFooter(metrics, session, provider, model);
    const plain = stripAnsi(turnLine);
    expect(plain).toContain("in: 2000");
    expect(plain).toContain("out: 300");
    expect(plain).toContain("ttft:");
    expect(plain).toContain(model);
  });
});
