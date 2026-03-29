/**
 * Tests for the append-only context store.
 */

import { describe, it, expect, beforeEach, afterEach } from "bun:test";
import { appendContextMessage, buildContextRecord, type ContextRecord } from "./context-store.js";
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
  it("creates the file and writes one message as JSONL with hash and time", async () => {
    const msg: Anthropic.Beta.Messages.BetaMessageParam = { role: "user", content: "hello" };
    await appendContextMessage(msg, contextFile);

    const raw = await readFile(contextFile, "utf-8");
    const lines = raw.trim().split("\n");
    expect(lines).toHaveLength(1);
    const record: ContextRecord = JSON.parse(lines[0]!);
    // Must have the original fields plus hash and time
    expect(record.role).toBe("user");
    expect(record.content).toBe("hello");
    expect(typeof record.hash).toBe("string");
    expect(record.hash).toHaveLength(12);
    expect(record.hash).toMatch(/^[0-9a-f]{12}$/);
    expect(typeof record.time).toBe("string");
    expect(record.time).toMatch(/^\d{4}-\d{2}-\d{2}T/);
  });

  it("returns the hash of the stored record", async () => {
    const msg: Anthropic.Beta.Messages.BetaMessageParam = { role: "user", content: "hello" };
    const hash = await appendContextMessage(msg, contextFile);
    expect(typeof hash).toBe("string");
    expect(hash).toHaveLength(12);

    // The returned hash must match what's in the file
    const raw = await readFile(contextFile, "utf-8");
    const record: ContextRecord = JSON.parse(raw.trim());
    expect(record.hash).toBe(hash);
  });

  it("appends a second message on a new line", async () => {
    const msg1: Anthropic.Beta.Messages.BetaMessageParam = { role: "user", content: "hello" };
    const msg2: Anthropic.Beta.Messages.BetaMessageParam = {
      role: "assistant",
      content: [{ type: "text", text: "world" }],
    };

    await appendContextMessage(msg1, contextFile);
    await appendContextMessage(msg2, contextFile);

    const raw = await readFile(contextFile, "utf-8");
    const lines = raw.trim().split("\n");
    expect(lines).toHaveLength(2);
    const r1: ContextRecord = JSON.parse(lines[0]!);
    const r2: ContextRecord = JSON.parse(lines[1]!);
    expect(r1.role).toBe("user");
    expect(r1.content).toBe("hello");
    expect(r2.role).toBe("assistant");
    expect((r2.content as any)[0].text).toBe("world");
  });

  it("round-trips complex content (tool use blocks)", async () => {
    const msg: Anthropic.Beta.Messages.BetaMessageParam = {
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

  it("two calls with identical content produce different hashes (random, not content-based)", async () => {
    const msg: Anthropic.Beta.Messages.BetaMessageParam = { role: "user", content: "ok" };
    const hash1 = await appendContextMessage(msg, contextFile);
    const hash2 = await appendContextMessage(msg, contextFile);
    // Hashes are random — collision probability is negligible (2^48 space)
    expect(hash1).not.toBe(hash2);
  });

  it("returns the hash even when filePath is null (no file written)", async () => {
    const msg: Anthropic.Beta.Messages.BetaMessageParam = { role: "user", content: "hello" };
    const hash = await appendContextMessage(msg, null);
    expect(typeof hash).toBe("string");
    expect(hash).toHaveLength(12);
    expect(existsSync(contextFile)).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// buildContextRecord
// ---------------------------------------------------------------------------

describe("buildContextRecord", () => {
  it("returns a record with hash, time, role, and content", () => {
    const msg: Anthropic.Beta.Messages.BetaMessageParam = { role: "user", content: "hello" };
    const record = buildContextRecord(msg);
    expect(record.role).toBe("user");
    expect(record.content).toBe("hello");
    expect(typeof record.hash).toBe("string");
    expect(record.hash).toHaveLength(12);
    expect(record.hash).toMatch(/^[0-9a-f]{12}$/);
    expect(typeof record.time).toBe("string");
    expect(record.time).toMatch(/^\d{4}-\d{2}-\d{2}T/);
  });

  it("hash is 12 lowercase hex chars (6 random bytes)", () => {
    const msg: Anthropic.Beta.Messages.BetaMessageParam = { role: "user", content: "test" };
    const record = buildContextRecord(msg);
    // 12 hex chars = 6 bytes of random data
    expect(record.hash).toMatch(/^[0-9a-f]{12}$/);
  });

  it("two calls to buildContextRecord produce different hashes (random)", () => {
    const msg: Anthropic.Beta.Messages.BetaMessageParam = { role: "user", content: "ok" };
    const r1 = buildContextRecord(msg);
    const r2 = buildContextRecord(msg);
    // Hashes are random — collision probability is negligible
    expect(r1.hash).not.toBe(r2.hash);
  });

  it("does not write any file", () => {
    const msg: Anthropic.Beta.Messages.BetaMessageParam = { role: "user", content: "hello" };
    buildContextRecord(msg);
    expect(existsSync(contextFile)).toBe(false);
  });
});
