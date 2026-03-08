/**
 * Session continuity e2e tests.
 *
 * These tests verify that:
 *  1. Reconnecting (browser refresh) reuses the same server-side agent —
 *     no "corpse sessions" where a new Agent is created on each WebSocket open.
 *  2. A { type: "reset" } message explicitly kills the current session and
 *     creates a fresh one, so the user can escape a stuck/stale session.
 *
 * The test server simulates the real server's single-agent behaviour.
 * New control endpoints:
 *   GET  /control/agent-id   — returns the current agent instance ID (numeric counter)
 *   POST /control/reset      — existing: clears event log; now also resets agent ID
 */

import { test, expect } from "./fixtures/index.js";

// ---------------------------------------------------------------------------
// On reconnect, server reuses the existing session (does NOT create new agent)
// ---------------------------------------------------------------------------

test("reconnecting does not create a new agent — browser sees existing history", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  // Simulate a completed turn in the current session
  await server.sendEvent({ type: "user_message", content: "initial message" });
  await server.sendEvent({ type: "text", text: "initial response" });
  await server.sendEvent({
    type: "turn_end",
    metrics: { inputTokens: 5, outputTokens: 5, costUsd: 0.0001, savedUsd: 0, ttftMs: 50 },
    model: "claude-sonnet-4-6",
    provider: "anthropic",
  });
  await expect(page.locator(".block.user")).toBeVisible({ timeout: 3000 });

  // Get the agent ID before reload
  const agentIdBefore = await server.agentId();

  // Browser reload — reconnects to the same server
  await page.reload();
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  // Agent ID must not change — same session
  const agentIdAfter = await server.agentId();
  expect(agentIdAfter).toBe(agentIdBefore);

  // History should still be visible (replayed from event log)
  await expect(page.locator(".block.user")).toBeVisible({ timeout: 3000 });
});

// ---------------------------------------------------------------------------
// { type: "reset" } message creates a new agent (kills old session)
// ---------------------------------------------------------------------------

test("reset message creates a new agent and clears history", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  // Simulate a completed turn
  await server.sendEvent({ type: "user_message", content: "old session content" });
  await server.sendEvent({ type: "text", text: "old session response" });
  await server.sendEvent({
    type: "turn_end",
    metrics: { inputTokens: 5, outputTokens: 5, costUsd: 0.0001, savedUsd: 0, ttftMs: 50 },
    model: "claude-sonnet-4-6",
    provider: "anthropic",
  });
  await expect(page.locator(".block.user")).toBeVisible({ timeout: 3000 });

  const agentIdBefore = await server.agentId();

  // Send reset from the browser (simulating a user clicking "New session")
  await page.evaluate(() => {
    // @ts-ignore — access the WS from the page context
    const ws = (window as any).__omegaWs;
    if (ws) ws.send(JSON.stringify({ type: "reset" }));
  });

  // Server should acknowledge with a reset_done event
  // Wait for the UI to show empty state (no user blocks)
  await expect(page.locator(".block.user")).not.toBeVisible({ timeout: 5000 });

  // Agent ID must have changed — new session created
  const agentIdAfter = await server.agentId();
  expect(agentIdAfter).not.toBe(agentIdBefore);
});

// ---------------------------------------------------------------------------
// Second browser tab does not kill the first session
// ---------------------------------------------------------------------------

test("opening a second connection reuses the same agent (not a new one)", async ({ page, server, context }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  const agentIdFirst = await server.agentId();

  // Open a second browser tab
  const page2 = await context.newPage();
  await page2.goto("/");
  await page2.locator(".dot.connected").waitFor({ timeout: 5000 });

  // Agent should still be the same
  const agentIdSecond = await server.agentId();
  expect(agentIdSecond).toBe(agentIdFirst);

  await page2.close();
});
