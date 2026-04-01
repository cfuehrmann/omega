/**
 * Unit tests for lookupContextRecords (serves GET /context?hashes=...).
 *
 * Covers:
 *  - Missing file → []
 *  - Empty hashes array → []
 *  - All hashes found → records in requested-hash order (not file order)
 *  - Some hashes missing → only found records, no gaps
 *  - Malformed JSONL lines → skipped, valid records still returned
 *  - Duplicate hashes in request → first file occurrence returned once per request position
 */

import { describe, it, expect, beforeEach, afterEach } from "bun:test";
import { mkdtemp, rm, writeFile } from "fs/promises";
import { tmpdir } from "os";
import { join } from "path";

import { lookupContextRecords } from "./server.js";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

let tmpDir: string;
let contextFile: string;

beforeEach(async () => {
  tmpDir = await mkdtemp(join(tmpdir(), "omega-ctx-lookup-test-"));
  contextFile = join(tmpDir, "context.jsonl");
});

afterEach(async () => {
  await rm(tmpDir, { recursive: true, force: true });
});

/** Write an array of context records as JSONL. */
async function writeRecords(records: object[]): Promise<void> {
  await writeFile(contextFile, records.map(r => JSON.stringify(r)).join("\n") + "\n", "utf-8");
}

/** Minimal valid context record. */
function rec(hash: string, role: "user" | "assistant", content: string) {
  return { hash, time: "2025-01-01T00:00:00.000Z", role, content };
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("lookupContextRecords — missing file", () => {
  it("returns [] when context file does not exist", async () => {
    const result = await lookupContextRecords("/nonexistent/path/context.jsonl", ["abc123456789"]);
    expect(result).toEqual([]);
  });
});

describe("lookupContextRecords — empty hashes", () => {
  it("returns [] when hashes array is empty", async () => {
    await writeRecords([rec("aabbccddeeff", "user", "hello")]);
    const result = await lookupContextRecords(contextFile, []);
    expect(result).toEqual([]);
  });
});

describe("lookupContextRecords — all hashes found", () => {
  it("returns all records when all hashes are present", async () => {
    const r1 = rec("aabbccddeeff", "user", "first");
    const r2 = rec("112233445566", "assistant", "second");
    await writeRecords([r1, r2]);

    const result = await lookupContextRecords(contextFile, ["aabbccddeeff", "112233445566"]);
    expect(result).toHaveLength(2);
    expect(result[0]!.hash as string).toBe("aabbccddeeff");
    expect(result[1]!.hash as string).toBe("112233445566");
  });

  it("preserves requested-hash order, not file order", async () => {
    const r1 = rec("aabbccddeeff", "user", "first in file");
    const r2 = rec("112233445566", "assistant", "second in file");
    await writeRecords([r1, r2]);

    // Request in reverse order
    const result = await lookupContextRecords(contextFile, ["112233445566", "aabbccddeeff"]);
    expect(result).toHaveLength(2);
    expect(result[0]!.hash as string).toBe("112233445566");
    expect(result[1]!.hash as string).toBe("aabbccddeeff");
  });
});

describe("lookupContextRecords — partial matches", () => {
  it("returns only the found records when some hashes are missing", async () => {
    const r1 = rec("aabbccddeeff", "user", "exists");
    await writeRecords([r1]);

    const result = await lookupContextRecords(contextFile, ["aabbccddeeff", "000000000000"]);
    expect(result).toHaveLength(1);
    expect(result[0]!.hash as string).toBe("aabbccddeeff");
  });

  it("returns [] when no requested hashes are present in the file", async () => {
    await writeRecords([rec("aabbccddeeff", "user", "irrelevant")]);
    const result = await lookupContextRecords(contextFile, ["000000000000", "111111111111"]);
    expect(result).toEqual([]);
  });
});

describe("lookupContextRecords — malformed JSONL", () => {
  it("skips malformed lines and returns valid records", async () => {
    const good = rec("aabbccddeeff", "user", "good record");
    await writeFile(
      contextFile,
      [
        "{ this is not valid json",
        JSON.stringify(good),
        "",
        "another bad line!!!",
      ].join("\n") + "\n",
      "utf-8",
    );

    const result = await lookupContextRecords(contextFile, ["aabbccddeeff"]);
    expect(result).toHaveLength(1);
    expect(result[0]!.hash as string).toBe("aabbccddeeff");
  });

  it("skips records that fail schema validation", async () => {
    const invalid = { hash: "tooshort", time: "2025-01-01T00:00:00.000Z", role: "user", content: "hi" };
    const valid   = rec("aabbccddeeff", "user", "valid");
    await writeRecords([invalid, valid]);

    const result = await lookupContextRecords(contextFile, ["tooshort", "aabbccddeeff"]);
    expect(result).toHaveLength(1);
    expect(result[0]!.hash as string).toBe("aabbccddeeff");
  });
});

describe("lookupContextRecords — record content", () => {
  it("returns full record including role and content", async () => {
    const r = rec("aabbccddeeff", "assistant", "Hello from the model");
    await writeRecords([r]);

    const result = await lookupContextRecords(contextFile, ["aabbccddeeff"]);
    expect(result).toHaveLength(1);
    expect(result[0]!.role).toBe("assistant");
    expect(result[0]!.content as string).toBe("Hello from the model");
    expect(result[0]!.time as string).toBe("2025-01-01T00:00:00.000Z");
  });

  it("returns complex array content intact", async () => {
    const content = [{ type: "text" as const, text: "tool result output" }];
    const r = { hash: "aabbccddeeff", time: "2025-01-01T00:00:00.000Z", role: "user" as const, content };
    await writeRecords([r]);

    const result = await lookupContextRecords(contextFile, ["aabbccddeeff"]);
    expect(result).toHaveLength(1);
    expect(result[0]!.content as unknown).toEqual(content);
  });
});
