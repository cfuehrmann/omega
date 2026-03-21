import { describe, it, expect } from "bun:test";
import { existsSync, readFileSync } from "fs";
import { join } from "path";
import { corePrompt } from "./system-prompt/core.js";

/**
 * Structural invariant tests for the planning file system.
 *
 * These are Omega-on-itself self-tests: they verify invariants that hold
 * specifically when Omega is running against its own repo. They do not
 * imply that any project Omega works on must follow this structure.
 *
 * Rules:
 *  - backlog.md must exist (the issue tracker we keep).
 *  - past.md and present.md must NOT exist (deleted — redundant with the
 *    system-prompt-append file).
 *  - README.md must exist and reference the backlog.md and docs/ folders.
 *  - The core prompt must NOT reference past.md or present.md.
 */

const ROOT = join(import.meta.dir, "..");
const readme = readFileSync(join(ROOT, "README.md"), "utf-8");
// Instantiate the core prompt with representative values for invariant checks.
const corePromptText = corePrompt({ cwd: "/test/cwd", maxOutputTokens: 32768 });

describe("planning files", () => {
  it("backlog.md exists", () => {
    expect(existsSync(join(ROOT, "backlog.md"))).toBe(true);
  });

  it("past.md does not exist (redundant with system-prompt-append)", () => {
    expect(existsSync(join(ROOT, "past.md"))).toBe(false);
  });

  it("present.md does not exist (near-zero value)", () => {
    expect(existsSync(join(ROOT, "present.md"))).toBe(false);
  });

  it("README.md exists", () => {
    expect(existsSync(join(ROOT, "README.md"))).toBe(true);
  });

  it("README.md references backlog.md", () => {
    expect(readme).toContain("backlog.md");
  });

  it("README.md references docs/ folder", () => {
    expect(readme).toContain("docs/");
  });

  it("core prompt does not reference past.md", () => {
    expect(corePromptText).not.toContain("past.md");
  });

  it("core prompt does not reference present.md", () => {
    expect(corePromptText).not.toContain("present.md");
  });
});
