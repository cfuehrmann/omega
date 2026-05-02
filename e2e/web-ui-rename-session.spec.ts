/**
 * Rename session e2e test — Phase 2d regression.
 *
 * Exercises the rename UI against the real production omega-server (Rust,
 * port 3003) via the mock-omega-server fixture.  The bug being tested lives
 * in the Rust handler (`handle_rename_session`): it was missing the
 * `session_renamed` WebSocket broadcast, so the SolidJS client's
 * `renamedSessions` map was never populated and the picker reverted to the
 * old (empty) name immediately after the rename input was dismissed.
 *
 * A test against the chromium test-server cannot catch this regression
 * because test-server never calls the Rust rename handler.
 */

import { test, expect } from "@playwright/test";

const connectedDot = (page: import("@playwright/test").Page) =>
  page.locator('[data-testid="omega-btn"][data-status="connected"]');

/**
 * Create a fresh session so each test starts clean.  Mirrors the pattern
 * used by real-server-replay.spec.ts.
 */
async function ensureSession(page: import("@playwright/test").Page) {
  const picker = page.getByTestId("session-picker-modal");
  const sessionsBtn = page.getByTestId("sessions-btn");
  if (!(await picker.isVisible())) {
    await sessionsBtn.click();
    await expect(picker).toBeVisible({ timeout: 3000 });
  }
  await expect(page.getByTestId("session-picker-new")).toBeEnabled({ timeout: 5000 });
  const priorDir = (await sessionsBtn.getAttribute("data-session-dir")) ?? "";
  await page.getByTestId("session-picker-new").click();
  await expect(picker).not.toBeVisible({ timeout: 5000 });
  await expect.poll(
    async () => (await sessionsBtn.getAttribute("data-session-dir")) ?? "",
    { timeout: 5000 },
  ).not.toBe(priorDir);
  await expect(page.getByTestId("status-label")).toHaveText("Ready", { timeout: 5000 });
}

// ---------------------------------------------------------------------------
// Rename the active session and verify the name persists in the picker
// ---------------------------------------------------------------------------

test("renaming the active session shows the new name in the session picker", async ({ page }) => {
  await page.goto("/");
  await connectedDot(page).waitFor({ timeout: 5000 });
  await ensureSession(page);

  // Capture the current session dir so we can identify its item in the picker.
  const sessionsBtn = page.getByTestId("sessions-btn");
  const currentDir = await sessionsBtn.getAttribute("data-session-dir") ?? "";
  expect(currentDir).toBeTruthy();
  // The picker list uses the basename of the session dir to identify each entry.
  const currentDirBasename = currentDir.split("/").pop() ?? currentDir;

  // Open the session picker.
  await sessionsBtn.click();
  await expect(page.getByTestId("session-picker-modal")).toBeVisible({ timeout: 3000 });

  // Find the item that corresponds to the current session dir.
  const list = page.getByTestId("session-picker-list");
  const currentItem = list
    .getByTestId("session-picker-item")
    .filter({ hasText: currentDirBasename })
    .first();
  await expect(currentItem).toBeVisible({ timeout: 3000 });

  // Click Rename on the current session.
  await currentItem.getByTestId("session-picker-rename").click();

  // The inline input must appear.
  const renameInput = page.getByTestId("session-picker-rename-input");
  await expect(renameInput).toBeVisible({ timeout: 2000 });

  // Type the new name.
  await renameInput.fill("my-renamed-session");

  // Save.
  await page.getByTestId("session-picker-save").click();

  // The input must disappear (rename mode exits).
  await expect(renameInput).not.toBeVisible({ timeout: 3000 });

  // The picker must show the new name immediately — without a page reload.
  // Before the Phase 2d fix the server never sent session_renamed, so the
  // renamedSessions map was empty and the item reverted to "(unnamed)".
  await expect(currentItem).toContainText("my-renamed-session", { timeout: 3000 });
});
