/**
 * Tests for writeDiagnostic and checkDiagnostics.
 */

import { describe, it, expect, beforeEach, afterEach } from "bun:test";
import { mkdtempSync, rmSync, readFileSync, readdirSync } from "fs";
import { join } from "path";
import { tmpdir } from "os";
import {
  writeDiagnostic,
} from "./diagnosis.js";

// ---------------------------------------------------------------------------
// writeDiagnostic
// ---------------------------------------------------------------------------

describe("writeDiagnostic", () => {
  let tmpDir: string;

  beforeEach(() => {
    tmpDir = mkdtempSync(join(tmpdir(), "omega-diag-test-"));
  });

  afterEach(() => {
    rmSync(tmpDir, { recursive: true, force: true });
  });

  it("writes a file with standard diagnostic fields", async () => {
    const path = await writeDiagnostic(
      {
        summary: "test error",
        errorMessage: "boom",
        httpStatus: 500,
        provider: "anthropic",
        model: "claude-sonnet-4-6",
        requestMessages: [{ role: "user", content: "hello" }],
        history: [{ role: "user", content: "hello" }],
      },
      tmpDir,
    );

    expect(path).not.toBeNull();
    const contents = JSON.parse(readFileSync(path!, "utf-8"));
    expect(contents._omega_diagnostic).toBe(true);
    expect(contents.summary).toBe("test error");
    expect(contents.errorMessage).toBe("boom");
    expect(contents.httpStatus).toBe(500);
    expect(contents.provider).toBe("anthropic");
    expect(contents.model).toBe("claude-sonnet-4-6");
    expect(contents.requestMessages).toEqual([{ role: "user", content: "hello" }]);
  });

  it("does NOT include a logFile field (pino retired)", async () => {
    const path = await writeDiagnostic(
      {
        summary: "some error",
        errorMessage: "x",
        provider: "anthropic",
        model: "claude-sonnet-4-6",
        requestMessages: [],
        history: [],
      },
      tmpDir,
    );

    const contents = JSON.parse(readFileSync(path!, "utf-8"));
    expect(contents.logFile).toBeUndefined();
  });

  it("does NOT include an eventBuffer field", async () => {
    const path = await writeDiagnostic(
      {
        summary: "test",
        errorMessage: "e",
        provider: "anthropic",
        model: "claude-sonnet-4-6",
        requestMessages: [],
        history: [],
      },
      tmpDir,
    );

    const contents = JSON.parse(readFileSync(path!, "utf-8"));
    expect(contents.eventBuffer).toBeUndefined();
  });

  it("includes standard fields alongside requestMessages", async () => {
    const path = await writeDiagnostic(
      {
        summary: "API 400",
        errorMessage: "bad request",
        httpStatus: 400,
        provider: "anthropic",
        model: "claude-opus-4-6",
        requestMessages: [{ role: "user", content: "hello" }],
        history: [{ role: "user", content: "hello" }],
        extra: { foo: "bar" },
      },
      tmpDir,
    );

    const contents = JSON.parse(readFileSync(path!, "utf-8"));
    expect(contents.summary).toBe("API 400");
    expect(contents.httpStatus).toBe(400);
    expect(contents.model).toBe("claude-opus-4-6");
    expect(contents.requestMessages).toEqual([{ role: "user", content: "hello" }]);
    expect(contents.extra).toEqual({ foo: "bar" });
  });

  it("silently swallows write errors and returns null", async () => {
    const result = await writeDiagnostic(
      {
        summary: "err",
        errorMessage: "x",
        provider: "anthropic",
        model: "m",
        requestMessages: [],
        history: [],
      },
      "/this/path/does/not/exist/at/all/ever",
    );
    expect(result).toBeNull();
  });

  it("returns null when diagDir is null (disabled)", async () => {
    const result = await writeDiagnostic(
      {
        summary: "disabled",
        errorMessage: "x",
        provider: "anthropic",
        model: "m",
        requestMessages: [],
        history: [],
      },
      null,
    );
    expect(result).toBeNull();
    // No files written anywhere
    expect(readdirSync(tmpDir)).toHaveLength(0);
  });

  it("creates the directory if it does not exist", async () => {
    const nestedDir = join(tmpDir, "nested", "diag");
    const path = await writeDiagnostic(
      {
        summary: "nested",
        errorMessage: "x",
        provider: "anthropic",
        model: "m",
        requestMessages: [],
        history: [],
      },
      nestedDir,
    );
    expect(path).not.toBeNull();
    const files = readdirSync(nestedDir);
    expect(files).toHaveLength(1);
    expect(files[0]).toMatch(/\.json$/);
  });
});
