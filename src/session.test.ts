/**
 * Tests for session persistence (src/session.ts)
 *
 * Red-green: these tests are written BEFORE the implementation.
 * They must fail first, then pass once session.ts is implemented.
 */

import { describe, it, expect, beforeEach, afterEach } from "bun:test";
import { mkdirSync, rmSync, existsSync, readdirSync } from "fs";
import { join } from "path";
import { tmpdir } from "os";
import type { MessageParam } from "@anthropic-ai/sdk/resources/messages";

// We import the module under test. It doesn't exist yet — tests will fail.
import {
  saveSession,
  loadLatestSession,
  type Session,
} from "./session.js";

// Use a temporary directory so tests never touch real session storage
const TEST_DIR = join(tmpdir(), `omega-session-test-${Date.now()}`);

// Override the session directory for tests by passing it explicitly
// (the module accepts an optional dir parameter on each function)

beforeEach(() => {
  mkdirSync(TEST_DIR, { recursive: true });
});

afterEach(() => {
  rmSync(TEST_DIR, { recursive: true, force: true });
});

// --- saveSession ---

describe("saveSession", () => {
  it("writes a JSON file to the given directory", async () => {
    const session: Session = {
      id: "test-session-1",
      savedAt: new Date().toISOString(),
      model: "claude-sonnet-4-6",
      history: [
        { role: "user", content: "hello" },
        { role: "assistant", content: "hi there" },
      ],
    };

    await saveSession(session, TEST_DIR);

    const files = readdirSync(TEST_DIR);
    expect(files.length).toBe(1);
    expect(files[0]).toEndWith(".json");
  });

  it("filename contains the session id", async () => {
    const session: Session = {
      id: "my-unique-id",
      savedAt: new Date().toISOString(),
      model: "claude-sonnet-4-6",
      history: [],
    };

    await saveSession(session, TEST_DIR);

    const files = readdirSync(TEST_DIR);
    expect(files[0]).toContain("my-unique-id");
  });

  it("written file is valid JSON containing the history", async () => {
    const history: MessageParam[] = [
      { role: "user", content: "what is 2+2?" },
      { role: "assistant", content: "4" },
    ];
    const session: Session = {
      id: "json-test",
      savedAt: new Date().toISOString(),
      model: "claude-sonnet-4-6",
      history,
    };

    await saveSession(session, TEST_DIR);

    const files = readdirSync(TEST_DIR);
    const raw = await Bun.file(join(TEST_DIR, files[0])).text();
    const parsed = JSON.parse(raw);
    expect(parsed.id).toBe("json-test");
    expect(parsed.history).toEqual(history);
    expect(parsed.model).toBe("claude-sonnet-4-6");
    expect(typeof parsed.savedAt).toBe("string");
  });

  it("overwrites the file on repeated saves with same id", async () => {
    const session: Session = {
      id: "overwrite-test",
      savedAt: new Date().toISOString(),
      model: "claude-sonnet-4-6",
      history: [{ role: "user", content: "first" }],
    };

    await saveSession(session, TEST_DIR);

    const updated: Session = {
      ...session,
      history: [
        { role: "user", content: "first" },
        { role: "assistant", content: "response" },
        { role: "user", content: "second" },
      ],
    };

    await saveSession(updated, TEST_DIR);

    const files = readdirSync(TEST_DIR);
    // Should still be one file (overwritten, not duplicated)
    expect(files.length).toBe(1);

    const raw = await Bun.file(join(TEST_DIR, files[0])).text();
    const parsed = JSON.parse(raw);
    expect(parsed.history.length).toBe(3);
  });
});

// --- loadLatestSession ---

describe("loadLatestSession", () => {
  it("returns null when directory is empty", async () => {
    const result = await loadLatestSession(TEST_DIR);
    expect(result).toBeNull();
  });

  it("returns null when directory does not exist", async () => {
    const result = await loadLatestSession(join(TEST_DIR, "nonexistent"));
    expect(result).toBeNull();
  });

  it("returns the session when one exists", async () => {
    const session: Session = {
      id: "solo-session",
      savedAt: new Date().toISOString(),
      model: "claude-sonnet-4-6",
      history: [{ role: "user", content: "hello" }],
    };
    await saveSession(session, TEST_DIR);

    const loaded = await loadLatestSession(TEST_DIR);
    expect(loaded).not.toBeNull();
    expect(loaded!.id).toBe("solo-session");
    expect(loaded!.history).toEqual(session.history);
  });

  it("returns the most recently saved session when multiple exist", async () => {
    const older: Session = {
      id: "older-session",
      savedAt: new Date(Date.now() - 10_000).toISOString(),
      model: "claude-sonnet-4-6",
      history: [{ role: "user", content: "older" }],
    };
    const newer: Session = {
      id: "newer-session",
      savedAt: new Date().toISOString(),
      model: "claude-sonnet-4-6",
      history: [{ role: "user", content: "newer" }],
    };

    await saveSession(older, TEST_DIR);
    // Small delay so file mtimes differ if needed
    await Bun.sleep(10);
    await saveSession(newer, TEST_DIR);

    const loaded = await loadLatestSession(TEST_DIR);
    expect(loaded!.id).toBe("newer-session");
  });

  it("returns a session with the correct shape", async () => {
    const session: Session = {
      id: "shape-test",
      savedAt: "2025-01-01T00:00:00.000Z",
      model: "claude-sonnet-4-6",
      history: [
        { role: "user", content: "hi" },
        { role: "assistant", content: "hello" },
      ],
    };
    await saveSession(session, TEST_DIR);

    const loaded = await loadLatestSession(TEST_DIR);
    expect(typeof loaded!.id).toBe("string");
    expect(typeof loaded!.savedAt).toBe("string");
    expect(typeof loaded!.model).toBe("string");
    expect(Array.isArray(loaded!.history)).toBe(true);
  });
});


