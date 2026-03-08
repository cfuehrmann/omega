/**
 * Omega Web UI — session persistence e2e tests.
 *
 * These tests verify that session history survives a server restart by
 * testing the persistence mechanism via the control API.
 *
 * The test server exposes additional control endpoints used here:
 *   POST /control/save         — flush in-memory log to disk
 *   POST /control/load         — reload log from disk (clears in-memory first)
 *   POST /control/reset        — clear both in-memory AND disk state
 *   GET  /control/disk-snapshot — return current persisted events without changing state
 *
 * Simulating a server restart:
 *   1. Call /control/save  (flush to disk — what clean shutdown does)
 *   2. Call /control/load  (re-read disk into memory — what startup does)
 *      This clears in-memory state first so only disk contents remain.
 *   3. Reload the browser page — client reconnects, server replays from loaded log
 */

import { test, expect } from "./fixtures/index.js";

// ---------------------------------------------------------------------------
// Disk snapshot: each event is persisted after turn_end
// ---------------------------------------------------------------------------

test("incremental save: each turn_end persists immediately to disk", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  // Turn 1
  await server.sendEvent({ type: "user_message", content: "first question" });
  await server.sendEvent({ type: "text", text: "First answer." });
  await server.sendEvent({
    type: "turn_end",
    metrics: { inputTokens: 5, outputTokens: 5, costUsd: 0.0001, savedUsd: 0, ttftMs: 50 },
    model: "claude-sonnet-4-6",
    provider: "anthropic",
  });

  // Turn 2
  await server.sendEvent({ type: "user_message", content: "second question" });
  await server.sendEvent({ type: "text", text: "Second answer." });
  await server.sendEvent({
    type: "turn_end",
    metrics: { inputTokens: 10, outputTokens: 8, costUsd: 0.0002, savedUsd: 0, ttftMs: 60 },
    model: "claude-sonnet-4-6",
    provider: "anthropic",
  });

  await expect(page.locator(".block.user")).toHaveCount(2, { timeout: 3000 });

  // Check disk contents — both turns should be persisted
  const persisted = await server.diskSnapshot() as object[];
  const contents = (persisted as any[]).map((e: any) => e.content).filter(Boolean);
  expect(contents).toContain("first question");
  expect(contents).toContain("second question");
});

// ---------------------------------------------------------------------------
// Persistence file ordering
// ---------------------------------------------------------------------------

test("persistence file contains events in correct order", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "order-test" });
  await server.sendEvent({ type: "text", text: "response" });
  await server.sendEvent({
    type: "turn_end",
    metrics: { inputTokens: 5, outputTokens: 5, costUsd: 0.0001, savedUsd: 0, ttftMs: 50 },
    model: "claude-sonnet-4-6",
    provider: "anthropic",
  });

  const persisted = await server.diskSnapshot() as object[];
  const types = persisted.map((e: any) => e.type);

  // text events are NOT persisted (they are ephemeral streaming fragments,
  // same as Agent never writes them to events.jsonl).
  // user_message comes before turn_end in the persisted log.
  const ui = types.indexOf("user_message");
  const te = types.indexOf("turn_end");

  expect(ui).toBeGreaterThanOrEqual(0);
  expect(te).toBeGreaterThan(ui);
  expect(types).not.toContain("text");
});

// ---------------------------------------------------------------------------
// Simulated server restart (save → load)
// ---------------------------------------------------------------------------

test("history survives a simulated server restart (save + load cycle)", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "persist me" });
  await server.sendEvent({ type: "text", text: "I will survive." });
  await server.sendEvent({
    type: "turn_end",
    metrics: { inputTokens: 5, outputTokens: 5, costUsd: 0.0001, savedUsd: 0, ttftMs: 50 },
    model: "claude-sonnet-4-6",
    provider: "anthropic",
  });
  await expect(page.locator(".block.user")).toBeVisible({ timeout: 3000 });

  // Simulate clean shutdown: flush in-memory log to disk
  await server.save();

  // Simulate server restart: clear in-memory log, reload from disk
  await server.load();

  // Verify in-memory state came ONLY from disk (diskSnapshot should match)
  const disk = await server.diskSnapshot() as object[];
  expect(disk.length).toBeGreaterThan(0);
  const diskContents = disk.filter((e: any) => e.type === "user_message").map((e: any) => e.content);
  expect(diskContents).toContain("persist me");

  // Browser reconnects — server replays the loaded log
  await page.reload();
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  // After replay, the user block is visible.
  // The assistant text block is NOT replayed — text events are ephemeral
  // streaming fragments that are never persisted (same as events.jsonl).
  // The turn_end footer block IS replayed, confirming the turn completed.
  await expect(page.locator(".block.user")).toBeVisible({ timeout: 3000 });
  await expect(page.locator(".block.footer")).toBeVisible({ timeout: 3000 });
});

test("after load, reset clears both memory and disk", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "temporary" });
  await server.sendEvent({ type: "turn_end",
    metrics: { inputTokens: 5, outputTokens: 2, costUsd: 0, savedUsd: 0, ttftMs: 10 },
    model: "claude-sonnet-4-6", provider: "anthropic" });
  await expect(page.locator(".block.user")).toBeVisible({ timeout: 3000 });

  // Save to disk then reset (reset should clear disk too)
  await server.save();
  await server.reset();

  // Disk should now be empty
  const disk = await server.diskSnapshot() as object[];
  expect(disk).toHaveLength(0);

  // Page reload should show empty state
  await page.reload();
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });
  await expect(page.locator(".block.user")).not.toBeVisible({ timeout: 2000 });
});
