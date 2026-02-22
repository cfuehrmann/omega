import { describe, it, expect } from "bun:test";
import { shouldFallbackToCodex } from "./fallback.js";

describe("shouldFallbackToCodex", () => {
  it("returns true on Anthropic rate limit (429)", () => {
    const err = { status: 429, message: "rate limit" };
    expect(shouldFallbackToCodex(err, true)).toBe(true);
  });

  it("returns false when fallback disabled", () => {
    const err = { status: 429, message: "rate limit" };
    expect(shouldFallbackToCodex(err, false)).toBe(false);
  });

  it("returns false for non-rate-limit errors", () => {
    const err = { status: 500, message: "server error" };
    expect(shouldFallbackToCodex(err, true)).toBe(false);
  });
});
