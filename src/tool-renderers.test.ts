/**
 * Tests for split tool-rendering: start immediately, result with new timestamp.
 */

import { describe, it, expect } from "bun:test";
import { renderToolStart, renderToolResult } from "./ui-raw.js";

describe("renderToolStart", () => {
  it("is a function", () => {
    expect(typeof renderToolStart).toBe("function");
  });

  it("returns an array of strings", () => {
    const lines = renderToolStart("read_file", { path: "src/foo.ts" });
    expect(Array.isArray(lines)).toBe(true);
    expect(lines.length).toBeGreaterThan(0);
  });

  it("includes the tool name", () => {
    const lines = renderToolStart("write_file", { path: "x.ts", content: "hi" });
    const joined = lines.join("\n");
    expect(joined).toContain("write_file");
  });

  it("includes the input JSON", () => {
    const input = { path: "foo.ts" };
    const lines = renderToolStart("read_file", input);
    const joined = lines.join("\n");
    expect(joined).toContain('"foo.ts"');
  });

  it("does NOT include 'result' or 'is_error'", () => {
    const lines = renderToolStart("run_command", { command: "ls" });
    const raw = lines.map(l => l.replace(/\x1b\[[0-9;]*m/g, "")).join("\n");
    expect(raw).not.toContain("result");
    expect(raw).not.toContain("is_error");
  });
});

describe("renderToolResult", () => {
  it("is a function", () => {
    expect(typeof renderToolResult).toBe("function");
  });

  it("returns an array of strings", () => {
    const lines = renderToolResult({ output: "ok", isError: false });
    expect(Array.isArray(lines)).toBe(true);
    expect(lines.length).toBeGreaterThan(0);
  });

  it("includes is_error false", () => {
    const lines = renderToolResult({ output: "done", isError: false });
    const raw = lines.map(l => l.replace(/\x1b\[[0-9;]*m/g, "")).join("\n");
    expect(raw).toContain("is_error: false");
  });

  it("includes is_error true for errors", () => {
    const lines = renderToolResult({ output: "boom", isError: true });
    const raw = lines.map(l => l.replace(/\x1b\[[0-9;]*m/g, "")).join("\n");
    expect(raw).toContain("is_error: true");
  });

  it("includes the output content", () => {
    const lines = renderToolResult({ output: "hello world", isError: false });
    const raw = lines.map(l => l.replace(/\x1b\[[0-9;]*m/g, "")).join("\n");
    expect(raw).toContain("hello world");
  });

  it("does NOT include tool name or input fields", () => {
    const lines = renderToolResult({ output: "done", isError: false });
    const raw = lines.map(l => l.replace(/\x1b\[[0-9;]*m/g, "")).join("\n");
    expect(raw).not.toContain("name:");
    expect(raw).not.toContain("input:");
  });
});
