/**
 * Tests for the append-only context store (Step 3a).
 */

import { describe, it, expect, beforeEach, afterEach } from "bun:test";
import { appendContextMessage, clearContextStore } from "./context-store.js";
import { mkdtemp, rm, readFile } from "fs/promises";
import { tmpdir } from "os";
import { join } from "path";
import type Anthropic from "@anthropic-ai/sdk";

let tempDir: string;
let contextFile: string;

beforeEach(async () => {
  tempDir = await mkdtemp(join(tmpdir(), "omega-ctx-test-"));
  contextFile = join(tempDir, "context.jsonl");
});

afterEach(async () => {
  await rm(tempDir, { recursive: true, force: true });
});

describe("appendContextMessage", () => {
  it("creates the file and writes one message as JSONL", async () => {
    const msg: Anthropic.MessageParam = { role: "user", content: "hello" };
    await appendContextMessage(msg, contextFile);

    const raw = await readFile(contextFile, "utf-8");
    const lines = raw.trim().split("\n");
    expect(lines).toHaveLength(1);
    expect(JSON.parse(lines[0])).toEqual(msg);
  });

  it("appends a second message on a new line", async () => {
    const msg1: Anthropic.MessageParam = { role: "user", content: "hello" };
    const msg2: Anthropic.MessageParam = {
      role: "assistant",
      content: [{ type: "text", text: "world" }],
    };

    await appendContextMessage(msg1, contextFile);
    await appendContextMessage(msg2, contextFile);

    const raw = await readFile(contextFile, "utf-8");
    const lines = raw.trim().split("\n");
    expect(lines).toHaveLength(2);
    expect(JSON.parse(lines[0])).toEqual(msg1);
    expect(JSON.parse(lines[1])).toEqual(msg2);
  });

  it("round-trips complex content (tool use blocks)", async () => {
    const msg: Anthropic.MessageParam = {
      role: "assistant",
      content: [
        { type: "text", text: "Let me check that." },
        {
          type: "tool_use",
          id: "toolu_01",
          name: "read_file",
          input: { path: "src/agent.ts" },
        },
      ],
    };

    await appendContextMessage(msg, contextFile);

    const raw = await readFile(contextFile, "utf-8");
    const parsed = JSON.parse(raw.trim());
    expect(parsed).toEqual(msg);
  });
});

describe("appendContextMessage — null path (test-isolation mode)", () => {
  it("is a no-op and does not create any file", async () => {
    const msg: Anthropic.MessageParam = { role: "user", content: "hello" };
    // Should not throw and must not create any file
    await appendContextMessage(msg, null);

    const { existsSync } = await import("fs");
    expect(existsSync(contextFile)).toBe(false);
  });
});

describe("clearContextStore", () => {
  it("truncates the file to empty when it exists", async () => {
    const msg: Anthropic.MessageParam = { role: "user", content: "hello" };
    await appendContextMessage(msg, contextFile);

    await clearContextStore(contextFile);

    const raw = await readFile(contextFile, "utf-8");
    expect(raw).toBe("");
  });

  it("is a no-op when the file does not exist (no error)", async () => {
    // Should not throw
    await clearContextStore(contextFile);
  });

  it("is a no-op when filePath is null", async () => {
    // Should not throw
    await clearContextStore(null);
  });
});
