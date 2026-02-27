/**
 * Tests for the append-only context store (Step 3a / 3e-iii).
 */

import { describe, it, expect, beforeEach, afterEach } from "bun:test";
import { appendContextMessage, buildContextRecord, clearContextStore, rotateFile, prevPath, type ContextRecord } from "./context-store.js";
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
  it("creates the file and writes one message as JSONL with hash and ts", async () => {
    const msg: Anthropic.MessageParam = { role: "user", content: "hello" };
    await appendContextMessage(msg, contextFile);

    const raw = await readFile(contextFile, "utf-8");
    const lines = raw.trim().split("\n");
    expect(lines).toHaveLength(1);
    const record: ContextRecord = JSON.parse(lines[0]);
    // Must have the original fields plus hash and ts
    expect(record.role).toBe("user");
    expect(record.content).toBe("hello");
    expect(typeof record.hash).toBe("string");
    expect(record.hash).toHaveLength(8);
    expect(typeof record.ts).toBe("string");
    expect(record.ts).toMatch(/^\d{4}-\d{2}-\d{2}T/);
  });

  it("returns the hash of the stored record", async () => {
    const msg: Anthropic.MessageParam = { role: "user", content: "hello" };
    const hash = await appendContextMessage(msg, contextFile);
    expect(typeof hash).toBe("string");
    expect(hash).toHaveLength(8);

    // The returned hash must match what's in the file
    const raw = await readFile(contextFile, "utf-8");
    const record: ContextRecord = JSON.parse(raw.trim());
    expect(record.hash).toBe(hash);
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
    const r1: ContextRecord = JSON.parse(lines[0]);
    const r2: ContextRecord = JSON.parse(lines[1]);
    expect(r1.role).toBe("user");
    expect(r1.content).toBe("hello");
    expect(r2.role).toBe("assistant");
    expect((r2.content as any)[0].text).toBe("world");
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
    const record: ContextRecord = JSON.parse(raw.trim());
    expect(record.role).toBe("assistant");
    expect(Array.isArray(record.content)).toBe(true);
    const blocks = record.content as any[];
    expect(blocks[0].type).toBe("text");
    expect(blocks[1].type).toBe("tool_use");
    expect(blocks[1].name).toBe("read_file");
  });

  it("identical content at different times produces different hashes (ts prevents collision)", async () => {
    const msg: Anthropic.MessageParam = { role: "user", content: "ok" };
    // Small delay to ensure different ts values
    const hash1 = await appendContextMessage(msg, contextFile);
    await new Promise(r => setTimeout(r, 5));
    const hash2 = await appendContextMessage(msg, contextFile);
    // Different timestamps → different hashes
    expect(hash1).not.toBe(hash2);
  });

  it("returns the hash even when filePath is null (no file written)", async () => {
    const msg: Anthropic.MessageParam = { role: "user", content: "hello" };
    const hash = await appendContextMessage(msg, null);
    expect(typeof hash).toBe("string");
    expect(hash).toHaveLength(8);
    expect(existsSync(contextFile)).toBe(false);
  });
});

describe("appendContextMessage — null path (test-isolation mode)", () => {
  it("is a no-op (no file) but still returns a hash", async () => {
    const msg: Anthropic.MessageParam = { role: "user", content: "hello" };
    const hash = await appendContextMessage(msg, null);
    expect(existsSync(contextFile)).toBe(false);
    expect(typeof hash).toBe("string");
    expect(hash).toHaveLength(8);
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

    // .prev contains the previous content (as ContextRecord)
    const prev = await readFile(prevPath(contextFile), "utf-8");
    expect(prev.trim()).not.toBe("");
    const record: ContextRecord = JSON.parse(prev.trim());
    expect(record.role).toBe("user");
    expect(record.content).toBe("prior session");
    expect(record.hash).toHaveLength(8);
  });

  it("creates a fresh empty file when nothing existed before", async () => {
    expect(existsSync(contextFile)).toBe(false);
    await rotateFile(contextFile);
    const current = await readFile(contextFile, "utf-8");
    expect(current).toBe("");
    expect(existsSync(prevPath(contextFile))).toBe(false);
  });

  it("overwrites an existing .prev file", async () => {
    // First rotation
    await appendContextMessage({ role: "user", content: "session 1" }, contextFile);
    await rotateFile(contextFile);

    // Second rotation — session 1 is in .prev, add session 2 content
    await appendContextMessage({ role: "user", content: "session 2" }, contextFile);
    await rotateFile(contextFile);

    // .prev should now contain session 2 (session 1 is gone — only 1 prev retained)
    const prev = await readFile(prevPath(contextFile), "utf-8");
    const record: ContextRecord = JSON.parse(prev.trim());
    expect(record.content).toBe("session 2");
  });
});

describe("clearContextStore", () => {
  it("rotates by default: current file ends up empty, previous preserved as .prev", async () => {
    const msg: Anthropic.MessageParam = { role: "user", content: "hello" };
    await appendContextMessage(msg, contextFile);

    await clearContextStore(contextFile);

    const current = await readFile(contextFile, "utf-8");
    expect(current).toBe("");
    const prev = await readFile(prevPath(contextFile), "utf-8");
    const record: ContextRecord = JSON.parse(prev.trim());
    expect(record.role).toBe("user");
    expect(record.content).toBe("hello");
  });

  it("truncates in-place when rotate:false (used for /compact rewrite)", async () => {
    const msg: Anthropic.MessageParam = { role: "user", content: "hello" };
    await appendContextMessage(msg, contextFile);

    await clearContextStore(contextFile, { rotate: false });

    const current = await readFile(contextFile, "utf-8");
    expect(current).toBe("");
    expect(existsSync(prevPath(contextFile))).toBe(false);
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

// ---------------------------------------------------------------------------
// buildContextRecord (Step 3e-iii)
// ---------------------------------------------------------------------------

describe("buildContextRecord", () => {
  it("returns a record with hash, ts, role, and content", async () => {
    const msg: Anthropic.MessageParam = { role: "user", content: "hello" };
    const record = await buildContextRecord(msg);
    expect(record.role).toBe("user");
    expect(record.content).toBe("hello");
    expect(typeof record.hash).toBe("string");
    expect(record.hash).toHaveLength(8);
    expect(typeof record.ts).toBe("string");
    expect(record.ts).toMatch(/^\d{4}-\d{2}-\d{2}T/);
  });

  it("hash matches the sha256 of { ts, role, content } without hash", async () => {
    const msg: Anthropic.MessageParam = { role: "user", content: "test" };
    const record = await buildContextRecord(msg);
    // Recompute manually
    const input = JSON.stringify({ ts: record.ts, role: record.role, content: record.content });
    const data = new TextEncoder().encode(input);
    const buf = await crypto.subtle.digest("SHA-256", data);
    const expectedHex = Array.from(new Uint8Array(buf))
      .map(b => b.toString(16).padStart(2, "0"))
      .join("")
      .slice(0, 8);
    expect(record.hash).toBe(expectedHex);
  });

  it("two calls to buildContextRecord produce different hashes (different ts)", async () => {
    const msg: Anthropic.MessageParam = { role: "user", content: "ok" };
    const r1 = await buildContextRecord(msg);
    await new Promise(r => setTimeout(r, 5));
    const r2 = await buildContextRecord(msg);
    expect(r1.hash).not.toBe(r2.hash);
  });

  it("does not write any file", async () => {
    const msg: Anthropic.MessageParam = { role: "user", content: "hello" };
    await buildContextRecord(msg);
    expect(existsSync(contextFile)).toBe(false);
  });
});
