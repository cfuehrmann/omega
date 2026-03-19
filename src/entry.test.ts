import { describe, it, expect } from "bun:test";
import { readFileSync, existsSync } from "fs";
import { join } from "path";

/**
 * Smoke tests that catch broken entry points after refactors.
 * These verify that package.json scripts point to files that exist
 * and that entry-point files actually export their main function.
 */

const ROOT = join(import.meta.dir, "..");

describe("entry points", () => {
  it("package.json start script points to a file that exists", () => {
    const pkg = JSON.parse(readFileSync(join(ROOT, "package.json"), "utf-8"));
    const startScript = pkg.scripts?.start;
    expect(startScript).toBeDefined();

    const match = startScript.match(/bun run (\S+)/);
    expect(match).not.toBeNull();

    const entryFile = join(ROOT, match![1]);
    expect(() => readFileSync(entryFile)).not.toThrow();
  });

  it("web split: src/web/server.ts and src/web/client/index.html exist", () => {
    expect(existsSync(join(ROOT, "src/web/server.ts"))).toBe(true);
    expect(existsSync(join(ROOT, "src/web/client/index.html"))).toBe(true);
  });

  it("web split: src/web/server.ts exports runWebApp", () => {
    const source = readFileSync(join(ROOT, "src/web/server.ts"), "utf-8");
    expect(source).toContain("runWebApp");
  });
});
