import { describe, it, expect, beforeEach, afterEach } from "bun:test";
import { writeFile, mkdir, rm } from "fs/promises";
import { join } from "path";
import { executeTool, formatToolCall } from "./tools.js";

// ---------------------------------------------------------------------------
// Unit tests for tools.ts
// ---------------------------------------------------------------------------

const TMP = "/tmp/omega-tools-test";

beforeEach(async () => {
  await mkdir(TMP, { recursive: true });
});

afterEach(async () => {
  await rm(TMP, { recursive: true, force: true });
});

// --- formatToolCall ---

describe("formatToolCall", () => {
  it("formats read_file without options", () => {
    expect(formatToolCall("read_file", { path: "src/agent.ts" })).toBe(
      "read_file: src/agent.ts"
    );
  });

  it("formats read_file with offset", () => {
    expect(formatToolCall("read_file", { path: "x.ts", offset: 10 })).toBe(
      "read_file: x.ts (from line 10)"
    );
  });

  it("formats read_file with limit", () => {
    expect(formatToolCall("read_file", { path: "x.ts", limit: 50 })).toBe(
      "read_file: x.ts (50 lines)"
    );
  });

  it("formats write_file with byte count", () => {
    expect(
      formatToolCall("write_file", { path: "x.ts", content: "hello" })
    ).toBe("write_file: x.ts (5 bytes)");
  });

  it("formats run_command", () => {
    expect(formatToolCall("run_command", { command: "ls -la" })).toBe(
      "run_command: ls -la"
    );
  });

  it("formats list_files without recursive", () => {
    expect(formatToolCall("list_files", { path: "src" })).toBe(
      "list_files: src"
    );
  });

  it("formats list_files recursive", () => {
    expect(
      formatToolCall("list_files", { path: "src", recursive: true })
    ).toBe("list_files: src (recursive)");
  });

  it("formats unknown tool as JSON", () => {
    expect(formatToolCall("unknown", { foo: 1 })).toBe(
      'unknown: {"foo":1}'
    );
  });
});

// --- executeTool: read_file ---

describe("executeTool: read_file", () => {
  it("reads a file successfully", async () => {
    const path = join(TMP, "hello.txt");
    await writeFile(path, "hello world\n");
    const result = await executeTool("read_file", { path });
    expect(result.isError).toBe(false);
    expect(result.output).toContain("hello world");
    expect(result.durationMs).toBeGreaterThanOrEqual(0);
  });

  it("returns error for missing file", async () => {
    const result = await executeTool("read_file", {
      path: join(TMP, "nonexistent.txt"),
    });
    expect(result.isError).toBe(true);
    expect(result.output).toContain("Error:");
  });

  it("supports offset and limit", async () => {
    const path = join(TMP, "lines.txt");
    const content = Array.from({ length: 10 }, (_, i) => `line ${i + 1}`).join("\n");
    await writeFile(path, content);
    const result = await executeTool("read_file", { path, offset: 3, limit: 2 });
    expect(result.isError).toBe(false);
    expect(result.output).toContain("line 3");
    expect(result.output).toContain("line 4");
    expect(result.output).not.toContain("line 1");
    expect(result.output).not.toContain("line 5");
  });
});

// --- executeTool: write_file ---

describe("executeTool: write_file", () => {
  it("writes a file successfully", async () => {
    const path = join(TMP, "output.txt");
    const result = await executeTool("write_file", {
      path,
      content: "test content\n",
    });
    expect(result.isError).toBe(false);
    expect(result.output).toContain("Wrote");

    const { readFile } = await import("fs/promises");
    const content = await readFile(path, "utf-8");
    expect(content).toBe("test content\n");
  });

  it("creates parent directories", async () => {
    const path = join(TMP, "nested", "dir", "file.txt");
    const result = await executeTool("write_file", {
      path,
      content: "nested",
    });
    expect(result.isError).toBe(false);
    const { readFile } = await import("fs/promises");
    expect(await readFile(path, "utf-8")).toBe("nested");
  });

  it("overwrites existing file", async () => {
    const path = join(TMP, "existing.txt");
    await writeFile(path, "old");
    await executeTool("write_file", { path, content: "new" });
    const { readFile } = await import("fs/promises");
    expect(await readFile(path, "utf-8")).toBe("new");
  });
});

// --- executeTool: run_command ---

describe("executeTool: run_command", () => {
  it("runs a simple command", async () => {
    const result = await executeTool("run_command", { command: "echo hello" });
    expect(result.isError).toBe(false);
    expect(result.output).toContain("hello");
  });

  it("captures stderr", async () => {
    const result = await executeTool("run_command", {
      command: "echo error >&2",
    });
    expect(result.output).toContain("error");
  });

  it("includes exit code for non-zero exits", async () => {
    const result = await executeTool("run_command", {
      command: "exit 1",
    });
    expect(result.output).toContain("exit code: 1");
  });

  it("returns (no output) for empty output", async () => {
    const result = await executeTool("run_command", {
      command: "true",
    });
    expect(result.output).toBe("(no output)");
  });
});

// --- executeTool: list_files ---

describe("executeTool: list_files", () => {
  it("lists files in a directory", async () => {
    await writeFile(join(TMP, "a.txt"), "");
    await writeFile(join(TMP, "b.txt"), "");
    const result = await executeTool("list_files", { path: TMP });
    expect(result.isError).toBe(false);
    expect(result.output).toContain("a.txt");
    expect(result.output).toContain("b.txt");
  });

  it("lists directories with trailing slash", async () => {
    await mkdir(join(TMP, "subdir"));
    const result = await executeTool("list_files", { path: TMP });
    expect(result.output).toContain("subdir/");
  });

  it("lists recursively", async () => {
    await mkdir(join(TMP, "sub"));
    await writeFile(join(TMP, "sub", "deep.txt"), "");
    const result = await executeTool("list_files", { path: TMP, recursive: true });
    expect(result.output).toContain("deep.txt");
  });
});

// --- executeTool: unknown tool ---

describe("executeTool: unknown tool", () => {
  it("returns an error for unknown tool name", async () => {
    const result = await executeTool("nonexistent_tool", {});
    expect(result.isError).toBe(true);
    expect(result.output).toContain("Unknown tool");
  });
});
