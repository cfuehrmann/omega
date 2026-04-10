import { describe, it, expect, beforeEach, afterEach } from "bun:test";
import { writeFile, mkdir, rm } from "fs/promises";
import { join } from "path";
import { executeTool, formatToolCall } from "./tools.js";
import { primaryToolArg } from "./tools.schema.js";

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

  it("kills the process and reports timeout when it exceeds the timeout", async () => {
    const start = Date.now();
    // The inner bash keeps the pipes alive even after the outer bash is killed —
    // this is the "orphaned child holds pipe FDs open" scenario that caused a
    // 60 s timeout to run for 906 s in practice.
    const result = await executeTool("run_command", {
      command: "bash -c 'sleep 300'",
      timeout: 2,
    });
    const elapsed = Date.now() - start;
    // Should finish well within 5 s (2 s timeout + generous buffer)
    expect(elapsed).toBeLessThan(5_000);
    expect(result.isError).toBe(false);
    expect(result.output).toContain("timeout");
  });

  it("errors when command is missing", async () => {
    const result = await executeTool("run_command", {});
    expect(result.isError).toBe(true);
    // Zod validation error — message contains the field name and expected type
    expect(result.output).toMatch(/command/i);
    expect(result.output).toMatch(/string/i);
  });
});

// --- tool output cap ---

describe("executeTool: output cap", () => {
  it("caps run_command output at MAX_TOOL_OUTPUT_CHARS and appends a note", async () => {
    const { writeFile, unlink } = await import("fs/promises");
    const tmpPath = `/tmp/omega-cap-cmd-${Date.now()}.txt`;
    // Write 200k chars to a file, then cat it — output will exceed the 100k cap
    await writeFile(tmpPath, "z".repeat(200_000));
    try {
      const result = await executeTool("run_command", { command: `cat ${tmpPath}` });
      expect(result.isError).toBe(false);
      expect(result.output.length).toBeLessThan(110_000); // well under 200k
      expect(result.output).toContain("[truncated");
      expect(result.output).toMatch(/tool output was \d+ chars/);
    } finally {
      await unlink(tmpPath).catch(() => {});
    }
  });

  it("does not truncate output under the cap", async () => {
    const result = await executeTool("run_command", { command: "echo hi" });
    expect(result.output).not.toContain("[truncated");
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

// --- executeTool: edit_file ---

describe("executeTool: edit_file", () => {
  it("replaces exact text in a file", async () => {
    const path = join(TMP, "edit.txt");
    await writeFile(path, "hello world\ngoodbye world\n");
    const result = await executeTool("edit_file", {
      path,
      replacements: [{ old_text: "hello world", new_text: "hi world" }],
    });
    expect(result.isError).toBe(false);
    expect(result.output).toContain("edit_file");

    const { readFile } = await import("fs/promises");
    const content = await readFile(path, "utf-8");
    expect(content).toBe("hi world\ngoodbye world\n");
  });

  it("returns error when old_text is not found", async () => {
    const path = join(TMP, "edit2.txt");
    await writeFile(path, "hello world\n");
    const result = await executeTool("edit_file", {
      path,
      replacements: [{ old_text: "not found text", new_text: "replacement" }],
    });
    expect(result.isError).toBe(true);
    expect(result.output).toContain("not found");
  });

  it("returns error when old_text matches multiple times", async () => {
    const path = join(TMP, "edit3.txt");
    await writeFile(path, "foo bar\nfoo baz\n");
    const result = await executeTool("edit_file", {
      path,
      replacements: [{ old_text: "foo", new_text: "qux" }],
    });
    expect(result.isError).toBe(true);
    expect(result.output).toContain("2 times");
  });

  it("returns error for missing file", async () => {
    const result = await executeTool("edit_file", {
      path: join(TMP, "nonexistent.txt"),
      replacements: [{ old_text: "x", new_text: "y" }],
    });
    expect(result.isError).toBe(true);
  });

  it("handles multi-line old_text and new_text", async () => {
    const path = join(TMP, "multiline.txt");
    await writeFile(path, "line 1\nline 2\nline 3\nline 4\n");
    const result = await executeTool("edit_file", {
      path,
      replacements: [{ old_text: "line 2\nline 3", new_text: "replaced 2\nreplaced 3\nextra line" }],
    });
    expect(result.isError).toBe(false);

    const { readFile } = await import("fs/promises");
    const content = await readFile(path, "utf-8");
    expect(content).toBe("line 1\nreplaced 2\nreplaced 3\nextra line\nline 4\n");
  });

  it("applies multiple replacements via replacements array", async () => {
    const path = join(TMP, "multi-edit.txt");
    await writeFile(path, "aaa\nbbb\nccc\n");
    const result = await executeTool("edit_file", {
      path,
      replacements: [
        { old_text: "aaa", new_text: "AAA" },
        { old_text: "ccc", new_text: "CCC" },
      ],
    });
    expect(result.isError).toBe(false);
    expect(result.output).toContain("2 replacements");

    const { readFile } = await import("fs/promises");
    const content = await readFile(path, "utf-8");
    expect(content).toBe("AAA\nbbb\nCCC\n");
  });

  it("fails on second replacement if old_text not found after first edit", async () => {
    const path = join(TMP, "multi-edit-fail.txt");
    await writeFile(path, "aaa\nbbb\n");
    const result = await executeTool("edit_file", {
      path,
      replacements: [
        { old_text: "aaa", new_text: "bbb" },   // now two "bbb"
        { old_text: "bbb", new_text: "ccc" },   // ambiguous
      ],
    });
    expect(result.isError).toBe(true);
    expect(result.output).toContain("2 times");
    expect(result.output).toContain("replacement 2/2");
    // File should NOT have been written (error on second replacement)
    const { readFile } = await import("fs/promises");
    const content = await readFile(path, "utf-8");
    expect(content).toBe("aaa\nbbb\n");
  });

  it("errors when replacements array is empty", async () => {
    const path = join(TMP, "multi-edit-empty.txt");
    await writeFile(path, "hello\n");
    const result = await executeTool("edit_file", { path, replacements: [] });
    expect(result.isError).toBe(true);
    expect(result.output).toContain("requires");
  });
});

describe("formatToolCall: edit_file", () => {
  it("formats edit_file with path and replacement count", () => {
    const formatted = formatToolCall("edit_file", {
      path: "src/agent.ts",
      replacements: [{ old_text: "hello", new_text: "world!" }],
    });
    expect(formatted).toContain("edit_file");
    expect(formatted).toContain("src/agent.ts");
    expect(formatted).toContain("1 replacement");
  });

  it("formats edit_file with replacements count", () => {
    const formatted = formatToolCall("edit_file", {
      path: "src/agent.ts",
      replacements: [
        { old_text: "a", new_text: "b" },
        { old_text: "c", new_text: "d" },
      ],
    });
    expect(formatted).toContain("2 replacements");
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

// --- executeTool: web_search ---

describe("executeTool: web_search", () => {
  it("returns a non-error result for a simple query", async () => {
    const result = await executeTool("web_search", { query: "bun javascript runtime" });
    expect(result.isError).toBe(false);
    expect(result.durationMs).toBeGreaterThanOrEqual(0);
  }, 15_000);

  it("result contains at least one URL or snippet", async () => {
    const result = await executeTool("web_search", { query: "TypeScript handbook" });
    expect(result.isError).toBe(false);
    // Should contain http in a URL or some meaningful text
    expect(result.output.length).toBeGreaterThan(20);
  }, 15_000);

  it("returns an error result when query is empty", async () => {
    const result = await executeTool("web_search", { query: "" });
    expect(result.isError).toBe(true);
  });

  it("uses Brave Search when BRAVE_SEARCH_API_KEY is set and returns full https:// URLs", async () => {
    // This test only runs when the key is available (CI may skip via env)
    if (!process.env.BRAVE_SEARCH_API_KEY) return;
    const result = await executeTool("web_search", { query: "TypeScript official documentation" });
    expect(result.isError).toBe(false);
    // Brave returns real full URLs, not bare domain names
    expect(result.output).toMatch(/https?:\/\/[a-z]/);
  }, 15_000);

  it("formatToolCall formats web_search", () => {
    expect(formatToolCall("web_search", { query: "hello world" })).toBe(
      "web_search: hello world"
    );
  });
});

// --- executeTool: fetch_url ---

describe("executeTool: fetch_url", () => {
  it("fetches a real URL and returns text content", async () => {
    const result = await executeTool("fetch_url", { url: "https://example.com" });
    expect(result.isError).toBe(false);
    expect(result.output).toContain("Example Domain");
  }, 15_000);

  it("result is truncated if page is very long", async () => {
    // example.com is short, so test truncation indirectly via the cap
    const result = await executeTool("fetch_url", { url: "https://example.com" });
    expect(result.isError).toBe(false);
    // Output should never exceed ~10000 chars (our cap)
    expect(result.output.length).toBeLessThanOrEqual(10_000);
  }, 15_000);

  it("returns an error for an invalid URL", async () => {
    const result = await executeTool("fetch_url", { url: "not-a-url" });
    expect(result.isError).toBe(true);
  });

  it("returns an error for an unreachable host", async () => {
    const result = await executeTool("fetch_url", { url: "https://this-host-does-not-exist.invalid" });
    expect(result.isError).toBe(true);
  }, 15_000);

  it("formatToolCall formats fetch_url without offset", () => {
    expect(formatToolCall("fetch_url", { url: "https://example.com" })).toBe(
      "fetch_url: https://example.com"
    );
  });

  it("formatToolCall formats fetch_url with offset", () => {
    expect(formatToolCall("fetch_url", { url: "https://example.com", offset: 5000 })).toBe(
      "fetch_url: https://example.com (offset 5000)"
    );
  });

  it("offset=0 returns the same content as no offset", async () => {
    const [r1, r2] = await Promise.all([
      executeTool("fetch_url", { url: "https://example.com" }),
      executeTool("fetch_url", { url: "https://example.com", offset: 0 }),
    ]);
    expect(r1.isError).toBe(false);
    expect(r2.isError).toBe(false);
    expect(r1.output).toBe(r2.output);
  }, 15_000);

  it("offset skips the leading characters", async () => {
    const r = await executeTool("fetch_url", { url: "https://example.com", offset: 10 });
    expect(r.isError).toBe(false);
    // The full page starts with "Example Domain" (or similar); offset=10 skips the first 10 chars
    const full = await executeTool("fetch_url", { url: "https://example.com" });
    expect(r.output).toBe(full.output.slice(10));
  }, 15_000);

  it("offset beyond page length returns a no-more-content message", async () => {
    const r = await executeTool("fetch_url", { url: "https://example.com", offset: 999_999 });
    expect(r.isError).toBe(false);
    expect(r.output).toContain("No more content");
  }, 15_000);
});

// --- executeTool: grep_files ---

describe("executeTool: grep_files", () => {
  it("finds a literal pattern in a file", async () => {
    const path = join(TMP, "alpha.ts");
    await writeFile(path, "const foo = 1;\nconst bar = 2;\nconst foo2 = 3;\n");
    const result = await executeTool("grep_files", {
      pattern: "foo",
      path: TMP,
    });
    expect(result.isError).toBe(false);
    expect(result.output).toContain("alpha.ts");
    expect(result.output).toContain("foo");
  });

  it("returns structured file:line:text output", async () => {
    const path = join(TMP, "beta.ts");
    await writeFile(path, "hello world\ngoodbye world\n");
    const result = await executeTool("grep_files", {
      pattern: "hello",
      path: TMP,
    });
    expect(result.isError).toBe(false);
    // Should contain line number
    expect(result.output).toMatch(/1:/);
    expect(result.output).toContain("hello world");
  });

  it("respects file_glob to restrict search", async () => {
    await writeFile(join(TMP, "code.ts"), "const x = myFunc();\n");
    await writeFile(join(TMP, "notes.txt"), "myFunc is important\n");
    const result = await executeTool("grep_files", {
      pattern: "myFunc",
      path: TMP,
      file_glob: "*.ts",
    });
    expect(result.isError).toBe(false);
    expect(result.output).toContain("code.ts");
    expect(result.output).not.toContain("notes.txt");
  });

  it("returns no matches message when pattern not found", async () => {
    await writeFile(join(TMP, "empty.ts"), "nothing here\n");
    const result = await executeTool("grep_files", {
      pattern: "zzz_not_present_zzz",
      path: TMP,
    });
    expect(result.isError).toBe(false);
    expect(result.output).toContain("No matches");
  });

  it("is case-insensitive by default", async () => {
    await writeFile(join(TMP, "case.ts"), "Hello World\n");
    const result = await executeTool("grep_files", {
      pattern: "hello",
      path: TMP,
    });
    expect(result.isError).toBe(false);
    expect(result.output).toContain("Hello World");
  });

  it("case_sensitive=true respects case", async () => {
    await writeFile(join(TMP, "case2.ts"), "Hello World\nhello world\n");
    const result = await executeTool("grep_files", {
      pattern: "Hello",
      path: TMP,
      case_sensitive: true,
      context_lines: 0,
    });
    expect(result.isError).toBe(false);
    expect(result.output).toContain("Hello World");
    // Should not match "hello world" (lowercase)
    const lines = result.output.split("\n").filter(l => l.includes("hello world"));
    expect(lines.length).toBe(0);
  });

  it("respects max_results cap and annotates truncation", async () => {
    // Write a file with 300 matching lines
    const lines = Array.from({ length: 300 }, (_, i) => `match line ${i}`).join("\n");
    await writeFile(join(TMP, "big.ts"), lines);
    const result = await executeTool("grep_files", {
      pattern: "match line",
      path: TMP,
      max_results: 10,
    });
    expect(result.isError).toBe(false);
    const matchLines = result.output.split("\n").filter(l => l.includes("match line"));
    expect(matchLines.length).toBeLessThanOrEqual(10);
    expect(result.output).toContain("truncated");
  });

  it("default max_results caps at 200", async () => {
    const lines = Array.from({ length: 300 }, (_, i) => `item ${i}`).join("\n");
    await writeFile(join(TMP, "large.ts"), lines);
    const result = await executeTool("grep_files", {
      pattern: "item",
      path: TMP,
    });
    expect(result.isError).toBe(false);
    const matchLines = result.output.split("\n").filter(l => l.includes("item"));
    expect(matchLines.length).toBeLessThanOrEqual(200);
  });

  it("returns error when pattern is missing", async () => {
    const result = await executeTool("grep_files", { path: TMP });
    expect(result.isError).toBe(true);
    expect(result.output).toContain("pattern");
  });

  it("returns error when path is missing", async () => {
    const result = await executeTool("grep_files", { pattern: "foo" });
    expect(result.isError).toBe(true);
    expect(result.output).toContain("path");
  });

  it("searches multiple files and returns matches from each", async () => {
    await writeFile(join(TMP, "file1.ts"), "export function alpha() {}\n");
    await writeFile(join(TMP, "file2.ts"), "import { alpha } from './file1';\n");
    const result = await executeTool("grep_files", {
      pattern: "alpha",
      path: TMP,
    });
    expect(result.isError).toBe(false);
    expect(result.output).toContain("file1.ts");
    expect(result.output).toContain("file2.ts");
  });
});

describe("formatToolCall: grep_files", () => {
  it("formats grep_files with pattern and path", () => {
    const s = formatToolCall("grep_files", { pattern: "compactTurn", path: "src" });
    expect(s).toBe("grep_files: compactTurn in src");
  });

  it("includes file_glob when provided", () => {
    const s = formatToolCall("grep_files", { pattern: "foo", path: "src", file_glob: "*.ts" });
    expect(s).toBe("grep_files: foo in src [*.ts]");
  });
});

// --- executeTool: find_files ---

describe("executeTool: find_files", () => {
  it("finds files matching a glob pattern", async () => {
    await writeFile(join(TMP, "alpha.ts"), "");
    await writeFile(join(TMP, "beta.js"), "");
    await writeFile(join(TMP, "gamma.ts"), "");
    const result = await executeTool("find_files", {
      pattern: "*.ts",
      path: TMP,
    });
    expect(result.isError).toBe(false);
    expect(result.output).toContain("alpha.ts");
    expect(result.output).toContain("gamma.ts");
    expect(result.output).not.toContain("beta.js");
  });

  it("finds directories when type=d", async () => {
    await mkdir(join(TMP, "mydir"));
    await writeFile(join(TMP, "myfile.txt"), "");
    const result = await executeTool("find_files", {
      pattern: "mydir",
      path: TMP,
      type: "d",
    });
    expect(result.isError).toBe(false);
    expect(result.output).toContain("mydir");
    expect(result.output).not.toContain("myfile.txt");
  });

  it("returns no matches message when nothing found", async () => {
    await writeFile(join(TMP, "readme.txt"), "");
    const result = await executeTool("find_files", {
      pattern: "*.xyz",
      path: TMP,
    });
    expect(result.isError).toBe(false);
    expect(result.output).toContain("No files found");
  });

  it("respects max_results cap and annotates truncation", async () => {
    // Create 15 .ts files
    for (let i = 0; i < 15; i++) {
      await writeFile(join(TMP, `file${i}.ts`), "");
    }
    const result = await executeTool("find_files", {
      pattern: "*.ts",
      path: TMP,
      max_results: 5,
    });
    expect(result.isError).toBe(false);
    const lines = result.output.split("\n").filter(l => l.endsWith(".ts"));
    expect(lines.length).toBeLessThanOrEqual(5);
    expect(result.output).toContain("truncated");
  });

  it("finds files recursively in subdirectories", async () => {
    await mkdir(join(TMP, "sub"));
    await writeFile(join(TMP, "sub", "deep.ts"), "");
    const result = await executeTool("find_files", {
      pattern: "*.ts",
      path: TMP,
    });
    expect(result.isError).toBe(false);
    expect(result.output).toContain("deep.ts");
  });

  it("returns error when path is missing", async () => {
    const result = await executeTool("find_files", { pattern: "*.ts" });
    expect(result.isError).toBe(true);
    expect(result.output).toContain("path");
  });

  it("returns error when pattern is missing", async () => {
    const result = await executeTool("find_files", { path: TMP });
    expect(result.isError).toBe(true);
    expect(result.output).toContain("pattern");
  });

  it("does not include hidden files by default", async () => {
    await writeFile(join(TMP, ".hidden.ts"), "");
    await writeFile(join(TMP, "visible.ts"), "");
    const result = await executeTool("find_files", {
      pattern: "*.ts",
      path: TMP,
    });
    expect(result.isError).toBe(false);
    expect(result.output).toContain("visible.ts");
    expect(result.output).not.toContain(".hidden.ts");
  });

  it("includes hidden files when hidden=true", async () => {
    await writeFile(join(TMP, ".secret.ts"), "");
    const result = await executeTool("find_files", {
      pattern: "*.ts",
      path: TMP,
      hidden: true,
    });
    expect(result.isError).toBe(false);
    expect(result.output).toContain(".secret.ts");
  });
});

describe("formatToolCall: find_files", () => {
  it("formats find_files with pattern and path", () => {
    const s = formatToolCall("find_files", { pattern: "*.ts", path: "src" });
    expect(s).toBe("find_files: *.ts in src");
  });

  it("includes type when provided", () => {
    const s = formatToolCall("find_files", { pattern: "*.ts", path: "src", type: "f" });
    expect(s).toBe("find_files: *.ts in src [type=f]");
  });
});

// --- executeTool: run_background ---

describe("executeTool: run_background", () => {
  it("returns pid and logFile immediately without blocking", async () => {
    // sleep is a long-running process; run_background must not wait for it
    const result = await executeTool("run_background", {
      command: "sleep 30",
    });
    expect(result.isError).toBe(false);
    const data = JSON.parse(result.output);
    expect(typeof data.pid).toBe("number");
    expect(data.pid).toBeGreaterThan(0);
    expect(typeof data.logFile).toBe("string");
    expect(data.logFile.length).toBeGreaterThan(0);
    // Clean up
    try { process.kill(data.pid, "SIGKILL"); } catch {}
  });

  it("log file captures stdout of the process", async () => {
    const result = await executeTool("run_background", {
      command: "echo hello_background",
    });
    expect(result.isError).toBe(false);
    const data = JSON.parse(result.output);
    // Wait briefly for the fast command to finish
    await new Promise(r => setTimeout(r, 200));
    const { readFile: rf } = await import("fs/promises");
    const log = await rf(data.logFile, "utf-8");
    expect(log).toContain("hello_background");
  });

  it("returns error when command is missing", async () => {
    const result = await executeTool("run_background", {});
    expect(result.isError).toBe(true);
    expect(result.output).toContain("command");
  });

  it("formatToolCall formats run_background", () => {
    const s = formatToolCall("run_background", { command: "bun run dev" });
    expect(s).toBe("run_background: bun run dev");
  });
});

// --- executeTool: wait_for_output ---

describe("executeTool: wait_for_output", () => {
  it("returns immediately when pattern appears in log", async () => {
    // Start a background process that writes a known pattern after a short delay
    const bgResult = await executeTool("run_background", {
      command: "sleep 0.1 && echo 'Server listening on port 3001'",
    });
    const { logFile } = JSON.parse(bgResult.output);

    const result = await executeTool("wait_for_output", {
      logFile,
      timeoutMs: 3000,
      pattern: "listening on port",
    });
    expect(result.isError).toBe(false);
    const r = JSON.parse(result.output);
    expect(r.matched).toBe(true);
    expect(r.timedOut).toBe(false);
    expect(r.output).toContain("listening on port");
  });

  it("returns when minBytes threshold is reached", async () => {
    const bgResult = await executeTool("run_background", {
      command: "sleep 0.1 && printf 'x%.0s' {1..100}",
    });
    const { logFile } = JSON.parse(bgResult.output);

    const result = await executeTool("wait_for_output", {
      logFile,
      timeoutMs: 3000,
      minBytes: 50,
    });
    expect(result.isError).toBe(false);
    const r = JSON.parse(result.output);
    expect(r.minBytesReached).toBe(true);
    expect(r.timedOut).toBe(false);
    expect(r.output.length).toBeGreaterThanOrEqual(50);
  });

  it("returns on any output when neither pattern nor minBytes given", async () => {
    const bgResult = await executeTool("run_background", {
      command: "sleep 0.1 && echo hello",
    });
    const { logFile } = JSON.parse(bgResult.output);

    const result = await executeTool("wait_for_output", {
      logFile,
      timeoutMs: 3000,
    });
    expect(result.isError).toBe(false);
    const r = JSON.parse(result.output);
    expect(r.minBytesReached).toBe(true);
    expect(r.timedOut).toBe(false);
    expect(r.output).toContain("hello");
  });

  it("times out and returns whatever is in the log", async () => {
    const bgResult = await executeTool("run_background", {
      command: "sleep 10 && echo never",
    });
    const { logFile } = JSON.parse(bgResult.output);

    const result = await executeTool("wait_for_output", {
      logFile,
      timeoutMs: 300,
      pattern: "never",
    });
    expect(result.isError).toBe(false);
    const r = JSON.parse(result.output);
    expect(r.timedOut).toBe(true);
    expect(r.matched).toBe(false);
  });

  it("pattern fires before minBytes when both given (or semantics)", async () => {
    // Pattern 'ready' appears quickly; minBytes=10000 would take much longer
    const bgResult = await executeTool("run_background", {
      command: "sleep 0.1 && echo ready",
    });
    const { logFile } = JSON.parse(bgResult.output);

    const result = await executeTool("wait_for_output", {
      logFile,
      timeoutMs: 3000,
      pattern: "ready",
      minBytes: 10000,
    });
    expect(result.isError).toBe(false);
    const r = JSON.parse(result.output);
    expect(r.matched).toBe(true);
    expect(r.timedOut).toBe(false);
  });

  it("handles log file that does not exist yet", async () => {
    const nonExistent = "/tmp/omega-test-nonexistent-" + Date.now() + ".log";
    const result = await executeTool("wait_for_output", {
      logFile: nonExistent,
      timeoutMs: 300,
    });
    expect(result.isError).toBe(false);
    const r = JSON.parse(result.output);
    expect(r.timedOut).toBe(true);
    expect(r.output).toBe("");
  });

  it("formatToolCall formats wait_for_output", () => {
    const s = formatToolCall("wait_for_output", {
      logFile: "/tmp/omega-bg-123.log",
      timeoutMs: 5000,
      pattern: "ready",
    });
    expect(s).toBe('wait_for_output: /tmp/omega-bg-123.log (timeout 5000ms) pattern="ready"');
  });

  it("formatToolCall formats wait_for_output with minBytes", () => {
    const s = formatToolCall("wait_for_output", {
      logFile: "/tmp/omega-bg-123.log",
      timeoutMs: 5000,
      minBytes: 100,
    });
    expect(s).toBe("wait_for_output: /tmp/omega-bg-123.log (timeout 5000ms) minBytes=100");
  });
});

// --- executeTool: write_stdin ---

describe("executeTool: write_stdin", () => {
  it("writes a line to a process that reads stdin", async () => {
    const bgResult = await executeTool("run_background", {
      command: "read line; echo got:$line",
    });
    expect(bgResult.isError).toBe(false);
    const { pid, logFile } = JSON.parse(bgResult.output);

    const writeResult = await executeTool("write_stdin", { pid, text: "hello\n" });
    expect(writeResult.isError).toBe(false);
    expect(writeResult.output).toContain(String(pid));

    const waitResult = await executeTool("wait_for_output", {
      logFile,
      timeoutMs: 3000,
      pattern: "got:hello",
    });
    expect(waitResult.isError).toBe(false);
    const r = JSON.parse(waitResult.output);
    expect(r.matched).toBe(true);
  });

  it("closes stdin with end_stdin=true causing cat to exit", async () => {
    const bgResult = await executeTool("run_background", { command: "cat" });
    expect(bgResult.isError).toBe(false);
    const { pid, logFile } = JSON.parse(bgResult.output);

    const writeResult = await executeTool("write_stdin", {
      pid,
      text: "hello world\n",
      end_stdin: true,
    });
    expect(writeResult.isError).toBe(false);
    expect(writeResult.output).toContain("closed stdin");

    // cat exits once stdin is closed (EOF) — wait for its output in the log
    const waitResult = await executeTool("wait_for_output", {
      logFile,
      timeoutMs: 3000,
      pattern: "hello world",
    });
    expect(waitResult.isError).toBe(false);
    const r = JSON.parse(waitResult.output);
    expect(r.matched).toBe(true);
  });

  it("returns error for unknown pid", async () => {
    const result = await executeTool("write_stdin", { pid: 999_999_999, text: "hi\n" });
    expect(result.isError).toBe(true);
    expect(result.output).toContain("999999999");
  });

  it("returns error when stdin is already closed", async () => {
    const bgResult = await executeTool("run_background", { command: "cat" });
    const { pid } = JSON.parse(bgResult.output);

    // Close stdin on first call
    await executeTool("write_stdin", { pid, text: "", end_stdin: true });

    // Second call should fail
    const result = await executeTool("write_stdin", { pid, text: "more\n" });
    expect(result.isError).toBe(true);
    expect(result.output).toMatch(/already closed/i);

    try { process.kill(pid, "SIGKILL"); } catch {}
  });

  it("returns error when pid is missing", async () => {
    const result = await executeTool("write_stdin", { text: "hello\n" });
    expect(result.isError).toBe(true);
    expect(result.output).toContain("pid");
  });

  it("formatToolCall formats write_stdin", () => {
    const s = formatToolCall("write_stdin", { pid: 12345, text: "yes\n" });
    expect(s).toBe("write_stdin: pid 12345 (4 chars)");
  });

  it("formatToolCall formats write_stdin with end_stdin", () => {
    const s = formatToolCall("write_stdin", { pid: 12345, text: "yes\n", end_stdin: true });
    expect(s).toBe("write_stdin: pid 12345 (4 chars) [close stdin]");
  });
});

// ---------------------------------------------------------------------------
// primaryToolArg (shared display helper from tools.schema.ts)
// ---------------------------------------------------------------------------

describe("primaryToolArg", () => {
  it("extracts path for file tools", () => {
    expect(primaryToolArg("read_file", { path: "src/agent.ts" })).toBe("src/agent.ts");
    expect(primaryToolArg("write_file", { path: "out.txt", content: "hi" })).toBe("out.txt");
    expect(primaryToolArg("edit_file", { path: "f.ts", replacements: [{ old_text: "a", new_text: "b" }] })).toBe("f.ts");
  });

  it("extracts path for list_files", () => {
    expect(primaryToolArg("list_files", { path: "src/" })).toBe("src/");
  });

  it("extracts pattern for find_files", () => {
    expect(primaryToolArg("find_files", { pattern: "*.ts", path: "." })).toBe("*.ts");
  });

  it("extracts command for run_command and run_background", () => {
    expect(primaryToolArg("run_command", { command: "ls -la" })).toBe("ls -la");
    expect(primaryToolArg("run_background", { command: "npm start" })).toBe("npm start");
  });

  it("extracts pattern @ path for grep_files", () => {
    expect(primaryToolArg("grep_files", { pattern: "TODO", path: "src/" })).toBe("TODO @ src/");
  });

  it("extracts url for fetch_url", () => {
    expect(primaryToolArg("fetch_url", { url: "https://example.com" })).toBe("https://example.com");
  });

  it("extracts query for web_search", () => {
    expect(primaryToolArg("web_search", { query: "bun test" })).toBe("bun test");
  });

  it("extracts logFile for wait_for_output", () => {
    expect(primaryToolArg("wait_for_output", { logFile: "/tmp/bg.log", timeoutMs: 5000 })).toBe("/tmp/bg.log");
  });

  it("extracts text for write_stdin", () => {
    expect(primaryToolArg("write_stdin", { pid: 123, text: "yes\n" })).toBe("yes\n");
  });

  it("falls back to JSON for unknown tools", () => {
    expect(primaryToolArg("mystery_tool", { x: 1 })).toBe('{"x":1}');
  });

  it("returns (none) for null input", () => {
    expect(primaryToolArg("read_file", null)).toBe("(none)");
  });
});
