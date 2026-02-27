import { describe, it, expect } from "bun:test";
import { assertNotProductionPath } from "./test-guard.js";

// Note: OMEGA_TEST=1 is always set by the bun test preload (src/test-setup.ts).
// These tests verify the guard's behaviour in that environment.

describe("assertNotProductionPath", () => {
  it("throws for sessions/ prefix", () => {
    expect(() => assertNotProductionPath("sessions/events.jsonl", "fn")).toThrow(
      "[OMEGA_TEST]"
    );
  });

  it("throws for sessions/context.jsonl", () => {
    expect(() => assertNotProductionPath("sessions/context.jsonl", "fn")).toThrow(
      "[OMEGA_TEST]"
    );
  });

  it("throws for diagnosis/ prefix", () => {
    expect(() => assertNotProductionPath("diagnosis/", "fn")).toThrow("[OMEGA_TEST]");
  });

  it("throws for diagnosis/some-file.json", () => {
    expect(() => assertNotProductionPath("diagnosis/2025-01-01.json", "fn")).toThrow(
      "[OMEGA_TEST]"
    );
  });

  it("includes the function name in the error message", () => {
    expect(() => assertNotProductionPath("sessions/events.jsonl", "appendSessionEvent")).toThrow(
      "appendSessionEvent"
    );
  });

  it("includes the path in the error message", () => {
    expect(() => assertNotProductionPath("sessions/events.jsonl", "fn")).toThrow(
      "sessions/events.jsonl"
    );
  });

  it("does NOT throw for a temp-dir path", () => {
    expect(() =>
      assertNotProductionPath("/tmp/omega-test-123/events.jsonl", "fn")
    ).not.toThrow();
  });

  it("does NOT throw for a null-like scenario (guard is only called with non-null)", () => {
    // The guard is only ever called after the null check, so this is just
    // documenting that arbitrary non-production paths are fine.
    expect(() => assertNotProductionPath("some/other/path.json", "fn")).not.toThrow();
  });

  it("does NOT throw when OMEGA_TEST is unset", () => {
    const prev = process.env.OMEGA_TEST;
    delete process.env.OMEGA_TEST;
    try {
      expect(() => assertNotProductionPath("sessions/events.jsonl", "fn")).not.toThrow();
    } finally {
      process.env.OMEGA_TEST = prev;
    }
  });
});
