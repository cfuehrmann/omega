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

// ---------------------------------------------------------------------------
// Regression test: no false-positive modal when the session already has history
//
// If hasPendingChanges is true but the session already has events (i.e. the
// agent has already started working), the modal must NOT appear — dirty git is
// intentional / already accepted at that point.  This covers the browser-
// refresh false-positive: the server's cached hasPendingChanges value can
// outlive the git cleanup, so the client guards on `events.length === 0`.
// ---------------------------------------------------------------------------

test("no pending-changes modal when session already has history (git may be stale-dirty)", async ({ page, server }) => {
  // Server will keep reporting hasPendingChanges: true (simulating a stale
  // cached value that was set when the session was created and hasn't been
  // re-checked since the user cleaned up git).
  await server.setPendingChanges(true);

  // Inject a user_message event into the session's events file *before* the
  // page loads — simulating a session where work has already started.
  // sendEvent persists to disk even without an active WebSocket connection.
  await server.sendEvent({ type: "user_message", content: "hello" });

  await page.goto("/");
  await page.locator(CONNECTED_SELECTOR).waitFor({ timeout: 5000 });

  // Modal must not appear: the session has history, so the warning is moot.
  await expect(page.getByTestId("pending-changes-modal")).not.toBeVisible();
  await expect(page.locator("textarea")).toBeEnabled();
});

// ---------------------------------------------------------------------------
// Test: modal appears after a reset (new session) when working tree is dirty
//
// This is the regression test for the bug where reset_done cleared
// hasPendingChanges, so the modal never appeared on a fresh new session —
// only showing after a manual browser refresh.
// ---------------------------------------------------------------------------

test("pending-changes modal appears after new-session reset when working tree is dirty", async ({ page, server }) => {
  // Start with a dirty tree.
  await server.setPendingChanges(true);
  await page.goto("/");
  // Wait for the initial modal to appear (confirms hasPendingChanges works on first connect).
  await page.getByTestId("pending-changes-modal").waitFor({ timeout: 5000 });

  // Dismiss the modal.
  await page.getByTestId("pending-changes-ok-btn").click();
  await expect(page.getByTestId("pending-changes-modal")).not.toBeVisible();

  // Open the session picker and create a new session — this sends `reset` to
  // the server, which responds with session_info (hasPendingChanges: true) +
  // history + reset_done. The modal must reappear for the new session.
  await page.getByTestId("sessions-btn").click(); // open session picker
  await page.getByTestId("session-picker-new").click();

  // After reset the modal must reappear because the new session was also
  // created with a dirty working tree.
  await expect(page.getByTestId("pending-changes-modal")).toBeVisible({ timeout: 5000 });
});
