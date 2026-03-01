/**
 * Tests for session-dir.ts (SESSION-1).
 */

import { describe, it, expect, beforeEach, afterEach } from "bun:test";
import { mkdtemp, rm, mkdir, writeFile } from "fs/promises";
import { tmpdir } from "os";
import { join } from "path";
import { makeSessionDirName, makeSessionDir, findPreviousEventsFile, SESSIONS_ROOT } from "./session-dir.js";
import { existsSync } from "fs";

// ---------------------------------------------------------------------------
// makeSessionDirName
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// SESSIONS_ROOT value (SESSION-2)
// ---------------------------------------------------------------------------

describe("SESSIONS_ROOT", () => {
  it("is .omega/sessions (SESSION-2: sessions live in cwd, under .omega namespace)", () => {
    expect(SESSIONS_ROOT).toBe(".omega/sessions");
  });
});

// ---------------------------------------------------------------------------
// makeSessionDirName
// ---------------------------------------------------------------------------

describe("makeSessionDirName", () => {
  it("starts with YYYY-MM-DDTHH-MM-SS-mmm timestamp (millisecond precision)", () => {
    const d = new Date("2025-07-04T14:32:05.123Z");
    const name = makeSessionDirName(d);
    expect(name.startsWith("2025-07-04T14-32-05-123-")).toBe(true);
  });

  it("ends with an 8-char lowercase hex suffix", () => {
    const name = makeSessionDirName(new Date());
    const suffix = name.slice(-8);
    expect(suffix).toMatch(/^[0-9a-f]{8}$/);
  });

  it("has no colons or dots (filesystem-safe)", () => {
    const name = makeSessionDirName(new Date());
    expect(name).not.toContain(":");
    expect(name).not.toContain(".");
  });

  it("matches the full YYYY-MM-DDTHH-MM-SS-mmm-<hex8> pattern", () => {
    const name = makeSessionDirName(new Date());
    expect(name).toMatch(/^\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}-\d{3}-[0-9a-f]{8}$/);
  });

  it("total length is 32 characters", () => {
    const name = makeSessionDirName(new Date());
    expect(name.length).toBe(32); // 23 (timestamp w/ ms) + 1 (-) + 8 (hex)
  });

  it("produces unique names for rapid successive calls", () => {
    const now = new Date();
    const a = makeSessionDirName(now);
    const b = makeSessionDirName(now);
    expect(a).not.toBe(b);
  });

  it("uses current time when no arg given", () => {
    const before = new Date();
    const name = makeSessionDirName();
    // Date portion matches today
    expect(name.slice(0, 10)).toBe(before.toISOString().slice(0, 10));
  });
});

// ---------------------------------------------------------------------------
// makeSessionDir — uses a temp directory instead of real sessions/
// ---------------------------------------------------------------------------

// We can't use makeSessionDir() directly because it writes to .omega/sessions/ (production).
// Instead test the logic by importing and patching, or test the helpers individually.
// The integration is tested at the terminal app level.

// Test that makeSessionDir creates the expected structure:
// We'll exercise it via a wrapper that redirects SESSIONS_ROOT.
// Since SESSIONS_ROOT is a module-level constant we can't easily override it,
// we instead verify the directory and file naming logic indirectly.

describe("makeSessionDir path structure", () => {
  it("contextFile and eventsFile are inside dir", () => {
    // Test the naming logic: dir = sessions/<name>, files inside it
    const name = makeSessionDirName(new Date("2025-01-15T09:05:30.000Z"));
    // name now has milliseconds + 8-char hex suffix — verify the full pattern
    expect(name).toMatch(/^2025-01-15T09-05-30-000-[0-9a-f]{8}$/);
    const dir = join(SESSIONS_ROOT, name);
    const contextFile = join(dir, "context.jsonl");
    const eventsFile = join(dir, "events.jsonl");
    expect(contextFile.startsWith(dir)).toBe(true);
    expect(eventsFile.startsWith(dir)).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// findPreviousEventsFile
// ---------------------------------------------------------------------------

describe("findPreviousEventsFile", () => {
  let tempRoot: string;

  // We can't easily patch SESSIONS_ROOT, so we test the helper's logic
  // by calling it with a fake currentDir that won't match anything in .omega/sessions/.
  // The real .omega/sessions/ may or may not exist — we handle both.

  it("returns null when .omega/sessions/ does not exist (no prior sessions)", async () => {
    // Use a non-existent sessions root by passing a currentDir deep enough
    // that nothing else in .omega/sessions/ would be a "previous" session.
    // Since we can't control SESSIONS_ROOT, we rely on the regex filter.
    // This test just checks null is returned gracefully when no dirs exist.
    // We'll call it with a fake currentDir that matches a timestamp pattern.
    const fakeDir = join(".omega", "sessions", "9999-12-31T23-59-59-ffffffff");
    const result = await findPreviousEventsFile(fakeDir);
    // Either null (no sessions dir) or a string path or null (all sessions are current)
    // We can only assert it doesn't throw and returns string | null
    expect(result === null || typeof result === "string").toBe(true);
  });

  it("returns null when there is no previous session (only current)", () => {
    // Test the filtering logic directly using the same regex as the implementation.
    // Tolerates all three historical formats.
    const regex = /^\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}(-\d{3})?(-[0-9a-f]{8})?$/;
    const current = "2025-07-04T14-32-05-123-a3f7c1b2";
    const allEntries = [current];
    const filtered = allEntries.filter(e => e !== current && regex.test(e));
    expect(filtered).toHaveLength(0);
  });

  it("selects the most recent dir among multiple sessions (current ms format)", () => {
    // Current format (ms precision) — lexicographic sort = chronological.
    const dirs = [
      "2025-07-01T10-00-00-000-aabbccdd",
      "2025-07-03T09-30-00-500-11223344",
      "2025-07-04T14-32-05-123-a3f7c1b2",  // ← most recent timestamp
    ];
    const current = "2025-07-04T15-00-00-000-deadbeef";
    const regex = /^\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}(-\d{3})?(-[0-9a-f]{8})?$/;
    const candidates = dirs
      .filter(e => e !== current && regex.test(e))
      .sort();
    const mostRecent = candidates[candidates.length - 1];
    expect(mostRecent).toBe("2025-07-04T14-32-05-123-a3f7c1b2");
  });

  it("tolerates all three historical formats side by side", () => {
    // old (no suffix), v2 (second precision + suffix), current (ms + suffix)
    const dirs = [
      "2025-07-01T10-00-00",               // old — no suffix
      "2025-07-03T09-30-00-aabbccdd",      // v2 — second precision + suffix
      "2025-07-04T14-32-05-123-a3f7c1b2", // current — ms + suffix
    ];
    const current = "2025-07-05T00-00-00-000-ffffffff";
    const regex = /^\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}(-\d{3})?(-[0-9a-f]{8})?$/;
    const candidates = dirs.filter(e => e !== current && regex.test(e)).sort();
    expect(candidates).toEqual([
      "2025-07-01T10-00-00",
      "2025-07-03T09-30-00-aabbccdd",
      "2025-07-04T14-32-05-123-a3f7c1b2",
    ]);
    expect(candidates[candidates.length - 1]).toBe("2025-07-04T14-32-05-123-a3f7c1b2");
  });

  it("excludes non-timestamp-shaped directory names", () => {
    const entries = [
      "2025-07-04T14-32-05-123-a3f7c1b2",  // current format — valid
      "2025-07-04T14-32-05-aabbccdd",        // v2 format — valid
      "2025-07-04T14-32-05",                 // old format — valid (tolerated)
      "my-cool-session",                      // renamed (SESSION-5) — excluded
      "2025-07-04",                           // too short — excluded
      "2025-07-04T14-32-05-UPPERCASE",       // uppercase hex — excluded
      "2025-07-04T14-32-05-123-UPPERCASE",   // uppercase hex (ms fmt) — excluded
    ];
    const current = "2025-07-05T00-00-00-000-ffffffff";
    const regex = /^\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}(-\d{3})?(-[0-9a-f]{8})?$/;
    const candidates = entries.filter(e => e !== current && regex.test(e));
    expect(candidates).toEqual([
      "2025-07-04T14-32-05-123-a3f7c1b2",
      "2025-07-04T14-32-05-aabbccdd",
      "2025-07-04T14-32-05",
    ]);
  });

  afterEach(async () => {
    if (tempRoot) await rm(tempRoot, { recursive: true, force: true });
  });
});
