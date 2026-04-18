/**
 * Unit tests for listFilesForCompletion (serves GET /files?prefix=...).
 *
 * Covers:
 *  - Empty prefix → lists cwd root (dirs first, then files, alphabetically)
 *  - Prefix ending with "/" → lists that directory
 *  - Partial name → filters to entries starting with that string
 *  - Non-existent directory → returns []
 *  - Directories are sorted before files within the same level
 *  - Results capped at 50
 *  - Directories suffixed with "/"
 *  - Absolute path prefix → resolves from filesystem root, not cwd
 */

import { describe, it, expect, beforeEach, afterEach } from "bun:test";
import { mkdtemp, rm, mkdir, writeFile } from "fs/promises";
import { tmpdir } from "os";
import { join } from "path";

import { listFilesForCompletion } from "./server.js";

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

let tmp: string;

beforeEach(async () => {
  tmp = await mkdtemp(join(tmpdir(), "omega-fc-test-"));
  // Layout:
  //   dira/
  //     nested.txt
  //   dirb/
  //   alpha.txt
  //   beta.txt
  await mkdir(join(tmp, "dira"));
  await mkdir(join(tmp, "dirb"));
  await writeFile(join(tmp, "dira", "nested.txt"), "");
  await writeFile(join(tmp, "alpha.txt"), "");
  await writeFile(join(tmp, "beta.txt"), "");
});

afterEach(async () => {
  await rm(tmp, { recursive: true, force: true });
});

// ---------------------------------------------------------------------------
// Empty prefix — lists root
// ---------------------------------------------------------------------------

describe("listFilesForCompletion — empty prefix", () => {
  it("lists all entries in cwd with no filter", async () => {
    const result = await listFilesForCompletion("", tmp);
    expect(result).toContain("dira/");
    expect(result).toContain("dirb/");
    expect(result).toContain("alpha.txt");
    expect(result).toContain("beta.txt");
  });

  it("sorts directories before files", async () => {
    const result = await listFilesForCompletion("", tmp);
    const dirIndices  = result.filter(r => r.endsWith("/")).map(r => result.indexOf(r));
    const fileIndices = result.filter(r => !r.endsWith("/")).map(r => result.indexOf(r));
    expect(Math.max(...dirIndices)).toBeLessThan(Math.min(...fileIndices));
  });

  it("suffixes directories with /", async () => {
    const result = await listFilesForCompletion("", tmp);
    expect(result.filter(r => r.endsWith("/"))).toEqual(["dira/", "dirb/"]);
  });

  it("sorts alphabetically within dirs group and files group", async () => {
    const result = await listFilesForCompletion("", tmp);
    const dirs  = result.filter(r => r.endsWith("/"));
    const files = result.filter(r => !r.endsWith("/"));
    expect(dirs).toEqual(["dira/", "dirb/"]);
    expect(files).toEqual(["alpha.txt", "beta.txt"]);
  });
});

// ---------------------------------------------------------------------------
// Prefix ending with "/" — list a subdirectory
// ---------------------------------------------------------------------------

describe("listFilesForCompletion — directory prefix", () => {
  it("lists contents of the specified subdirectory", async () => {
    const result = await listFilesForCompletion("dira/", tmp);
    expect(result).toEqual(["dira/nested.txt"]);
  });

  it("preserves the dir prefix in returned paths", async () => {
    const result = await listFilesForCompletion("dira/", tmp);
    // Every returned path starts with "dira/"
    expect(result.every(r => r.startsWith("dira/"))).toBe(true);
  });

  it("returns [] for an empty directory", async () => {
    const result = await listFilesForCompletion("dirb/", tmp);
    expect(result).toEqual([]);
  });
});

// ---------------------------------------------------------------------------
// Partial name — filters
// ---------------------------------------------------------------------------

describe("listFilesForCompletion — partial name filter", () => {
  it("returns only entries whose name starts with the filter string", async () => {
    const result = await listFilesForCompletion("dir", tmp);
    expect(result).toContain("dira/");
    expect(result).toContain("dirb/");
    expect(result).not.toContain("alpha.txt");
    expect(result).not.toContain("beta.txt");
  });

  it("is case-sensitive", async () => {
    const result = await listFilesForCompletion("Dir", tmp);
    expect(result).toEqual([]);
  });

  it("returns [] when no entries match the filter", async () => {
    const result = await listFilesForCompletion("zzz", tmp);
    expect(result).toEqual([]);
  });

  it("filters within a subdirectory", async () => {
    await writeFile(join(tmp, "dira", "other.ts"), "");
    const result = await listFilesForCompletion("dira/ne", tmp);
    expect(result).toEqual(["dira/nested.txt"]);
  });
});

// ---------------------------------------------------------------------------
// Non-existent directory
// ---------------------------------------------------------------------------

describe("listFilesForCompletion — non-existent directory", () => {
  it("returns [] without throwing when the directory does not exist", async () => {
    const result = await listFilesForCompletion("no-such-dir/", tmp);
    expect(result).toEqual([]);
  });

  it("returns [] for a deeply nested non-existent path", async () => {
    const result = await listFilesForCompletion("a/b/c/d/", tmp);
    expect(result).toEqual([]);
  });
});

// ---------------------------------------------------------------------------
// 50-item cap
// ---------------------------------------------------------------------------

describe("listFilesForCompletion — result cap", () => {
  it("returns at most 50 entries", async () => {
    const capDir = join(tmp, "many");
    await mkdir(capDir);
    for (let i = 0; i < 60; i++) {
      await writeFile(join(capDir, `file${String(i).padStart(2, "0")}.txt`), "");
    }
    const result = await listFilesForCompletion("many/", tmp);
    expect(result.length).toBe(50);
  });
});

// ---------------------------------------------------------------------------
// Absolute path prefix
// ---------------------------------------------------------------------------

describe("listFilesForCompletion — absolute path prefix", () => {
  it("resolves absolute prefix from filesystem root, not cwd", async () => {
    // tmp is an absolute path; querying it directly should work regardless of cwd
    const prefix = tmp + "/";
    const result = await listFilesForCompletion(prefix, "/completely/different/cwd");
    // Should list our known entries under tmp
    expect(result).toContain(tmp + "/dira/");
    expect(result).toContain(tmp + "/alpha.txt");
  });

  it("preserves the absolute prefix in returned paths", async () => {
    const prefix = tmp + "/";
    const result = await listFilesForCompletion(prefix, "/other");
    expect(result.every(r => r.startsWith(tmp + "/"))).toBe(true);
  });

  it("filters by name even with an absolute prefix", async () => {
    const prefix = tmp + "/al";
    const result = await listFilesForCompletion(prefix, "/other");
    expect(result).toEqual([tmp + "/alpha.txt"]);
  });
});
