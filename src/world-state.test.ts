/**
 * Tests for world-state persistence.
 */

import { describe, it, expect, beforeEach, afterEach } from "bun:test";
import { readWorldState, writeWorldState } from "./world-state.js";
import { mkdtemp, rm } from "fs/promises";
import { tmpdir } from "os";
import { join } from "path";

let tempDir: string;

beforeEach(async () => {
  tempDir = await mkdtemp(join(tmpdir(), "omega-ws-test-"));
});

afterEach(async () => {
  await rm(tempDir, { recursive: true, force: true });
});

describe("readWorldState", () => {
  it("returns null when file does not exist", async () => {
    const result = await readWorldState(join(tempDir, "world-state.md"));
    expect(result).toBeNull();
  });

  it("returns content when file exists", async () => {
    const path = join(tempDir, "world-state.md");
    await writeWorldState("Hello world state.", path);
    const result = await readWorldState(path);
    expect(result).toBe("Hello world state.");
  });
});

describe("writeWorldState", () => {
  it("writes content to file", async () => {
    const path = join(tempDir, "world-state.md");
    await writeWorldState("State: all good.", path);
    const result = await readWorldState(path);
    expect(result).toBe("State: all good.");
  });

  it("overwrites existing content", async () => {
    const path = join(tempDir, "world-state.md");
    await writeWorldState("Old state.", path);
    await writeWorldState("New state.", path);
    const result = await readWorldState(path);
    expect(result).toBe("New state.");
  });

  it("creates parent directories if needed", async () => {
    const path = join(tempDir, "nested", "dir", "world-state.md");
    await writeWorldState("Nested state.", path);
    const result = await readWorldState(path);
    expect(result).toBe("Nested state.");
  });
});
