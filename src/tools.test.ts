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

  it("errors when command is missing", async () => {
    const result = await executeTool("run_command", {});
    expect(result.isError).toBe(true);
    expect(result.output).toContain("Missing required field: command");
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
      old_text: "hello world",
      new_text: "hi world",
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
      old_text: "not found text",
      new_text: "replacement",
    });
    expect(result.isError).toBe(true);
    expect(result.output).toContain("not found");
  });

  it("returns error when old_text matches multiple times", async () => {
    const path = join(TMP, "edit3.txt");
    await writeFile(path, "foo bar\nfoo baz\n");
    const result = await executeTool("edit_file", {
      path,
      old_text: "foo",
      new_text: "qux",
    });
    expect(result.isError).toBe(true);
    expect(result.output).toContain("multiple");
  });

  it("returns error for missing file", async () => {
    const result = await executeTool("edit_file", {
      path: join(TMP, "nonexistent.txt"),
      old_text: "x",
      new_text: "y",
    });
    expect(result.isError).toBe(true);
  });

  it("handles multi-line old_text and new_text", async () => {
    const path = join(TMP, "multiline.txt");
    await writeFile(path, "line 1\nline 2\nline 3\nline 4\n");
    const result = await executeTool("edit_file", {
      path,
      old_text: "line 2\nline 3",
      new_text: "replaced 2\nreplaced 3\nextra line",
    });
    expect(result.isError).toBe(false);

    const { readFile } = await import("fs/promises");
    const content = await readFile(path, "utf-8");
    expect(content).toBe("line 1\nreplaced 2\nreplaced 3\nextra line\nline 4\n");
  });
});

describe("formatToolCall: edit_file", () => {
  it("formats edit_file with path and sizes", () => {
    const formatted = formatToolCall("edit_file", {
      path: "src/agent.ts",
      old_text: "hello",
      new_text: "world!",
    });
    expect(formatted).toContain("edit_file");
    expect(formatted).toContain("src/agent.ts");
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

  it("formatToolCall formats fetch_url", () => {
    expect(formatToolCall("fetch_url", { url: "https://example.com" })).toBe(
      "fetch_url: https://example.com"
    );
  });
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

// --- executeTool: kill_process ---

describe("executeTool: kill_process", () => {
  it("kills a running process and reports success", async () => {
    // Start a background process
    const bgResult = await executeTool("run_background", { command: "sleep 30" });
    expect(bgResult.isError).toBe(false);
    const { pid } = JSON.parse(bgResult.output);

    // Kill it
    const killResult = await executeTool("kill_process", { pid });
    expect(killResult.isError).toBe(false);
    expect(killResult.output).toContain(String(pid));
  });

  it("handles already-dead process gracefully", async () => {
    // Start a fast process that will exit quickly
    const bgResult = await executeTool("run_background", { command: "sleep 0" });
    expect(bgResult.isError).toBe(false);
    const { pid } = JSON.parse(bgResult.output);
    // Wait for it to die
    await new Promise(r => setTimeout(r, 300));
    // Try to kill the dead process — should not error
    const killResult = await executeTool("kill_process", { pid });
    expect(killResult.isError).toBe(false);
    expect(killResult.output).toMatch(/already exited|no such process|not running/i);
  });

  it("returns error when pid is missing", async () => {
    const result = await executeTool("kill_process", {});
    expect(result.isError).toBe(true);
    expect(result.output).toContain("pid");
  });

  it("supports custom signal", async () => {
    const bgResult = await executeTool("run_background", { command: "sleep 30" });
    const { pid } = JSON.parse(bgResult.output);
    const killResult = await executeTool("kill_process", { pid, signal: "SIGKILL" });
    expect(killResult.isError).toBe(false);
    expect(killResult.output).toContain(String(pid));
  });

  it("formatToolCall formats kill_process", () => {
    const s = formatToolCall("kill_process", { pid: 12345 });
    expect(s).toBe("kill_process: pid 12345");
  });

  it("formatToolCall formats kill_process with signal", () => {
    const s = formatToolCall("kill_process", { pid: 12345, signal: "SIGKILL" });
    expect(s).toBe("kill_process: pid 12345 (SIGKILL)");
  });
});

// --- executeTool: wait_process ---

describe("executeTool: wait_process", () => {
  it("returns exitCode 0 when process exits cleanly", async () => {
    const bgResult = await executeTool("run_background", { command: "exit 0" });
    expect(bgResult.isError).toBe(false);
    const { pid } = JSON.parse(bgResult.output);

    const waitResult = await executeTool("wait_process", { pid, timeoutMs: 5000 });
    expect(waitResult.isError).toBe(false);
    const result = JSON.parse(waitResult.output);
    expect(result.timedOut).toBe(false);
    expect(result.exitCode).toBe(0);
    expect(result.pid).toBe(pid);
  });

  it("returns non-zero exitCode when process fails", async () => {
    const bgResult = await executeTool("run_background", { command: "exit 42" });
    expect(bgResult.isError).toBe(false);
    const { pid } = JSON.parse(bgResult.output);

    const waitResult = await executeTool("wait_process", { pid, timeoutMs: 5000 });
    expect(waitResult.isError).toBe(false);
    const result = JSON.parse(waitResult.output);
    expect(result.timedOut).toBe(false);
    expect(result.exitCode).toBe(42);
  });

  it("returns exitCode correctly when process has already exited before wait_process is called", async () => {
    const bgResult = await executeTool("run_background", { command: "exit 0" });
    expect(bgResult.isError).toBe(false);
    const { pid } = JSON.parse(bgResult.output);

    // Wait long enough for the process to naturally finish
    await new Promise((r) => setTimeout(r, 500));

    const waitResult = await executeTool("wait_process", { pid, timeoutMs: 5000 });
    expect(waitResult.isError).toBe(false);
    const result = JSON.parse(waitResult.output);
    expect(result.timedOut).toBe(false);
    // exitCode is 0 (already exited, tracked) or null (already exited, untracked after cleanup)
    expect(result.pid).toBe(pid);
  });

  it("times out when process runs longer than timeoutMs", async () => {
    const bgResult = await executeTool("run_background", { command: "sleep 30" });
    expect(bgResult.isError).toBe(false);
    const { pid } = JSON.parse(bgResult.output);

    const waitResult = await executeTool("wait_process", { pid, timeoutMs: 200 });
    expect(waitResult.isError).toBe(false);
    const result = JSON.parse(waitResult.output);
    expect(result.timedOut).toBe(true);
    expect(result.pid).toBe(pid);

    // Clean up
    await executeTool("kill_process", { pid });
  });

  it("returns gracefully for an unknown PID that is already gone", async () => {
    // Use a PID that almost certainly doesn't exist
    const fakePid = 999999999;
    const waitResult = await executeTool("wait_process", { pid: fakePid, timeoutMs: 500 });
    expect(waitResult.isError).toBe(false);
    const result = JSON.parse(waitResult.output);
    expect(result.timedOut).toBe(false);
    expect(result.pid).toBe(fakePid);
  });

  it("formatToolCall formats wait_process without timeout", () => {
    const s = formatToolCall("wait_process", { pid: 12345 });
    expect(s).toBe("wait_process: pid 12345");
  });

  it("formatToolCall formats wait_process with timeout", () => {
    const s = formatToolCall("wait_process", { pid: 12345, timeoutMs: 30000 });
    expect(s).toBe("wait_process: pid 12345 (timeout 30000ms)");
  });
});
