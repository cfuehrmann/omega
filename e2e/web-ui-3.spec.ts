/**
 * Omega Web UI — pre/inter-turn event rendering tests (Playwright).
 *
 * Covers the structural gap where events that don't belong to any turn
 * (session_start, server_started, server_stopped, and future inter-turn events) must still
 * appear in the feed.
 *
 * Tests marked RED will fail before the flat-store refactoring and pass after.
 */

import { test, expect } from "./fixtures/index.js";

// Shorthand for waiting until the Ω button is in "connected" state.
const connectedDot = (page: import("@playwright/test").Page) =>
  page.locator('[data-testid="omega-btn"][data-status="connected"]');

// ---------------------------------------------------------------------------
// session_start renders even before any turn exists
// RED before flat-store refactoring (appendEvent is a no-op when turns=[])
// ---------------------------------------------------------------------------

test("session_start event renders as info block even before any turn", async ({ page, server }) => {
  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });

  // Send session_start with no user_message before or after
  await server.sendEvent({
    type: "session_start",
    sessionId: "",
    authMode: "api-key",
    model: "claude-sonnet-4-6",
    systemPrompt: "You are Omega.",
  });

  // The info block should be visible in the feed
  const infoBlock = page.getByTestId("block-info");
  await expect(infoBlock).toBeVisible({ timeout: 3000 });
  await expect(infoBlock).toContainText("claude-sonnet-4-6");
});

// ---------------------------------------------------------------------------
// server_stopped renders after a completed turn
// ---------------------------------------------------------------------------

test("server_stopped event renders as info block in the feed", async ({ page, server }) => {
  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });

  await server.sendEvent({ type: "user_message", content: "hi" });
  await server.sendEvent({
    type: "turn_end",
    metrics: { inputTokens: 5, outputTokens: 3 },
  });
  await server.sendEvent({ type: "server_stopped", outcome: "clean" });

  // server_stopped should render as an info block
  const infoBlocks = page.getByTestId("block-info");
  await expect(infoBlocks.filter({ hasText: "server_stopped" })).toBeVisible({ timeout: 3000 });
});

// ---------------------------------------------------------------------------
// History replay shows session_start before the first turn
// RED before flat-store refactoring (dropped during history dispatch)
// ---------------------------------------------------------------------------

test("history replay shows session_start block before the first turn", async ({ page, server }) => {
  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });

  // Write fixture with session_start before any user_message
  await server.loadFixture([
    JSON.stringify({ type: "session_start", time: "2026-01-15T10:00:00.000Z", sessionId: "", authMode: "api-key", model: "claude-sonnet-4-6", systemPrompt: "You are Omega." }),
    JSON.stringify({ type: "user_message", time: "2026-01-15T10:00:01.000Z", content: "hello" }),
    JSON.stringify({ type: "llm_response", time: "2026-01-15T10:00:02.000Z", stopReason: "end_turn", usage: { input_tokens: 5, output_tokens: 3 }, contextHash: "ab12cd34ef56", text: "hi there" }),
    JSON.stringify({ type: "turn_end", time: "2026-01-15T10:00:02.001Z", metrics: { inputTokens: 5, outputTokens: 3 } }),
  ]);

  await page.reload();
  await connectedDot(page).waitFor({ timeout: 5000 });

  const feed = page.getByTestId("feed");

  // session_start block must be visible
  const infoBlocks = feed.getByTestId("block-info");
  await expect(infoBlocks.filter({ hasText: "claude-sonnet-4-6" })).toBeVisible({ timeout: 3000 });

  // The turn must also be visible
  await expect(feed.getByTestId("block-user")).toBeVisible({ timeout: 3000 });
});

// ---------------------------------------------------------------------------
// session_start appears BEFORE the first user_message in the rendered order
// RED before flat-store refactoring
// ---------------------------------------------------------------------------

test("session_start block appears before the first user_message block", async ({ page, server }) => {
  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });

  await server.loadFixture([
    JSON.stringify({ type: "session_start", time: "2026-01-15T10:00:00.000Z", sessionId: "", authMode: "api-key", model: "claude-sonnet-4-6", systemPrompt: "You are Omega." }),
    JSON.stringify({ type: "user_message", time: "2026-01-15T10:00:01.000Z", content: "first message" }),
    JSON.stringify({ type: "turn_end", time: "2026-01-15T10:00:02.000Z", metrics: { inputTokens: 5, outputTokens: 3 } }),
  ]);

  await page.reload();
  await connectedDot(page).waitFor({ timeout: 5000 });

  const feed = page.getByTestId("feed");
  // Select all blocks by their data-testid prefix
  const blocks = feed.locator("[data-testid^='block-']");

  // Get all blocks and check ordering: session_start (info) must come before user block
  const count = await blocks.count();
  expect(count).toBeGreaterThanOrEqual(2);

  // Find index of session_start info block and user_message block
  let sessionStartIdx = -1;
  let userMessageIdx = -1;
  for (let i = 0; i < count; i++) {
    const block = blocks.nth(i);
    const testId = await block.getAttribute("data-testid") ?? "";
    const text = await block.textContent() ?? "";
    if (testId === "block-info" && text.includes("claude-sonnet-4-6")) sessionStartIdx = i;
    if (testId === "block-user") userMessageIdx = i;
  }

  expect(sessionStartIdx).toBeGreaterThanOrEqual(0); // session_start must render
  expect(userMessageIdx).toBeGreaterThanOrEqual(0);  // user_message must render
  expect(sessionStartIdx).toBeLessThan(userMessageIdx); // session_start before user_message
});
