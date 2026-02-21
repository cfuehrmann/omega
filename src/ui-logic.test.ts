/**
 * Unit tests for pure UI logic extracted from ui.tsx.
 */

import { describe, it, expect } from "bun:test";
import { formatTokenDelta } from "./ui-logic.js";

describe("formatTokenDelta", () => {
  it("returns empty string when no previous call exists", () => {
    expect(formatTokenDelta(1000, null)).toBe("");
  });

  it("shows positive delta when context grew", () => {
    expect(formatTokenDelta(1500, 1000)).toBe("Δ+500 tok");
  });

  it("shows negative delta when truncation fired", () => {
    expect(formatTokenDelta(800, 1200)).toBe("Δ-400 tok");
  });

  it("shows zero delta when unchanged", () => {
    expect(formatTokenDelta(1000, 1000)).toBe("Δ+0 tok");
  });

  it("formats large numbers without locale separators for compactness", () => {
    const result = formatTokenDelta(15000, 12000);
    expect(result).toBe("Δ+3000 tok");
  });
});
