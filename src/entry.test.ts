import { describe, it, expect } from "bun:test";
import { readFileSync } from "fs";
import { join } from "path";

/**
 * Smoke tests that catch broken entry points after refactors.
 * These verify that package.json scripts point to files that exist
 * and that entry-point files actually invoke their main function.
 */

const ROOT = join(import.meta.dir, "..");

describe("entry points", () => {
  it("package.json start script points to a file that exists", () => {
    const pkg = JSON.parse(readFileSync(join(ROOT, "package.json"), "utf-8"));
    const startScript = pkg.scripts?.start;
    expect(startScript).toBeDefined();

    // Extract filename from "bun run src/foo.ts"
    const match = startScript.match(/bun run (\S+)/);
    expect(match).not.toBeNull();

    const entryFile = join(ROOT, match![1]);
    expect(() => readFileSync(entryFile)).not.toThrow();
  });

  it("start entry file invokes runApp() at top level or under import.meta.main guard", () => {
    const pkg = JSON.parse(readFileSync(join(ROOT, "package.json"), "utf-8"));
    const match = pkg.scripts.start.match(/bun run (\S+)/);
    const source = readFileSync(join(ROOT, match![1]), "utf-8");

    // Must have a top-level call (not just export).
    // Accept bare `runApp()` at start of a line, or guarded by import.meta.main.
    const hasBareCall   = /^runApp\(/m.test(source);
    const hasGuardedCall = /import\.meta\.main/.test(source) && source.includes("runApp(");
    expect(hasBareCall || hasGuardedCall).toBe(true);
  });

  it("package.json login script points to a file that exists", () => {
    const pkg = JSON.parse(readFileSync(join(ROOT, "package.json"), "utf-8"));
    const loginScript = pkg.scripts?.login;
    if (!loginScript) return; // optional

    const match = loginScript.match(/bun run (\S+)/);
    expect(match).not.toBeNull();

    const entryFile = join(ROOT, match![1]);
    expect(() => readFileSync(entryFile)).not.toThrow();
  });
});
