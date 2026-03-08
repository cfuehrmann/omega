/**
 * Tests for the src/system-prompt/ module.
 *
 * Three layers tested:
 *  1. core.ts     — corePrompt() interpolates args; contains required sections
 *  2. append.ts   — file I/O (read/write) and formatAppendSection()
 *  3. index.ts    — buildSystemPrompt() assembles all parts correctly
 */

import { describe, it, expect } from "bun:test";
import { mkdtemp, rm } from "fs/promises";
import { tmpdir } from "os";
import { join } from "path";

import { corePrompt } from "./core.js";
import {
  readSystemPromptAppend,
  writeSystemPromptAppend,
  systemPromptAppendPath,
  formatAppendSection,
  APPEND_SECTION_HEADER,
} from "./append.js";
import { buildSystemPrompt } from "./index.js";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async function withTempDir<T>(fn: (dir: string) => Promise<T>): Promise<T> {
  const dir = await mkdtemp(join(tmpdir(), "omega-sysprompt-test-"));
  try {
    return await fn(dir);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
}

// ---------------------------------------------------------------------------
// 1. core.ts
// ---------------------------------------------------------------------------

describe("corePrompt", () => {
  const prompt = corePrompt({ cwd: "/my/project", maxOutputTokens: 8000 });

  it("interpolates cwd into the prompt", () => {
    expect(prompt).toContain("/my/project");
  });

  it("interpolates maxOutputTokens into the prompt", () => {
    expect(prompt).toContain("8000");
  });

  it("identifies the agent as Omega", () => {
    expect(prompt).toContain("You are Omega");
  });

  it("mentions README for project orientation", () => {
    expect(prompt).toContain("README");
  });

  it("does not reference past.md", () => {
    expect(prompt).not.toContain("past.md");
  });

  it("does not reference present.md", () => {
    expect(prompt).not.toContain("present.md");
  });

  it("different cwd values produce different prompts", () => {
    const a = corePrompt({ cwd: "/proj-a", maxOutputTokens: 1000 });
    const b = corePrompt({ cwd: "/proj-b", maxOutputTokens: 1000 });
    expect(a).not.toBe(b);
    expect(a).toContain("/proj-a");
    expect(b).toContain("/proj-b");
  });

  it("different maxOutputTokens values produce different prompts", () => {
    const a = corePrompt({ cwd: "/proj", maxOutputTokens: 1000 });
    const b = corePrompt({ cwd: "/proj", maxOutputTokens: 99999 });
    expect(a).toContain("1000");
    expect(b).toContain("99999");
  });

  it("is a non-empty string", () => {
    expect(prompt.length).toBeGreaterThan(100);
  });

  it("does not start or end with extra blank lines", () => {
    expect(prompt.startsWith("\n")).toBe(false);
    expect(prompt.endsWith("\n")).toBe(false);
  });

  it("does not contain the Claude Code identity string", () => {
    expect(prompt).not.toContain("You are Claude Code");
  });
});

// ---------------------------------------------------------------------------
// 2. append.ts — file I/O
// ---------------------------------------------------------------------------

describe("systemPromptAppendPath", () => {
  it("returns path under .omega/ in the given cwd", () => {
    const p = systemPromptAppendPath("/some/project");
    expect(p).toBe("/some/project/.omega/system-prompt-append.md");
  });

  it("uses process.cwd() when no argument given", () => {
    const p = systemPromptAppendPath();
    expect(p).toContain(".omega/system-prompt-append.md");
  });
});

describe("readSystemPromptAppend", () => {
  it("returns null when file does not exist", async () => {
    await withTempDir(async (dir) => {
      const result = await readSystemPromptAppend(join(dir, "nonexistent.md"));
      expect(result).toBeNull();
    });
  });

  it("returns file content when file exists", async () => {
    await withTempDir(async (dir) => {
      const path = join(dir, "append.md");
      await writeSystemPromptAppend("Hello content.", path);
      const result = await readSystemPromptAppend(path);
      expect(result).toBe("Hello content.");
    });
  });
});

describe("writeSystemPromptAppend", () => {
  it("writes content to file", async () => {
    await withTempDir(async (dir) => {
      const path = join(dir, "append.md");
      await writeSystemPromptAppend("State: all good.", path);
      expect(await readSystemPromptAppend(path)).toBe("State: all good.");
    });
  });

  it("overwrites existing content", async () => {
    await withTempDir(async (dir) => {
      const path = join(dir, "append.md");
      await writeSystemPromptAppend("Old.", path);
      await writeSystemPromptAppend("New.", path);
      expect(await readSystemPromptAppend(path)).toBe("New.");
    });
  });

  it("creates parent directories if needed", async () => {
    await withTempDir(async (dir) => {
      const path = join(dir, "nested", "deep", "append.md");
      await writeSystemPromptAppend("Deep content.", path);
      expect(await readSystemPromptAppend(path)).toBe("Deep content.");
    });
  });
});

describe("formatAppendSection", () => {
  it("returns null when content is null", () => {
    expect(formatAppendSection(null)).toBeNull();
  });

  it("returns a string starting with the section header when content is given", () => {
    const result = formatAppendSection("some content");
    expect(result).not.toBeNull();
    expect(result!.startsWith(APPEND_SECTION_HEADER)).toBe(true);
  });

  it("includes the content after the header", () => {
    const result = formatAppendSection("my state text");
    expect(result).toContain("my state text");
  });

  it("separates header from content with a blank line", () => {
    const result = formatAppendSection("body");
    expect(result).toContain(`${APPEND_SECTION_HEADER}\n\nbody`);
  });
});

// ---------------------------------------------------------------------------
// 3. index.ts — buildSystemPrompt assembly
// ---------------------------------------------------------------------------

describe("buildSystemPrompt", () => {
  const base = {
    cwd: "/test/project",
    maxOutputTokens: 32768,
  };

  it("does not contain the Claude Code identity string", () => {
    const prompt = buildSystemPrompt({ ...base, appendContent: null });
    expect(prompt).not.toContain("You are Claude Code");
  });

  it("starts with the core prompt (You are Omega)", () => {
    const prompt = buildSystemPrompt({ ...base, appendContent: null });
    expect(prompt.startsWith("You are Omega")).toBe(true);
  });

  it("contains the core prompt content", () => {
    const prompt = buildSystemPrompt({ ...base, appendContent: null });
    expect(prompt).toContain("You are Omega");
    expect(prompt).toContain("/test/project");
  });

  it("no append section when appendContent is null", () => {
    const prompt = buildSystemPrompt({ ...base, appendContent: null });
    expect(prompt).not.toContain(APPEND_SECTION_HEADER);
  });

  it("includes append section when appendContent is provided", () => {
    const prompt = buildSystemPrompt({ ...base, appendContent: "my state" });
    expect(prompt).toContain(APPEND_SECTION_HEADER);
    expect(prompt).toContain("my state");
  });

  it("append section appears after the core prompt", () => {
    const prompt = buildSystemPrompt({ ...base, appendContent: "APPENDED" });
    expect(prompt.indexOf("You are Omega")).toBeLessThan(prompt.indexOf("APPENDED"));
  });

  it("core and append are separated by double newlines", () => {
    const prompt = buildSystemPrompt({ ...base, appendContent: "STATE" });
    expect(prompt).toContain(`\n\n${APPEND_SECTION_HEADER}`);
  });

  it("is stable: same args produce identical string", () => {
    const args = { ...base, appendContent: "stable" };
    expect(buildSystemPrompt(args)).toBe(buildSystemPrompt(args));
  });

  it("cwd is reflected in the output", () => {
    const prompt = buildSystemPrompt({ ...base, cwd: "/custom/path", appendContent: null });
    expect(prompt).toContain("/custom/path");
  });

  it("maxOutputTokens is reflected in the output", () => {
    const prompt = buildSystemPrompt({ ...base, maxOutputTokens: 12345, appendContent: null });
    expect(prompt).toContain("12345");
  });
});
