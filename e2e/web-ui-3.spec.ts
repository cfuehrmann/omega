/**
 * Omega Web UI — pre/inter-turn event rendering tests (Playwright).
 *
 * Covers the structural gap where events that don't belong to any turn
 * (session_start, session_end, and future inter-turn events) must still
 * appear in the feed.
 *
 * Tests marked RED will fail before the flat-store refactoring and pass after.
 */

import { test, expect } from "./fixtures/index.js";

// ---------------------------------------------------------------------------
// session_start renders even before any turn exists
// RED before flat-store refactoring (appendEvent is a no-op when turns=[])
// ---------------------------------------------------------------------------

test("session_start event renders as info block even before any turn", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  // Send session_start with no user_message before or after
  await server.sendEvent({
    type: "session_start",
    authMode: "api-key",
    model: "claude-sonnet-4-6",
    provider: "anthropic",
    systemPrompt: "You are Omega.",
  });

  // The info block should be visible in the feed
  const infoBlock = page.locator(".block.info");
  await expect(infoBlock).toBeVisible({ timeout: 3000 });
  await expect(infoBlock).toContainText("claude-sonnet-4-6");
});

// ---------------------------------------------------------------------------
// session_end renders after a completed turn
// GREEN before refactoring (appendEvent appends to last turn) — keeps working after
// ---------------------------------------------------------------------------

test("session_end event renders as info block in the feed", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hi" });
  await server.sendEvent({
    type: "turn_end",
    metrics: { inputTokens: 5, outputTokens: 3 },
  });
  await server.sendEvent({ type: "session_end", outcome: "clean" });

  // session_end should render as an info block
  const infoBlocks = page.locator(".block.info");
  await expect(infoBlocks.filter({ hasText: "session end" })).toBeVisible({ timeout: 3000 });
});

// ---------------------------------------------------------------------------
// History replay shows session_start before the first turn
// RED before flat-store refactoring (dropped during history dispatch)
// ---------------------------------------------------------------------------

test("history replay shows session_start block before the first turn", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  // Write fixture with session_start before any user_message
  await server.loadFixture([
    JSON.stringify({ type: "session_start", ts: "2026-01-15T10:00:00.000Z", authMode: "api-key", model: "claude-sonnet-4-6", provider: "anthropic", systemPrompt: "You are Omega." }),
    JSON.stringify({ type: "user_message", ts: "2026-01-15T10:00:01.000Z", content: "hello" }),
    JSON.stringify({ type: "llm_response", ts: "2026-01-15T10:00:02.000Z", stopReason: "end_turn", usage: { input_tokens: 5, output_tokens: 3 }, contextHash: "ab12cd34", text: "hi there" }),
    JSON.stringify({ type: "turn_end", ts: "2026-01-15T10:00:02.001Z", metrics: { inputTokens: 5, outputTokens: 3 } }),
  ]);

  await page.reload();
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  const feed = page.locator(".feed");

  // session_start block must be visible
  const infoBlocks = feed.locator(".block.info");
  await expect(infoBlocks.filter({ hasText: "claude-sonnet-4-6" })).toBeVisible({ timeout: 3000 });

  // The turn must also be visible
  await expect(feed.locator(".block.user")).toBeVisible({ timeout: 3000 });
});

// ---------------------------------------------------------------------------
// session_start appears BEFORE the first user_message in the rendered order
// RED before flat-store refactoring
// ---------------------------------------------------------------------------

test("session_start block appears before the first user_message block", async ({ page, server }) => {
  await page.goto("/");
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  await server.loadFixture([
    JSON.stringify({ type: "session_start", ts: "2026-01-15T10:00:00.000Z", authMode: "api-key", model: "claude-sonnet-4-6", provider: "anthropic", systemPrompt: "You are Omega." }),
    JSON.stringify({ type: "user_message", ts: "2026-01-15T10:00:01.000Z", content: "first message" }),
    JSON.stringify({ type: "turn_end", ts: "2026-01-15T10:00:02.000Z", metrics: { inputTokens: 5, outputTokens: 3 } }),
  ]);

  await page.reload();
  await page.locator(".dot.connected").waitFor({ timeout: 5000 });

  const feed = page.locator(".feed");
  const blocks = feed.locator(".block");

  // Get all blocks and check ordering: session_start (info) must come before user block
  const count = await blocks.count();
  expect(count).toBeGreaterThanOrEqual(2);

  // Find index of session_start info block and user_message block
  let sessionStartIdx = -1;
  let userMessageIdx = -1;
  for (let i = 0; i < count; i++) {
    const block = blocks.nth(i);
    const cls = await block.getAttribute("class") ?? "";
    const text = await block.textContent() ?? "";
    if (cls.includes("info") && text.includes("claude-sonnet-4-6")) sessionStartIdx = i;
    if (cls.includes("user")) userMessageIdx = i;
  }

  expect(sessionStartIdx).toBeGreaterThanOrEqual(0); // session_start must render
  expect(userMessageIdx).toBeGreaterThanOrEqual(0);  // user_message must render
  expect(sessionStartIdx).toBeLessThan(userMessageIdx); // session_start before user_message
});
