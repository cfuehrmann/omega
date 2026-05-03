/**
 * Pending-changes gate — end-to-end tests.
 *
 * Verifies that when the server reports `hasPendingChanges: true` in the
 * opening `session_info` frame, the web client shows a blocking modal and
 * disables the composer until the user acknowledges.
 */

import { test, expect } from "./fixtures/index.js";

const CONNECTED_SELECTOR = '[data-testid="omega-btn"][data-status="connected"]';

// ---------------------------------------------------------------------------
// Helper: set pending-changes flag then load the page.
// The flag must be set BEFORE the WS connection opens, because the server
// reads it when it builds the opening session_info frame.
// ---------------------------------------------------------------------------
async function loadWithPendingChanges(
  page: import("@playwright/test").Page,
  server: { setPendingChanges(v: boolean): Promise<void> },
) {
  await server.setPendingChanges(true);
  await page.goto("/");
  await page.locator(CONNECTED_SELECTOR).waitFor({ timeout: 5000 });
}

// ---------------------------------------------------------------------------
// Test: modal appears when hasPendingChanges is true
// ---------------------------------------------------------------------------

test("pending-changes modal appears when server reports dirty working tree", async ({ page, server }) => {
  await loadWithPendingChanges(page, server);
  await expect(page.getByTestId("pending-changes-modal")).toBeVisible({ timeout: 5000 });
});

// ---------------------------------------------------------------------------
// Test: composer is disabled while modal is showing
// ---------------------------------------------------------------------------

test("composer textarea is disabled while pending-changes modal is showing", async ({ page, server }) => {
  await loadWithPendingChanges(page, server);
  await page.getByTestId("pending-changes-modal").waitFor({ timeout: 5000 });
  await expect(page.locator("textarea")).toBeDisabled();
});

// ---------------------------------------------------------------------------
// Test: acknowledging the modal enables the composer and hides the modal
// ---------------------------------------------------------------------------

test("clicking Proceed dismisses the modal and enables the composer", async ({ page, server }) => {
  await loadWithPendingChanges(page, server);
  await page.getByTestId("pending-changes-modal").waitFor({ timeout: 5000 });

  await page.getByTestId("pending-changes-ok-btn").click();

  await expect(page.getByTestId("pending-changes-modal")).not.toBeVisible();
  await expect(page.locator("textarea")).toBeEnabled();
});

// ---------------------------------------------------------------------------
// Test: no modal when working tree is clean (default)
// ---------------------------------------------------------------------------

test("no pending-changes modal when working tree is clean", async ({ page, server }) => {
  // Default fixture state has hasPendingChanges: false.
  await page.goto("/");
  await page.locator(CONNECTED_SELECTOR).waitFor({ timeout: 5000 });

  await expect(page.getByTestId("pending-changes-modal")).not.toBeVisible();
  await expect(page.locator("textarea")).toBeEnabled();
});
