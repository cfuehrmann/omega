/**
 * Session picker e2e tests — search, scroll, resume progress, deletion.
 *
 * These tests exercise the session picker modal (opened from the bottom panel)
 * against the test server, which mirrors the real server's /sessions endpoint
 * and resume_session / delete_session WebSocket handling.
 */

import { test, expect } from "./fixtures/index.js";

// Shorthand for waiting until the Ω button is in "connected" state.
const connectedDot = (page: import("@playwright/test").Page) =>
  page.locator('[data-testid="omega-btn"][data-status="connected"]');

/** Open the bottom panel and click the session trigger button to show the session modal. */
async function openSessionPicker(page: import("@playwright/test").Page) {
  // Open bottom panel by clicking Ω
  await page.getByTestId("omega-btn").click();
  // Wait for the session trigger button (requires sessionDir to be set via session_info)
  const triggerBtn = page.getByTestId("session-trigger-btn");
  await expect(triggerBtn).toBeVisible({ timeout: 3000 });
  await triggerBtn.click();
  // Wait for the modal to appear
  await expect(page.getByTestId("session-picker-modal")).toBeVisible({ timeout: 3000 });
}

// ---------------------------------------------------------------------------
// Session picker displays sessions with name, description, continuationOf
// ---------------------------------------------------------------------------

test("session picker shows name, description, and continuationOf metadata", async ({ page, server }) => {
  // Create past sessions with metadata
  await server.createPastSession({
    metadata: { name: "auth refactor", description: "Refactored JWT auth flow" },
    events: [{ type: "user_message", content: "hello" }],
  });
  await server.createPastSession({
    metadata: { name: "bug fix", continuationOf: "2025-01-01T00-00-00-000-abcdef01" },
    events: [{ type: "user_message", content: "fix the bug" }],
  });

  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });

  await openSessionPicker(page);

  const list = page.getByTestId("session-picker-list");
  await expect(list).toBeVisible({ timeout: 5000 });

  // Should see both sessions (plus the current one potentially)
  const items = list.getByTestId("session-picker-item");
  // At least 2 past sessions
  expect(await items.count()).toBeGreaterThanOrEqual(2);

  // Check that name/description/continuationOf are displayed
  await expect(list).toContainText("auth refactor");
  await expect(list).toContainText("Refactored JWT auth flow");
  await expect(list).toContainText("bug fix");
  await expect(list).toContainText("continues");
});

// ---------------------------------------------------------------------------
// Session picker search filters sessions
// ---------------------------------------------------------------------------

test("session picker search filters by name and description", async ({ page, server }) => {
  await server.createPastSession({
    metadata: { name: "login flow" },
    events: [{ type: "user_message", content: "hello" }],
  });
  await server.createPastSession({
    metadata: { name: "database migration" },
    events: [{ type: "user_message", content: "migrate" }],
  });
  await server.createPastSession({
    metadata: { name: "auth tests", description: "testing login edge cases" },
    events: [{ type: "user_message", content: "test" }],
  });

  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });

  await openSessionPicker(page);

  const searchInput = page.getByTestId("session-picker-search");
  await expect(searchInput).toBeVisible();

  // Type "login" — should show "login flow" and "auth tests" (description contains "login")
  await searchInput.fill("login");
  const items = page.getByTestId("session-picker-item");
  // "database migration" should be filtered out
  await expect(page.getByTestId("session-picker-list")).not.toContainText("database migration");
  // "login flow" should be visible
  await expect(page.getByTestId("session-picker-list")).toContainText("login flow");

  // Clear and type "migration"
  await searchInput.fill("migration");
  await expect(page.getByTestId("session-picker-list")).toContainText("database migration");
  await expect(page.getByTestId("session-picker-list")).not.toContainText("login flow");
});

// ---------------------------------------------------------------------------
// Session picker list is scrollable (does not overflow the modal)
// ---------------------------------------------------------------------------

test("session picker list is scrollable when many sessions exist", async ({ page, server }) => {
  // Create enough sessions to overflow
  for (let i = 0; i < 25; i++) {
    await server.createPastSession({
      metadata: { name: `session ${i}` },
      events: [{ type: "user_message", content: `msg ${i}` }],
    });
  }

  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });

  await openSessionPicker(page);

  const list = page.getByTestId("session-picker-list");
  await expect(list).toBeVisible();

  // The list should be scrollable (scrollHeight > clientHeight)
  const isScrollable = await list.evaluate(el => el.scrollHeight > el.clientHeight);
  expect(isScrollable).toBe(true);
});

// ---------------------------------------------------------------------------
// Resuming a session shows progress and then completes
// ---------------------------------------------------------------------------

test("resuming session shows progress indicator then completes", async ({ page, server }) => {
  const { dir } = await server.createPastSession({
    metadata: { name: "old session" },
    events: [{ type: "user_message", content: "old work" }],
  });
  // Extract just the dir name from the full path
  const dirName = dir.split("/").pop()!;

  // Set a delay so we can observe the "resuming" state
  await server.setResumeDelay(1000);

  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });

  await openSessionPicker(page);

  // Click the Continue button on the matching session
  await page.getByTestId("session-picker-item").filter({ hasText: "old session" }).first()
    .getByTestId("session-picker-continue").click();

  // Should see a "resuming" indicator within the modal (modal stays open)
  await expect(page.getByTestId("session-picker-modal")).toBeVisible();
  await expect(page.getByTestId("session-picker-resuming")).toBeVisible({ timeout: 2000 });

  // Eventually the modal closes and we're connected with a session_resumed event
  await expect(page.getByTestId("session-picker-modal")).not.toBeVisible({ timeout: 5000 });
  await connectedDot(page).waitFor({ timeout: 5000 });

  // The feed should show the session_resumed event
  await expect(page.getByTestId("block-session-resumed")).toBeVisible({ timeout: 3000 });

  // Clean up delay
  await server.setResumeDelay(0);
});

// ---------------------------------------------------------------------------
// Aborting a resumption returns to the session list
// ---------------------------------------------------------------------------

test("aborting a resumption returns to the session list", async ({ page, server }) => {
  await server.createPastSession({
    metadata: { name: "slow session" },
    events: [{ type: "user_message", content: "work" }],
  });

  // Long delay so abort can fire before completion
  await server.setResumeDelay(5000);

  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });

  await openSessionPicker(page);

  // Click the Continue button on the matching session
  await page.getByTestId("session-picker-item").filter({ hasText: "slow session" }).first()
    .getByTestId("session-picker-continue").click();

  // Wait for resuming state
  await expect(page.getByTestId("session-picker-resuming")).toBeVisible({ timeout: 2000 });

  // Click cancel/abort
  await page.getByTestId("session-picker-cancel").click();

  // Should return to the session list (modal stays open)
  await expect(page.getByTestId("session-picker-list")).toBeVisible({ timeout: 3000 });
  await expect(page.getByTestId("session-picker-resuming")).not.toBeVisible();

  // Clean up delay
  await server.setResumeDelay(0);
});

// ---------------------------------------------------------------------------
// Deleting a session removes it from the list
// ---------------------------------------------------------------------------

test("deleting a session removes it from the picker list", async ({ page, server }) => {
  await server.createPastSession({
    metadata: { name: "doomed session" },
    events: [{ type: "user_message", content: "delete me" }],
  });
  await server.createPastSession({
    metadata: { name: "keeper session" },
    events: [{ type: "user_message", content: "keep me" }],
  });

  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });

  await openSessionPicker(page);

  // Both sessions visible
  await expect(page.getByTestId("session-picker-list")).toContainText("doomed session");
  await expect(page.getByTestId("session-picker-list")).toContainText("keeper session");

  // Click the delete button on the first "doomed session"
  const doomedItem = page.getByTestId("session-picker-item").filter({ hasText: "doomed session" }).first();
  await doomedItem.getByTestId("session-picker-delete").click();

  // "doomed session" should disappear
  await expect(page.getByTestId("session-picker-list")).not.toContainText("doomed session", { timeout: 3000 });
  // "keeper session" should still be there
  await expect(page.getByTestId("session-picker-list")).toContainText("keeper session");
});
