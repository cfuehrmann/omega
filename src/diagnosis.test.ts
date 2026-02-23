/**
 * Tests for RollingEventBuffer and its integration with writeDiagnostic.
 */

import { describe, it, expect, beforeEach, afterEach } from "bun:test";
import { mkdtempSync, rmSync, readFileSync, readdirSync } from "fs";
import { join } from "path";
import { tmpdir } from "os";
import {
  RollingEventBuffer,
  writeDiagnosticWithBuffer,
  type BufferedEvent,
} from "./diagnosis.js";

// ---------------------------------------------------------------------------
// RollingEventBuffer unit tests
// ---------------------------------------------------------------------------

describe("RollingEventBuffer", () => {
  it("starts empty", () => {
    const buf = new RollingEventBuffer(10);
    expect(buf.snapshot()).toEqual([]);
  });

  it("stores events in order", () => {
    const buf = new RollingEventBuffer(10);
    buf.push({ type: "user_prompt", content: "hello" });
    buf.push({ type: "api_request", callNumber: 1, provider: "anthropic", model: "claude-sonnet-4-6" });
    const snap = buf.snapshot();
    expect(snap).toHaveLength(2);
    expect(snap[0].event.type).toBe("user_prompt");
    expect(snap[1].event.type).toBe("api_request");
  });

  it("each entry has a monotonic seqNo and timestamp", () => {
    const buf = new RollingEventBuffer(10);
    buf.push({ type: "user_prompt", content: "a" });
    buf.push({ type: "user_prompt", content: "b" });
    const [e0, e1] = buf.snapshot();
    expect(e1.seqNo).toBeGreaterThan(e0.seqNo);
    expect(typeof e0.ts).toBe("string"); // ISO string
  });

  it("evicts oldest events when capacity is exceeded", () => {
    const buf = new RollingEventBuffer(3);
    buf.push({ type: "user_prompt", content: "1" });
    buf.push({ type: "user_prompt", content: "2" });
    buf.push({ type: "user_prompt", content: "3" });
    buf.push({ type: "user_prompt", content: "4" });
    const snap = buf.snapshot();
    expect(snap).toHaveLength(3);
    expect((snap[0].event as any).content).toBe("2");
    expect((snap[2].event as any).content).toBe("4");
  });

  it("snapshot returns a copy — mutations don't affect the buffer", () => {
    const buf = new RollingEventBuffer(5);
    buf.push({ type: "user_prompt", content: "x" });
    const snap = buf.snapshot();
    snap.pop();
    expect(buf.snapshot()).toHaveLength(1);
  });

  it("clear empties the buffer", () => {
    const buf = new RollingEventBuffer(5);
    buf.push({ type: "user_prompt", content: "x" });
    buf.clear();
    expect(buf.snapshot()).toHaveLength(0);
  });
});

// ---------------------------------------------------------------------------
// BufferedEvent types
// ---------------------------------------------------------------------------

describe("BufferedEvent types", () => {
  it("accepts all documented event kinds", () => {
    const buf = new RollingEventBuffer(20);

    const events: BufferedEvent[] = [
      { type: "user_prompt", content: "do something" },
      { type: "api_request", callNumber: 1, provider: "anthropic", model: "claude-sonnet-4-6", url: "https://x", requestSummary: { messages: 3 } },
      { type: "api_response", provider: "anthropic", stopReason: "tool_use", usage: { input_tokens: 100, output_tokens: 50 } },
      { type: "tool_call", id: "tool_abc", name: "read_file", input: { path: "foo.ts" } },
      { type: "tool_result", id: "tool_abc", name: "read_file", isError: false, outputPreview: "content..." },
      { type: "agent_error", message: "something went wrong", retryable: false },
      { type: "session_compacted", turnCount: 3, summaryLength: 500 },
    ];

    for (const e of events) buf.push(e);
    expect(buf.snapshot()).toHaveLength(events.length);
  });
});

// ---------------------------------------------------------------------------
// writeDiagnosticWithBuffer
// ---------------------------------------------------------------------------

describe("writeDiagnosticWithBuffer", () => {
  let tmpDir: string;

  beforeEach(() => {
    tmpDir = mkdtempSync(join(tmpdir(), "omega-diag-test-"));
  });

  afterEach(() => {
    rmSync(tmpDir, { recursive: true, force: true });
  });

  it("writes a file containing the event buffer", async () => {
    const buf = new RollingEventBuffer(10);
    buf.push({ type: "user_prompt", content: "fix the bug" });
    buf.push({ type: "api_request", callNumber: 1, provider: "anthropic", model: "claude-sonnet-4-6" });
    buf.push({ type: "api_response", provider: "anthropic", stopReason: "end_turn", usage: { input_tokens: 50, output_tokens: 20 } });

    const path = await writeDiagnosticWithBuffer(
      {
        summary: "test error",
        errorMessage: "boom",
        httpStatus: 500,
        provider: "anthropic",
        model: "claude-sonnet-4-6",
        requestMessages: [],
        history: [],
      },
      buf,
      tmpDir,
    );

    expect(path).not.toBeNull();
    const contents = JSON.parse(readFileSync(path!, "utf-8"));
    expect(contents.eventBuffer).toBeDefined();
    expect(Array.isArray(contents.eventBuffer)).toBe(true);
    expect(contents.eventBuffer).toHaveLength(3);
    expect(contents.eventBuffer[0].event.type).toBe("user_prompt");
    expect(contents.eventBuffer[0].event.content).toBe("fix the bug");
  });

  it("includes standard diagnostic fields alongside the buffer", async () => {
    const buf = new RollingEventBuffer(5);

    const path = await writeDiagnosticWithBuffer(
      {
        summary: "API 400",
        errorMessage: "bad request",
        httpStatus: 400,
        provider: "anthropic",
        model: "claude-opus-4-6",
        requestMessages: [{ role: "user", content: "hello" }],
        history: [{ role: "user", content: "hello" }],
        extra: { foo: "bar" },
      },
      buf,
      tmpDir,
    );

    const contents = JSON.parse(readFileSync(path!, "utf-8"));
    expect(contents.summary).toBe("API 400");
    expect(contents.httpStatus).toBe(400);
    expect(contents.model).toBe("claude-opus-4-6");
    expect(contents.requestMessages).toEqual([{ role: "user", content: "hello" }]);
    expect(contents.extra).toEqual({ foo: "bar" });
    expect(contents.eventBuffer).toEqual([]); // empty buffer
  });

  it("silently swallows write errors and returns null", async () => {
    const buf = new RollingEventBuffer(5);
    const result = await writeDiagnosticWithBuffer(
      {
        summary: "err",
        errorMessage: "x",
        provider: "anthropic",
        model: "m",
        requestMessages: [],
        history: [],
      },
      buf,
      "/this/path/does/not/exist/at/all/ever",
    );
    expect(result).toBeNull();
  });
});
