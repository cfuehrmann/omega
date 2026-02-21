/**
 * Unit tests for pure UI logic extracted from ui.tsx.
 *
 * These test the shortcut-guard logic that prevents `i`/`q` from
 * firing while the user is typing in the prompt.
 */

import { describe, it, expect } from "bun:test";
import { shouldHandleShortcut } from "./ui-logic.js";

describe("shouldHandleShortcut", () => {
  // Base idle state: ready, not streaming, no pending tool, prompt empty
  const idle = { inputValue: "", isStreaming: false, hasPendingTool: false, isReady: true, resumeDone: true };

  it("allows i when idle and prompt is empty", () => {
    expect(shouldHandleShortcut("i", idle)).toBe(true);
  });

  it("allows q when idle and prompt is empty", () => {
    expect(shouldHandleShortcut("q", idle)).toBe(true);
  });

  it("blocks i when prompt has text", () => {
    expect(shouldHandleShortcut("i", { ...idle, inputValue: "inspect something" })).toBe(false);
  });

  it("blocks q when prompt has text", () => {
    expect(shouldHandleShortcut("q", { ...idle, inputValue: "quit? no" })).toBe(false);
  });

  it("blocks i when streaming", () => {
    expect(shouldHandleShortcut("i", { ...idle, isStreaming: true })).toBe(false);
  });

  it("blocks i when pending tool confirmation", () => {
    expect(shouldHandleShortcut("i", { ...idle, hasPendingTool: true })).toBe(false);
  });

  it("blocks i when not ready", () => {
    expect(shouldHandleShortcut("i", { ...idle, isReady: false })).toBe(false);
  });

  it("blocks i when resume prompt not done", () => {
    expect(shouldHandleShortcut("i", { ...idle, resumeDone: false })).toBe(false);
  });

  it("does not interfere with other keys", () => {
    expect(shouldHandleShortcut("x", idle)).toBe(false);
    expect(shouldHandleShortcut("a", idle)).toBe(false);
  });

  it("blocks even a single character in the prompt", () => {
    expect(shouldHandleShortcut("i", { ...idle, inputValue: "i" })).toBe(false);
    expect(shouldHandleShortcut("q", { ...idle, inputValue: "q" })).toBe(false);
  });
});
