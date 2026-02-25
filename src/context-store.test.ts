/**
 * Tests for the append-only context store (Step 3a).
 */

import { describe, it, expect, beforeEach, afterEach } from "bun:test";
import { appendContextMessage, clearContextStore, rotateFile } from "./context-store.js";
import { mkdtemp, rm, readFile } from "fs/promises";
import { tmpdir } from "os";
import { join } from "path";
import { existsSync } from "fs";
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
    await appendContextMessage(msg, null);
    expect(existsSync(contextFile)).toBe(false);
  });
});

describe("rotateFile", () => {
  it("renames existing file to .prev and creates fresh empty file", async () => {
    const msg: Anthropic.MessageParam = { role: "user", content: "prior session" };
    await appendContextMessage(msg, contextFile);

    await rotateFile(contextFile);

    // Current file is empty
    const current = await readFile(contextFile, "utf-8");
    expect(current).toBe("");

    // .prev contains the previous content
    const prev = await readFile(contextFile + ".prev", "utf-8");
    expect(prev.trim()).not.toBe("");
    const parsed = JSON.parse(prev.trim());
    expect(parsed).toEqual(msg);
  });

  it("creates a fresh empty file when nothing existed before", async () => {
    expect(existsSync(contextFile)).toBe(false);
    await rotateFile(contextFile);
    const current = await readFile(contextFile, "utf-8");
    expect(current).toBe("");
    expect(existsSync(contextFile + ".prev")).toBe(false);
  });

  it("overwrites an existing .prev file", async () => {
    // First rotation
    await appendContextMessage({ role: "user", content: "session 1" }, contextFile);
    await rotateFile(contextFile);

    // Second rotation — session 1 is in .prev, add session 2 content
    await appendContextMessage({ role: "user", content: "session 2" }, contextFile);
    await rotateFile(contextFile);

    // .prev should now contain session 2 (session 1 is gone — only 1 prev retained)
    const prev = await readFile(contextFile + ".prev", "utf-8");
    const parsed = JSON.parse(prev.trim());
    expect(parsed.content).toBe("session 2");
  });
});

describe("clearContextStore", () => {
  it("rotates by default: current file ends up empty, previous preserved as .prev", async () => {
    const msg: Anthropic.MessageParam = { role: "user", content: "hello" };
    await appendContextMessage(msg, contextFile);

    await clearContextStore(contextFile);

    const current = await readFile(contextFile, "utf-8");
    expect(current).toBe("");
    const prev = await readFile(contextFile + ".prev", "utf-8");
    expect(JSON.parse(prev.trim())).toEqual(msg);
  });

  it("truncates in-place when rotate:false (used for /compact rewrite)", async () => {
    const msg: Anthropic.MessageParam = { role: "user", content: "hello" };
    await appendContextMessage(msg, contextFile);

    await clearContextStore(contextFile, { rotate: false });

    const current = await readFile(contextFile, "utf-8");
    expect(current).toBe("");
    expect(existsSync(contextFile + ".prev")).toBe(false);
  });

  it("is a no-op when the file does not exist (no error)", async () => {
    await clearContextStore(contextFile);
    // file ends up as empty (created by rotateFile)
    expect(existsSync(contextFile)).toBe(true);
    const content = await readFile(contextFile, "utf-8");
    expect(content).toBe("");
  });

  it("is a no-op when filePath is null", async () => {
    await clearContextStore(null);
  });
});
