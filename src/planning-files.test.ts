import { describe, it, expect } from "bun:test";
import { existsSync, readFileSync } from "fs";
import { join } from "path";
import { config } from "./config";

/**
 * Structural invariant tests for the planning file system.
 *
 * Rules:
 *  - future.md must exist (the issue tracker we keep).
 *  - past.md and present.md must NOT exist (deleted — redundant with world-state.md).
 *  - The system prompt must tell the agent to read README.md for orientation.
 *  - The system prompt must mention world-state.md and future.md (generically).
 *  - README.md must reference world-state.md and future.md.
 *  - The system prompt must NOT reference past.md or present.md.
 */

const ROOT = join(import.meta.dir, "..");
const readme = readFileSync(join(ROOT, "README.md"), "utf-8");

describe("planning files", () => {
  it("future.md exists", () => {
    expect(existsSync(join(ROOT, "plan/future.md"))).toBe(true);
  });

  it("past.md does not exist (redundant with world-state.md)", () => {
    expect(existsSync(join(ROOT, "plan/past.md"))).toBe(false);
  });

  it("present.md does not exist (near-zero value)", () => {
    expect(existsSync(join(ROOT, "plan/present.md"))).toBe(false);
  });

  it("README.md exists", () => {
    expect(existsSync(join(ROOT, "README.md"))).toBe(true);
  });

  it("system prompt tells agent to read README.md", () => {
    expect(config.systemPrompt).toContain("README.md");
  });

  it("system prompt mentions world-state.md", () => {
    expect(config.systemPrompt).toContain("world-state.md");
  });

  it("system prompt mentions future.md", () => {
    expect(config.systemPrompt).toContain("future.md");
  });

  it("README.md references world-state.md", () => {
    expect(readme).toContain("world-state.md");
  });

  it("README.md references future.md", () => {
    expect(readme).toContain("future.md");
  });

  it("README.md references manifest.md", () => {
    expect(readme).toContain("manifest.md");
  });

  it("system prompt does not reference past.md", () => {
    expect(config.systemPrompt).not.toContain("past.md");
  });

  it("system prompt does not reference present.md", () => {
    expect(config.systemPrompt).not.toContain("present.md");
  });
});
